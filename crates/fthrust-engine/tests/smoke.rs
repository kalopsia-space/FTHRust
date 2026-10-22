#[test]
fn binary_prints_version() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_fthrust-engine"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        format!("fthrust-engine {}", fthrust_engine::VERSION)
    );
}
