#!/usr/bin/env bash
# Every Settings tab and every toolbar popup opens, is screenshotted and closes.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
run uidrive shot clips
step "settings tabs"; run uidrive click settingsGearBtn
run uidrive wait settingsTabBar --timeout 5
for tab in General Clip Audio Visuals Customize Performance Shortcuts; do
  step "tab $tab"
  run uidrive click "settingsTabBtn[$tab]"
  run uidrive wait "settingsPage[$tab]" --timeout 5
  run uidrive shot "settings-$tab"
done
step "back to clips"; run uidrive click homeBtn
run uidrive wait filterBar --timeout 5
for pair in "TopBarButton[Source]:source_popup" "TopBarButton[Hotkeys]:hotkey_popup" "TopBarButton[Game detection:game_popup" "TopBarButton[30s:capture_popup"; do
  btn=${pair%%:*}; popup=${pair##*:}
  step "popup $popup"
  run uidrive click "$btn"
  run uidrive wait "$popup" --timeout 5
  run uidrive shot "$popup"
  run uidrive key Escape
  run uidrive wait "$popup" --gone --timeout 5
done
step "record button present"; run uidrive find "QPushButton[Record]"
echo PASS
