//! Measures the CPU time per frame of the two colour-conversion paths.
//! Run with `cargo test -p fthrust-media --test encoder_throughput -- --ignored --nocapture`.

mod support;

use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::encoder::{Conversion, Encoder, EncoderConfig};
use fthrust_media::vaapi::vaapi_available;

const FRAMES: u32 = 600;

fn thread_cpu_ns() -> i64 {
    // SAFETY: getrusage fills the zeroed struct we pass.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_THREAD, &mut usage), 0);
        usage
    };
    let ns = |t: libc::timeval| t.tv_sec * 1_000_000_000 + t.tv_usec * 1000;
    ns(usage.ru_utime) + ns(usage.ru_stime)
}

fn measure(mut enc: Encoder) -> f64 {
    let frame = support::synthetic_bgra(1920, 1080, 0);
    let mut out = Vec::new();
    let start = thread_cpu_ns();
    for i in 0..FRAMES {
        enc.encode(
            &frame,
            1920 * 4,
            1920,
            1080,
            i64::from(i) * 1_000_000_000 / 60,
            &mut out,
        )
        .unwrap();
        out.clear();
    }
    enc.flush(&mut out).unwrap();
    (thread_cpu_ns() - start) as f64 / 1e6 / f64::from(FRAMES)
}

/// `(VRAM, GTT)` in MiB that this process's DRM clients hold, from the
/// `drm-total-vram` and `drm-memory-gtt` lines amdgpu writes into
/// `/proc/self/fdinfo`, each client counted once.
fn process_gpu_memory_mib() -> Option<(f64, f64)> {
    let mut clients = std::collections::HashSet::new();
    let (mut vram, mut gtt, mut found) = (0u64, 0u64, false);
    for entry in std::fs::read_dir("/proc/self/fdinfo").ok()?.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .map(|v| v.trim().trim_end_matches(" KiB").trim().to_owned())
        };
        let Some(client) = field("drm-client-id:") else {
            continue;
        };
        if !clients.insert(client) {
            continue;
        }
        if let Some(v) = field("drm-total-vram:").and_then(|v| v.parse::<u64>().ok()) {
            vram += v;
            found = true;
        }
        if let Some(v) = field("drm-memory-gtt:").and_then(|v| v.parse::<u64>().ok()) {
            gtt += v;
        }
    }
    found.then(|| (vram as f64 / 1024.0, gtt as f64 / 1024.0))
}

/// The GPU memory a 4K VA-API encoder holds with GPU colour conversion,
/// right after opening and while capture writes into its dmabuf surfaces.
#[test]
#[ignore = "measurement, run by hand (amdgpu)"]
fn zero_copy_gpu_memory_at_4k() {
    fthrust_media::init().unwrap();
    assert!(vaapi_available(), "this measurement needs a VA-API device");
    let (w, h) = (3840u32, 2160u32);
    let cfg = EncoderConfig {
        src_width: w,
        src_height: h,
        enc_width: w,
        enc_height: h,
        fps: 60,
        bitrate_kbps: 40_000,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 4,
    };
    let base = process_gpu_memory_mib().unwrap_or((0.0, 0.0));
    let mut enc = Encoder::open(&cfg).unwrap();
    assert_eq!(enc.conversion(), Conversion::Gpu);
    let opened = process_gpu_memory_mib().expect("amdgpu fdinfo");
    enc.dmabuf_targets(w, h, 4).unwrap();
    let targets = process_gpu_memory_mib().unwrap();
    let mut out = Vec::new();
    for i in 0..60u32 {
        enc.encode_surface(i as usize % 4, i64::from(i) * 1_000_000_000 / 60, &mut out)
            .unwrap();
    }
    let capturing = process_gpu_memory_mib().unwrap();
    let mib = |m: (f64, f64)| format!("{:.0} MiB VRAM + {:.0} MiB GTT", m.0 - base.0, m.1 - base.1);
    println!(
        "4K VA-API encoder: opened {}; dmabuf targets {}; zero-copy capturing {}",
        mib(opened),
        mib(targets),
        mib(capturing)
    );
}

/// The GPU memory a 4K VA-API encoder holds while capture copies frames
/// through shared memory and the GPU converts them.
#[test]
#[ignore = "measurement, run by hand (amdgpu)"]
fn shared_memory_gpu_memory_at_4k() {
    fthrust_media::init().unwrap();
    assert!(vaapi_available(), "this measurement needs a VA-API device");
    let (w, h) = (3840u32, 2160u32);
    let cfg = EncoderConfig {
        src_width: w,
        src_height: h,
        enc_width: w,
        enc_height: h,
        fps: 60,
        bitrate_kbps: 40_000,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 4,
    };
    let base = process_gpu_memory_mib().unwrap_or((0.0, 0.0));
    let mut enc = Encoder::open(&cfg).unwrap();
    assert_eq!(enc.conversion(), Conversion::Gpu);
    let frame = support::synthetic_bgra(w, h, 0);
    let mut out = Vec::new();
    for i in 0..60u32 {
        enc.encode(
            &frame,
            w as usize * 4,
            w,
            h,
            i64::from(i) * 1_000_000_000 / 60,
            &mut out,
        )
        .unwrap();
    }
    let capturing = process_gpu_memory_mib().expect("amdgpu fdinfo");
    println!(
        "4K VA-API encoder, shared-memory frames: {:.0} MiB VRAM + {:.0} MiB GTT",
        capturing.0 - base.0,
        capturing.1 - base.1
    );
}

#[test]
#[ignore = "measurement, run by hand"]
fn gpu_conversion_uses_less_cpu_than_cpu_conversion() {
    fthrust_media::init().unwrap();
    assert!(vaapi_available(), "this measurement needs a VA-API device");
    let cfg = EncoderConfig {
        src_width: 1920,
        src_height: 1080,
        enc_width: 1920,
        enc_height: 1080,
        fps: 60,
        bitrate_kbps: 12000,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 4,
    };
    let mut transfer = Encoder::open(&cfg).unwrap();
    transfer.force_transfer_upload();
    let gpu = Encoder::open(&cfg).unwrap();
    assert_eq!(gpu.conversion(), Conversion::Gpu);
    println!("gpu: {:.2} ms CPU/frame", measure(gpu));
    assert_eq!(transfer.conversion(), Conversion::Gpu);
    println!("gpu-transfer: {:.2} ms CPU/frame", measure(transfer));
    let cpu = Encoder::open_forcing_cpu_conversion(&cfg).unwrap();
    assert_eq!(cpu.conversion(), Conversion::Cpu);
    println!("cpu: {:.2} ms CPU/frame", measure(cpu));
}
