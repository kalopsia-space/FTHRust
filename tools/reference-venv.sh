#!/usr/bin/env bash
# Create .venv-reference (used only to generate test fixtures from the reference).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
python3 -m venv --system-site-packages .venv-reference
.venv-reference/bin/pip install --quiet opencv-python-headless numpy sounddevice jeepney
.venv-reference/bin/python -c "import PySide6, cv2, numpy, sounddevice, jeepney; print('reference venv ok')"
