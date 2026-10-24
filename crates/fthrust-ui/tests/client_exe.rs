//! The executable compositor hotkey binds run: stable across AppImage launches.

use fthrust_ui::hotkey_client_exe;
use std::path::Path;

#[test]
fn appimage_path_wins_over_the_per_launch_mount() {
    let got = hotkey_client_exe(
        Some("/home/u/Apps/FTHRust-2.0.0-x86_64.AppImage".as_ref()),
        Some(Path::new("/tmp/.mount_FTHRabc123/usr/bin/fthrust")),
    );
    assert_eq!(got, "/home/u/Apps/FTHRust-2.0.0-x86_64.AppImage");
}

#[test]
fn current_exe_used_outside_an_appimage() {
    let got = hotkey_client_exe(None, Some(Path::new("/opt/fthrust/fthrust")));
    assert_eq!(got, "/opt/fthrust/fthrust");
}

#[test]
fn empty_appimage_variable_is_ignored() {
    let got = hotkey_client_exe(Some("".as_ref()), Some(Path::new("/opt/fthrust/fthrust")));
    assert_eq!(got, "/opt/fthrust/fthrust");
}

#[test]
fn falls_back_to_the_bare_name() {
    assert_eq!(hotkey_client_exe(None, None), "fthrust");
}
