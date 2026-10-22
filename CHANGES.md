# Changes relative to FTHR Clips linux-2.0 (aa0dc9b)

## Fixed

- Settings file that is valid JSON but not an object is quarantined instead of crashing the loader.
- A failed quarantine rename and undecodable settings files no longer stop startup; defaults are used as in the reference's intent.
- Hotkeys loader drops non-string bindings instead of failing on the whole file.
- Theme loader replaces invalid colour strings with the token default instead of propagating them.
- Theme import validates archive entry names and the asset names inside `theme.json`, so a crafted theme cannot read or write outside the theme folders.
- Re-selecting an already installed custom icon, sound or font no longer truncates the file; unknown sound keys are rejected.
- Clip metadata store surfaces a locked database instead of falling back to stale JSON, and patches rows without dropping unknown keys or unparseable drafts.
- Library scan ignores an imported folder that duplicates the clips root or another import, so owned clips are not flagged as imported.
- Clip rename restores the media name when the sidecar step fails, and the rollback is covered by a test.
- Thumbnail cache keys hash raw path bytes, so clips with non-UTF-8 names reuse their thumbnails.
- Diagnostic event fields named like secrets are redacted wholesale, not only when their value looks like a credential.
- Error-bar titles are normalised (trimmed, upper-cased) before code lookup exactly as the reference does.
