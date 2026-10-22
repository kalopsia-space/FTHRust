#!/usr/bin/env python3
"""Generate parity fixtures by executing the FTHR Clips reference code.

Run with .venv-reference/bin/python from the repository root.  Every value in
the fixtures is produced by running the reference modules under
reference/FTHR_UI; nothing is typed in by hand except the *inputs* of cases.

Each case runs in a fresh subprocess with HOME pointing at a throwaway
directory, because the reference computes paths at import time.  The parent
process never imports reference code.  main.py is never imported (the one
helper needed from it is lifted out with ast).
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REF = ROOT / 'reference' / 'FTHR_UI'
OUT = ROOT / 'crates' / 'fthrust-core' / 'tests' / 'fixtures'


# --------------------------------------------------------------------------
# Child side: each function runs inside a fresh subprocess.
# --------------------------------------------------------------------------

def _plain(value):
    import dataclasses
    import enum
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        return {f.name: _plain(getattr(value, f.name))
                for f in dataclasses.fields(value)}
    if isinstance(value, enum.Enum):
        return value.name
    if isinstance(value, dict):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple, set, frozenset)):
        return [_plain(v) for v in value]
    return value


def _qt_app():
    from PySide6.QtCore import QCoreApplication
    return QCoreApplication.instance() or QCoreApplication([])


def _try(fn, *args):
    try:
        return fn(*args)
    except Exception:
        return None


def c_settings(payload):
    from core.settings_manager import SettingsManager
    sm = SettingsManager()
    return {k: v for k, v in sm.settings.items() if not k.startswith('_')}


def c_theme_defaults(payload):
    from core import theme_manager as tm
    from ui.icons import ICON_ASSETS
    return {
        'colors': tm.DEFAULT_COLORS,
        'capture_card': tm.DEFAULT_CAPTURE_CARD_COLORS,
        'fonts': tm.DEFAULT_FONTS,
        'icon_tint': tm.DEFAULT_ICON_TINT,
        'customizable_icons': tm.CUSTOMIZABLE_ICONS,
        'customizable_sounds': tm.CUSTOMIZABLE_SOUNDS,
        'color_groups': tm.CUSTOMIZABLE_COLOR_GROUPS,
        'legacy': {
            'v1': tm._LEGACY_COLORS, 'v2': tm._V2_COLORS, 'v3': tm._V3_COLORS,
            'cc_v1': tm._LEGACY_CAPTURE_CARD_COLORS,
            'cc_v2': tm._V2_CAPTURE_CARD_COLORS,
            'cc_v3': tm._V3_CAPTURE_CARD_COLORS,
        },
        'icon_assets': ICON_ASSETS,
    }


def c_theme(payload):
    from core.theme_manager import ThemeManager
    return ThemeManager()._data


def c_navigation(payload):
    _qt_app()
    from core import navigation_bindings as nb
    inputs = ['ctrl+s', 'Shift+Ctrl+S', 'esc', 'enter', 'win+x', 'Tab+S',
              'cmd+q', 'ctrl+', '', 'F13']
    return {
        'definitions': [list(row) for row in nb.BINDING_DEFINITIONS],
        'normalize_cases': [[i, _try(nb.normalize_binding, i)] for i in inputs],
    }


def c_hotkeys(payload):
    _qt_app()
    from core import hotkey_manager as hm
    from core import portal_shortcuts as ps
    hk = hm.HotkeyManager()
    norm_in = ['ctrl+shift+s', 'f9', 'Shift+Ctrl+a', 'page up', 'WIN+F1',
               'ctrl+', '', 'a']
    portal_in = ['Ctrl+Shift+S', 'F9', 'Esc', 'Page Up', 'Scroll Lock',
                 'Win+A', 'Alt+Shift+F5']
    return {
        'actions': list(hm._HOTKEY_ACTIONS),
        'descriptions': dict(hm._ACTION_DESCRIPTIONS),
        'defaults': dict(hk.hotkeys),
        'controller_defaults': dict(hk.controller_hotkeys),
        'normalize_cases': [[i, _try(hm.normalize_keyboard_combo, i)]
                            for i in norm_in],
        'portal_trigger_cases': [[i, _try(ps.to_portal_trigger, i)]
                                 for i in portal_in],
    }


def c_hotkeys_legacy(payload):
    _qt_app()
    from core import hotkey_manager as hm
    hk = hm.HotkeyManager()
    return {'hotkeys': dict(hk.hotkeys),
            'controller_hotkeys': dict(hk.controller_hotkeys)}


def c_error_codes(payload):
    from core.error_codes import APP_FAILURE_CODES
    return [_plain(c) for c in APP_FAILURE_CODES]


def c_export_presets(payload):
    from core import export_profiles as ep
    base = {'preset_id': 'case', 'name': 'Case', 'target_size_mb': 10}

    def accepted(extra):
        d = dict(base)
        d.update(extra)
        try:
            ep.ExportPreset(**d).validated()
            return [d, True]
        except Exception:
            return [d, False]

    cases = []
    for v in (0.5, 1, 10000, 10001):
        cases.append(accepted({'target_size_mb': v}))
    for v in (31, 32, 512, 513):
        cases.append(accepted({'audio_bitrate_kbps': v}))
    for v in (0, 1, 240, 241):
        cases.append(accepted({'target_fps': v}))
    for v in (0.79, 0.80, 0.99, 1.0):
        cases.append(accepted({'safety_margin': v}))
    cases.append(accepted({'target_width': 640}))
    return {
        'builtin_discord': _plain(ep.DISCORD_PRESET),
        'catbox_adhoc': _plain(ep.provider_compression_preset('catbox')),
        'validation_cases': cases,
    }


def _lift_main_function(name):
    """Extract one pure function from main.py without importing main.py."""
    import ast
    src = (Path(__file__).resolve().parent.parent / 'reference' / 'FTHR_UI'
           / 'main.py').read_text(encoding='utf-8')
    tree = ast.parse(src)
    node = next(n for n in tree.body
                if isinstance(n, ast.FunctionDef) and n.name == name)
    ns = {}
    exec(compile(ast.Module([node], []), 'main.py', 'exec'), ns)
    return ns[name]


def c_clip_files(payload):
    from core import clip_files as cf
    names = ['a.mp4', 'a.mkv', 'a.avi', 'a.mp4.partial',
             '.a.partial.0123abcd.mp4', 'a.png', 'A.MP4', 'a.webm']
    sanitize = _lift_main_function('_sanitize_foldername')
    san_in = ['Over:watch*?', 'CON', '', 'abcdefghij' * 4, 'Game With Spaces',
              '../etc', '\u0001ctl']
    return {
        'completed_video_cases': [[n, cf.is_completed_video_path(n)]
                                  for n in names],
        'sanitize_cases': [[i, _try(sanitize, i)] for i in san_in],
        'partial_marker': cf._FTHR_CLIP_MARKER,
        'sanitize_source': 'main.py:_sanitize_foldername (lifted via ast)',
    }


def c_redaction(payload):
    import logging
    from core.diagnostics import _RedactingFilter
    inputs = ['Authorization: Bearer abc.def', 'token=abcdef123456',
              'api_key: "xyz"', 'password=hunter2 user=bob',
              'https://user:pw@host/path', 'plain text 123', 'secret=1',
              'X-Auth-Header: foo']
    flt = _RedactingFilter()
    out = []
    for text in inputs:
        rec = logging.LogRecord('t', logging.INFO, __file__, 1, text, (), None)
        flt.filter(rec)
        out.append([text, rec.msg])
    return out


class _Screen:
    def __init__(self, spec):
        from PySide6.QtCore import QRect
        self._name = spec['name']
        self._rect = QRect(spec['x'], spec['y'], spec['w'], spec['h'])

    def name(self):
        return self._name

    def availableGeometry(self):
        return self._rect

    def geometry(self):
        return self._rect


def c_window_geometry(payload):
    from core import window_geometry as wg
    screens_spec = [
        {'name': 'DP-3', 'x': 0, 'y': 0, 'w': 2560, 'h': 1440, 'primary': True},
        {'name': 'HDMI-1', 'x': 2560, 'y': 0, 'w': 1920, 'h': 1080},
    ]

    def ng(x, y, w, h):
        return {'x': x, 'y': y, 'width': w, 'height': h}

    cases = [
        ('saved-screen-present',
         {'normal_geometry': ng(2700, 100, 1200, 800),
          'screen_name': 'HDMI-1', 'maximized': False}),
        ('saved-screen-missing-overlap',
         {'normal_geometry': ng(2400, 100, 1300, 800),
          'screen_name': 'DP-9', 'maximized': False}),
        ('no-overlap-primary',
         {'normal_geometry': ng(9000, 9000, 1200, 800),
          'screen_name': 'DP-9', 'maximized': False}),
        ('rect-too-small',
         {'normal_geometry': ng(100, 100, 300, 200),
          'screen_name': 'DP-3', 'maximized': False}),
        ('maximized-flag',
         {'normal_geometry': ng(50, 50, 1200, 800),
          'screen_name': 'DP-3', 'maximized': True}),
        ('empty-saved-centred-default', {}),
    ]
    screens = [_Screen(s) for s in screens_spec]
    primary = screens[0]
    out = []
    for label, saved in cases:
        rect, maximized, screen = wg.restore_window_geometry(
            saved, screens, primary)
        out.append({
            'name': label, 'saved': saved, 'screens': screens_spec,
            'expected': {'rect': wg.rect_to_dict(rect),
                         'maximized': maximized,
                         'screen_name': screen.name() if screen else None},
        })
    return {'restore_cases': out,
            'constants': {'default_min_width': wg.DEFAULT_MIN_WIDTH,
                          'default_min_height': wg.DEFAULT_MIN_HEIGHT,
                          'min_visible_width': wg.MIN_VISIBLE_WIDTH,
                          'min_visible_height': wg.MIN_VISIBLE_HEIGHT}}


def c_capture_config(payload):
    from core import capture_settings as cs
    buf_cases = [(30, []), (30, [60, 120]), (300, [900]), (5, []),
                 (60, [45]), (240, [600])]
    ring_cases = [(25000, 32, 32768), (200000, 905, 8192), (500, 7, 1024)]
    return {
        'buffer_seconds_cases': [
            [n, c, _try(cs.compute_buffer_seconds, n, c)] for n, c in buf_cases],
        'ring_budget_cases': [
            [b, s, r, cs.linux_ring_budget_mb(b, s, r)] for b, s, r in ring_cases],
        'constants': {'max_clip_seconds': cs.MAX_CLIP_SECONDS,
                      'ring_min_mb': cs.LINUX_RING_BUDGET_MIN_MB,
                      'ring_max_mb': cs.LINUX_RING_BUDGET_MAX_MB},
    }


def c_capture_health(payload):
    from core import capture_health as ch
    mon = ch.CaptureHealthMonitor

    def snap(s):
        return {
            'state': s.state.value, 'previous_state': s.previous_state.value,
            'reason': s.reason, 'changed': s.changed,
            'request_recovery': s.request_recovery,
            'seconds_since_progress': s.seconds_since_progress,
            'fresh_buffer_seconds': s.fresh_buffer_seconds,
            'save_allowed': s.save_allowed,
        }

    seqs = {
        'healthy-progress': [
            (True, 0, 1, 1, 0.0), (True, 10, 1, 1, 0.5), (True, 20, 1, 1, 1.0),
            (True, 30, 1, 1, 1.5), (True, 30, 1, 1, 2.0),
            (True, 40, 1, 1, 2.5), (False, 0, 0, 1, 3.0)],
        'stall-then-recovery': [
            (True, 100, 1, 1, 0.0), (True, 110, 1, 1, 0.5),
            (True, 110, 1, 1, 3.5), (True, 110, 1, 1, 8.5),
            (True, 110, 1, 1, 9.0), (True, 110, 3, 1, 10.0),
            (True, 120, 1, 1, 11.0), (True, 130, 1, 1, 11.5)],
        'new-generation-rebaseline': [
            (True, 500, 1, 1, 0.0), (True, 510, 1, 1, 0.5),
            (True, 520, 1, 1, 1.0), (True, 900, 1, 2, 1.5),
            (True, 900, 1, 2, 2.0), (True, 910, 1, 2, 2.5),
            (True, 920, 1, 2, 3.0)],
    }
    out = {}
    for name, steps in seqs.items():
        m = mon()
        rows = []
        for connected, count, flags, gen, t in steps:
            s = m.observe(connected=connected, frame_count=count,
                          engine_flags=flags, generation=gen, now=t)
            rows.append({'input': [connected, count, flags, gen, t],
                         'expected': snap(s)})
        out[name] = rows
    return {
        'thresholds': {'warning_after_s': mon.WARNING_AFTER_S,
                       'stalled_after_s': mon.STALLED_AFTER_S,
                       'recovery_budget_reset_s': mon.RECOVERY_BUDGET_RESET_S},
        'flags': {f.name: int(f.value) for f in ch.CaptureHealthFlag},
        'states': [s.value for s in ch.CaptureHealthState],
        'observe_sequences': out,
    }


def c_save_state(payload):
    import ctypes
    from core.capture_bridge import (CaptureBridge, SharedMemoryLayout,
                                     ResponseType)
    from core.save_state import SaveStateMachine, EngineEvent, OutcomeKind

    class FakeEngine:
        def __init__(self):
            self._buf = (ctypes.c_byte * ctypes.sizeof(SharedMemoryLayout))()
            self.layout = SharedMemoryLayout.from_buffer(self._buf)
            self.layout.is_initialized = True

        def respond(self, response, detail=''):
            self._write_string(detail)
            self.layout.engine_response = response

        def _write_string(self, detail):
            if isinstance(self.layout.engine_string, bytes):
                self.layout.engine_string = detail.encode('utf-8')
            else:
                self.layout.engine_string = detail

    class FakeBridge(CaptureBridge):
        def __init__(self, engine):
            self._layout = engine.layout
            self._initialized = True
            self._logged_read_errors = set()

    class Driver:
        def __init__(self):
            self.engine = FakeEngine()
            self.bridge = FakeBridge(self.engine)
            self.log = []
            self.sm = SaveStateMachine(ack_timeout_s=1.0,
                                       completion_timeout_s=60.0,
                                       logger=self.log.append)
            self.now = 1000.0
            self.outcomes = []
            self.steps = []

        def record(self, label, **extra):
            row = {'step': label, 't': self.now, 'state': self.sm.state.name,
                   'outcome_kinds': [o.kind.name for o in self.outcomes],
                   'results': sum(1 for o in self.outcomes if o.kind in (
                       OutcomeKind.COMPLETED, OutcomeKind.FAILED))}
            row.update(extra)
            self.steps.append(row)

        def advance(self, s):
            self.now += s

        def submit(self, path='/clips/a.mp4', duration=30):
            self.pump()
            assert self.bridge.save_clip(path, duration) is True
            res = self.sm.submit(path, duration, self.now)
            return res

        def pump(self):
            peeked = self.bridge.peek_save_response()
            if peeked is None:
                outcome = self.sm.on_tick(self.now)
            else:
                kind, detail = peeked
                event = {'started': EngineEvent.SAVE_STARTED,
                         'saved': EngineEvent.CLIP_SAVED,
                         'error': EngineEvent.ERROR_OCCURRED}[kind]
                outcome = self.sm.on_event(event, detail, self.now)
                self.bridge.consume_save_response(kind)
            if outcome is not None:
                self.outcomes.append(outcome)
            return outcome

        def last_outcome(self):
            if not self.outcomes:
                return None
            o = self.outcomes[-1]
            return {'kind': o.kind.name, 'detail': o.detail, 'late': o.late}

    R = ResponseType
    out = {}

    d = Driver()
    d.submit(); d.record('submit')
    d.pump(); d.record('pump (NONE -> WAITING_FOR_ACK)')
    d.advance(0.05); d.engine.respond(R.SAVE_STARTED); d.pump()
    d.record('SAVE_STARTED')
    d.advance(0.8); d.pump(); d.record('still encoding')
    d.engine.respond(R.CLIP_SAVED); d.pump()
    d.record('CLIP_SAVED', last=d.last_outcome())
    out['T1'] = d.steps

    d = Driver()
    d.submit(); d.record('submit')
    d.advance(0.02); d.engine.respond(R.CLIP_SAVED); d.pump()
    d.record('CLIP_SAVED without SAVE_STARTED', last=d.last_outcome())
    out['T2'] = d.steps

    d = Driver()
    d.submit(); d.record('submit')
    d.engine.respond(R.ERROR_OCCURRED, 'SaveClip failed: disk full'); d.pump()
    d.record('ERROR_OCCURRED', last=d.last_outcome())
    out['T3'] = d.steps

    d = Driver()
    d.submit(); d.record('submit')
    d.engine.respond(R.SAVE_STARTED); d.pump(); d.record('SAVE_STARTED')
    d.advance(0.3); d.engine.respond(R.ERROR_OCCURRED, 'mux failed'); d.pump()
    d.record('ERROR_OCCURRED', last=d.last_outcome())
    out['T4'] = d.steps

    d = Driver()
    d.submit(); d.record('submit')
    d.pump(); d.record('pump')
    d.advance(1.5); d.pump(); d.record('ack timeout', last=d.last_outcome())
    d.advance(70.0); d.pump()
    d.record('completion deadline', last=d.last_outcome())
    out['T5'] = d.steps

    d = Driver()
    d.submit(); d.record('submit')
    d.advance(1.5); d.pump(); d.record('ack timeout', last=d.last_outcome())
    d.advance(0.4); d.engine.respond(R.CLIP_SAVED); d.pump()
    d.record('late CLIP_SAVED', last=d.last_outcome(),
             log=list(d.log))
    out['T5_late_success'] = d.steps

    d = Driver()
    d.submit('/clips/a.mp4'); d.record('submit A')
    d.engine.respond(R.CLIP_SAVED); d.record('CLIP_SAVED written, unread')
    d.submit('/clips/b.mp4'); d.record('submit B (pumps A verdict first)',
                                       last=d.last_outcome())
    d.engine.respond(R.CLIP_SAVED); d.pump()
    d.record('B CLIP_SAVED', last=d.last_outcome())
    out['T6'] = d.steps
    return out


CASES = {
    'settings': c_settings, 'theme-defaults': c_theme_defaults,
    'theme': c_theme, 'navigation': c_navigation, 'hotkeys': c_hotkeys,
    'hotkeys-legacy': c_hotkeys_legacy, 'error-codes': c_error_codes,
    'export-presets': c_export_presets, 'clip-files': c_clip_files,
    'redaction': c_redaction, 'window-geometry': c_window_geometry,
    'capture-config': c_capture_config, 'capture-health': c_capture_health,
    'save-state': c_save_state,
}


def child_main(case):
    sys.path.insert(0, str(REF))
    payload = json.loads(sys.stdin.read() or 'null')
    result = CASES[case](payload)
    sys.stdout.write('\n@@FIXTURE@@' + json.dumps(
        _plain(result), sort_keys=True, ensure_ascii=False))


# --------------------------------------------------------------------------
# Parent side.
# --------------------------------------------------------------------------

def _rewrite_home(value, homes):
    if isinstance(value, str):
        for h in homes:
            value = value.replace(h, '$HOME')
        return value
    if isinstance(value, list):
        return [_rewrite_home(v, homes) for v in value]
    if isinstance(value, dict):
        return {_rewrite_home(k, homes): _rewrite_home(v, homes)
                for k, v in value.items()}
    return value


def run_case(case, payload=None, files=None):
    """Run one case in a fresh subprocess with a throwaway HOME."""
    with tempfile.TemporaryDirectory(prefix='fthr-gen-') as tmp:
        home = Path(tmp).resolve()
        for rel, content in (files or {}).items():
            p = home / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(json.dumps(content), encoding='utf-8')
        env = dict(os.environ, HOME=str(home), QT_QPA_PLATFORM='offscreen',
                   PYTHONHASHSEED='0', XDG_CONFIG_HOME=str(home / '.config'),
                   XDG_DATA_HOME=str(home / '.local' / 'share'))
        for var in ('XDG_VIDEOS_DIR', 'XDG_RUNTIME_DIR'):
            env.pop(var, None)
        proc = subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), '--case', case],
            input=json.dumps(payload), capture_output=True, text=True,
            env=env, cwd=str(ROOT))
        if proc.returncode != 0:
            raise RuntimeError(f'case {case} failed:\n{proc.stderr}')
        result = json.loads(proc.stdout.rsplit('@@FIXTURE@@', 1)[1])
        return _rewrite_home(result, {str(home), tmp})


def write_json(rel, data):
    path = OUT / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True,
                               ensure_ascii=False) + '\n', encoding='utf-8')


SETTINGS_CASES = {
    'legacy-no-clips-dir': {},
    'retired-keys': {
        'multiband_audio_enabled': True,
        'audio_categories': {'game': True},
        'auto_crop_enabled': True,
        'extended_clip_length': 120,
        'keyboard_overlay_enabled': True,
        'hotkeys': {'save_extended_clip': 'F10'},
    },
    'recording-seed': {'framerate': 120, 'resolution': '1080p',
                       'bitrate_level': 'high', 'custom_bitrate_kbps': 40000},
    'image-overlay-singular': {'image_overlay_enabled': True,
                               'image_overlay_path': '/tmp/x.png',
                               'image_overlay_opacity': 50},
    'camera-legacy': {'camera_position': 'top-left', 'camera_size': 'small'},
    'nav-v1': {'navigation_bindings_version': 1,
               'navigation_bindings': {
                   'editor_next': 'Right', 'editor_previous': 'Left',
                   'editor_minimize': 'Escape', 'editor_close': 'Shift+Escape',
                   'editor_split': 'Ctrl+S'}},
    'one-level-merge': {'source_volumes': {'game': 50}},
    'unknown-keys-kept': {'my_custom_key': 7},
}


def theme_cases():
    d = run_case('theme-defaults')
    return {
        'v1-untouched-defaults': {
            'palette_version': 1, 'colors': dict(d['legacy']['v1'])},
        'v1-user-override': {
            'palette_version': 1, 'colors': {'ACCENT': '#123456'}},
        'v3-global-tint': {
            'palette_version': 3, 'icon_tints': {'_global': '#b4bdc7'}},
        'segoe-font': {
            'typography_version': 1, 'fonts': {'body': 'Segoe UI'}},
        'missing-keys': {'colors': {'BG': '#000000'}},
    }


HOTKEY_LEGACY = {
    'flat': {'save_clip': 'ctrl+shift+s', 'save_screenshot': 'f12'},
    'nested-keyboard-controller': {
        'save_clip': {'keyboard': 'F10', 'controller': 'a+lb'},
        'save_screenshot': {'keyboard': 'f11', 'controller': ''}},
    'legacy-top-level-keyboard-map': {
        'keyboard': {'save_clip': 'f6', 'game_capture_accept': 'F5',
                     'game_capture_dismiss': 'F4'}},
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--case')
    args = ap.parse_args()
    if args.case:
        child_main(args.case)
        return

    write_json('settings-defaults.json', run_case('settings'))
    for name, data in SETTINGS_CASES.items():
        files = {'.fthr/settings.json': data}
        write_json(f'settings-migrations/{name}/input.json', data)
        write_json(f'settings-migrations/{name}/expected.json',
                   run_case('settings', files=files))

    write_json('theme-defaults.json', run_case('theme-defaults'))
    for name, data in theme_cases().items():
        write_json(f'theme-migrations/{name}/input.json', data)
        write_json(f'theme-migrations/{name}/expected.json',
                   run_case('theme', files={'.fthr/theme/theme.json': data}))

    nav = run_case('navigation')
    write_json('navigation-bindings.json', nav)

    hk = run_case('hotkeys')
    legacy = {}
    for name, data in HOTKEY_LEGACY.items():
        res = run_case('hotkeys-legacy', files={'.fthr/hotkeys.json': data})
        legacy[name] = {'file': data, **res}
    hk['legacy_files'] = legacy
    write_json('hotkeys.json', hk)

    write_json('error-codes.json', run_case('error-codes'))
    write_json('export-presets.json', run_case('export-presets'))
    write_json('clip-files.json', run_case('clip-files'))
    write_json('diagnostics-redaction.json', run_case('redaction'))
    write_json('window-geometry.json', run_case('window-geometry'))
    write_json('capture-config.json', run_case('capture-config'))
    write_json('save-state.json', run_case('save-state'))
    write_json('capture-health.json', run_case('capture-health'))


if __name__ == '__main__':
    main()
