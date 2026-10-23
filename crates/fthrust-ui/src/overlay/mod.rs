//! The capture-card overlay: a thread that owns the layer-shell surface
//! (created when a card shows, destroyed when it is gone) and renders the
//! card at 60 Hz; without layer-shell a desktop notification is sent instead.

pub mod capture_card;
pub mod surface;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use capture_card::{CardModel, content, paint, visual};
use fthrust_wayland::layer_shell;

use crate::services::CardKind;
use crate::tokens::Tokens;

/// Distance from the screen edges.
pub const MARGIN: i32 = 16;
/// Frame period.
pub const FRAME: Duration = Duration::from_micros(16_667);

enum Cmd {
    Show {
        kind: CardKind,
        hold_ms: Option<u32>,
        output: Option<String>,
    },
    Tokens(Box<Tokens>),
    Hide,
    Shutdown,
}

#[derive(Default)]
struct Stats {
    frames: AtomicU64,
    error: Mutex<Option<String>>,
}

/// The overlay handle (UI thread side).
pub struct Overlay {
    tx: Sender<Cmd>,
    stats: Arc<Stats>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Overlay {
    /// Starts the overlay thread; `output` is the preferred output name.
    pub fn start(tokens: Tokens, output: Option<String>, assets: PathBuf) -> Overlay {
        let (tx, rx) = channel();
        let stats = Arc::new(Stats::default());
        let thread = {
            let stats = stats.clone();
            std::thread::Builder::new()
                .name("fthrust-overlay".into())
                .spawn(move || run(rx, tokens, output, assets, stats))
                .ok()
        };
        Overlay { tx, stats, thread }
    }

    /// Shows a card (`hold_ms` overrides the default hold).
    pub fn show(&self, kind: CardKind, hold_ms: Option<u32>) {
        self.show_on(kind, hold_ms, None);
    }

    /// Shows a card on `output` (`None`/`"auto"` = highest refresh).
    pub fn show_on(&self, kind: CardKind, hold_ms: Option<u32>, output: Option<String>) {
        let _ = self.tx.send(Cmd::Show {
            kind,
            hold_ms,
            output,
        });
    }

    /// New theme tokens.
    pub fn set_tokens(&self, tokens: Tokens) {
        let _ = self.tx.send(Cmd::Tokens(Box::new(tokens)));
    }

    /// Hides the card.
    pub fn hide(&self) {
        let _ = self.tx.send(Cmd::Hide);
    }

    /// Frames rendered so far.
    pub fn frames_rendered(&self) -> u64 {
        self.stats.frames.load(Ordering::Relaxed)
    }

    /// The last surface error.
    pub fn last_error(&self) -> Option<String> {
        self.stats.error.lock().ok().and_then(|e| e.clone())
    }

    /// Stops the thread.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    rx: Receiver<Cmd>,
    mut tokens: Tokens,
    mut output: Option<String>,
    _assets: PathBuf,
    stats: Arc<Stats>,
) {
    let layer_ok = layer_shell::available();
    if !layer_ok {
        tracing::warn!("no layer-shell: capture cards fall back to desktop notifications");
    }
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx, &tokens);
    let mut model = CardModel::default();
    let mut surface: Option<surface::OverlaySurface> = None;
    let mut scale = 1.0f32;
    let epoch = Instant::now();
    let now = || epoch.elapsed().as_secs_f64();
    let notify_rt = (!layer_ok)
        .then(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()
        })
        .flatten();
    loop {
        let cmd = if model.is_live() {
            match rx.recv_timeout(FRAME) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => break,
            }
        };
        match cmd {
            Some(Cmd::Shutdown) => break,
            Some(Cmd::Tokens(t)) => {
                tokens = *t;
                crate::fonts::install(&ctx, &tokens);
            }
            Some(Cmd::Hide) => {
                model.hide();
                surface = None;
            }
            Some(Cmd::Show {
                kind,
                hold_ms,
                output: out,
            }) => {
                if let Some(o) = out {
                    if output.as_deref() != Some(o.as_str()) {
                        surface = None;
                    }
                    output = Some(o);
                }
                if !visual(&kind) {
                    continue;
                }
                if !layer_ok {
                    let c = content(&kind);
                    if let Some(rt) = &notify_rt {
                        rt.block_on(send_notification(&c.headline, &c.detail));
                    }
                    continue;
                }
                let was_compact = model.size();
                model.show(kind, hold_ms, now());
                if model.size() != was_compact {
                    surface = None;
                }
                if surface.is_none() {
                    let wanted = output.clone().unwrap_or_default();
                    let outputs = layer_shell::list_outputs().unwrap_or_default();
                    let chosen = surface::pick_output(&outputs, &wanted);
                    scale = chosen.map(|o| surface::scale_for(o.height)).unwrap_or(1.0);
                    let name = chosen.map(|o| o.name.clone());
                    let (w, h) = model.size();
                    let size = (
                        (w * scale).round() as u32 + MARGIN as u32,
                        (h * scale).round() as u32,
                    );
                    match surface::OverlaySurface::new(
                        name.as_deref(),
                        size,
                        (MARGIN, 0, 0, 0),
                        ctx.clone(),
                    ) {
                        Ok(s) => {
                            if let Ok(mut e) = stats.error.lock() {
                                *e = None;
                            }
                            surface = Some(s);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "capture card surface failed");
                            if let Ok(mut err) = stats.error.lock() {
                                *err = Some(e.to_string());
                            }
                            model.hide();
                        }
                    }
                }
            }
            None => {}
        }
        if !model.is_live() {
            continue;
        }
        let t = now();
        match model.frame(t) {
            None => surface = None,
            Some(frame) => {
                if let Some(s) = surface.as_mut() {
                    if !s.pump() {
                        surface = None;
                        model.hide();
                        continue;
                    }
                    let (w, h) = model.size();
                    let card_w = (w * scale).round();
                    let card_h = (h * scale).round();
                    let rect = egui::Rect::from_min_size(
                        egui::Pos2::new(s.size().0 as f32 - MARGIN as f32 - card_w, 0.0),
                        egui::Vec2::new(card_w, card_h),
                    );
                    let tokens_ref = &tokens;
                    let frame_ref = &frame;
                    if let Err(e) = s.render(t, |ctx, painter, _| {
                        paint(painter, ctx, tokens_ref, frame_ref, rect, scale);
                    }) {
                        tracing::warn!(error = %e, "capture card render failed");
                        if let Ok(mut err) = stats.error.lock() {
                            *err = Some(e.to_string());
                        }
                        surface = None;
                        model.hide();
                    } else {
                        stats.frames.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

async fn send_notification(title: &str, body: &str) {
    use ashpd::desktop::notification::{Notification, NotificationProxy};
    let result = async {
        let proxy = NotificationProxy::new().await?;
        let n = Notification::new(title).body(if body.is_empty() { None } else { Some(body) });
        proxy.add_notification("fthrust-capture-card", n).await
    };
    match tokio::time::timeout(Duration::from_secs(5), result).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "notification failed"),
        Err(_) => tracing::warn!("notification timed out"),
    }
}
