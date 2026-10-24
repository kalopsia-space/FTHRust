#!/usr/bin/env bash
# Screenshots for the documentation. Run through tools/uidrive/docs-shots.sh,
# which seeds demo clips and copies the named shots into docs/images.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 30
run uidrive wait cardThumb --timeout 15
sleep 4
run uidrive shot library

popup() { # popup BUTTON POPUP NAME
  step "popup $3"
  run uidrive click "$1"
  run uidrive wait "$2" --timeout 5
  sleep 0.5; run uidrive shot "$3"
  run uidrive key Escape
  run uidrive wait "$2" --gone --timeout 5
}
popup "TopBarButton[30s" capture_popup capture
popup "TopBarButton[Source]" source_popup source
popup "TopBarButton[Hotkeys]" hotkey_popup hotkeys
popup "TopBarButton[Game detection" game_popup game-detection

step "editor"; run uidrive click cardThumb
run uidrive wait ctrlBar --timeout 10
run uidrive wait-text "0:00 →" --timeout 10
at() { uidrive find timelineTrack | awk -v f="$1" '{printf "%d,%d\n", $1 + $3 * f, $2 + $4 / 2}'; }
run uidrive click "$(at 0.3)"; sleep 0.5; run uidrive key Ctrl+C
run uidrive wait-text "1 cut" --timeout 5
run uidrive click "$(at 0.7)"; sleep 0.5; run uidrive key Ctrl+C
run uidrive wait-text "2 cuts" --timeout 5
run uidrive click "$(at 0.85)"; sleep 0.5; run uidrive key Ctrl+D
run uidrive click "$(at 0.5)"; sleep 1
run uidrive shot editor
step "export queue"; run uidrive key Ctrl+E
run uidrive click "QPushButton[Export jobs]"
run uidrive wait-text "· Completed" --timeout 90
sleep 0.5; run uidrive shot export-jobs
step "close the queue and the editor"
run uidrive click "$(uidrive find fthrDialogHeader | awk '{printf "%d,%d\n", $1 + $3 - 18, $2 + $4 / 2}')"
run uidrive wait fthrDialogHeader --gone --timeout 5
for _ in 1 2 3; do
  uidrive find cardThumb >/dev/null 2>&1 && break
  uidrive key Escape >/dev/null; sleep 0.5
done
run uidrive wait cardThumb --timeout 5

step "settings"; run uidrive click settingsGearBtn
run uidrive wait settingsTabBar --timeout 5
for tab in General Clip Audio Visuals Customize Performance Shortcuts; do
  run uidrive click "settingsTabBtn[$tab]"
  run uidrive wait "settingsPage[$tab]" --timeout 5
  sleep 0.6; run uidrive shot "settings-$(echo "$tab" | tr '[:upper:]' '[:lower:]')"
done
echo PASS
