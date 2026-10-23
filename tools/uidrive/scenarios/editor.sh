#!/usr/bin/env bash
# Open a saved clip in the editor, play, split, park it in the rail,
# restore it and close it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
step "save a clip"; run uidrive hotkey save_clip
wait_file "$BENCH/clips/*/*.mp4" 15 || fail
run uidrive wait "cardThumb" --timeout 15
step "open it"; run uidrive click cardThumb
run uidrive wait ctrlBar --timeout 10
run uidrive wait-text "0:00 →" --timeout 10
run uidrive shot editor-open
step "play"; run uidrive click playBtn
run uidrive wait-text "0:01 /" --timeout 10
run uidrive shot editor-playing
step "pause and split"; run uidrive click playBtn
run uidrive click timelineAction
run uidrive wait-text "1 cut" --timeout 5
run uidrive shot editor-split
step "park"; run uidrive click editorMin
run uidrive wait editorSidebarButton --timeout 5
run uidrive shot editor-parked
step "restore"; run uidrive click editorSidebarButton
run uidrive wait ctrlBar --timeout 10
run uidrive wait-text "1 cut" --timeout 5
run uidrive shot editor-restored
step "close"; run uidrive click editorClose
run uidrive wait editorSidebarButton --gone --timeout 5
run uidrive wait cardThumb --timeout 5
run uidrive shot editor-closed
echo PASS
