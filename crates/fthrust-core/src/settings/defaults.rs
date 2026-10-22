//! Reference default settings, written out key by key.

use crate::FthrPaths;
use serde_json::{Map, Value, json};

/// Returns the default settings map (84 keys) for the given home layout.
///
/// `clips_directory` and `recording_directory` are derived from `paths`.
pub fn defaults(paths: &FthrPaths) -> Map<String, Value> {
    let clips = paths.default_clips_directory();
    let recordings = clips.join("Recordings");
    let mut m = Map::new();
    m.insert("anticheat_detection_enabled".to_string(), json!(false));
    m.insert("audio_capture_enabled".to_string(), json!(true));
    m.insert("audio_capture_mode".to_string(), json!("combined"));
    m.insert("bitrate_level".to_string(), json!("medium"));
    m.insert("camera_device_index".to_string(), json!(0));
    m.insert("camera_enabled".to_string(), json!(false));
    m.insert(
        "camera_overlay_rect".to_string(),
        json!({"h": 0.34, "w": 0.25, "x": 0.72, "y": 0.64}),
    );
    m.insert("camera_position".to_string(), json!("bottom-right"));
    m.insert("camera_size".to_string(), json!("medium"));
    m.insert("capture_card_enabled".to_string(), json!(true));
    m.insert("capture_mode".to_string(), json!("desktop"));
    m.insert("capture_monitor".to_string(), json!(""));
    m.insert("clip_editor_enabled".to_string(), json!(true));
    m.insert("clip_editor_live_preview".to_string(), json!(true));
    m.insert("clip_length".to_string(), json!(30));
    m.insert(
        "clips_directory".to_string(),
        json!(clips.to_string_lossy()),
    );
    m.insert("close_to_tray".to_string(), json!(true));
    m.insert("codec_pref".to_string(), json!("auto"));
    m.insert("custom_bitrate_kbps".to_string(), json!(25000));
    m.insert("custom_clip_lengths".to_string(), json!([]));
    m.insert("editor_color_preview_enabled".to_string(), json!(true));
    m.insert("encoder_pref".to_string(), json!("auto"));
    m.insert("encoder_preset".to_string(), json!(4));
    m.insert("error_notifications_enabled".to_string(), json!(true));
    m.insert("export_quality".to_string(), json!("quality"));
    m.insert("framerate".to_string(), json!(60));
    m.insert("game_detection_custom_games".to_string(), json!([]));
    m.insert("game_detection_enabled".to_string(), json!(false));
    m.insert("game_detection_fallback_desktop".to_string(), json!(false));
    m.insert("game_detection_mode".to_string(), json!("auto"));
    m.insert("gary_image_path".to_string(), json!(null));
    m.insert("gary_max_level".to_string(), json!(85));
    m.insert("gary_min_level".to_string(), json!(55));
    m.insert("gary_mode_enabled".to_string(), json!(false));
    m.insert("grid_show_edited_duration".to_string(), json!(false));
    m.insert("hide_deleted_timeline_segments".to_string(), json!(false));
    m.insert(
        "hotkeys".to_string(),
        json!({"save_clip": "F9", "save_screenshot": "F12"}),
    );
    m.insert("image_overlay_enabled".to_string(), json!(false));
    m.insert("image_overlay_fit".to_string(), json!("fit"));
    m.insert("image_overlay_opacity".to_string(), json!(100));
    m.insert("image_overlay_path".to_string(), json!(""));
    m.insert(
        "image_overlay_rect".to_string(),
        json!({"h": 0.24, "w": 0.2, "x": 0.76, "y": 0.04}),
    );
    m.insert("image_overlays".to_string(), json!([]));
    m.insert("imported_clip_folders".to_string(), json!([]));
    m.insert("input_overlay_preview_background".to_string(), json!(""));
    m.insert("library_thumbnails_enabled".to_string(), json!(true));
    m.insert("master_volume".to_string(), json!(80));
    m.insert("mic_device_id".to_string(), json!(null));
    m.insert("mic_device_name".to_string(), json!(null));
    m.insert("mic_loopback".to_string(), json!(false));
    m.insert("mic_volume".to_string(), json!(100));
    m.insert("navigation_bindings".to_string(), json!({"category_next": "Shift+Right", "category_previous": "Shift+Left", "editor_close": "Escape", "editor_crop": "Ctrl+Q", "editor_delete": "Ctrl+D", "editor_export": "Ctrl+E", "editor_minimize": "Shift+Escape", "editor_next": "Tab+Right", "editor_play": "Space", "editor_previous": "Tab+Left", "editor_redo": "Ctrl+Y", "editor_redo_alt": "Ctrl+Shift+Z", "editor_restore": "Ctrl+W", "editor_split": "Ctrl+C", "editor_undo": "Ctrl+Z", "library_scroll_down": "Down", "library_scroll_up": "Up", "restart_capture": "Alt+Shift+R", "sidebar_next": "Shift+Down", "sidebar_previous": "Shift+Up", "switch_panel": "Tab+S", "toggle_detection": "Shift+G"}));
    m.insert("navigation_bindings_version".to_string(), json!(5));
    m.insert("notification_monitor".to_string(), json!("auto"));
    m.insert("notification_sounds_enabled".to_string(), json!(true));
    m.insert("parallel_export_limit".to_string(), json!(2));
    m.insert("parallel_exports".to_string(), json!(false));
    m.insert("pause_ui_in_background".to_string(), json!(true));
    m.insert("quick_crop".to_string(), json!(null));
    m.insert("recording_bitrate_level".to_string(), json!("medium"));
    m.insert("recording_custom_bitrate_kbps".to_string(), json!(25000));
    m.insert(
        "recording_directory".to_string(),
        json!(recordings.to_string_lossy()),
    );
    m.insert("recording_framerate".to_string(), json!(60));
    m.insert("recording_resolution".to_string(), json!("source"));
    m.insert("reduce_ui_animations".to_string(), json!(false));
    m.insert("resolution".to_string(), json!("source"));
    m.insert("scaling_mode".to_string(), json!("stretch"));
    m.insert("selected_export_preset".to_string(), json!("discord"));
    m.insert("settings_visual_previews_enabled".to_string(), json!(true));
    m.insert("shortcut_categories_enabled".to_string(), json!({}));
    m.insert("shortcuts_enabled".to_string(), json!(true));
    m.insert("show_kofi_widget".to_string(), json!(true));
    m.insert("sound_volume_clip".to_string(), json!(100));
    m.insert("sound_volume_error".to_string(), json!(100));
    m.insert("sound_volume_screenshot".to_string(), json!(100));
    m.insert("sound_volume_startup".to_string(), json!(100));
    m.insert("sound_volume_upload_failed".to_string(), json!(100));
    m.insert("sound_volume_upload_successful".to_string(), json!(100));
    m.insert(
        "source_volumes".to_string(),
        json!({"browser": 100, "discord": 100, "game": 100, "music": 100}),
    );
    m.insert("target_hwnd".to_string(), json!(0));
    m.insert("target_window_name".to_string(), json!(""));
    m.insert("third_party_keyboard".to_string(), json!({"color": "#00ff00", "enabled": false, "hwnd": 0, "intensity": 58, "rect": {"h": 0.25, "w": 0.4, "x": 0.3, "y": 0.7}, "window_name": ""}));
    m.insert("timeline_preview_strip_enabled".to_string(), json!(true));
    m.insert("watermark_enabled".to_string(), json!(false));
    m
}
