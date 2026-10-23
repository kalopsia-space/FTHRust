//! The GlobalShortcuts portal (`hotkey_portal.py`): bind the configured
//! actions, adopt what the desktop actually bound, and deliver activations.

use std::collections::{BTreeMap, BTreeSet};

use fthrust_core::hotkeys::{
    HotkeyAction, HotkeyBindings, normalize_keyboard_combo, to_portal_trigger,
};
use futures_util::StreamExt;

use super::{Event, Outbox};

/// `_on_portal_bound`: adopts the desktop's triggers (first alternative,
/// normalised) and lists the actions where the desktop kept a different key.
/// Returns `(changed, kept)`.
pub fn adopt_bound(
    bindings: &mut HotkeyBindings,
    requested: &BTreeMap<String, String>,
    cleared: &BTreeSet<String>,
    bound: &[(String, String)],
) -> (bool, Vec<String>) {
    let mut changed = false;
    let mut kept = Vec::new();
    for (action, trigger) in bound {
        if trigger.is_empty() || cleared.contains(action) {
            continue;
        }
        let Some(current) = bindings.get(action).map(str::to_owned) else {
            continue;
        };
        let first = trigger.split(',').next().unwrap_or("").trim();
        let actual = normalize_keyboard_combo(first).unwrap_or_default();
        if let Some(req) = requested.get(action)
            && !req.is_empty()
            && actual != *req
        {
            let desc = HotkeyAction::parse(action)
                .map(|a| a.description())
                .unwrap_or_else(|| action.clone());
            kept.push(format!("{desc}: {trigger}"));
        }
        if actual != current {
            bindings.set_keyboard(action, &actual);
            changed = true;
        }
    }
    (changed, kept)
}

/// The `DESKTOP KEPT ITS SHORTCUT` detail.
pub fn kept_warning(kept: &[String]) -> String {
    format!(
        "Your desktop manages these keys and kept {}. Change them in your desktop shortcut settings.",
        kept.join(", ")
    )
}

/// The triggers requested from the portal (`action` → normalised combo).
pub fn requested_triggers(
    bindings: &HotkeyBindings,
    cleared: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    bindings
        .keyboard_map()
        .iter()
        .filter(|(a, _)| !cleared.contains(*a))
        .map(|(a, c)| (a.clone(), c.clone()))
        .collect()
}

/// A running portal session.
pub struct PortalShortcuts {
    handle: tokio::runtime::Handle,
    task: tokio::task::JoinHandle<()>,
    configure_tx: tokio::sync::mpsc::UnboundedSender<Msg>,
}

enum Msg {
    Configure,
    Rebind(Vec<(String, String, String)>),
    Stop,
}

impl PortalShortcuts {
    /// Creates the session on `handle` and binds `bindings` (minus `cleared`).
    pub fn start(
        handle: tokio::runtime::Handle,
        outbox: Outbox,
        bindings: &HotkeyBindings,
        cleared: &BTreeSet<String>,
    ) -> Self {
        let (configure_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let shortcuts = shortcut_list(bindings, cleared);
        let task = handle.spawn(run(outbox, shortcuts, rx));
        PortalShortcuts {
            handle,
            task,
            configure_tx,
        }
    }

    /// Opens the desktop's shortcut configuration dialog.
    pub fn configure(&self) {
        let _ = self.configure_tx.send(Msg::Configure);
    }

    /// Re-binds after the bindings changed.
    pub fn rebind(&self, bindings: &HotkeyBindings, cleared: &BTreeSet<String>) {
        let _ = self
            .configure_tx
            .send(Msg::Rebind(shortcut_list(bindings, cleared)));
    }

    /// Closes the session.
    pub fn stop(self) {
        let _ = self.configure_tx.send(Msg::Stop);
        let task = self.task;
        let _ = self.handle.block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(500), task).await
        });
    }
}

fn shortcut_list(
    bindings: &HotkeyBindings,
    cleared: &BTreeSet<String>,
) -> Vec<(String, String, String)> {
    bindings
        .keyboard_map()
        .iter()
        .filter(|(a, _)| !cleared.contains(*a))
        .filter_map(|(a, combo)| {
            let action = HotkeyAction::parse(a)?;
            let trigger = if combo.is_empty() {
                String::new()
            } else {
                to_portal_trigger(combo)
            };
            Some((a.clone(), action.description(), trigger))
        })
        .collect()
}

async fn run(
    outbox: Outbox,
    shortcuts: Vec<(String, String, String)>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Msg>,
) {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    // Host apps need an app id (xdg-desktop-portal 1.18+ refuses GlobalShortcuts
    // sessions without one); registration fails harmlessly until a desktop
    // file for the id is installed.
    match ashpd::AppID::try_from(super::APP_ID) {
        Ok(id) => match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ashpd::register_host_app(id),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::info!(error = %e, "portal host-app registration unavailable"),
            Err(_) => tracing::info!("portal host-app registration timed out"),
        },
        Err(e) => tracing::warn!(error = %e, "invalid portal app id"),
    }
    let proxy =
        match tokio::time::timeout(std::time::Duration::from_secs(5), GlobalShortcuts::new()).await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                outbox.send(Event::PortalShortcutsFailed(format!(
                    "GlobalShortcuts portal unavailable: {e}"
                )));
                return;
            }
            Err(_) => {
                outbox.send(Event::PortalShortcutsFailed(
                    "GlobalShortcuts portal did not answer.".into(),
                ));
                return;
            }
        };
    let session = match proxy.create_session(Default::default()).await {
        Ok(s) => s,
        Err(e) => {
            outbox.send(Event::PortalShortcutsFailed(format!(
                "GlobalShortcuts session failed: {e}"
            )));
            return;
        }
    };
    let mut activated = match proxy.receive_activated().await {
        Ok(s) => s,
        Err(e) => {
            outbox.send(Event::PortalShortcutsFailed(format!(
                "GlobalShortcuts signals failed: {e}"
            )));
            return;
        }
    };
    let mut changed = proxy.receive_shortcuts_changed().await.ok();
    let bind = |list: Vec<(String, String, String)>| {
        let proxy = &proxy;
        let session = &session;
        async move {
            let new: Vec<NewShortcut> = list
                .iter()
                .map(|(id, desc, trig)| {
                    let s = NewShortcut::new(id.clone(), desc.clone());
                    if trig.is_empty() {
                        s
                    } else {
                        s.preferred_trigger(trig.as_str())
                    }
                })
                .collect();
            let req = proxy
                .bind_shortcuts(session, &new, None, Default::default())
                .await?;
            let bound = req.response()?;
            Ok::<Vec<(String, String)>, ashpd::Error>(
                bound
                    .shortcuts()
                    .iter()
                    .map(|s| (s.id().to_owned(), s.trigger_description().to_owned()))
                    .collect(),
            )
        }
    };
    match tokio::time::timeout(std::time::Duration::from_secs(60), bind(shortcuts)).await {
        Ok(Ok(bound)) => outbox.send(Event::PortalShortcutsBound(bound)),
        Ok(Err(e)) => {
            outbox.send(Event::PortalShortcutsFailed(format!(
                "GlobalShortcuts bind failed: {e}"
            )));
            return;
        }
        Err(_) => {
            outbox.send(Event::PortalShortcutsFailed(
                "GlobalShortcuts bind timed out.".into(),
            ));
            return;
        }
    }
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(Msg::Configure) => {
                    if let Err(e) = proxy.configure_shortcuts(&session, None, Default::default()).await {
                        outbox.send(Event::PortalShortcutsFailed(format!("Could not open the desktop shortcut settings: {e}")));
                    }
                }
                Some(Msg::Rebind(list)) => match bind(list).await {
                    Ok(bound) => outbox.send(Event::PortalShortcutsBound(bound)),
                    Err(e) => outbox.send(Event::PortalShortcutsFailed(format!("GlobalShortcuts bind failed: {e}"))),
                },
                Some(Msg::Stop) | None => break,
            },
            a = activated.next() => match a {
                Some(a) => {
                    if let Some(action) = HotkeyAction::parse(a.shortcut_id()) {
                        outbox.send(Event::Hotkey(action));
                    }
                }
                None => break,
            },
            c = async {
                match changed.as_mut() {
                    Some(s) => s.next().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(c) = c {
                    let bound = c.shortcuts().iter().map(|s| (s.id().to_owned(), s.trigger_description().to_owned())).collect();
                    outbox.send(Event::PortalShortcutsBound(bound));
                }
            }
        }
    }
    let _ = session.close().await;
}
