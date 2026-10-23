"""pytest for tools/render-qt-reference.py (run under .venv-reference)."""
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
PALETTE = {'#070a09', '#0a0f0d', '#101512', '#141b17', '#1b231e', '#181f1b', '#242c27',
           '#49584f', '#f3f6f4', '#a0aba5', '#7e8a83', '#20e3a0', '#16bf87', '#0d2b20',
           '#ff795d', '#270b0f', '#e7b56e', '#b84f45'}


@pytest.fixture(scope='session')
def rendered(tmp_path_factory):
    out = tmp_path_factory.mktemp('qt-screens')
    env = dict(os.environ, QT_QPA_PLATFORM='offscreen')
    subprocess.run([sys.executable, str(ROOT / 'tools' / 'render-qt-reference.py'), str(out)],
                   check=True, env=env, timeout=600)
    return out


def load(out, name):
    return json.loads((out / f'{name}.json').read_text(encoding='utf-8'))


def test_settings_tab_bar_and_tabs(rendered):
    data = load(rendered, 'settings')
    assert data['size'] == [1440, 900]
    names = [w['name'] for w in data['widgets']]
    bar = next(w for w in data['widgets'] if w['name'] == 'settingsTabBar')
    assert bar['rect'][2] == 1440 - 94, 'the tab bar spans the page right of the 94 px rail'
    tabs = [w for w in data['widgets'] if w['name'].startswith('settingsTabBtn[')]
    assert [t['name'] for t in tabs] == [
        'settingsTabBtn[General]', 'settingsTabBtn[Clip]', 'settingsTabBtn[Audio]',
        'settingsTabBtn[Visuals]', 'settingsTabBtn[Customize]', 'settingsTabBtn[Performance]',
        'settingsTabBtn[Shortcuts]']
    assert tabs[0]['checked'] is True
    assert 'topBar' in names and 'navigationPanel' in names and 'statusLabel' in names
    assert (rendered / 'settings.png').exists()


def test_samples_are_palette_colours_or_text(rendered):
    data = load(rendered, 'settings')
    fills = [s for w in data['widgets'] for s in w['samples'] if s['role'] == 'fill']
    tokenised = sum(1 for s in fills if s['token'])
    assert tokenised >= len(fills) // 2, f'{tokenised} of {len(fills)} fills map to a token'


def test_missing_screens_exist_with_sizes(rendered):
    for name in ('performance', 'source-popup', 'game-detection-popup', 'hotkey-popup',
                 'dialog-message', 'dialog-question', 'dialog-input', 'custom-clip-length',
                 'export-preset', 'error-bar-error', 'error-bar-warning'):
        data = load(rendered, name)
        assert data['size'][0] > 0 and data['size'][1] > 0, name
        assert data['widgets'], name
    assert load(rendered, 'performance')['size'] == [1440, 900]
    assert load(rendered, 'error-bar-error')['size'][1] == 36
    popup = load(rendered, 'capture-popup')
    assert popup['size'][0] >= 460


def test_text_metrics_present(rendered):
    data = load(rendered, 'settings')
    brand = next(w for w in data['widgets'] if w.get('text', {}).get('content') == 'FTHR Clips')
    assert brand['text']['px'] == 17 and brand['text']['weight'] == 700
    assert brand['text']['height'] > 0


def test_editor_variants(rendered):
    def names(screen):
        return [w['name'] for w in load(rendered, screen)['widgets']]

    assert 'audioMixName#3' in names('clip-editor-sidebar'), 'MASTER plus two source rows'
    danger = [w for w in load(rendered, 'clip-editor-timeline')['widgets']
              if w['name'] == 'timelineActionDanger']
    assert danger and danger[0]['text']['content'] == 'RESTORE SEGMENT'
    # The dump lists a root's children, never the root: the dialogs are
    # identified by their accept buttons and their default sizes.
    assert 'QPushButton[Apply crop]' in names('clip-editor-crop')
    assert load(rendered, 'clip-editor-crop')['size'] == [900, 580]
    assert 'stretchDialogApply' in names('clip-editor-stretch')
    assert load(rendered, 'clip-editor-stretch')['size'] == [760, 540]
    rail = [n for n in names('clip-editor-rail') if n.startswith('editorSidebarButton')]
    assert len(rail) == 2, rail
    for screen in ('clip-editor-sidebar', 'clip-editor-timeline', 'clip-editor-crop',
                   'clip-editor-stretch', 'clip-editor-rail'):
        assert (rendered / f'{screen}.png').exists(), screen
