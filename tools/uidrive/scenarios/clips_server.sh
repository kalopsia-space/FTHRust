#!/usr/bin/env bash
# The app against a real fthr-clips-server (kalopsia-space/fthr-clips-server):
# messages for a bare URL, a missing token and a wrong token, the error bar's
# settings action, then a working upload. Optional: set FTHR_CLIPS_SERVER to
# the server binary, else it prints SKIP.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SRV=${FTHR_CLIPS_SERVER:-}
[ -x "$SRV" ] || { echo "SKIP (set FTHR_CLIPS_SERVER to a fthr-clips-server binary)"; exit 0; }
PORT=18472; TOKEN=bench-token-0123456789
UPLOAD_TOKEN=$TOKEN ADMIN_PASSWORD=bench-admin-pass-0123456789 DATA_DIR=$BENCH/srvdata PORT=$PORT \
  BIND_HOST=127.0.0.1 PUBLIC_BASE_URL=http://127.0.0.1:$PORT LOG_LEVEL=warn "$SRV" serve > $BENCH/logs/server.log 2>&1 &
SP=$!; trap 'kill $SP 2>/dev/null' EXIT
status() { uidrive find uploadStatus | cut -d' ' -f5-; }
set_field() { # set_field MARKER TEXT
  run uidrive click "$1"; run uidrive key Ctrl+A; run uidrive key BackSpace
  [ -n "$2" ] && run uidrive type "$2"; true; }
test_conn() { # test_conn EXPECTED_STATUS
  run uidrive click "QPushButton[Test connection]"
  run uidrive wait-text "$1" --timeout 10; echo "    status: $(status)"; }
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
sleep 2
step "save a clip"; run uidrive hotkey save_clip
wait_file "$BENCH/clips/*/*.mp4" 15 || fail
run uidrive wait cardThumb --timeout 15
step "enable the uploader"; run uidrive click settingsGearBtn
run uidrive click "settingsTabBtn[General]"
run uidrive wait "QCheckBox[Enable optional clip uploader]" --timeout 5
run uidrive click "QCheckBox[Enable optional clip uploader]"
run uidrive wait-text "Connect Catbox" --timeout 5
run uidrive click "QCheckBox[catbox]"; run uidrive click "QPushButton[Accept & continue]"
run uidrive wait "QLabel[Host]" --timeout 5
step "choose your server"; run scroll_to "QComboBox[Catbox]"
run uidrive click "QComboBox[Catbox]"
run uidrive click "QComboBox[Catbox]/item[Your server]"
run uidrive wait "QLineEdit[Server URL]" --timeout 5
step "bare URL, no token"; set_field "QLineEdit[Server URL]" "http://127.0.0.1:$PORT"
run scroll_to "QPushButton[Test connection]"; test_conn "Your server answered (HTTP 200), but the URL has no path. FTHR Clips Server takes uploads at /upload."; run uidrive shot bare-url
step "upload path, no token"; run scroll_to "QLineEdit[Server URL]"
set_field "QLineEdit[Server URL]" "http://127.0.0.1:$PORT/upload"
run scroll_to "QPushButton[Test connection]"; test_conn 'Your server needs an upload token (HTTP 401). Set the Auth header to "Bearer <your upload token>".'; run uidrive shot no-token
step "bare token"; run scroll_to "QLineEdit[Auth header]"
set_field "QLineEdit[Auth header]" "$TOKEN"
run scroll_to "QPushButton[Test connection]"; test_conn 'Your server rejected the authorization header (HTTP 401). The Auth header usually looks like "Bearer <your upload token>".'; run uidrive shot bare-token
step "upload with the wrong token"; run scroll_to "QLineEdit[Auth header]"
set_field "QLineEdit[Auth header]" "Bearer wrong"
run scroll_to "QPushButton[Save upload settings]"; run uidrive click "QPushButton[Save upload settings]"
run uidrive wait-text "Settings saved" --timeout 5
run uidrive click homeBtn; run uidrive wait cardShareBtn --timeout 5
run uidrive click cardShareBtn
for i in $(seq 1 30); do
  uidrive find "[Error 020] UPLOAD FAILED" >/dev/null 2>&1 && break
  if uidrive find errorBar >/dev/null 2>&1; then
    read -r x y w h < <(uidrive find errorBar | awk '{print int($1),int($2),int($3),int($4)}')
    uidrive click "$((x + w - 34)),$((y + h / 2))" >/dev/null
  fi
  sleep 1
done
run uidrive wait "[Error 020] UPLOAD FAILED" --timeout 5
uidrive find errorBar | cut -d' ' -f5-
run uidrive shot upload-failed
step "open settings from the bar"; run uidrive click "QPushButton[OPEN UPLOAD SETTINGS]"
run uidrive wait "QLineEdit[Auth header]" --timeout 5
run scroll_to "QLineEdit[Auth header]"
set_field "QLineEdit[Auth header]" "Bearer $TOKEN"
run scroll_to "QPushButton[Test connection]"; test_conn "Your server reachable (HTTP 200)."; run uidrive shot good
run scroll_to "QPushButton[Save upload settings]"; run uidrive click "QPushButton[Save upload settings]"
run uidrive wait-text "Settings saved" --timeout 5
step "upload works"; run uidrive click homeBtn; run uidrive wait cardShareBtn --timeout 5
run uidrive click cardShareBtn
run uidrive wait cardUploadedBadge --timeout 30
run uidrive shot uploaded
python3 - "$BENCH/home/.fthr/uploader/upload_history.json" "$PORT" <<'PY' || fail "history"
import json, sys
(entry,) = json.load(open(sys.argv[1])).values()
assert entry["status"] == "ok" and entry["url"].startswith(f"http://127.0.0.1:{sys.argv[2]}/w/"), entry
print("    history:", entry["url"])
PY
echo PASS
