#!/usr/bin/env bash
# The bench works end to end: capture comes up, a click changes the page.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
run uidrive shot start
step "open settings"; run uidrive click settingsGearBtn
run uidrive wait settingsTabBar --timeout 5
uidrive state | grep -q "page settings" || fail
run uidrive shot settings
step "back to clips"; run uidrive click homeBtn
run uidrive wait filterBar --timeout 5
step "escape closes nothing"; run uidrive key Escape
echo PASS
