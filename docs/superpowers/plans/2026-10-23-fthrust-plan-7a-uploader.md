# FTHRust Plan 7a: Uploader Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user upload clips to Catbox, an SFTP server or their own server after accepting the uploader terms, with the reference's settings, consent dialogs, card and editor actions, toasts and history, through a separate `fthrust-uploader` binary so the main app links no network upload code.

**Architecture:** `fthrust-core::upload` holds everything without network code: the uploader settings file, the history file, consent versions, SFTP and URL validation, link rules and what may be uploaded. `crates/fthrust-uploader` (lib + bin) speaks the reference's one-shot contract (one JSON request on stdin, one JSON line on stdout) and does the network work with `reqwest` (rustls) and `russh`/`russh-sftp`, keeping SFTP passwords in the Secret Service through `keyring-core`. The UI's upload service queues uploads, spawns the binary per request, waits for clip readiness, offers compression for oversized Catbox uploads through the plan 6c export runner, and drives the settings widget, dialogs, cards, editor button and toasts.

**Tech Stack:** Rust 2024; `reqwest` 0.13 (`rustls`, `multipart`, `stream`), `russh` 0.64, `russh-sftp` 3, `keyring-core` 1 + `zbus-secret-service-keyring-store` 1, `tokio` (uploader only); egui 0.37 in the UI.

**Spec:** `docs/superpowers/specs/2026-10-22-fthrust-design.md` 5.9 and 6.5. Reference digest: `docs/superpowers/notes-uploader-reference.md` (section numbers below are its sections).

## Global Constraints

- Contract (digest 1): `fthrust-uploader` reads one JSON object from stdin, writes one compact JSON line to stdout, exit 0 iff `ok`. Actions `upload` (`path`, `allowed_roots`), `test_connection` (`config`), `sftp_save` (`config`), `sftp_forget` (`config`); anything else `Unsupported uploader action: <action or (missing)>`. Request `config` overrides the settings file for `test_connection`. The caller parses the last non-blank stdout line; no line gives stderr or `Uploader exited with code <n>`; bad JSON gives `Uploader returned an invalid response.` Timeouts: upload 600 s, test 75 s, sftp actions 30 s; on timeout the child is killed and the message is `The uploader did not finish in time.`
- No activation receipt, bundle hash or executable hash (spec 6.5: shipped inside the AppImage). Gating instead reads `~/.fthr/uploader/settings.json`: for `upload`/`test_connection`, `upload_enabled` must be true (`The Upload Extension is disabled in FTHR Clips.`) and `uploader_terms_accepted_version`/`uploader_privacy_accepted_version` must equal the current version (`Uploader Terms of Service must be accepted again.` / `Uploader Privacy Policy must be accepted again.`); Catbox needs `catbox_legal_accepted_version == "catbox-legal-2021-03-06"` (`Accept Catbox’s legal policies in FTHR Clips first.`, U+2019).
- Terms version: the reference texts describe a Windows package, extraction and Windows Credential Manager, so the Linux texts (in `crates/fthrust-core/src/upload/legal/`) rewrite only those sentences and the versions become `2026-10-23-v4` for both. Every other string in the texts is the reference's.
- Settings file keys and defaults, load and save rules: digest 3.1 exactly (sorted keys, indent 2, atomic replace, `sftp_password` never written, unknown keys kept, retired provider resets to disabled Catbox). History: digest 3.2 (keyed by resolved absolute path, successes only, compressed-copy aliases, legacy copy-once migration, rename moves the key). Legacy core-settings migration: digest 3.5.
- Providers: Catbox, custom and SFTP exactly as digest 2.1–2.3 (endpoint, boundary `FTHRClipBoundary8f61d4`, `User-Agent: FTHR-Clips/Desktop-Uploader`, field names, response parsing, Catbox 200 MiB limit message, SFTP validation and error strings, `.part` flow, permissions, URL building, host-key pin and known_hosts including `~/.fthr/uploader/known_hosts`, the remembered-host write, disabled weak algorithms). Messages naming Windows Credential Manager say `desktop keyring` instead.
- UI strings are the reference's (digest 4, 5, 6): settings widget labels, placeholders, statuses, dialogs, card and editor states, `CLIP UPLOADED` with `{name} · ORIGINAL|COMPRESSED COPY`, `UPLOADED ORIGINAL`/`UPLOADED COMPRESSED COPY`, Error 020–023 toasts with `RETRY NOW` / `OPEN UPLOAD SETTINGS`, `Compress before upload?`.
- Uploadable: completed videos only (`.mp4/.mkv/.avi`, not partial) whose readiness allows access; allowed roots are the clips root plus `imported_clip_folders`.
- Fixed reference defects (each gets a `CHANGES.md` line): test connection fails on 4xx/5xx (digest 7.1; custom: 2xx/3xx/405 ok, 401/403 `Your server rejected the authorization header (HTTP {code}).`, other ≥400 `Your server returned HTTP {code}.`; Catbox: < 500 ok, ≥ 500 `Catbox returned HTTP {code}.`); SFTP passwords saved on Linux (7.3); retries only on transient errors (connect, timeout, 429, 5xx) and never past 540 s of the request (7.5); the card `Uploading…` state clears on completion (7.9); the card context menu has the `Upload` entry the settings label promises (7.8); interval scans include imported folders (7.15); quitting kills an in-flight uploader (7.16); the editor Upload button follows the enabled state live (7.13).
- The main app (`fthrust-ui`, `fthrust-engine`) must not depend on `reqwest`, `russh` or `keyring-core` (spec 6.5); a test checks `cargo tree`.

## Review Focus

1. The uploader binary is missing, crashes, prints junk or hangs: the card returns to its upload state and an `UPLOAD FAILED` toast with a message appears; nothing stays "Uploading…". Test in Task 5.
2. A clip is renamed, deleted or moved while uploading or after: rename/delete are refused while uploading (`This clip is uploading. Try again when the upload finishes.`), rename carries the history entry, a missing file fails cleanly. Test in Tasks 5 and 6.
3. Secrets: the SFTP password never reaches `settings.json`, the log, a toast or an error message (auth failures are sanitised). Test in Tasks 1 and 4.
4. A hostile or broken server: a non-URL body, a huge body, an unknown host key, a changed host key, a folder that is a file — each gives its reference message and no credentials are sent to an unverified host. Test in Tasks 3 and 4.
5. The user toggles the uploader off, cancels a consent dialog or switches provider mid-way: settings stay consistent with what the widget shows and nothing uploads while disabled. Test in Task 7.

---

## File structure

```
crates/fthrust-core/src/upload/{mod,settings,history,validate}.rs   no-network model
crates/fthrust-core/src/upload/legal/{TERMS_OF_SERVICE,PRIVACY_POLICY}.txt
crates/fthrust-uploader/src/{lib,main,protocol,http,sftp,vault}.rs  the binary
crates/fthrust-ui/src/services/upload.rs       queue, process runner, readiness, compression, modes
crates/fthrust-ui/src/settings/upload.rs       the upload settings widget
crates/fthrust-ui/src/dialogs/upload_consent.rs  install and Catbox consent dialogs
crates/fthrust-ui/src/library/{card,page}.rs   link, badge, menu, states
crates/fthrust-ui/src/editor/sidebar.rs        Upload button
tools/uidrive/scenarios/upload.sh, tools/uidrive/upload_server.py
docs/uploads.md
```

### Task 1: Core upload model

**Files:** Create `crates/fthrust-core/src/upload/*`, `crates/fthrust-core/tests/upload_model.rs`; Modify `crates/fthrust-core/src/lib.rs`.

**Interfaces:**
- Produces: `upload::{TERMS_VERSION, PRIVACY_VERSION, CATBOX_LEGAL_VERSION, TERMS_TEXT, PRIVACY_TEXT, CATBOX_SUPPORT_URL, CATBOX_LIMIT_BYTES}`; `upload::settings::UploaderSettings` (a `serde_json::Map` wrapper: `load(&FthrPaths)`, `save(&FthrPaths) -> io::Result<()>`, `get/set`, `provider() -> Provider{Catbox, Custom, Sftp}`, `enabled()`, `consent_current()`, `catbox_consent_current()`, `accept_terms(now)`, `accept_catbox(now)`), `migrate_legacy_core_keys(&mut SettingsStore, &mut UploaderSettings) -> bool`; `upload::history::{History, Entry}` (`load(&FthrPaths)` cached by mtime by the caller, `get(path) -> Option<&Entry>`, `is_uploaded(path)`, `link(path) -> Option<Link{url, sftp: bool}>` with digest 6.3 validity, `record(path, entry)`, `alias_compressed(original, compressed, size)`, `rename(old, new)`, `save`, `migrate_legacy(&FthrPaths)`); `upload::validate::{sftp_config(&Map) -> Result<SftpConfig, String>, public_base_url(&Map) -> Result<String, String>, custom_url(&str) -> Result<String, String>, auth_header(&str) -> Result<Option<String>, String>, fingerprint(&str) -> Result<Option<String>, String>, vaultscope_paste(&str) -> Result<SftpLogin, String>, allowed_roots(clips_root, &SettingsStore) -> Vec<PathBuf>, is_uploadable(path) -> bool, under_roots(path, &[PathBuf]) -> bool}`.

- [ ] Step 1: Failing tests with digest values: `settings_defaults_and_load_rules` (every 3.1 default; `sftp_password` and `lustful_*` dropped; unknown key kept; `fthr` provider → disabled catbox; corrupt file → defaults), `settings_save_sorted_atomic_without_password`, `consent_versions` (`accept_terms` writes both versions and `uploader_accepted_at` UTC seconds; old v3 is not current), `legacy_core_keys_migrate` (custom when a server URL exists, else catbox; keys removed), `history_rules` (successes only, `is_uploaded`, expired non-favourite is not uploaded, `link` accepts https and a valid `sftp://` but not `sftp://user@h/` or `sftp://h/`, compressed alias fields, rename moves key, legacy copied only when the new file is absent), `sftp_validation_messages` (each row of the 2.2 table, public URL rule incl. trailing slash, fingerprint regex vs round-trip message), `custom_url_and_header` (`clips.example/upload` → `https://clips.example/upload`, `ftp://x` rejected, empty message, CR/LF header message), `vaultscope_paste` (valid JSON fills; wrong domain, bool port, missing password → the one 5.2 message), `uploadable_and_roots` (`.partial.mp4` refused, `.png` refused, root itself and children accepted, sibling with shared prefix refused).
- [ ] Step 2: FAIL. Step 3: Implement; write the two legal texts (reference text with the Windows/package/extraction/Credential Manager sentences rewritten for a bundled Linux component and the desktop keyring). Step 4: PASS; clippy.
- [ ] Step 5: Commit `feat(core): uploader settings, history and validation`.

### Task 2: Uploader binary, contract and gating

**Files:** Create `crates/fthrust-uploader/{Cargo.toml, src/lib.rs, src/main.rs, src/protocol.rs}`, `crates/fthrust-uploader/tests/contract.rs`; Modify workspace `Cargo.toml` (deps), `deny.toml` if a licence is new.

**Interfaces:**
- Consumes: Task 1.
- Produces: `fthrust_uploader::Env{paths: FthrPaths, catbox_url: String, retry_delays: Vec<Duration>, deadline: Duration, vault: Box<dyn Vault>}` (`Env::from_process()` reads `HOME`, and hidden `FTHRUST_UPLOADER_CATBOX_URL` / `FTHRUST_UPLOADER_NO_RETRY_DELAY` for tests and the bench); `fthrust_uploader::handle(&Env, &serde_json::Value) -> Response{ok, message, upload_info: Option<Value>}`; the binary wraps `handle` in a tokio runtime, catches panics as `{ok:false}`, writes the line and exits.

- [ ] Step 1: Failing tests: `unknown_and_missing_action`, `retired_actions_rejected` (`register`, `login`, `account_status`, `logout`), `disabled_refuses_network_actions` (upload and test; `sftp_save` still validates), `old_terms_refused` (v3 → terms message; then privacy message), `catbox_needs_consent`, `custom_and_sftp_need_no_provider_consent`, `retired_provider_unsupported` (`lustful`), `upload_outside_roots_refused` (`The uploader may only read approved clip folders.`), `missing_clip` (`The selected clip does not exist.`), `binary_round_trip` (spawn `CARGO_BIN_EXE_fthrust-uploader` with `HOME` = temp: one line on stdout, exit code 1 on refusal, 0 never for a refusal; garbage stdin → `{ok:false}` line).
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS; clippy; `cargo deny check`.
- [ ] Step 5: Commit `feat(uploader): one-shot contract and consent gating`.

### Task 3: Catbox and custom server

**Files:** Create `crates/fthrust-uploader/src/http.rs`, `crates/fthrust-uploader/tests/http.rs` (a std `TcpListener` test server that records the request and replies as told).

**Interfaces:**
- Produces: `http::upload_catbox(&Env, &UploaderSettings, &Path) -> Result<Value, String>`, `http::upload_custom(...)`, `http::test_catbox(&Env)`, `http::test_custom(&UploaderSettings)`; the shared wrapper `upload_with_retry` (digest 2.0 steps 1–7 with the transient-only rule) used by Task 4 too (SFTP: one attempt).

- [ ] Step 1: Failing tests: `catbox_multipart_fields` (boundary, `reqtype=fileupload`, `userhash` only when set, file field `fileToUpload`, filename `"` and `\` → `_`, content type `video/mp4`, User-Agent, no Authorization; url from body; history entry `{status, provider, uploaded_at, url, raw_url, favorite:false}`), `catbox_non_url_body_is_error` (body text is the message), `catbox_size_limit_message` (sparse 201 MiB file, no request made), `custom_field_header_and_json_url` (field `clip`, no extra fields, verbatim `Authorization`, 201 + `{"url":…}`; `raw_url` fallback; bare URL body; empty 2xx → `Upload complete.` with empty url), `custom_http_allowed`, `retry_transient_only` (503 then 200 → two requests and success; 400 → one request), `retry_never_past_deadline`, `auto_delete_after_success` (and the `Uploaded, but local delete failed:` message when the file cannot be removed), `test_connection_statuses` (custom 200/405 ok `Your server reachable (HTTP n).`; 401 rejected message; 404 `Your server returned HTTP 404.`; Catbox 412 ok, 502 error; refused connection `Could not reach your server: …`).
- [ ] Step 2: FAIL. Step 3: Implement with streamed multipart bodies (1 MiB chunks, explicit length), 120 s read timeout. Step 4: PASS; clippy.
- [ ] Step 5: Commit `feat(uploader): Catbox and custom server uploads`.

### Task 4: SFTP and the keyring

**Files:** Create `crates/fthrust-uploader/src/sftp.rs`, `crates/fthrust-uploader/src/vault.rs`, `crates/fthrust-uploader/tests/sftp.rs` (an in-process `russh` server with a `russh-sftp` handler over a temp directory, password auth, a fixed ed25519 host key).

**Interfaces:**
- Produces: `vault::Vault` trait (`get/set/delete(service, user)`), `vault::SecretService` (keyring-core + zbus store; any store error maps to the digest message with `desktop keyring`), `vault::Memory` for tests; `vault::service_name(host, port, user)` = `"FTHR Clips/SFTP/" + sha256("{host}\0{port}\0{user}")`; `sftp::test(&Env, &Map)`, `sftp::upload(&Env, &Map, &Path)`, `sftp::save(&Env, &Map)`, `sftp::forget(&Env, &Map)`.

- [ ] Step 1: Failing tests: `pin_mismatch_never_authenticates` (server sees no auth attempt), `unknown_host_without_pin_refused`, `known_hosts_trusts` (key in a temp `~/.ssh/known_hosts`), `verified_pin_is_remembered` (`[127.0.0.1]:<port>` line added, other lines kept), `auth_failure_sanitised` (password absent from the message, one attempt), `test_checks_folder_without_writing` (exact success text; a file as folder → `The configured remote folder is not a directory.`), `upload_part_rename_and_urls` (remote name `fthr-<32 hex>.mp4`, no `.part` left, mode 0600 private, 0644 with public links, `public_url` = base + basename, `sftp_url` form), `failed_rename_cleans_part_and_keeps_clip`, `unsafe_public_url_rejected_before_network`, `password_from_vault_scoped_by_host_port_user` (save then upload without a transient password; another user gets `Enter and save an SFTP password first.`), `save_requires_password_text` and `forget_removes`.
- [ ] Step 2: FAIL. Step 3: Implement (russh client config: 8 s timeouts, preferred lists without the digest 2.2 weak algorithms). Step 4: PASS; clippy.
- [ ] Step 5: Commit `feat(uploader): SFTP uploads with host-key pinning and keyring passwords`.

### Task 5: UI upload service

**Files:** Create `crates/fthrust-ui/src/services/upload.rs`, `crates/fthrust-ui/tests/upload_service.rs` (a fake uploader shell script selected through `FTHRUST_UPLOADER`); Modify `services/mod.rs`, `state.rs`, `services/save.rs` (immediate mode).

**Interfaces:**
- Consumes: Task 1; `Request::Upload(PathBuf)` and `state.upload_requested` (plan 5); the export runner and share size presets (plan 6c) for the compressed copy; `error_bar.push_error`; capture card `CardKind::{Upload, UploadFailed}`.
- Produces: `UploadService` (`new(paths)`, `enqueue(state, path)`, `enqueue_compressed(state, path)`, `tick(state, now)`, `notify_saved(state, path)`, `is_busy(path)`, `shutdown()`), `uploader_exe() -> Option<PathBuf>` (`FTHRUST_UPLOADER`, else beside `current_exe`), `run_request(exe, &Value, timeout) -> Response` (kills on timeout), `state.upload_status: HashMap<PathBuf, String>`, `state.upload_history` (reloaded on mtime change), `state.uploader_settings`.

- [ ] Step 1: Failing tests: `runner_parses_last_line_and_failures` (junk line then JSON; no output → stderr; bad JSON; exit without output → `Uploader exited with code 3`; hang → killed with the timeout message within the test's short timeout), `queue_dedups_and_waits_for_readiness` (finalizing clip waits; finalization failed → `Clip finalization failed; upload was not started`), `success_updates_card_and_toasts` (`CLIP UPLOADED` + `{name} · ORIGINAL`, status bar `UPLOADED ORIGINAL`, status cleared, history reloaded), `failure_toast_with_retry` (Error 020 with `RETRY NOW` re-queues; empty message → `The upload service did not provide a failure reason.`; status cleared), `disabled_does_nothing`, `oversized_catbox_asks_then_compresses` (question text from digest 6.4; auto-compress setting skips the question; compressed copy in `Shared/<stem>_catbox_upload_<HH-MM-SS>.mp4`, alias written, `UPLOADED COMPRESSED COPY`), `immediate_mode_on_save`, `interval_scan_includes_imports_and_skips_fresh_files`, `shutdown_kills_child`, `missing_binary_message` (`The uploader component is missing from this installation.`).
- [ ] Step 2: FAIL. Step 3: Implement (one worker thread, one upload at a time, results through the services event channel). Step 4: PASS; clippy.
- [ ] Step 5: Commit `feat(ui): upload queue, uploader process and feedback`.

### Task 6: Cards, context menu and editor button

**Files:** Modify `library/card.rs`, `library/page.rs`, `editor/sidebar.rs`, `editor/mod.rs`; Test `crates/fthrust-ui/tests/clip_card.rs`, `crates/fthrust-ui/tests/upload_ui.rs`.

**Interfaces:**
- Consumes: Task 5 state.

- [ ] Step 1: Failing tests: `card_states` (no link + enabled → `Upload clip`; disabled → tooltip `Enable the uploader in Settings to upload this clip`; uploading → disabled with overlay `Uploading…`; https link → `Copy link`, click copies and shows `Link copied`; sftp link → `Copy SFTP location` with its tooltip and `SFTP location copied`; badge `Uploaded`), `card_context_menu` (items in order `Open in editor`, `Rename`, `Show in file manager`, `Copy path`, `Upload` (enabled, video, no link), `Open uploaded link`/`Open SFTP location`, `Delete clip`), `rename_and_delete_blocked_while_uploading`, `rename_moves_history`, `editor_upload_button` (hidden while disabled, appears when enabled without reopening; click → `Queued` then `Upload` after 2500 ms; disabled path → `Upload off` + `UPLOAD NOT CONFIGURED` with `OPEN UPLOAD SETTINGS`).
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS; fidelity suite unchanged; clippy.
- [ ] Step 5: Commit `feat(ui): upload actions on cards, context menu and editor`.

### Task 7: Upload settings widget and consent dialogs

**Files:** Create `crates/fthrust-ui/src/settings/upload.rs`, `crates/fthrust-ui/src/dialogs/upload_consent.rs`, `crates/fthrust-ui/tests/upload_settings.rs`; Modify `settings/general.rs` (replace the plan 4 stub), `dialogs/mod.rs`.

**Interfaces:**
- Consumes: Tasks 1 and 5 (`run_request` for test/save/forget on a thread).

- [ ] Step 1: Failing tests: `enable_shows_consent_then_catbox` (dialog `Enable FTHR Upload Extension`, both checkboxes gate `ACCEPT & ENABLE UPLOADER`; accept writes the versions and enables; then `Connect Catbox`; cancelling it disables), `cancel_install_unticks`, `body_only_while_enabled`, `provider_combo_order_and_panels` (`Catbox, VaultScope, Your server, SFTP`; panels per digest 5; support button only for Catbox; `OPEN CATBOX`/`OPEN VAULTSCOPE`), `switch_to_catbox_without_consent_reverts_on_cancel`, `save_writes_fields` (custom URL normalised into the field; VaultScope stored as `sftp` + `sftp_provider_name`; interval value fallback 5; `Settings saved` then `Not tested` after 3 s; password cleared and absent from the file), `sftp_save_failure_saves_nothing`, `test_connection_flow` (`Enter a server URL first`; `Testing…`; ok green, error red + Error 022 toast `OPEN UPLOAD SETTINGS`; empty → `The upload provider did not return a reason.`), `forget_password_status`, `vaultscope_paste` (fills fields, status text; bad JSON → warning title `VaultScope login`), `interval_controls_follow_radio`, `note_mentions_keyring`.
- [ ] Step 2: FAIL. Step 3: Implement. Step 4: PASS; the settings fidelity check (uploader disabled) still matches; clippy.
- [ ] Step 5: Commit `feat(ui): upload settings and consent dialogs`.

### Task 8: Bench, dependency gate, docs

**Files:** Create `tools/uidrive/scenarios/upload.sh`, `tools/uidrive/upload_server.py` (a POST receiver answering `{"url": …}`), `tools/check-upload-isolation.sh`, `docs/uploads.md`; Modify `.github/workflows/ci.yml` (build the uploader, run the scenario and the isolation check), `CHANGES.md`, the roadmap.

- [ ] Step 1: The isolation script fails if `cargo tree -p fthrust-ui -p fthrust-engine -e normal` lists `reqwest`, `russh` or `keyring-core`; run it (PASS expected) and once against a deliberately added dep to see it FAIL, then revert.
- [ ] Step 2: Bench scenario: settings with the uploader enabled, consent current, custom provider at the local server; click a card's upload button; expect the server to receive the clip, the history entry, the card marker `Copy link`. Run: `tools/uidrive/bench.sh run upload` → PASS.
- [ ] Step 3: Docs and `CHANGES.md` lines for each fixed defect; roadmap row 7a shipped.
- [ ] Step 4: Commit `feat: upload bench scenario, isolation gate; docs: uploads`.
