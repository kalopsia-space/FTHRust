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
    let gpu = Encoder::open(&cfg).unwrap();
    assert_eq!(gpu.conversion(), Conversion::Gpu);
    println!("gpu: {:.2} ms CPU/frame", measure(gpu));
    let cpu = Encoder::open_forcing_cpu_conversion(&cfg).unwrap();
    assert_eq!(cpu.conversion(), Conversion::Cpu);
    println!("cpu: {:.2} ms CPU/frame", measure(cpu));
}
