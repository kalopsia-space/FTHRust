#!/usr/bin/env bash
# Open a saved clip in the editor, play, split, park it in the rail,
# restore it and close it; then the same by keyboard.
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
step "keys: reopen (the draft keeps the cut)"; run uidrive click cardThumb
run uidrive wait-text "1 cut" --timeout 10
step "keys: Space plays"; run uidrive key Space
run uidrive wait-text "0:01 /" --timeout 10
run uidrive key Space
step "keys: Ctrl+C splits"; run uidrive key Ctrl+C
run uidrive wait-text "2 cuts" --timeout 5
step "crop dialog"; run uidrive click cropBtn
run uidrive wait "QPushButton[Apply crop]" --timeout 5
sleep 1; run uidrive shot editor-crop
run uidrive key Escape
run uidrive wait "QPushButton[Apply crop]" --gone --timeout 5
step "stretch dialog"; run uidrive click stretchBtn
run uidrive wait stretchDialogApply --timeout 5
sleep 1; run uidrive shot editor-stretch
run uidrive click "stretchDialogSecondary[Cancel]"
run uidrive wait stretchDialogApply --gone --timeout 5
step "keys: Shift+Escape parks"; run uidrive key Shift+Escape
run uidrive wait editorSidebarButton --timeout 5
run uidrive click editorSidebarButton
run uidrive wait ctrlBar --timeout 10
step "keys: Escape closes"; run uidrive key Escape
run uidrive wait editorSidebarButton --gone --timeout 5
echo PASS
