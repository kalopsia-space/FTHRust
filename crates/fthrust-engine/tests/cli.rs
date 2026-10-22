use std::path::PathBuf;

use fthrust_engine::cli::{CliMode, classify};
use fthrust_engine::devices::{Endpoint, enumerate_sinks, error_json, listing_json};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn classify_rules() {
    assert_eq!(
        classify(&args(&["--list-playback-devices"])),
        CliMode::ListPlaybackDevices
    );
    assert_eq!(
        classify(&args(&["--list-microphones"])),
        CliMode::ListMicrophones
    );
    assert_eq!(classify(&args(&["--version"])), CliMode::Version);
    assert_eq!(
        classify(&args(&["--socket", "/tmp/x.sock"])),
        CliMode::Serve {
            socket: PathBuf::from("/tmp/x.sock")
        }
    );
    match classify(&args(&["--socket", "/tmp/x.sock", "--bogus"])) {
        CliMode::UsageError(msg) => assert!(msg.contains("--bogus"), "{msg}"),
        other => panic!("{other:?}"),
    }
    match classify(&args(&["--list-playback-devices", "extra"])) {
        CliMode::UsageError(msg) => assert!(msg.contains("only argument"), "{msg}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        classify(&args(&["--socket"])),
        CliMode::UsageError(_)
    ));
    assert!(
        matches!(classify(&args(&["-"])), CliMode::UsageError(_)),
        "a lone dash is a value, not a flag: still unusable here"
    );
    assert!(
        matches!(classify(&args(&["-1"])), CliMode::UsageError(m) if !m.contains("unknown flag"))
    );
    match classify(&args(&[])) {
        CliMode::UsageError(msg) => assert!(msg.contains("--socket is required"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

fn ep(id: &str, name: &str, default: bool) -> Endpoint {
    Endpoint {
        endpoint_id: id.into(),
        display_name: name.into(),
        is_default: default,
    }
}

#[test]
fn json_shape() {
    assert_eq!(
        listing_json(&[]),
        r#"{"schema_version":1,"microphones":[]}"#
    );
    let json = listing_json(&[
        ep("alsa_output.pci", "Built-in Audio", true),
        ep("hdmi", "", false),
    ]);
    assert_eq!(
        json,
        r#"{"schema_version":1,"microphones":[{"endpoint_id":"alsa_output.pci","display_name":"Built-in Audio","device_state":1,"is_default":true},{"endpoint_id":"hdmi","display_name":"hdmi","device_state":1,"is_default":false}]}"#
    );
    let json = listing_json(&[ep("q\"b\\s", "tab\tnl\ncr\rbell\x07 ünï", false)]);
    assert!(json.contains(r#""endpoint_id":"q\"b\\s""#), "{json}");
    assert!(
        json.contains(r#""display_name":"tab\tnl\ncr\rbell\u0007 ünï""#),
        "{json}"
    );
    assert!(!json.contains('\n'));
    assert_eq!(
        error_json("boom \"x\""),
        r#"{"schema_version":1,"error":"boom \"x\""}"#
    );
}

#[test]
fn live_enumeration_lists_default_sink() {
    match enumerate_sinks() {
        Ok(sinks) => {
            assert!(!sinks.is_empty());
            assert_eq!(
                sinks.iter().filter(|s| s.is_default).count(),
                1,
                "{sinks:?}"
            );
            assert!(
                sinks
                    .iter()
                    .all(|s| !s.endpoint_id.is_empty() && !s.display_name.is_empty())
            );
        }
        Err(e) => eprintln!("skipping: no PipeWire ({e})"),
    }
}
