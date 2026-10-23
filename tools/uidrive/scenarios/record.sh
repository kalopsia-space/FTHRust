#!/usr/bin/env bash
# The Record button starts and stops a manual recording.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
step "start recording"; run uidrive click "QPushButton[Record]"
run uidrive wait-text "Stop" --timeout 25
run uidrive shot recording
sleep 3
step "stop recording"; run uidrive click "QPushButton[Record]"
run uidrive wait-status "RECORDING SAVED" --timeout 30
step "recording file appears"; wait_file "$BENCH/clips/Recordings/FTHR_recording_*.mp4" 10 || fail
run uidrive shot recorded
echo PASS
