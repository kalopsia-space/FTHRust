#!/usr/bin/env bash
# Export an edit (two kept ranges after two splits and a delete) through
# the queue and watch it complete in the Exports window; then share the
# clip at full quality and wait for the share window to be ready.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
# A few seconds of buffer so the clip is long enough for two splits.
sleep 5
step "save a clip"; run uidrive hotkey save_clip
wait_file "$BENCH/clips/*/*.mp4" 15 || fail
run uidrive wait "cardThumb" --timeout 15
step "open it"; run uidrive click cardThumb
run uidrive wait ctrlBar --timeout 10
run uidrive wait-text "0:00 →" --timeout 10
step "split at 0:01"; run uidrive click playBtn
run uidrive wait-text "0:01 /" --timeout 10
run uidrive click playBtn
run uidrive key Ctrl+C
run uidrive wait-text "1 cut" --timeout 5
step "split at 0:02 and delete the tail"; run uidrive click playBtn
run uidrive wait-text "0:02 /" --timeout 10
run uidrive click playBtn
run uidrive key Ctrl+C
run uidrive wait-text "2 cuts" --timeout 5
run uidrive key Ctrl+D
sleep 0.5
step "Ctrl+Q (quick crop; egui once quit on it) and live exposure"; run uidrive key Ctrl+Q
for h in panelHeader panelHeader#2 panelHeader#3 panelHeader#4; do run uidrive click "$h"; done
run uidrive wait effectSlider --timeout 5
read -r FROM TO < <(uidrive find effectSlider | awk '{printf "%d,%d %d,%d\n", $1+$3/2, $2+$4/2, $1+$3*0.85, $2+$4/2}')
run uidrive drag "$FROM" "$TO"
sleep 0.5
EXPOSURE=$(uidrive state --json | python3 -c "import json,sys; print(next(m.get('text') for m in json.load(sys.stdin)['markers'] if m['name']=='effectValue'))")
[ "$EXPOSURE" != "0" ] || { echo "exposure did not move"; fail; }
run uidrive shot live-preview
step "Ctrl+E exports"; run uidrive key Ctrl+E
step "the Exports window shows it complete"; run uidrive click "QPushButton[Export jobs]"
run uidrive wait-text "Keep browsing while your clips export" --timeout 5
run uidrive wait-text "· Completed" --timeout 90
run uidrive shot export-jobs
wait_file "$BENCH/clips/Exported/*_export_*.mp4" 5 || fail
if ls -A "$BENCH/clips/Exported" | grep -q partial; then
    echo "staged files left behind"; fail
fi
run uidrive click fthrDialogClose
run uidrive wait fthrDialogHeader --gone --timeout 5
step "share at full quality"; run uidrive click shareBtn
run uidrive wait "QPushButton[Share clip]" --timeout 5
run uidrive shot share-picker
run uidrive click "QPushButton[Share clip]"
run uidrive wait swHdr --timeout 5
run uidrive wait-text "Ready" --timeout 90
run uidrive shot share-ready
wait_file "$BENCH/clips/Shared/*_share_*.mp4" 5 || fail
echo PASS
