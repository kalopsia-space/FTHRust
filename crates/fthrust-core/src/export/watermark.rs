//! The animated "CAPTURED WITH FTHRClips" card (`capture_card_watermark`):
//! four filter-graph snippets appended after the video is labelled
//! `vbase`. It shows for the first 2.9 s of the exported timeline.

/// Card colours (`0xRRGGBB`) and asset paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatermarkStyle {
    /// `CAPTURE_CARD_BG`.
    pub bg: String,
    /// `CAPTURE_CARD_ACCENT`.
    pub accent: String,
    /// `CAPTURE_CARD_TEXT`.
    pub text: String,
    /// `CAPTURE_CARD_DIVIDER`.
    pub divider: String,
    /// `CAPTURE_CARD_STATS_DIM`.
    pub stats_dim: String,
    /// `assets/fonts/Oswald-Bold.ttf` (unescaped).
    pub font_path: String,
    /// `assets/favicon.ico` (unescaped).
    pub icon_path: String,
}

/// `_filter_path`: forward slashes, escaped `:` and `'`.
pub fn filter_path(path: &str) -> String {
    path.replace('\\', "/")
        .replace(':', r"\:")
        .replace('\'', r"\'")
}

/// `_ffmpeg_color`: `#RGB`/`#RRGGBB` → `0xRRGGBB`, anything else black.
pub fn ffmpeg_color(value: &str) -> String {
    let clean = value.trim().trim_start_matches('#');
    let clean: String = if clean.chars().count() == 3 {
        clean.chars().flat_map(|c| [c, c]).collect()
    } else {
        clean.to_owned()
    };
    if clean.chars().count() == 6 {
        format!("0x{clean}")
    } else {
        "0x000000".into()
    }
}

/// The four snippets: card, icon, card+icon, the timed overlay onto `base`.
pub fn filters(style: &WatermarkStyle, base: &str, output: &str) -> [String; 4] {
    let font = filter_path(&style.font_path);
    let icon = filter_path(&style.icon_path);
    let card = format!(
        "color=c={bg}@0.92:s=320x76:r=60,format=rgba,colorchannelmixer=aa=0.92,\
drawbox=x=0:y=0:w=3:h=ih:color={accent}@1:t=fill,\
drawbox=x=76:y=12:w=1:h=52:color={divider}@1:t=fill,\
drawtext=fontfile='{font}':text='CAPTURED WITH':fontcolor={dim}:fontsize=10:x=94:y=10,\
drawtext=fontfile='{font}':text='FTHRClips':fontcolor={text}:fontsize=22:x=94:y=29[fthr_watermark]",
        bg = style.bg,
        accent = style.accent,
        divider = style.divider,
        dim = style.stats_dim,
        text = style.text,
    );
    let icon = format!(
        "movie=filename='{icon}':loop=1,format=rgba,scale=48:48:flags=neighbor,\
pad=48:48:(ow-iw)/2:(oh-ih)/2:color=black@0[fthr_watermark_icon]"
    );
    let composed = "[fthr_watermark][fthr_watermark_icon]overlay=x=14:y=14:format=auto:eof_action=repeat[fthr_watermark_composed]".to_owned();
    let x = "if(lt(t,0.38),W-(w+24)*(1-pow(1-t/0.38,3)),if(lt(t,2.55),W-w-24,W-w-24+(w+24)*pow(min(1,(t-2.55)/0.35),3)))";
    let overlay = format!(
        "[{base}][fthr_watermark_composed]overlay=x='{x}':y='H-h-24':enable='between(t,0,2.9)':eof_action=pass[{output}]"
    );
    [card, icon, composed, overlay]
}
