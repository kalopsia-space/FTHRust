#!/usr/bin/env bash
# Post-save overlays: with an image layer on (run with
# --settings tools/uidrive/overlays-settings.json), a saved clip shows
# Finalizing, then becomes ready with the layer burned in, leaving no
# temporary files.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
step "capture comes up"; run uidrive wait-status CAPTURING --timeout 20
sleep 2
step "save a clip"; run uidrive hotkey save_clip
wait_file "$BENCH/clips/*/*.mp4" 15 || fail
run uidrive wait "cardThumb" --timeout 15
step "the clip finalizes"
for i in $(seq 1 120); do
    grep -q "overlays applied" "$BENCH/logs/app.log" 2>/dev/null && break
    sleep 0.5
done
grep -q "overlays applied" "$BENCH/logs/app.log" || { echo "no burn"; fail; }
run uidrive wait cardFinalizingBadge --gone --timeout 10
run uidrive shot overlays-ready
if compgen -G "$BENCH/clips/*/.fthr-finalize-*" >/dev/null; then
    echo "finalize directory left behind"; fail
fi
step "it opens"; run uidrive click cardThumb
run uidrive wait ctrlBar --timeout 10
echo PASS
