#!/usr/bin/env bash
# Turn the uploader on through Settings (both consent dialogs), point it at
# a local server, upload a clip from its card and copy the returned link.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 "$HERE/upload_server.py" "$BENCH/received" "$BENCH/upload-port" &
SERVER=$!
trap 'kill $SERVER 2>/dev/null' EXIT
wait_file "$BENCH/upload-port" 5 || fail "upload server"
PORT=$(cat "$BENCH/upload-port")
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
sleep 2
step "save a clip"; run uidrive hotkey save_clip
wait_file "$BENCH/clips/*/*.mp4" 15 || fail
run uidrive wait cardThumb --timeout 15
step "enable the uploader"; run uidrive click settingsGearBtn
run uidrive click "settingsTabBtn[General]"
run uidrive wait "QCheckBox[Enable optional clip uploader]" --timeout 5
run uidrive click "QCheckBox[Enable optional clip uploader]"
run uidrive wait-text "Enable FTHR Upload Extension" --timeout 5
run uidrive shot consent
run uidrive click "QCheckBox[terms]"
run uidrive click "QCheckBox[privacy]"
run uidrive click "QPushButton[accept]"
step "accept Catbox"; run uidrive wait-text "Connect Catbox" --timeout 5
run uidrive click "QCheckBox[catbox]"
run uidrive click "QPushButton[Accept & continue]"
run uidrive wait "QLabel[Host]" --timeout 5
step "choose your server"; run scroll_to "QComboBox[Catbox]"
run uidrive click "QComboBox[Catbox]"
run uidrive click "QComboBox[Catbox]/item[Your server]"
run uidrive wait "QLineEdit[Server URL]" --timeout 5
run uidrive click "QLineEdit[Server URL]"
run uidrive type "http://127.0.0.1:$PORT/upload"
run scroll_to "QPushButton[Save upload settings]"
run uidrive click "QPushButton[Save upload settings]"
run uidrive wait-text "Settings saved" --timeout 5
run uidrive shot upload-settings
python3 - "$BENCH/home/.fthr/uploader/settings.json" "$PORT" <<'PY' || fail "settings file"
import json, sys
s = json.load(open(sys.argv[1]))
assert s["upload_enabled"] is True, s
assert s["upload_provider"] == "custom", s
assert s["upload_server_url"] == f"http://127.0.0.1:{sys.argv[2]}/upload", s
assert s["uploader_terms_accepted_version"] and s["catbox_legal_accepted_version"], s
PY
step "upload from the card"; run uidrive click homeBtn
run uidrive wait cardShareBtn --timeout 5
run uidrive click cardShareBtn
wait_file "$BENCH/received/upload-1.bin" 30 || fail "nothing received"
run uidrive wait cardUploadedBadge --timeout 15
grep -q 'name="clip"' "$BENCH/received/upload-1.bin" || fail "multipart field"
python3 - "$BENCH/home/.fthr/uploader/upload_history.json" "$PORT" <<'PY' || fail "history"
import json, sys
h = json.load(open(sys.argv[1]))
(entry,) = h.values()
assert entry["status"] == "ok" and entry["url"] == f"http://127.0.0.1:{sys.argv[2]}/f/1.mp4", entry
PY
step "copy the link"; run uidrive click cardShareBtn
run uidrive wait-text "Link copied" --timeout 5
run uidrive shot uploaded
echo PASS
