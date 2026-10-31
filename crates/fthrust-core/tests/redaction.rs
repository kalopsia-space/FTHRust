//! Credentials the diagnostics redaction used to let through.

use fthrust_core::diagnostics::redact::{redact, redact_json};
use serde_json::json;

fn check(cases: &[(&str, &str)]) {
    for (input, expected) in cases {
        assert_eq!(redact(input), *expected, "input {input:?}");
    }
}

#[test]
fn secrets_with_a_prefix_are_redacted() {
    check(&[
        (
            "sftp_password=hunter2 host=example.org",
            "sftp_password=<redacted> host=example.org",
        ),
        (
            "client_secret=abc123&grant_type=refresh",
            "client_secret=<redacted>&grant_type=refresh",
        ),
        (
            "upload_token=tok_12345 next",
            "upload_token=<redacted> next",
        ),
        (
            r#"{"sftp_password": "hunter2", "host": "h"}"#,
            r#"{"sftp_password": <redacted>, "host": "h"}"#,
        ),
        ("clientSecret: s3cr3t", "clientSecret: <redacted>"),
        ("my.api_key=k", "my.api_key=<redacted>"),
        ("oauth=xyz", "oauth=<redacted>"),
        ("catbox_userhash=0123456789", "catbox_userhash=<redacted>"),
        ("userhash: abcdef0123", "userhash: <redacted>"),
    ]);
}

#[test]
fn the_original_cases_still_hold() {
    check(&[
        ("password=hunter2 user=bob", "password=<redacted> user=bob"),
        ("token=abcdef123456", "token=<redacted>"),
        ("api_key: \"xyz\"", "api_key: <redacted>"),
        ("secret=1", "secret=<redacted>"),
        ("X-Auth-Header: foo", "X-Auth-Header: foo"),
        (
            "author=Jo auth_required=true",
            "author=Jo auth_required=true",
        ),
        ("tokens=3 passwords=none", "tokens=3 passwords=none"),
        ("plain text 123", "plain text 123"),
    ]);
}

#[test]
fn a_userhash_in_running_text_is_redacted() {
    check(&[
        (
            "uploading with userhash 0123456789abcdef to catbox",
            "uploading with userhash <redacted> to catbox",
        ),
        (
            "the Catbox userhash is 0123456789abcdef0123456789abcdef.",
            "the Catbox userhash is <redacted>.",
        ),
        // No hash, nothing to hide.
        ("Invalid userhash supplied", "Invalid userhash supplied"),
        ("userhash", "userhash"),
    ]);
}

#[test]
fn every_pair_of_a_cookie_header_is_redacted() {
    check(&[
        ("Cookie: a=1; b=2; session=abc123", "Cookie: <redacted>"),
        (
            "request headers\nCookie: a=1; b=2; session=abc123\nHost: example.org",
            "request headers\nCookie: <redacted>\nHost: example.org",
        ),
        (
            "Set-Cookie: id=7; Path=/; HttpOnly; Secure",
            "Set-Cookie: <redacted>",
        ),
        (
            r#"{"cookie": "a=1; b=2", "accept": "*/*"}"#,
            r#"{"cookie: <redacted>, "accept": "*/*"}"#,
        ),
        ("cookie=session=abc; other=def", "cookie: <redacted>"),
    ]);
    // The other headers keep their rules.
    check(&[(
        "Authorization: Bearer abc.def",
        "Authorization: <redacted> <redacted>",
    )]);
}

#[test]
fn json_keys_that_end_in_a_secret_word_lose_their_value() {
    let mut v = json!({
        "sftp_password": "hunter2",
        "client_secret": 12345,
        "upload_token": {"nested": "x"},
        "clientSecret": "camel",
        "X-Api-Key": "k",
        "Set-Cookie": "a=1; b=2",
        "catbox_userhash": "0123456789",
        "upload_auth_header": "Bearer z",
        "oauth": "o",
        "keep": "visible",
        "auth_required": true,
        "tokens": 3,
        "note": "password=hunter2",
        "list": [{"password": "p", "ok": 1}],
    });
    redact_json(&mut v);
    for key in [
        "sftp_password",
        "client_secret",
        "upload_token",
        "clientSecret",
        "X-Api-Key",
        "Set-Cookie",
        "catbox_userhash",
        "upload_auth_header",
        "oauth",
    ] {
        assert_eq!(v[key], "<redacted>", "{key}");
    }
    assert_eq!(v["keep"], "visible");
    assert_eq!(v["auth_required"], true);
    assert_eq!(v["tokens"], 3);
    assert_eq!(
        v["note"], "password=<redacted>",
        "string values are scrubbed too"
    );
    assert_eq!(v["list"][0]["password"], "<redacted>");
    assert_eq!(v["list"][0]["ok"], 1);
}
