#!/usr/bin/env python3
"""Render the reference Qt shell offscreen and dump per-screen acceptance
geometry for the egui port.

Run from the repository root with the reference venv:

    QT_QPA_PLATFORM=offscreen .venv-reference/bin/python tools/render-qt-reference.py [OUT_DIR]

OUT_DIR defaults to docs/reference/qt-screens. For every screen it writes
<screen>.png (what Qt rendered) and <screen>.json (the widgets a port must
reproduce: rect, sampled colours mapped to palette tokens, text metrics).
This file started as a copy of reference/tools/render_ui_preview.py.
"""
from __future__ import annotations

import json
import logging
import os
import sys
import tempfile
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
REF = ROOT / 'reference'
sys.path.insert(0, str(REF / 'FTHR_UI'))
os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')

from PySide6.QtCore import QPoint, QRect  # noqa: E402
from PySide6.QtGui import QFontInfo, QFontMetrics, QImage  # noqa: E402
from PySide6.QtWidgets import (  # noqa: E402
    QAbstractButton, QApplication, QComboBox, QLabel, QLineEdit, QWidget,
)
from PySide6.QtTest import QTest  # noqa: E402

SCHEMA_VERSION = 1


class _CaptureCard:
    def __init__(self, *_args):
        pass

    def __getattr__(self, _name):
        return lambda *_args, **_kwargs: None


class _CaptureBridge:
    def get_audio_mappings(self):
        return {}


def _hex(rgb) -> str:
    return '#%02x%02x%02x' % (rgb.red(), rgb.green(), rgb.blue())


def _palette():
    from core.theme_manager import DEFAULT_COLORS
    return {v.lower(): k for k, v in DEFAULT_COLORS.items()}


def _widget_name(w: QWidget) -> str | None:
    name = QWidget.objectName(w)
    text = ''
    if isinstance(w, (QAbstractButton, QLabel)):
        text = w.text().replace('\n', ' ').strip()
    elif isinstance(w, QComboBox):
        text = w.currentText().strip()
    elif isinstance(w, QLineEdit):
        text = (w.text() or w.placeholderText()).strip()
    if name:
        # Shared object names (tab buttons, rows) get the text as a key.
        if text and name in ('settingsTabBtn', 'hotkeyActionRow', 'popupPanel',
                             'topClusterMain') or (text and _shared_name(w, name)):
            return f'{name}[{text[:40]}]'
        return name
    if text:
        return f'{type(w).__name__}[{text[:40]}]'
    return None


def _shared_name(w: QWidget, name: str) -> bool:
    parent = w.parentWidget()
    if parent is None:
        return False
    return sum(1 for s in parent.findChildren(QWidget) if s.objectName() == name) > 1


def _text_info(w: QWidget):
    if isinstance(w, (QAbstractButton, QLabel, QLineEdit, QComboBox)):
        if isinstance(w, QComboBox):
            text = w.currentText()
        elif isinstance(w, QLineEdit):
            text = w.text() or w.placeholderText()
        else:
            text = w.text()
        if not text.strip():
            return None
        font = QWidget.font(w)
        px = font.pixelSize()
        if px <= 0:
            px = round(font.pointSizeF() * 96.0 / 72.0)
        fm = QFontMetrics(font)
        info = QFontInfo(font)
        return {
            'matched_family': info.family(),
            'matched_weight': int(info.weight()),
            'matched_px': info.pixelSize(),
            'content': text.replace('\n', ' ')[:120],
            'px': px,
            'weight': int(font.weight()),
            'family': font.family(),
            'ascent': fm.ascent(),
            'descent': fm.descent(),
            'height': fm.height(),
            'leading': fm.leading(),
            'line_spacing': fm.lineSpacing(),
            'avg_char_width': fm.averageCharWidth(),
            'bounding_height': fm.boundingRect(text.split('\n')[0]).height(),
            'bounding_width': fm.boundingRect(text.split('\n')[0]).width(),
            'advance': fm.horizontalAdvance(text.split('\n')[0]),
        }
    return None


def dump_widgets(root: QWidget, image: QImage, palette: dict) -> list:
    """Every visible, named-or-textual descendant of `root`, in paint order.
    Repeated names get `#2`, `#3`, ... so each entry can be addressed."""
    out = []
    seen = {}
    root_rect = QRect(0, 0, QWidget.width(root), QWidget.height(root))
    for w in root.findChildren(QWidget):
        if not QWidget.isVisible(w) or QWidget.width(w) <= 0 or QWidget.height(w) <= 0:
            continue
        if QWidget.visibleRegion(w).isEmpty():
            continue
        name = _widget_name(w)
        if name is None:
            continue
        seen[name] = seen.get(name, 0) + 1
        if seen[name] > 1:
            name = f'{name}#{seen[name]}'
        top_left = QWidget.mapTo(w, root, QPoint(0, 0))
        rect = QRect(top_left, QWidget.size(w)).intersected(root_rect)
        if rect.isEmpty():
            continue
        x, y, wd, ht = rect.x(), rect.y(), rect.width(), rect.height()
        samples = []
        for role, (sx, sy) in (('fill', (x + wd // 2, y + ht // 2)),
                               ('edge', (x, y + ht // 2)),
                               ('top', (x + wd // 2, y))):
            if 0 <= sx < image.width() and 0 <= sy < image.height():
                value = _hex(image.pixelColor(sx, sy))
                samples.append({'role': role, 'at': [sx, sy], 'rgb': value,
                                'token': palette.get(value)})
        entry = {
            'name': name,
            'class': type(w).__name__,
            'rect': [x, y, wd, ht],
            'enabled': QWidget.isEnabled(w),
            'samples': samples,
        }
        if isinstance(w, QAbstractButton) and w.isCheckable():
            entry['checked'] = w.isChecked()
        text = _text_info(w)
        if text:
            entry['text'] = text
        out.append(entry)
    return out


def _dump_editor_variants(editor, profile: Path, output: Path, palette: dict) -> None:
    """Editor states the default `clip-editor` dump does not show (plan 6b)."""
    from core.playback_mix_model import PlaybackSource
    from ui.clip_viewer import AudioMixPanel, CropDialog, StretchDialog
    viewer = editor.clip_page
    # Sidebar: two sources in the mix, every panel expanded, tall enough to
    # show the whole content without scrolling.
    old = viewer._audio_mix_panel
    sources = (
        PlaybackSource('desktop', 'Desktop', 'desktop', 1, 0),
        PlaybackSource('mic', 'Mic', 'microphone', 2, 1),
    )
    mix = AudioMixPanel(master_vol=80, source_tracks=sources)
    old.parentWidget().layout().replaceWidget(old, mix)
    old.hide()
    sidebar = editor.findChild(QWidget, 'editorSidebarContent')
    for header in sidebar.findChildren(QAbstractButton, 'panelHeader'):
        if not header.isChecked():
            header.click()
    editor.resize(1440, 2200)
    QTest.qWait(280)
    dump_screen(sidebar, 'clip-editor-sidebar', output, palette)
    editor.resize(1440, 900)
    QTest.qWait(120)
    # Timeline: two splits, the middle segment deleted and selected.
    trim = viewer.trim_slider
    trim.set_playhead(0.3)
    trim.split_at_playhead()
    trim.set_playhead(0.6)
    trim.split_at_playhead()
    trim.selected_segment = 1
    trim.toggle_selected_deleted()
    trim.selection_changed.emit(1, True)
    trim.set_playhead(0.45)
    viewer._zoom_timeline(1.25)
    QTest.qWait(200)
    dump_screen(editor.findChild(QWidget, 'trimBar'), 'clip-editor-timeline', output, palette)
    clip = str(profile / 'Overwatch_clip.mp4')
    crop = CropDialog(clip)
    crop.show()
    QTest.qWait(280)
    dump_screen(crop, 'clip-editor-crop', output, palette)
    crop.close()
    stretch = StretchDialog(clip, 0, 1920, 1080)
    stretch.show()
    QTest.qWait(280)
    dump_screen(stretch, 'clip-editor-stretch', output, palette)
    stretch.close()


def _dump_editor_rail(window, profile: Path, output: Path, palette: dict) -> None:
    """The main window with two parked editors in the navigation rail."""
    window._set_main_page(0)
    for name in ('Overwatch_clip.mp4', 'Valorant_clip.mp4'):
        window._add_editor_session(str(profile / name))
    QTest.qWait(280)
    dump_screen(window, 'clip-editor-rail', output, palette)


def dump_screen(root: QWidget, name: str, output: Path, palette: dict,
                crop: QRect | None = None) -> dict:
    image = QWidget.grab(root).toImage()
    if crop is not None:
        image = image.copy(crop)
    image.save(str(output / f'{name}.png'))
    widgets = dump_widgets(root, QWidget.grab(root).toImage(), palette)
    if crop is not None:
        shifted = []
        for entry in widgets:
            x, y, w, h = entry['rect']
            r = QRect(x, y, w, h).intersected(crop)
            if r.isEmpty():
                continue
            entry['rect'] = [r.x() - crop.x(), r.y() - crop.y(), r.width(), r.height()]
            for s in entry['samples']:
                s['at'] = [s['at'][0] - crop.x(), s['at'][1] - crop.y()]
            shifted.append(entry)
        widgets = shifted
    data = {
        'schema': SCHEMA_VERSION,
        'screen': name,
        'size': [image.width(), image.height()],
        'pixels_per_point': 1,
        'widgets': widgets,
    }
    (output / f'{name}.json').write_text(json.dumps(data, indent=1, ensure_ascii=False) + '\n',
                                         encoding='utf-8')
    return data


def main(argv=None) -> Path:
    argv = sys.argv[1:] if argv is None else argv
    output = Path(argv[0]) if argv else ROOT / 'docs' / 'reference' / 'qt-screens'
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='fthr-ui-preview-') as temp, ExitStack() as stack:
        profile = Path(temp)
        stack.enter_context(patch('pathlib.Path.home', return_value=profile))
        original_streams = sys.stdout, sys.stderr
        import main as application
        from ui.app_style import apply_app_style
        from ui.clip_grid import ClipThumbnail
        from ui.dialogs import FthrInputDialog, FthrMessageDialog
        from ui.export_presets_widget import ExportPresetDialog

        app = QApplication.instance() or QApplication([])
        application._load_fonts()
        apply_app_style(app)
        palette = _palette()
        stack.enter_context(patch.object(application, 'CaptureCardClient', _CaptureCard))
        stack.enter_context(patch.object(application, 'CaptureBridge', _CaptureBridge))
        for name in ('_setup_hotkeys', '_start_mic_recorder', 'start_engine', 'stop_engine'):
            stack.enter_context(patch.object(application.MainWindow, name, lambda *_: None))
        stack.enter_context(patch.object(application._SettingsPage, '_start_encoder_probe', lambda *_: None))
        window = application.MainWindow()
        window.resize(1440, 900)
        window.show()
        QTest.qWait(180)
        settings = window.main_stack.widget(1)
        window._set_main_page(1)
        QTest.qWait(280)
        dump_screen(window, 'settings', output, palette)
        for index, name in ((1, 'clip-settings'), (2, 'audio'), (3, 'visuals'),
                            (4, 'customize'), (5, 'performance'), (6, 'shortcuts')):
            settings._tab_buttons[index].setChecked(True)
            settings.stack.setCurrentIndex(index)
            QTest.qWait(280)
            dump_screen(window, name, output, palette)
        settings._tab_buttons[4].setChecked(True)
        settings.stack.setCurrentIndex(4)
        QTest.qWait(280)
        settings._customize_page._typography_section.toggle()
        QTest.qWait(280)
        dump_screen(window, 'typography', output, palette)
        settings._customize_page._typography_section.collapse()
        settings._customize_page._colors_section.expand()
        QTest.qWait(280)
        dump_screen(window, 'colors', output, palette)
        settings._customize_page._colors_section.collapse()
        for width, height in ((1920, 1080), (2560, 1440), (3840, 2160)):
            window.resize(width, height)
            QTest.qWait(280)
            dump_screen(window, f'customize-{width}', output, palette)
        window.resize(1440, 900)
        window._set_main_page(0)
        QTest.qWait(280)
        dump_screen(window, 'library', output, palette)
        QTest.mouseMove(window.record_btn, window.record_btn.rect().center())
        QTest.qWait(40)
        dump_screen(window, 'record-hover', output, palette)
        window.resize(1100, 760)
        QTest.qWait(280)
        dump_screen(window, 'library-small', output, palette)
        window.resize(1440, 900)
        QTest.qWait(280)
        # Error bar states (cropped to the bar).
        window.error_bar.push('CAPTURE FAILED', 'The compositor stopped answering',
                              level='error', actions=[('RESTART ENGINE', lambda: None)])
        QTest.qWait(120)
        bar = window.error_bar
        bar_rect = QRect(QWidget.mapTo(bar, window, QPoint(0, 0)), QWidget.size(bar))
        dump_screen(window, 'error-bar-error', output, palette, crop=bar_rect)
        window.error_bar.push('LIMITED FEATURE SUPPORT', 'Game detection is unavailable on this compositor',
                              level='warning', actions=[('LEARN MORE', lambda: None)], key='compositor')
        window.error_bar.push('MIC NOT FOUND', 'No microphone is connected', level='warning')
        QTest.qWait(120)
        bar_rect = QRect(QWidget.mapTo(bar, window, QPoint(0, 0)), QWidget.size(bar))
        dump_screen(window, 'error-bar-warning', output, palette, crop=bar_rect)
        for _ in range(3):
            window.error_bar._dismiss_current() if hasattr(window.error_bar, '_dismiss_current') else None
        # Top-bar popups.
        window.cap_settings_popup.show_below(window.cap_btn)
        QTest.qWait(280)
        dump_screen(window.cap_settings_popup, 'capture-popup', output, palette)
        window.cap_settings_popup.hide()
        for popup, button, name in ((window.source_popup, window.source_btn, 'source-popup'),
                                    (window.game_detection_popup, window.game_detection_btn,
                                     'game-detection-popup'),
                                    (window.hotkey_popup, window.hotkey_btn, 'hotkey-popup')):
            popup.show_below(button)
            QTest.qWait(280)
            dump_screen(popup, name, output, palette)
            popup.hide()
        # Dialogs.
        dialog = FthrMessageDialog('Clip not found', 'The clip is no longer on disk.', window)
        dialog.show()
        QTest.qWait(200)
        dump_screen(dialog, 'dialog-message', output, palette)
        dialog.close()
        dialog = FthrMessageDialog('Delete clip', 'Delete Overwatch_clip.mp4?\nThis cannot be undone.',
                                   window, question=True)
        dialog.show()
        QTest.qWait(200)
        dump_screen(dialog, 'dialog-question', output, palette)
        dialog.close()
        dialog = FthrInputDialog('Rename clip', 'New name', 'Overwatch_clip', window)
        dialog.show()
        QTest.qWait(200)
        dump_screen(dialog, 'dialog-input', output, palette)
        dialog.close()
        dialog = application.CustomClipLengthDialog(30, window)
        dialog.show()
        QTest.qWait(200)
        dump_screen(dialog, 'custom-clip-length', output, palette)
        dialog.close()
        dialog = ExportPresetDialog(None, window)
        dialog.show()
        QTest.qWait(200)
        dump_screen(dialog, 'export-preset', output, palette)
        dialog.close()
        # Clip card and editor (plans 5 and 6 consume these).
        card = ClipThumbnail(str(profile / 'Overwatch_clip_from_04Oct2026_10-30-00.mp4'),
                             is_video=True, card_width=320, upload_enabled=True)
        card.set_image_thumbnail(QImage(str(REF / 'FTHR_UI/assets/preview_desktop.png')))
        card.show()
        QTest.qWait(280)
        dump_screen(card, 'clip-card', output, palette)
        QTest.mouseMove(card.share_btn, card.share_btn.rect().center())
        QTest.qWait(40)
        dump_screen(card, 'clip-card-hover', output, palette)
        card._show_action_feedback('Upload requested')
        dump_screen(card, 'clip-card-feedback', output, palette)
        card.close()
        from ui.clip_viewer import ClipViewer
        from ui.clip_editor_window import ClipEditorWindow
        with ExitStack() as editor_patches:
            editor_patches.enter_context(patch('ui.clip_grid.get_cached_clip_metadata',
                                              return_value=(10, 1920, 1080, 60, 25_000_000, 25_000_000)))
            for method in ('_set_media_source', '_prepare_playback_source',
                           '_discover_audio_sources_async', '_load_timeline_thumbnails_if_idle'):
                editor_patches.enter_context(patch.object(ClipViewer, method, lambda *_: None))
            editor = ClipEditorWindow(str(profile / 'Overwatch_clip.mp4'), None,
                                      settings_manager=window.settings_manager,
                                      thumb_pixmap=card._thumb_pixmap)
            editor.resize(1440, 900)
            editor.show()
            QTest.qWait(280)
            dump_screen(editor, 'clip-editor', output, palette)
            _dump_editor_variants(editor, profile, output, palette)
            editor.close()
        _dump_editor_rail(window, profile, output, palette)
        window._shutdown_requested = True
        window._perform_full_shutdown()
        window.close()
        logging.shutdown()
        for stream in (sys.stdout, sys.stderr):
            if isinstance(stream, application._LogTee) and stream._log is not None:
                stream._log.close()
        sys.stdout, sys.stderr = original_streams
    return output


if __name__ == '__main__':
    print(main())
