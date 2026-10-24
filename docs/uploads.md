# Uploads

Uploading is optional and off by default. Nothing is sent anywhere until
the uploader is enabled in **Settings → General → Upload** and its Terms of
Service and Privacy Policy are accepted.

## How it is built

- `fthrust-uploader` is a separate program shipped next to `fthrust`. The
  app links no upload code (`tools/check-upload-isolation.sh` fails CI if
  `reqwest`, `russh` or the keyring reach `fthrust-ui` or `fthrust-engine`).
- Each request runs the program once: one JSON object on stdin, one JSON
  line on stdout (`upload`, `test_connection`, `sftp_save`,
  `sftp_forget`). It refuses network actions unless the uploader settings
  say uploads are on and the current terms were accepted, and it reads
  only clips under the clips folder and the imported folders.
- `FTHRUST_UPLOADER` overrides where the app looks for the program.

## Providers

| Provider | How | Link |
|---|---|---|
| Catbox | `multipart/form-data` to `catbox.moe/user/api.php` (`fileToUpload`, optional userhash); 200 MB limit, larger clips can be compressed first | the returned `https://files.catbox.moe/…` |
| Your server | `multipart/form-data` with the field `clip` and an optional `Authorization` header; http or https | `url` or `raw_url` from a JSON reply, or a bare URL body |
| SFTP / VaultScope | password login after the host key matches a `SHA256:` pin or a known_hosts entry; written as `.part`, size-checked, renamed to `fthr-<random>.<ext>` | the public HTTPS folder URL + file name when public links are on, else `sftp://host:port/path` |

SFTP passwords are kept in the desktop keyring (Secret Service), never in
`~/.fthr/uploader/settings.json`. Keys verified through a pin are
remembered in `~/.fthr/uploader/known_hosts`.

### FTHR Clips Server

[fthr-clips-server](https://github.com/kalopsia-space/fthr-clips-server) is
built for the "Your server" provider and needs no extra setup in the app:

| Field | Value |
|---|---|
| URL | `https://clips.example.com/upload` |
| Auth header | `Bearer <UPLOAD_TOKEN or a token from Admin → Tokens>` |

Test connection sends `HEAD /upload` (200 with a good token, 401 without).
An upload returns `{"url": "…/w/<slug>", …}`; the same file uploaded
twice returns the same link.

What the app says when something is off:

| Problem | Message |
|---|---|
| URL without `/upload` | Your server answered (HTTP 200), but the URL has no path. FTHR Clips Server takes uploads at /upload. |
| No token | Your server needs an upload token (HTTP 401). Set the Auth header to "Bearer <your upload token>". |
| Token without `Bearer ` | … The Auth header usually looks like "Bearer <your upload token>". |
| Wrong token | Your server rejected the authorization header (HTTP 401). |
| `http://` behind an https redirect | Your server redirects to https://…/upload. Use that address as the server URL. |
| `https://` to a plain-HTTP port | Could not reach your server: it did not answer with HTTPS. Check whether the URL should start with http://. |
| Clip over `MAX_UPLOAD_BYTES` | The clip is too large for your server (HTTP 413). This clip is … MB. |
| 30 uploads a minute | Your server is limiting uploads right now (HTTP 429). Try again in a minute. (retried) |

URL and token problems are not retried and put OPEN UPLOAD SETTINGS on the
UPLOAD FAILED bar. `tools/uidrive/scenarios/clips_server.sh` drives all of
this against a real server when `FTHR_CLIPS_SERVER` points at its binary.

## Files

- `~/.fthr/uploader/settings.json`: provider, fields, auto-upload mode,
  consent versions. Compatible with FTHR Clips; the consent version is
  `2026-10-23-v4` because the Linux terms differ (no installer, desktop
  keyring), so the reference's acceptance is asked for again.
- `~/.fthr/uploader/upload_history.json`: successful uploads keyed by the
  clip's resolved path; renaming a clip in the app moves its entry.

## Where uploads start

- The clip card's upload button (it becomes **Copy link** afterwards) and
  its right-click menu.
- The editor's **Upload** button (shown while uploads are on).
- **Upload immediately after capture** and **Upload on an interval**
  (clips untouched for 30 s, including imported folders).

Results show as the `CLIP UPLOADED` / `UPLOAD FAILED` capture card, the
status bar (`UPLOADED ORIGINAL`, `UPLOADED COMPRESSED COPY`) and the error
bar (`Error 020` with **RETRY NOW**, `Error 022` for a failed connection
test).

## Bench

`tools/uidrive/scenarios/upload.sh` enables the uploader through both
consent dialogs, points it at `tools/uidrive/upload_server.py`, uploads a
clip from its card and copies the link.
