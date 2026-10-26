#[path = "support/decode.rs"]
mod decode;
mod support;

use fthrust_core::capture_config::{Codec, EncoderPref};
use fthrust_media::encoder::{Conversion, Encoder, EncoderConfig};
use fthrust_media::vaapi::{DRM_FORMAT_XRGB8888, render_node_dev, same_gpu, vaapi_available};
use support::{assert_rgb_near, solid};

fn cfg(width: u32, height: u32) -> EncoderConfig {
    EncoderConfig {
        src_width: width,
        src_height: height,
        enc_width: width,
        enc_height: height,
        fps: 30,
        bitrate_kbps: 4000,
        codec: Codec::H264,
        encoder: EncoderPref::Vaapi,
        preset: 4,
    }
}

fn open_gpu(width: u32, height: u32) -> Option<Encoder> {
    fthrust_media::init().unwrap();
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return None;
    }
    let enc = Encoder::open(&cfg(width, height)).unwrap();
    assert_eq!(enc.conversion(), Conversion::Gpu);
    Some(enc)
}

#[test]
fn surfaces_export_one_plane_bgrx_dmabufs() {
    let Some(mut enc) = open_gpu(1920, 1080) else {
        return;
    };
    let images = enc.dmabuf_targets(1920, 1080, 4).unwrap();
    assert_eq!(images.len(), 4);
    for image in &images {
        assert_eq!((image.width, image.height), (1920, 1080));
        assert_eq!(image.fourcc, DRM_FORMAT_XRGB8888);
        assert_eq!(image.planes.len(), 1);
        assert!(image.planes[0].2 >= 1920 * 4, "pitch {}", image.planes[0].2);
    }
    assert!(enc.render_node().is_some());
}

#[test]
fn encoding_an_uploaded_surface_matches_the_shm_path() {
    let (w, h) = (320u32, 180u32);
    let Some(mut enc) = open_gpu(w, h) else {
        return;
    };
    enc.dmabuf_targets(w, h, 4).unwrap();
    let red = solid(w, h, [0, 0, 255, 255]);
    enc.write_surface(0, &red, w as usize * 4).unwrap();
    let mut back = Vec::new();
    let stride = enc.download_surface(0, &mut back).unwrap();
    assert!(stride >= w as usize * 4);
    assert!(back.len() >= stride * (h as usize - 1) + w as usize * 4);
    assert_eq!(&back[..3], &[0, 0, 255]);
    let mut out = Vec::new();
    enc.encode_surface(0, 0, &mut out).unwrap();
    enc.flush(&mut out).unwrap();
    assert!(!out.is_empty());
    assert!(out[0].keyframe);
    assert_eq!(out[0].pts, 0);
    let centre = ((h / 2 * w + w / 2) * 3) as usize;
    let (rgb, dw, dh) = decode::decode_frame_rgb_bt709(&enc.extradata(), &out, 0);
    assert_eq!((dw, dh), (w, h));
    assert_rgb_near(&rgb[centre..][..3], [255, 0, 0], 6);

    // The same frame through the shm upload decodes alike.
    let mut shm = open_gpu(w, h).unwrap();
    let mut shm_out = Vec::new();
    shm.encode(&red, w as usize * 4, w, h, 0, &mut shm_out)
        .unwrap();
    shm.flush(&mut shm_out).unwrap();
    let (want, ..) = decode::decode_frame_rgb_bt709(&shm.extradata(), &shm_out, 0);
    assert_rgb_near(
        &rgb[centre..][..3],
        [want[centre], want[centre + 1], want[centre + 2]],
        6,
    );
}

/// The `card*` node whose `/sys/class/drm/<node>/device` is the render
/// node's device.
fn card_for(render: &str) -> Option<String> {
    let name = render.rsplit('/').next()?;
    let device = std::fs::canonicalize(format!("/sys/class/drm/{name}/device")).ok()?;
    let mut cards: Vec<String> = std::fs::read_dir("/dev/dri")
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card"))
        .collect();
    cards.sort();
    cards.into_iter().find_map(|card| {
        let d = std::fs::canonicalize(format!("/sys/class/drm/{card}/device")).ok()?;
        (d == device).then(|| format!("/dev/dri/{card}"))
    })
}

#[test]
fn same_gpu_matches_card_and_render_node() {
    let render = "/dev/dri/renderD128";
    if !std::path::Path::new(render).exists() {
        eprintln!("skipping: no /dev/dri");
        return;
    }
    let card = card_for(render).expect("a card node shares the render node's device");
    let r = render_node_dev(render).unwrap();
    let c = render_node_dev(&card).unwrap();
    assert_ne!(r, c);
    assert!(same_gpu(r, c), "{render} and {card} are one GPU");
    assert!(same_gpu(r, r));
    // Another GPU's render node, when the machine has one.
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        for e in entries.flatten() {
            let p = e.path().to_string_lossy().into_owned();
            if p.contains("renderD") && p != render {
                let other = render_node_dev(&p).unwrap();
                assert!(!same_gpu(r, other), "{render} and {p} are different GPUs");
            }
        }
    }
    assert_eq!(render_node_dev("/nonexistent/node"), None);
}

#[test]
fn dmabuf_targets_require_gpu_conversion() {
    fthrust_media::init().unwrap();
    if !vaapi_available() {
        eprintln!("skipping: no VA-API device");
        return;
    }
    let mut enc = Encoder::open_forcing_cpu_conversion(&cfg(320, 180)).unwrap();
    assert_eq!(enc.conversion(), Conversion::Cpu);
    assert!(enc.dmabuf_targets(320, 180, 4).is_err());
}

#[test]
fn a_failed_surface_encode_leaves_the_timestamps_untouched() {
    let (w, h) = (320u32, 180u32);
    let Some(mut enc) = open_gpu(w, h) else {
        return;
    };
    let mut plain = open_gpu(w, h).unwrap();
    let red = solid(w, h, [0, 0, 255, 255]);
    let ts = 5 * 1_000_000_000;
    let mut want = Vec::new();
    plain
        .encode(&red, w as usize * 4, w, h, ts, &mut want)
        .unwrap();
    plain.flush(&mut want).unwrap();

    enc.dmabuf_targets(w, h, 4).unwrap();
    enc.inject_gpu_convert_failure();
    let mut out = Vec::new();
    assert!(enc.encode_surface(0, ts, &mut out).is_err());
    assert!(out.is_empty());
    enc.encode(&red, w as usize * 4, w, h, ts, &mut out)
        .unwrap();
    enc.flush(&mut out).unwrap();
    assert_eq!(out[0].pts, want[0].pts);
    assert_eq!(out[0].pts, 0);
    assert!(out[0].keyframe);
}

#[test]
fn dmabuf_targets_reject_bad_counts() {
    let Some(mut enc) = open_gpu(320, 180) else {
        return;
    };
    for count in [0, 9] {
        match enc.dmabuf_targets(320, 180, count) {
            Err(fthrust_media::MediaError::Invalid(msg)) => {
                assert!(msg.contains("1 to 8"), "{msg}")
            }
            other => panic!("count {count}: {other:?}"),
        }
    }
    assert_eq!(enc.dmabuf_targets(320, 180, 8).unwrap().len(), 8);
}
