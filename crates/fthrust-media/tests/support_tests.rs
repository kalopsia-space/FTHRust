mod support;

#[test]
fn synthetic_frames_differ() {
    let a = support::synthetic_bgra(64, 32, 0);
    let b = support::synthetic_bgra(64, 32, 1);
    assert_eq!(a.len(), 64 * 32 * 4);
    assert_ne!(a, b);
    assert!(a.iter().skip(3).step_by(4).all(|&alpha| alpha == 0xff));
}

#[test]
fn init_is_idempotent() {
    fthrust_media::init().unwrap();
    fthrust_media::init().unwrap();
}
