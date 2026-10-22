use std::io::Read;

#[test]
fn binary_prints_version() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_fthrust"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        format!("fthrust {}", fthrust_ui::VERSION)
    );
}

#[test]
fn assets_match_manifest_and_reference() {
    use sha2::{Digest, Sha256};
    let assets = fthrust_ui::assets_dir();
    let manifest = std::fs::read_to_string(assets.join("manifest.sha256")).unwrap();
    let reference = assets.join("../reference/FTHR_UI/assets");
    let mut checked = 0;
    for line in manifest.lines() {
        let (sum, rel) = line.split_once("  ").unwrap();
        let rel = rel.trim_start_matches("./");
        let mut bytes = Vec::new();
        std::fs::File::open(assets.join(rel))
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(actual, sum, "{rel} differs from the manifest");
        if reference.is_dir() {
            let mut ref_bytes = Vec::new();
            std::fs::File::open(reference.join(rel))
                .unwrap()
                .read_to_end(&mut ref_bytes)
                .unwrap();
            assert_eq!(ref_bytes, bytes, "{rel} differs from the reference");
        }
        checked += 1;
    }
    assert!(checked >= 50, "{checked} assets");
    if !reference.is_dir() {
        eprintln!("reference/ absent: manifest checked only");
    }
}
