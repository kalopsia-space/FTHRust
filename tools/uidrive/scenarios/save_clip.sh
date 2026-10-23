#!/usr/bin/env bash
# A save-clip hotkey writes a clip and the library shows it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
step "hotkey save_clip"; run uidrive hotkey save_clip
step "clip file appears"; wait_file "$BENCH/clips/*/*.mp4" 15 || fail
step "card appears"; run uidrive wait "clipCard[0]" --timeout 15
run uidrive shot clip-saved
echo PASS
