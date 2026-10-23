# Uploader reference digest (Python app) for the Rust port

Source tree: `/home/kalopsia/fthrrst-clips/reference/`. Citations are `path:line` relative to it.
Abbreviations: `US` = `FTHR_Uploader/uploader_service.py`, `UR` = `FTHR_Uploader/upload_runtime.py`,
`SF` = `FTHR_Uploader/sftp_provider.py`, `UM` = `FTHR_UI/core/upload_manager.py`,
`USW` = `FTHR_UI/ui/upload_settings_widget.py`, `CG` = `FTHR_UI/ui/clip_grid.py`.

Everything below is what the code does, including its bugs. Section 7 lists the defects.

---------------------------------------------------------------------------------------------------

## 1. Process contract

### 1.1 Invocation (UM:526-560, US:133-149)

The core spawns the uploader once per request: a one-shot process, no daemon.

```
<receipt['executable']> --activation-receipt <path to activation.json>
```

- `UM:539-548`: `subprocess.run([...], input=json.dumps(request), text=True, capture_output=True, timeout=timeout, check=False, creationflags=CREATE_NO_WINDOW on win32)`.
- Timeouts (seconds, whole process): `upload` 600 (default, `UM:530`), `test_connection` 75 (`UM:930`), `sftp_save`/`sftp_forget` 30 (`UM:923`).
- `subprocess.TimeoutExpired` is not caught inside `_invoke_plugin`.
  - `_do_single_upload` catches it (`UM:820`) and uses `str(exc)` as the message.
  - `UploadManager.test_connection` and `sftp_credentials` do not catch it. The settings widget catches it for test (`USW:~694`) and for save/forget (see 5).
- `--activation-receipt` is a required argparse arg with `add_help=False` (US:135-136). A missing arg makes argparse exit 2 with its usage on stderr and nothing on stdout.

### 1.2 Request JSON (one object, whole of stdin)

Built in `UM:532-536`: `{"action": <str>, "activation_id": <receipt activation_id>, ...payload}`.

| action | extra fields | sender |
|---|---|---|
| `upload` | `path` (str), `allowed_roots` (list of str), `clips_root` (str, sent but never read by the uploader) | `UM:812-819` |
| `test_connection` | `config` (object) | `UM:929-930`. The widget builds config as `upload_provider` (`sftp` if UI choice is `vaultscope`), `catbox_userhash`, `upload_server_url`, `upload_auth_header` plus `_sftp_config()` (all `sftp_*` fields including `sftp_password` and `sftp_public_enabled`) (`USW:683-690`) |
| `sftp_save` | `config` (the same `_sftp_config()` dict) | `UM:921-923` |
| `sftp_forget` | `config` | same |

No other actions exist. Anything else raises `ValueError("Unsupported uploader action: <action or "(missing)">")` (US:130). The tests also assert `register`, `login`, `account_status` and `logout` are rejected (`tests/test_optional_uploader_runtime.py:117-119`).

For `upload`, the manager sends no `config`. Provider, credentials and options come from the settings file (see 3.1).
For `test_connection`, `_perform` does `runtime_settings = dict(settings); runtime_settings.update(request['config'])` (US:120-122), so config overrides the settings file.

### 1.3 Response (stdout, a single line)

`sys.stdout.write(json.dumps(response, separators=(',',':')) + '\n')` (US:147). The exit code is 0 if `response['ok']` is truthy, otherwise 1 (US:149).
During `_perform`, stdout is redirected to stderr (US:143), so library prints cannot corrupt the protocol.

Shapes:
- `{"ok":bool,"message":str}` for `test_connection`, `sftp_save`, `sftp_forget`, and every error.
- `{"ok":bool,"message":str,"upload_info":{...}}` for `upload` (US:129). On failure `upload_info` is `{}`.
- Any exception in validation or `_perform` becomes `{"ok":false,"message":str(exc)}` (US:145-146).

Core-side parsing (UM:549-560):
- Take non-blank stdout lines and parse the LAST one as JSON.
- No lines: `{ok:false, message: stderr.strip() or "Uploader exited with code <n>"}`.
- Invalid JSON or non-object: `{ok:false, message:"Uploader returned an invalid response."}`.
- The core uses only `ok` and `message`. `upload_info` is ignored; the UI reads upload state from the history file (see 3.2).

There is no progress reporting. Retry progress goes to stderr only (UR:233-234: `[Uploader] Attempt N failed; retrying in Ns: <exc>`), and the core discards stderr unless stdout is empty.

### 1.4 Activation and authorization checks, in order (US:41-65, run before any action)

All errors are `PermissionError` and come back as `{"ok":false,"message":...}`.
1. The receipt file does not exist: `The Upload Extension has not been installed in FTHR Clips.`
2. `plugin_id != "com.fthrclips.uploader"`: `Uploader activation belongs to a different package.`
3. `plugin_version != "1.4.0"`: `Uploader activation version does not match.`
4. `terms_version != "2026-10-15-v3"`: `Uploader Terms of Service must be accepted again.`
5. `privacy_version != "2026-10-15-v3"`: `Uploader Privacy Policy must be accepted again.`
6. `request.activation_id != receipt.activation_id`: `Uploader activation token is invalid.`
7. `sha256(sys.executable)` (the uploader's own frozen exe) is empty or differs from `receipt.executable_sha256`: `Uploader executable integrity check failed.`
8. Settings are read from `receipt.settings_file`. A missing file means `{}`.
9. For actions in `{test_connection, upload}` only: `settings.upload_enabled` falsy gives `The Upload Extension is disabled in FTHR Clips.`
   - `sftp_save` and `sftp_forget` are NOT gated on `upload_enabled`.

Constants (US:17-21): `PLUGIN_ID='com.fthrclips.uploader'`, `PLUGIN_VERSION='1.4.0'`, `TERMS_VERSION='2026-10-15-v3'`, `PRIVACY_VERSION='2026-10-15-v3'`, `CATBOX_LEGAL_VERSION='catbox-legal-2021-03-06'`.
The same constants live in `FTHR_UI/core/uploader_bundle_manifest.py:3-7`, plus `EXPECTED_UPLOADER_BUNDLE_SHA256='5d2e77c4…05a3'`.

### 1.5 `_perform` (US:107-130)

- `sftp_save` / `sftp_forget`:
  1. `config = dict(request.config or {})`.
  2. `configuration(config)` validates the whole SFTP config. Failures are SftpError messages (see 2.2).
  3. `forget`: `forget_password(config)`. `save`: `save_password(config, config['sftp_password'])` only if a password is supplied; otherwise this is validation only.
  4. Returns `{"ok":true,"message":"SFTP credentials updated."}`.
- Other actions:
  1. `provider = config.upload_provider` (request config first, then settings, default `catbox`), lowercased. `own_server` and `your_server` map to `custom` (US:68-74).
  2. `_require_provider_consent(provider, settings, request)` (US:77-87):
     - `catbox`: `settings.catbox_legal_accepted_version` must equal `catbox-legal-2021-03-06`, else `PermissionError("Accept Catbox’s legal policies in FTHR Clips first.")` (curly apostrophe U+2019).
     - `custom` and `sftp`: no consent.
     - Anything else: `ValueError("Unsupported upload provider.")`.
  3. Dispatch to `UploadRuntime`. `test_connection` returns `{ok,message}`; `upload` returns `{ok,message,upload_info}`.

### 1.6 Allowed-roots check (US:90-100), upload only

```
target = Path(request.path).expanduser().resolve(strict=True)   # FileNotFoundError if missing
if not target.is_file(): ValueError("The selected clip does not exist.")
roots = [Path(v).expanduser().resolve(strict=False) for v in request.allowed_roots]   # default []
if not roots or not any(target == root or root in target.parents for root in roots):
    PermissionError("The uploader may only read approved clip folders.")
```

The check runs after the settings, enabled and consent checks and before the network call.
A missing path surfaces Python's own `FileNotFoundError` text, e.g. `[Errno 2] No such file or directory: '...'`.

Core's approved roots (UM:509-524): `[clips_directory()] + settings_manager.get('imported_clip_folders', [])`, each `expanduser().resolve(strict=False)`.
Defaults: the `imported_clip_folders` key defaults to `[]` in `~/.fthr/settings.json` (`core/settings_manager.py:225`).

---------------------------------------------------------------------------------------------------

## 2. Providers

Provider ids accepted by the runtime: `catbox`, `custom` (aliases `own_server`, `your_server`), `sftp`. Anything else raises `ValueError('Unsupported upload provider.')` (UR:254-260). Tests also cover `lustful` and `fthr`, which are retired.
The UI additionally offers `vaultscope`, which is stored as `upload_provider='sftp'` plus `sftp_provider_name='VaultScope'` (USW:631-632, 641).

### 2.0 Common upload wrapper (`UploadRuntime.upload`, UR:189-236)

1. Not a file: `(False, "File no longer exists.", {})`.
2. Size limit, `catbox` only (200 MiB, `_PROVIDER_LIMIT_BYTES` UR:177-179). Message:
   `"{provider.title()} accepts files up to {limit//(1024*1024)} MB; this file is {actual/(1024*1024):.1f} MB. Use Automatically compress in FTHR Clips."`
   Stat failure: `"File size could not be checked: {exc}"`.
3. Retry: `_RETRY_DELAYS=(5,15,45)`. Catbox and custom make 4 attempts, with sleeps of 5, 15 and 45 seconds, and retry on ANY exception (including 4xx/validation errors like an invalid custom URL). SFTP makes exactly 1 attempt (`delays=(None,)`, UR:205-206).
4. On success an entry is written to history (`_record_success`, see 3.2). Entry: `{'status':'ok','provider':provider,'uploaded_at':UTC ISO seconds, **result}`.
5. If `upload_auto_delete` is true, `path.unlink()`. A failure returns `(True, "Uploaded, but local delete failed: {exc}", entry)`.
6. Success message is `str(result.get('url') or 'Upload complete.')`.
7. Final failure is `(False, str(last_exc) or ..., {})`. The default is `"Upload failed."`.

### 2.1 Catbox

- Endpoint: `https://catbox.moe/user/api.php` (UR:173). HTTPS is enforced (`require_https=True` default).
- `POST multipart/form-data` (`_multipart_post`, UR:348-403), boundary `FTHRClipBoundary8f61d4`, `User-Agent: FTHR-Clips/Desktop-Uploader`.
  - Fields: `reqtype=fileupload`, then `userhash=<value>` only if `catbox_userhash.strip()` is non-empty.
  - File part: field name `fileToUpload`, `filename="<name with " and \ replaced by _>"`, `Content-Type` from `mimetypes.guess_type` or `application/octet-stream`.
  - No `Authorization` header (`headers={}`).
  - Streamed in 1 MiB chunks. `Content-Length` is computed explicitly. Socket timeout is 120 s (`http.client` connection timeout).
- Response parsing (UR:268-278): `text = body.decode(utf-8, replace).strip()`. Success iff `200 <= status < 300` AND `text.startswith('https://')`. Result `{'url': text, 'raw_url': text, 'favorite': False}`. Otherwise `RuntimeError(text or "Catbox returned HTTP {status}.")`. The raw body text is surfaced as the error message.
- Consent required (1.5). Test connection: see 5.3 and 7.

### 2.2 SFTP (`SF`)

Dependency: paramiko. Password authentication only. There is no key-file auth: `allow_agent=False`, `look_for_keys=False` (SF:186-189).

Config validation, `configuration()` (SF:43-80). It first calls `public_base_url(settings)` (SF:27-40), so public-URL errors win.

| field | rule | error |
|---|---|---|
| `sftp_host` | strip, `rstrip('.')`, lowercase. Valid IP, or else IDNA-encoded ASCII with every dot-label matching `[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?`, total length ≤253 and non-empty | IDNA failure: `Enter a valid SFTP host domain or IP address.`; label/length failure: `Enter a host domain or IP address without a URL or path.` |
| `sftp_port` | `int(...)`, default 22, 1-65535 | `SFTP port must be between 1 and 65535.` (both for non-int and out of range) |
| `sftp_username` | stripped, non-empty, no char with ord<32 | `Enter an SFTP username without control characters.` |
| `sftp_directory` | `str(... or '.').strip()`, non-empty, no ord<32, no backslash | `Enter a remote folder using forward slashes.` |
| `sftp_fingerprint` | blank means no pin. Else must fullmatch `SHA256:[A-Za-z0-9+/]{43}`, and the base64 (+`=`, validated) must round-trip exactly | regex failure: `Enter the server SHA256 host-key fingerprint from its operator.`; decode/roundtrip failure: `Invalid SHA256 host-key fingerprint.` |

Returns `(host, port, username, folder, fingerprint)`.

Public URL mapping, `public_base_url()` (SF:27-40):
- `''` unless `sftp_public_enabled` is truthy.
- Otherwise the value must be https, with a hostname, no userinfo, no query, no fragment, a valid port, no chars ord<33 (this includes spaces), and no backslash.
- Failure message: `Enter a public HTTPS folder URL without credentials, query or fragment.`
- Returns `scheme://netloc + path.rstrip('/') + '/'`, so a trailing slash is always added.

Password storage and retrieval (SF:83-128):
- `_vault()`: if `sys.platform != 'win32'`, `SftpError('Saved SFTP passwords currently require Windows Credential Manager.')`. Otherwise `keyring.backends.Windows.WinVaultKeyring()`, with no plaintext fallback.
- Credential key: service `"FTHR Clips/SFTP/" + sha256(f"{host}\0{port}\0{username}")` hex, username = the SFTP username.
- `save_password`: empty or non-str gives `Enter a password to save.`; vault failure gives `Could not save the password in Windows Credential Manager.`
- `forget_password`: delete if present; failure gives `Could not remove the saved SFTP password.`
- `_password`: a non-empty `settings['sftp_password']` (transient, from the request) is used as-is (not stripped). Otherwise it reads the vault. Read failure gives `Could not read Windows Credential Manager.`; empty gives `Enter and save an SFTP password first.`

Host-key handling, `connection()` (SF:172-225):
- **Pinned** (fingerprint non-blank): policy `_PinnedKey`. `actual = 'SHA256:' + b64(sha256(key.asbytes())).rstrip('=')`, compared with `hmac.compare_digest`. Mismatch gives `SFTP host-key mismatch. Connection stopped before sending credentials.` known_hosts files are NOT consulted when a pin is set.
- **Unpinned**: load every existing file of `known_hosts_paths()` = `~/.ssh/known_hosts` and `~/.fthr/uploader/known_hosts` via `client.load_system_host_keys(path)`. Policy `_UnknownHost` raises: `This SFTP host is not trusted yet. Enter its SHA256 fingerprint from the server operator and test the connection once, or add its verified key to your SSH known_hosts file.`
- After a successful pinned connection, authentication and the remote-folder check, `remember_verified_host` stores the server key in `~/.fthr/uploader/known_hosts`. The name is `host` if port is 22, otherwise `[host]:port`. It writes via a mkstemp file and `os.replace`, merging with existing keys. A save failure is swallowed, except for test_connection (`require_remembered_host=True`), which raises `Host verified, but its trusted key could not be saved.`
- Connect params: `timeout=8, banner_timeout=8, auth_timeout=8, channel_timeout=8`, SFTP channel timeout 20 s. Disabled algorithms: keys/pubkeys `ssh-rsa`, `ssh-dss`; kex `diffie-hellman-group1-sha1`, `group14-sha1`, `group-exchange-sha1`; ciphers `3des-cbc`, `blowfish-cbc`, `aes128-cbc`, `aes192-cbc`, `aes256-cbc`; macs `hmac-md5`, `hmac-md5-96`, `hmac-sha1`, `hmac-sha1-96`.
- After connect: `sftp.normalize(folder)`, then `stat` must be a directory, else `The configured remote folder is not a directory.`

Exception mapping:
- `SftpError`: passthrough.
- `AuthenticationException`: `SFTP authentication failed. Check the username and password.`
- `BadHostKeyException`: `SFTP host-key mismatch. Connection stopped before sending credentials. Verify the changed key with the server operator.`
- `InvalidHostKey` / `ValueError`: `Could not read trusted SSH host keys. Check your known_hosts files.`
- `SSHException` / `OSError` / `EOFError`: `SFTP connection or transfer failed. Check the server, folder and permissions.`

Upload (SF:233-268):
1. Remote name: `fthr-<secrets.token_hex(16)><suffix>`. The suffix is `path.suffix.lower()` if it matches `\.[a-z0-9]{1,10}`, else `.clip`. The remote path is `posixpath.join(normalized_folder, name)`. Local file names are never disclosed.
2. Write to `<remote>.part` opened with mode `'wx'`, then `chmod 0o600`, then stream 1 MiB blocks.
3. Verify the `.part` size equals the local size, else `Remote file size verification failed; the local clip was retained.`
4. `sftp.rename(part, remote)`, then verify the final size, else `Final remote size verification failed; the local clip was retained.`
5. If a public base URL is configured, `chmod 0o644` after verification.
6. Any failure removes the `.part` file (errors ignored) and re-raises.
7. URLs:
   - `authority = [host]` if the host contains `:`, else `host`.
   - `sftp_url = f"sftp://{authority}:{port}{quote(remote, safe='/')}"`.
   - `public_url = base_url + quote(basename(remote), safe='')` if a base URL is set, else `''`.
   - `url = public_url or sftp_url`.
8. Result: `{'url','raw_url'(both = url),'sftp_url','public_url','remote_path','favorite':False}`.

Note that the public URL is `base + basename` only. The remote folder is not appended. The base URL must correspond to exactly the remote folder (the settings note says so).

Success message of `test_connection`: `SFTP authenticated; host key verified and remote folder found. Write access is checked on upload.` There is no write test.

### 2.3 Custom server

- URL (`_custom_server_url`, UR:406-415): `str(upload_server_url).strip()`. If it is non-empty and does not start with http:// or https:// (case-insensitive), `https://` is prefixed. Empty gives `ValueError('Set a server URL before connecting to your server.')`. Must parse with scheme http/https and a hostname, else `Your server URL must use http:// or https://.` http is allowed for uploads (`require_https=False`).
- Header (`_custom_server_headers`, UR:418-422): the single header `Authorization: <upload_auth_header.strip()>`, only if non-empty. The value is sent verbatim, with no `Bearer ` added. CR or LF in it gives `The authorization header contains an invalid line break.`
- Request: `POST` multipart, same boundary and User-Agent as Catbox, NO extra form fields (`{}`), file field name **`clip`**, filename and content type as above (UR:281-291). Query string in the URL is preserved.
- Response (UR:293-315):
  - Status not 2xx: `RuntimeError(text or "Your server returned HTTP {status}.")`.
  - Otherwise `json.loads(text)`.
    - If a dict: `url = str(data.get('url') or data.get('raw_url') or '').strip()`.
    - Else if the text starts with `http://` or `https://`: `url = text`.
    - Else `''`.
  - Result: `{'url': url, 'raw_url': url, 'response': text[:4096], 'favorite': False}`.
  - A 2xx response with no recognizable URL is still a success. The message is `Upload complete.` and the history `url` is `''`.
  - Only top-level `url` / `raw_url` keys are read. There is no nested JSON path.
- Test connection: see 5.3.

---------------------------------------------------------------------------------------------------

## 3. Settings, history, secrets

### 3.1 `~/.fthr/uploader/settings.json` (UM:57, 64-87)

Written by `_atomic_json` (indent 2, `sort_keys=True`, tmp file then `os.replace`, UM:146-153). `save_settings` pops `sftp_password` before writing (UM:211).
Load (UM:223-237): defaults merged with the file. `sftp_password` and all `lustful_*` keys are dropped. If `upload_provider` is not in `{catbox, custom, own_server, your_server, sftp}`, then `upload_enabled=False` and `upload_provider='catbox'`. A parse error or OSError gives the defaults. Unknown keys such as `sftp_provider_name` are preserved.

| key | default |
|---|---|
| `upload_enabled` | `false` |
| `upload_provider` | `'catbox'` |
| `catbox_userhash` | `''` |
| `upload_server_url` | `''` |
| `upload_auth_header` | `''` |
| `sftp_host` | `''` |
| `sftp_port` | `22` |
| `sftp_username` | `''` |
| `sftp_directory` | `'.'` |
| `sftp_fingerprint` | `''` |
| `sftp_public_enabled` | `false` |
| `sftp_public_base_url` | `''` |
| `upload_mode` | `'manual'` (`immediate` \| `interval` \| `manual`) |
| `upload_interval_value` | `5` |
| `upload_interval_unit` | `'minutes'` (`minutes` \| `hours` \| `days`) |
| `upload_auto_compress` | `false` |
| `upload_auto_delete` | `false` |
| `uploader_terms_accepted_version` | `''` |
| `uploader_privacy_accepted_version` | `''` |
| `uploader_accepted_at` | `''` (UTC ISO seconds) |
| `catbox_legal_accepted_version` | `''` |
| `catbox_legal_accepted_at` | `''` |
| `sftp_provider_name` (not in defaults, set by the widget) | `'VaultScope'` or `''` |

The `catbox_userhash` and the custom `upload_auth_header` are stored in plaintext in this file. Only the SFTP password is kept out of it.

### 3.2 `~/.fthr/uploader/upload_history.json`

- Written by the uploader (UR:334-345, `indent=2`, not sorted, tmp + `os.replace`). It is a JSON object keyed by the ABSOLUTE local path string of the file passed in `request.path` (resolved path, UR via `US:100`). Only successes are recorded.
- Entry shape:
  ```
  {"status":"ok","provider":"catbox|custom|sftp","uploaded_at":"2026-10-23T12:00:00+00:00",
   "url":...,"raw_url":...,"favorite":false,
   custom: "response":"<first 4096 chars>",
   sftp:   "sftp_url","public_url","remote_path"}
  ```
- Written by the core on a compressed upload (UM:781-802): `history[original] = {**entry_of_compressed, "uploaded_copy":"compressed","uploaded_path":<compressed path>,"uploaded_size_bytes":int,"original_preserved":true}`.
- Core also renames keys on a clip rename (UM:947-953) and migrates the legacy `~/.fthr/upload_history.json` by copying it if the new file does not exist (UM:59, 498-500, on activation).
- Read: cached by mtime (UM:882-897). `get_upload_info(path)` returns the entry only if `status == 'ok'`. `is_uploaded(path)`: true, except when `expires_at` is a parseable ISO timestamp (`Z` accepted) in the past and `favorite` is false. Nothing in the uploader ever writes `expires_at`, so it is dead for the current providers.
- Not pruned, and failures are never written.

### 3.3 `~/.fthr/uploader/known_hosts`: see 2.2 (also reads `~/.ssh/known_hosts`).

### 3.4 Secrets

- SFTP password: only in the OS credential vault (Windows Credential Manager via keyring `WinVaultKeyring`). The core refuses `set('sftp_password', ...)` with `ValueError('SFTP passwords must be stored through the credential vault.')` (UM:205-206). It is sent over stdin only for save/test, and removed from the in-memory config after the test (`USW:~692`).
- Catbox userhash and custom auth header: in plaintext in `uploader/settings.json`.

### 3.5 `~/.fthr/settings.json` (core settings)

- No upload or consent keys exist in the default settings (`settings_manager.py` has only the sound-volume keys `sound_volume_upload_successful`/`sound_volume_upload_failed` = 100 at :215-216, notification-sound entries at `main.py:11451-11452` `('upload_successful','Upload successful')`, `('upload_failed','Upload failed')`, and `imported_clip_folders` at :225).
- On first activation `_migrate_legacy_state` (UM:480-500) moves these legacy keys out of the core settings: `upload_enabled, upload_server_url, upload_auth_header, upload_mode, upload_interval_value, upload_interval_unit, upload_auto_delete`. If any were present, the provider is set to `custom` when `upload_server_url` is non-empty, else `catbox`, and core settings are saved.

### 3.6 Activation state (Windows layout, UM:51-55, 403-414)

- Root `%LOCALAPPDATA%/FTHR Clips/plugins/uploader/`, receipt `activation.json`, install dir `<root>/<plugin_version>/`, entrypoint `FTHR Uploader.exe`, bundle `FTHR-Uploader.fthrplugin` (a zip).
- Zip layout: `manifest.json`, `TERMS_OF_SERVICE.txt`, `PRIVACY_POLICY.txt`, `payload/…`. The manifest has `plugin_id`, `plugin_version`, `terms_version`, `privacy_version`, `entrypoint`, `files[{path,sha256,size}]`.
- Receipt JSON fields: `plugin_id, plugin_version, terms_version, privacy_version, accepted_at, activation_id (secrets.token_urlsafe(32)), bundle_sha256, executable, executable_sha256, settings_file`.
- `is_plugin_installed` (UM:314-325) requires: receipt `plugin_id` and `plugin_version` match; `bundle_sha256 == EXPECTED_UPLOADER_BUNDLE_SHA256`; the executable exists with a matching sha256; and the terms and privacy versions are current.
- Bundle located by `UM:241-254` (`_MEIPASS/plugin-packages`, then next to the frozen exe, then repo `plugin-packages/`).
- Errors from `_inspect_bundle` / `_activate_bundle`: `The dormant FTHR Upload Extension bundle is missing. Reinstall FTHR Clips.`; `This development build is not bound to a verified FTHR Upload Extension bundle.` (empty expected hash); `The FTHR Upload Extension bundle failed its integrity check.`; `… contains undeclared payload files.`; `… payload integrity failed: <path>`; `The uploader Terms of Service were not accepted.`; `The uploader Privacy Policy was not accepted.`; `FTHR Upload Extension installed.` (success).

For the Rust port, none of this packaging layer is needed unless the plan keeps a separate process. The behavioural contracts that matter are enabled gating, consent versions, and allowed roots.

---------------------------------------------------------------------------------------------------

## 4. Consent dialogs (`USW`)

### 4.1 Uploader install consent (`_legal_install_dialog`, USW:88-156)

Shown when the user ticks "Enable optional clip uploader" and `is_plugin_installed()` is false (USW:524-562). If `uploader_legal_text()` fails: warning dialog titled `Upload Extension Unavailable` with the exception text.

- Dialog title and heading label: `Install FTHR Upload Extension` (740 px wide, resized 740x660). The heading is styled uppercase, so it renders in capitals.
- Body: `This separately packaged component contains all provider network code. FTHR Clips Core remains upload-free.`
- Section label `Terms of service`, then a read-only text box with `TERMS_OF_SERVICE.txt`.
- Section label `Privacy policy`, then a read-only text box with `PRIVACY_POLICY.txt`.
- Checkbox 1: `I have read and accept the Terms of Service.`
- Checkbox 2: `I have read and accept the Privacy Policy.`
- Buttons: `Cancel`, and `ACCEPT & INSTALL UPLOADER` (disabled until both boxes are ticked).
- Accept: `activate_plugin('2026-10-15-v3','2026-10-15-v3')` writes the receipt (3.6) and sets `upload_enabled=true`, `uploader_terms_accepted_version`, `uploader_privacy_accepted_version`, `uploader_accepted_at` in settings, and runs the legacy migration. It emits `plugin_state_changed(True, True)`.
  - Failure: warning titled `Upload Extension Installation Failed` with the message, and the checkbox is unticked. `upload_enabled` is forced false.
  - Settings save failure: `The uploader was installed, but its settings could not be saved.`
- Cancel: the checkbox is silently unticked with no message.

The full terms text (`FTHR_Uploader/TERMS_OF_SERVICE.txt`, 28 lines, version `2026-10-15-v3`) and privacy text (`PRIVACY_POLICY.txt`, 50 lines, same version) are shown verbatim. The Rust port should embed them (they are in `reference/FTHR_Uploader/`).

### 4.2 Catbox provider consent (`_provider_consent_dialog`, USW:159-215)

Shown right after install, and again whenever Catbox is selected (provider combo change while enabled, Save, Test connection) and `catbox_legal_accepted_version != 'catbox-legal-2021-03-06'`. Not shown for custom, sftp or vaultscope (`_ensure_provider_ready`, USW:575-587).

- Title and heading: `Connect Catbox` (560 px wide).
- Body: `Catbox stores the clip, filename, file size, upload time, and your IP address. Files are publicly accessible to anyone with the link and may remain available until deleted. Review Catbox’s Terms, Privacy Policy, and Acceptable Use Policy before continuing.`
- Link text (rich text, opens `https://catbox.moe/legal.php` via the desktop opener): `OPEN CATBOX TERMS, PRIVACY & ACCEPTABLE USE`.
- Checkbox: `I accept Catbox’s Terms, Privacy Policy, and Acceptable Use Policy.`
- Buttons: `Cancel`, and `Accept & continue` (disabled until the box is ticked).
- Accept: `record_provider_consent('catbox','catbox-legal-2021-03-06')` sets `catbox_legal_accepted_version` and `catbox_legal_accepted_at` (UTC ISO seconds) and saves. Save failure: warning `Uploader` / `Provider consent could not be saved.`
- Cancel during enable: `set_plugin_enabled(False)` is called, the checkbox is unticked and the settings body hidden. The extension stays "installed".
- Cancel when switching the provider combo: the combo reverts to the previous provider.

---------------------------------------------------------------------------------------------------

## 5. Upload settings widget (`USW`, hosted in the Settings page, `main.py:10476-10484`)

Layout top to bottom:

1. Section header `Upload`.
2. Checkbox `Enable optional clip uploader` (`enable_check`). Toggling it takes effect immediately, without Save.
   - Flow in `_on_enabled_changed` (USW:524-562): not installed leads to the install dialog (4.1) then the provider consent (4.2). Already installed: `set_plugin_enabled(requested)`. Failure: warning titled `Upload Extension` with the message, and the box is reverted. Returned messages: `Upload Extension enabled.` / `Upload Extension disabled.`; `Accept the uploader terms and privacy policy before enabling it.`; `The uploader setting could not be saved. Please try again.`
   - The body below is visible only while enabled.
3. Button `Support Catbox hosting` (visible only when Catbox is the provider) opens `https://catbox.moe/support.php`. This button sits above the body, outside it.
4. Body, visible only when enabled:
   - Section header `Host`.
   - Row: label `Provider`; combo items in order `Catbox`(`catbox`), `VaultScope`(`vaultscope`), `Your server`(`custom`), `SFTP`(`sftp`); button `Open Catbox` / `OPEN VAULTSCOPE` (visible for catbox and vaultscope only). It opens `https://catbox.moe/`, or `https://vaultscope.de/ps/fthr` for VaultScope. (The code also has a branch to open the server URL for custom, but the button is hidden then.) The text of the button is initially `Open Catbox` and is set to `OPEN CATBOX` / `OPEN VAULTSCOPE` by `_apply_provider`.
   - **Catbox panel**: label `User hash`, placeholder `Optional Catbox account userhash`.
   - **Custom panel** (framed with an accent left border): label `Server URL`, placeholder `https://your-server.example.com/upload`; label `Auth header`, placeholder `Bearer token123  (optional)` (two spaces before the parenthesis).
   - **SFTP panel** (shown for sftp and vaultscope):
     - VaultScope sub-panel (vaultscope only): text `FTHR exclusive: get one month free when you sign up through our VaultScope link.`; buttons `Get one month free at VaultScope` (opens `https://vaultscope.de/ps/fthr`) and `Paste VaultScope login`.
     - Fields (label, placeholder):
       - `Host` / `sftp.example.com`
       - `Port` / `22`
       - `Username` / `Your SFTP username`
       - `Password` (masked) / `Blank keeps the saved password for this host and user`
       - `Remote folder` / `. (home directory) or /uploads`
       - `Host key (optional)` / `Blank uses trusted known_hosts; SHA256:… pins a specific key`
       - `Public folder URL` / `https://videos.example.com/clips/`
     - Checkbox `Generate public HTTPS sharing links for SFTP uploads`. It enables or disables the Public folder URL field; the field is disabled by default.
     - Note label: `Leave Host key blank to use a trusted key from your SSH known_hosts file or a key previously verified by FTHR. For a new host, enter the SHA256 fingerprint from the server operator and test once; FTHR remembers the verified key. Unknown or changed keys are rejected before sending credentials. Passwords are saved in Windows Credential Manager. Files get unique names. For sharing links, enter the HTTPS address that serves this exact remote folder. Enabling public links makes new files readable by the web server. Your server must already provide public web access; the URL setting does not set up hosting or call a publishing API. Leave it disabled for private SFTP storage.`
     - Button `Forget saved password for this host and user`.
   - Row: button `Test connection` and a status label initially `Not tested`.
   - Section header `Auto Upload`: radio buttons `Upload immediately after capture` (id 0, `immediate`), `Upload on an interval` (id 1, `interval`), `Manual only  (right-click a clip → Upload)` (id 2, `manual`; two spaces after "only").
     - Row: label `Every`, a combo with `1,2,5,10,15,30,60` (default index 2 = `5` if the saved value is not listed), and a unit combo `minutes, hours, days`. Both are enabled only while the interval radio is checked.
   - Section header `Upload Processing`: checkbox `Automatically compress oversized clips before upload`.
   - Section header `Post-Upload`: checkbox `Auto-delete local clip after a confirmed successful upload`.
   - Button `Save upload settings` (left aligned).

### 5.1 Save (`_on_save`, USW:627-671)

1. If enabled and the provider is not ready (Catbox consent), abort.
2. For sftp and vaultscope: `sftp_credentials(_sftp_config())` runs through the plugin (`sftp_save`), which validates and stores a non-empty password. Any exception in that call becomes `SFTP settings could not be saved. Enable the Upload Extension first.`
   - Failure: warning titled `SFTP settings` with the message, and nothing is saved.
   - Success: each `sftp_*` key except the password is `set()`; the password field is cleared.
3. `upload_provider` is set to `sftp` for vaultscope, else the provider value. For sftp/vaultscope, `sftp_provider_name` = `VaultScope` or `''`.
4. `catbox_userhash` is set stripped. `upload_server_url` is stripped, and `https://` is prepended (and the field updated) if there is no http(s) scheme and it is non-empty. `upload_auth_header` is stripped.
5. `upload_mode`, `upload_interval_value` (int, fallback 5), `upload_interval_unit`, `upload_auto_compress`, `upload_auto_delete` are set.
6. `set_plugin_enabled(enable_check.isChecked())` writes the file. Failure: warning titled `Uploader Settings`.
7. Success: status label `Settings saved`, reverting to `Not tested` after 3000 ms.

`_sftp_config()` (USW:~735): every `sftp_*` text field is stripped, except the password (verbatim), plus `sftp_public_enabled` from the checkbox.
`_load_settings` populates fields from the manager. Port defaults to `22` and the folder to `.` when missing. The password field is never populated.

### 5.2 VaultScope paste (`_paste_vaultscope_login`, USW:703-750)

- Reads the clipboard text, max 65536 chars, parses JSON.
- The JSON must be an object with `providerName == 'VaultScope'`, `domain` (str, lowercased equal to `sftp.vaultscope.de`), `username` (non-blank str, no control chars), `password` (non-empty str), and `port` (an int, not a bool, 1-65535).
- Failure: warning titled `VaultScope login` with `Copy the complete VaultScope login JSON, then paste again. It must include providerName, domain, port, username and password.` No field is changed.
- Success: sets host, port, username, password, `sftp_directory='.'`, clears the fingerprint and public URL, unticks the public checkbox. Status: `VaultScope login filled in. Test the connection, then save settings.`
- The VaultScope provider has no server-specific code path in the uploader. It is plain SFTP with those values.

### 5.3 Test connection (`_on_test_connection`, USW:673-700, `_on_test_done` USW:755-762)

- Runs `_ensure_provider_ready` (so it may show the Catbox consent dialog).
- Custom provider with an empty URL field: the label becomes `Enter a server URL first` and nothing is run.
- Otherwise the button is disabled, the label becomes `Testing…`, and the `test_connection` request runs on a daemon thread. An exception becomes `(False, str(exc))`.
- Done: the button is re-enabled and the message is stripped. If it failed with an empty message it becomes `The upload provider did not return a reason.` The label shows the message, green for ok and red for error. On failure the `connection_failed(message)` signal fires, and the main window shows a toast (6.5).
- Result messages:
  - Catbox: HEAD `https://catbox.moe/user/api.php`, timeout 10 s. Any HTTP response, even an error one, yields ok `Catbox reachable (HTTP {code}).`. A network error yields not-ok `Could not reach Catbox: {exc}`.
  - Custom: HEAD to the configured URL with the Authorization header, timeout 10 s. Any HTTP response yields ok `Your server reachable (HTTP {code}).`. A network error yields not-ok `Could not reach your server: {exc}`. A bad URL raises the 2.3 `ValueError` text.
  - SFTP: ok `SFTP authenticated; host key verified and remote folder found. Write access is checked on upload.`, or any of the SftpError texts in 2.2.
  - Process-level errors (activation, disabled, consent, unsupported provider, timeout text) as in 1.4-1.5.

### 5.4 Forget password (`_forget_sftp_password`, USW:~752)

Runs `sftp_forget` with the current config. Success: clears the password field and sets the label `Saved SFTP password removed`. Failure: warning titled `SFTP password` with the message. An exception gives `Enable the Upload Extension before managing saved credentials.`

---------------------------------------------------------------------------------------------------

## 6. App-level UI flows

### 6.1 Wiring (`main.py`)

- Create: `UploadManager(settings_manager, clip_readiness_registry)` (`main.py:3273-3279`). `start()` at :3645 starts a worker thread `fthr-uploader-plugin-worker` and the interval timer. `stop()` at :9567.
- Signals (`main.py:3636-3645`): `upload_finished`, `upload_error`, `compression_required`, `compression_started`, `compression_progress`.
- Grid wiring (`main.py:4079-4088`): card `clip_upload_requested` leads to `enqueue_upload`. `upload_started` calls `clip_grid.set_upload_status(path,'Uploading…')`. The grid uses `is_uploaded`, `get_upload_info` and `is_enabled` as checkers; `plugin_state_changed` re-applies the enabled checker.
- Viewer (`main.py:9657-9678`): `ClipViewer(... upload_enabled=manager.is_enabled())`, `upload_requested` leads to `enqueue_upload`.
- Clip card rename or delete reserves the path with `begin_local_mutation` (a clip being uploaded cannot be renamed or deleted: `This clip is uploading. Try again when the upload finishes.`, `main.py:4539-4556`).

### 6.2 What can be uploaded

- `is_completed_video_path`: suffix in `('.mp4','.mkv','.avi')` (case-insensitive) and not partial (name ends with `.partial` or contains `.partial.`) (`core/clip_files.py:14-17, 82-107`). Refusals print `[Uploader] Refused incomplete clip path: <name>` to the console and do nothing in the UI. Images are not uploadable. The card's upload action requires `is_video`.
- A clip waits until its readiness event opens (state `READY`, `READY_WITH_WARNING` or `FINALIZATION_FAILED`). If `can_access(path)` is false, the result is `(False, 'Clip finalization failed; upload was not started')` (UM:670-718).
- `_do_single_upload` pre-checks: `Incomplete clip files cannot be uploaded`; `File no longer exists (deleted before upload)`; `Upload Extension is disabled or not installed`. An empty response message becomes `Upload failed`.
- `enqueue_upload` does nothing and shows nothing when the extension is disabled. It also skips paths already uploaded, in flight or being mutated (`_enqueue`, UM:652-659; duplicate clicks are silently dropped).

### 6.3 Where uploads start

1. **Clip card share button** (`CG:880-890, 966-985, 1146-1154`). A single 48x52 icon button.
   - No link yet and `is_video` and uploads enabled: icon `upload`, accessible name `Upload clip`. A click shows the card overlay `Upload requested` (2000 ms) and emits `upload_requested(path)`.
   - Tooltip while disabled: `Enable the uploader in Settings to upload this clip`. The button is also disabled while the card is not ready/finalizing, mutating, or has an upload status set.
   - With a link: icon `link`, accessible name `Copy link`, or `Copy SFTP location` for `sftp://` links (tooltip `Copy SFTP location (requires an SFTP client and login)`). A click copies the link to the clipboard (`QApplication.clipboard().setText`) and shows the overlay `Link copied`, or `SFTP location copied` for sftp (`CG:1058-1062`).
2. **Card context menu** (`CG:1280-1306`). Items are `Open in editor`, `Rename`, `Show in file manager`, `Copy path`, `Open uploaded link` (or `Open SFTP location`; only when a link exists), `Delete clip`. There is NO "Upload" item, despite the settings radio label saying "right-click a clip → Upload".
3. **Clip viewer** (`ClipViewer`, `clip_viewer.py:5144-5149, 7970-7988`). A button `Upload` (38 px high; only visible when uploads are enabled at construction). A click:
   - If not enabled: button text `Upload off`, `export_error('UPLOAD NOT CONFIGURED', 'Install and enable the optional uploader in Upload Settings first.', 'warning')`, reset to `Upload` after 2500 ms.
   - Else: emits `upload_requested(clip_path)`, text `Queued`, disabled; reset to `Upload` and enabled after 2500 ms.
4. **Immediate mode**: `notify_clip_saved` (`main.py:7151`) enqueues a `preflight_upload` task after each saved clip when enabled and `upload_mode == 'immediate'` (UM:605-620).
5. **Interval mode** (UM:589-600, 829-875): a timer with `value * {minutes: 60_000, hours: 3_600_000, days: 86_400_000}` ms, default unit minutes. Each tick scans the whole clips directory (not imported folders) on a thread. Completed videos whose mtime is at least `_WRITE_SETTLE_S=30` s old and that are not uploaded go through `enqueue_upload`.
6. Card "Uploaded" badge: label `Uploaded` over the thumbnail when `is_uploaded` (`CG:836-839`). The card has a hidden `open_link_btn`; it is used by the context menu.

Link validity for the card (`CG:1037-1049`): takes `info['url']`, then `info['raw_url']`. It accepts `http(s)://` as-is, and `sftp://` only if it is valid with a host, no userinfo, a path longer than `/`, and a port of -1 or 1-65535.

### 6.4 Size limit and compression

- `provider_limit_mb` is `{'catbox': 200}` only (`core/export_profiles.py:468-469`). Custom and SFTP have no limit.
- If the file is oversized (`> 200 * MIB`):
  - Auto-compress on: the clip goes to the compress-then-upload task.
  - Else the `compression_required(path, provider, limit)` signal shows a question dialog (`main.py:7336-7358`). Title: `Compress before upload?`. Body: `{filename} is {actual_mb:.1f} MB. {Provider.title()} accepts files up to {limit_mb} MB.\n\nAutomatically compress a copy and upload it? The original clip will stay unchanged.` A yes answer calls `enqueue_compressed_upload`.
  - The main window is restored first if it was hidden (tray).
- Compression (UM:720-779): preset `provider_compression_preset` (`target_size_mb=200`, `target_fps=60`, `h264`, audio 128 kbps, `safety_margin=0.94`). Output `<clips>/Shared/<stem>_<provider>_upload_<HH-MM-SS>[_N].mp4`. The status bar shows `COMPRESSING {n}%`.
- Failures: `Could not create compression folder: {exc}`; `Automatic compression failed: {exc}` (also emits `upload_error('COMPRESSION FAILED', msg, 'warning', path)`); `Compressed file could not be verified: {exc}`; `Compressed file still exceeds the provider limit; upload was not started`.
- Success message: `Uploaded compressed copy ({size:.1f} MB); original preserved. {plan.summary}`. History is aliased to the original (3.2).

### 6.5 Progress, completion and failure feedback

- Start: the card overlay shows `Uploading…` (set on `upload_started`). It is never cleared (see 7.9). No toast is shown at start.
- Success (`main.py:7312-7334`):
  - The status bar shows `UPLOADED ORIGINAL`, or `UPLOADED COMPRESSED COPY` if the message contains `compressed copy` (casefolded). It reverts after 2000 ms.
  - The capture-card toast `show_upload(...)`: headline `CLIP UPLOADED`, detail `{basename} · ORIGINAL` or `{basename} · COMPRESSED COPY`, plus the upload-success sound. The hold is 2400 ms.
  - For compressed copies, an info toast (`push_error`) titled `COMPRESSED COPY UPLOADED` with the message.
  - The card refreshes its info (`set_upload_info`) and shows the badge.
  - No link is shown in the toast. The user copies the link from the card button.
- Failure (`main.py:7366-7386`, via `upload_error('UPLOAD FAILED', message, 'warning', path)` from `UM:826`):
  - Detail defaults to `The upload service did not provide a failure reason.` when empty.
  - The capture-card toast: headline `UPLOAD FAILED`, detail = message, plus the upload-failed sound.
  - A `push_error` error toast `UPLOAD FAILED` (public code `Error 020`) with the action button `RETRY NOW` (which re-calls `enqueue_upload(path)`).
  - `UPLOAD NOT CONFIGURED` (`Error 021`) has the action `OPEN UPLOAD SETTINGS` (restores the main window). The manager never emits this title; only the clip viewer does (6.3).
  - `COMPRESSION FAILED` (`Error 023`) has no action.
  - Failures that don't go through `upload_error` (finalization failure, readiness) only print `[Upload] Failed — {msg}  ({path})`.
- Test connection failure toast (`main.py:7388-7397`): title `UPLOAD CONNECTION FAILED` (`Error 022`), detail = message, or `The upload provider did not provide a failure reason.`, action `OPEN UPLOAD SETTINGS`.
- Error table: `core/error_codes.py:94-109`: Error 020 `UPLOAD FAILED`, 021 `UPLOAD NOT CONFIGURED`, 022 `UPLOAD CONNECTION FAILED`, 023 `COMPRESSION FAILED`.

### 6.6 Disabled uploader

- `is_enabled()` = `upload_enabled and is_plugin_installed()` (UM:602-603).
- Disabled: no upload button on cards (the tooltip hint above); the viewer button is hidden (decided at viewer construction); `enqueue_upload` silently returns; immediate and interval modes do nothing.
- Previously uploaded cards keep their `Uploaded` badge and the `Copy link` button, because history is independent of the enabled flag.
- The uploader process itself rejects network actions with `The Upload Extension is disabled in FTHR Clips.`

---------------------------------------------------------------------------------------------------

## 7. Known defects and quirks in the reference (do not necessarily replicate)

1. **Test connection is a reachability ping only**: `except urllib.error.HTTPError` returns `ok=True` for any status, 4xx/5xx included (UR:249-250, 327-330). A wrong URL, wrong token or 404 reports green "reachable". It also tests `HEAD`, which Catbox's API endpoint may not support. Authentication is never validated for Catbox or custom.
2. **Windows-only paths and hashes**: install root from `%LOCALAPPDATA%` (falls back to `~/AppData/Local`) (UM:51-55); entrypoint `FTHR Uploader.exe`; `CREATE_NO_WINDOW`; a single `EXPECTED_UPLOADER_BUNDLE_SHA256` in the manifest. `tests/test_uploader_builders.py` (not part of the listed 8 files) expects `EXPECTED_UPLOADER_LINUX_BUNDLE_SHA256` and `tools/build_linux_uploader.py` / `FTHR-Uploader-linux.fthrplugin` / entrypoint `FTHR-Uploader`, but the committed manifest and `UM` have no Linux constant, so the committed core does not support the Linux bundle.
3. **SFTP password vault is Windows-only**: `_vault()` raises `Saved SFTP passwords currently require Windows Credential Manager.` on non-win32 (SF:85-86), although the Linux builder hidden-imports the Secret Service keyring backend. On Linux the password can only be supplied transiently (`sftp_password` in the request), which the core never does for `upload`. Messages and the settings note say "Windows Credential Manager".
4. **The uploader trusts its own receipt**: the exe hash check hashes `sys.executable` (US:58). In dev mode this is the Python interpreter.
5. **Retry policy**: Catbox and custom retry any exception, including permanent ones (bad URL, header with a newline, non-2xx), for up to 5+15+45 s of sleeping plus 4 attempts of up to 120 s socket timeouts. Worst case exceeds the 600 s process timeout, which raises `TimeoutExpired` with Python's own message and leaves the subprocess possibly still running (`subprocess.run` kills the child on timeout).
6. **Custom server**: a 2xx reply without a recognizable URL counts as success with message `Upload complete.` and empty `url`, so the card gets no `Copy link` button but is badged `Uploaded` (history entry exists with `status: ok`).
7. **Plaintext secrets**: `catbox_userhash` and the custom `upload_auth_header` live in `settings.json` in plaintext. The privacy policy lists only the userhash.
8. **Settings label vs UI**: `Manual only  (right-click a clip → Upload)`, but the card context menu has no Upload entry (only the share button).
9. **`Uploading…` card status is never cleared**: `set_upload_status(path,'Uploading…')` is called on `upload_started` (`main.py:4081`) and nothing calls it with an empty message. After completion or failure the overlay stays and the share button stays disabled (`CG:985` includes `not self._upload_status`), and the status is re-applied when the card is rebuilt (`CG:2433`), so it persists until restart. This also blocks the `RETRY NOW` path from the card, though the toast action still works.
10. **Dead and unused fields**: `clips_root` is sent but ignored; `upload_info` in the response is ignored by the core; `expires_at` and `favorite` in history are never set to anything but `favorite: false`; `UPLOAD NOT CONFIGURED` is never emitted by the manager.
11. **`Catbox’s` apostrophe** in the consent error and dialog is the typographic U+2019.
12. **`auto_delete` on compressed uploads**: the uploaded path is the compressed copy, so the copy is deleted and the original is kept. It does not matter for originals.
13. **Viewer button gating**: the viewer's Upload button visibility is fixed at construction (`self._upload_enabled`), so enabling the uploader later does not show it in an already-open viewer.
14. **History keyed by absolute path**: moving a clip outside the app's rename loses its link and badge. Deleted clips' entries are never removed.
15. **Interval scan**: it ignores `imported_clip_folders` while the allowed-roots list includes them.
16. **No cancellation**: an in-flight upload cannot be cancelled. Quitting calls `stop()` which joins the worker for 3 s, and the subprocess is not explicitly killed.
17. **Unpinned SFTP first-time flow needs an out-of-band step**: there is no trust-on-first-use. The user must provide the fingerprint (or have the key in known_hosts).
18. **`sftp_save`/`sftp_forget` are available when `upload_enabled` is false** but still require a valid receipt (i.e. the extension must be installed).

---------------------------------------------------------------------------------------------------

## 8. Reference tests that pin uploader behaviour (`reference/tests/`)

- `test_optional_uploader_runtime.py`
  - `test_catbox_uses_documented_multipart_fields`: URL `https://catbox.moe/user/api.php`, fields `{reqtype: fileupload, userhash: ...}`, file field `fileToUpload`, result url taken from the body.
  - `test_custom_server_uses_configured_url_auth_and_clip_field`: fields `{}`, file field `clip`, headers `{'Authorization': 'Bearer test-token'}`, `require_https=False`, URL from a JSON `{"url": ...}` body on HTTP 201.
  - `test_provider_actions_require_versioned_consent`: Catbox without consent raises PermissionError mentioning Catbox.
  - `test_custom_provider_does_not_require_third_party_consent`.
  - `test_retired_providers_and_account_actions_are_rejected`: `lustful`/`fthr` give `Unsupported upload provider`; `register`/`login`/`account_status`/`logout` give `Unsupported uploader action`; no network call.
- `test_uploader_plugin_boundary.py`
  - `test_uploader_installs_only_after_consent`: wrong versions are refused, correct ones install.
  - `test_disabled_uploader_refuses_network_actions`: the message contains `disabled`.
  - `test_disabled_state_persists_across_manager_restart`; `test_toggle_reports_save_failure_and_restores_previous_state` (message contains `could not be saved`).
  - `test_core_has_no_provider_network_client_and_uploader_has_no_hardware_reader`; `test_consent_ui_is_the_only_activation_callsite`; the built-bundle tests; `test_retired_provider_is_disabled_on_load`.
- `test_upload_settings_consent.py`
  - SFTP UI: masked password, port default `22`, public checkbox off and its URL field disabled, no third-party consent for SFTP.
  - `test_sftp_save_uses_vault_and_keeps_password_out_of_settings`: the password is passed verbatim (`' secret '`), is not stored in settings, and the field is cleared.
  - `test_enable_installs_uploader_only_after_both_legal_acceptances`: the install dialog title is `Install FTHR Upload Extension`; Catbox consent is recorded with `CATBOX_LEGAL_VERSION`.
  - Enable toggle persists without Save; a failed connection test emits `connection_failed` with the reason.
  - `upload_auto_compress` is saved; the Catbox support button opens `CATBOX_SUPPORT_URL`; the support bar shows only for Catbox.
  - `test_custom_provider_exposes_and_saves_server_connection_fields`: `clips.example/upload` is saved as `https://clips.example/upload`.
  - VaultScope: import fills the fields and clears the pin/folder/public settings; invalid JSON keeps the form; the test connection uses `upload_provider='sftp'`; the combo items are exactly `['catbox','vaultscope','custom','sftp']`.
- `test_sftp_provider.py` (uses a real in-process paramiko server)
  - Blank pin uses trusted known_hosts keys; unknown or changed keys never reach authentication (`not trusted|before sending credentials`).
  - A verified pin is remembered and other hosts are kept (`[host]:port` naming).
  - Auth failure is sanitized (`authentication failed`, no password leak) and is not retried.
  - The connection test checks the directory without writing (`Write access is checked on upload`).
  - A failed rename cleans up the `.part` file and keeps the local clip.
  - Success with auto-delete removes the local file.
  - Public mapping: HTTPS link and 0644 permission; private by default; a chmod failure keeps the local file and returns no link; unsafe URLs are rejected before any network (`public HTTPS folder URL`); trailing slash normalized.
  - Invalid host/port/user/dir/fingerprint fail before the network.
  - The vault password is scoped by host/port/user and never returned; `sftp_password` is never persisted by the core (`credential vault`).
- `test_upload_manager_retry.py`
  - Oversized Catbox upload emits `compression_required(path,'catbox',200)` and queues nothing.
  - With auto-compress it queues a `compress_upload` task.
  - History aliasing sets `uploaded_copy='compressed'`, `original_preserved=True` and `uploaded_path`.
  - `upload_error` has four args `('TEST','detail','warning','')`.
  - Partial files are rejected by all entry points (`Incomplete clip`); interval scan skips partials; the upload waits for the real ready state; failed finalization never uploads.
- `test_uploader_builders.py`: the Windows and Linux builders keep each other's hashes in the manifest (`EXPECTED_UPLOADER_BUNDLE_SHA256`, `EXPECTED_UPLOADER_LINUX_BUNDLE_SHA256`). This is irrelevant to Rust behaviour but documents defect 2.

Not found in the reference: any progress percentage for the network transfer, upload cancel, key-file SFTP auth, a link-copy toast with the URL, Catbox "delete" or "expiry" handling, and any upload-related keys in `~/.fthr/settings.json` beyond the legacy-migration ones.
