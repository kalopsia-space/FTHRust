//! The Upload section of the General tab (`UploadSettingsWidget`): the
//! enable checkbox with its consent dialogs, the provider forms, test
//! connection, saved SFTP passwords, auto-upload mode and Save. Requests
//! that need the network go to the uploader program on a thread.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use egui::{Align, Ui, Vec2};
use fthrust_core::upload::settings::Provider;
use fthrust_core::upload::{CATBOX_SUPPORT_URL, validate};
use serde_json::{Map, Value, json};

use super::space;
use crate::fonts::Weight;
use crate::services::upload::{ChildSlot, OPEN_SETTINGS_ACTION, Reply, Timeouts, run_request};
use crate::shell::error_bar::{Action, Level};
use crate::state::{AppState, DialogKind, DialogResult};
use crate::tokens::{Color, size};
use crate::widgets::buttons::{self, Kind, Opts};
use crate::widgets::check;
use crate::widgets::combo::Combo;
use crate::widgets::input::{LineEditOpts, line_edit_with};
use crate::widgets::section;

/// The provider combo's entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Catbox.
    Catbox,
    /// VaultScope (SFTP with a sign-up link and login paste).
    VaultScope,
    /// Your own HTTP endpoint.
    Custom,
    /// SFTP.
    Sftp,
}

/// Combo order.
pub const PROVIDERS: [Choice; 4] = [
    Choice::Catbox,
    Choice::VaultScope,
    Choice::Custom,
    Choice::Sftp,
];

impl Choice {
    /// Combo text.
    pub fn label(self) -> &'static str {
        match self {
            Choice::Catbox => "Catbox",
            Choice::VaultScope => "VaultScope",
            Choice::Custom => "Your server",
            Choice::Sftp => "SFTP",
        }
    }

    /// Combo data.
    pub fn id(self) -> &'static str {
        match self {
            Choice::Catbox => "catbox",
            Choice::VaultScope => "vaultscope",
            Choice::Custom => "custom",
            Choice::Sftp => "sftp",
        }
    }

    fn from_id(id: &str) -> Choice {
        PROVIDERS
            .into_iter()
            .find(|c| c.id() == id)
            .unwrap_or(Choice::Catbox)
    }

    fn is_sftp(self) -> bool {
        matches!(self, Choice::Sftp | Choice::VaultScope)
    }

    /// The provider stored in the settings file.
    fn stored(self) -> &'static str {
        if self.is_sftp() { "sftp" } else { self.id() }
    }
}

/// Which parts of the form show for a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panels {
    /// The User hash field.
    pub catbox: bool,
    /// Server URL and Auth header.
    pub custom: bool,
    /// The SFTP fields.
    pub sftp: bool,
    /// The VaultScope offer and paste button.
    pub vaultscope: bool,
    /// `Support Catbox hosting`.
    pub support: bool,
    /// The provider's open button text.
    pub open: Option<&'static str>,
}

/// `_apply_provider`.
pub fn panels(c: Choice) -> Panels {
    Panels {
        catbox: c == Choice::Catbox,
        custom: c == Choice::Custom,
        sftp: c.is_sftp(),
        vaultscope: c == Choice::VaultScope,
        support: c == Choice::Catbox,
        open: match c {
            Choice::Catbox => Some("OPEN CATBOX"),
            Choice::VaultScope => Some("OPEN VAULTSCOPE"),
            _ => None,
        },
    }
}

/// Auto-upload modes: id and radio text.
pub const MODES: [(&str, &str); 3] = [
    ("immediate", "Upload immediately after capture"),
    ("interval", "Upload on an interval"),
    (
        "manual",
        "Manual only  (right-click a clip \u{2192} Upload)",
    ),
];
/// The interval combo's values.
pub const INTERVAL_VALUES: [u32; 7] = [1, 2, 5, 10, 15, 30, 60];
const UNITS: [&str; 3] = ["minutes", "hours", "days"];

/// The interval controls are live only in interval mode.
pub fn interval_enabled(mode: &str) -> bool {
    mode == "interval"
}

/// The SFTP note under the fields.
pub const SFTP_NOTE: &str = "Leave Host key blank to use a trusted key from your SSH known_hosts file or a key previously verified by FTHR. For a new host, enter the SHA256 fingerprint from the server operator and test once; FTHR remembers the verified key. Unknown or changed keys are rejected before sending credentials. Passwords are saved in your desktop keyring. Files get unique names. For sharing links, enter the HTTPS address that serves this exact remote folder. Enabling public links makes new files readable by the web server. Your server must already provide public web access; the URL setting does not set up hosting or call a publishing API. Leave it disabled for private SFTP storage.";
const VAULTSCOPE_URL: &str = "https://vaultscope.de/ps/fthr";
const NOT_TESTED: &str = "Not tested";

/// How the status label is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// TEXT_DIM.
    Neutral,
    /// Success colour.
    Ok,
    /// Error colour.
    Error,
}

enum Job {
    Test,
    Save,
    Forget,
}

/// The form's fields (the widget's line edits and toggles).
pub struct UploadForm {
    /// Provider.
    pub choice: Choice,
    /// Catbox user hash.
    pub userhash: String,
    /// Your server URL.
    pub server_url: String,
    /// Authorization header value.
    pub auth_header: String,
    /// SFTP host.
    pub sftp_host: String,
    /// SFTP port (text).
    pub sftp_port: String,
    /// SFTP user.
    pub sftp_username: String,
    /// Password typed this session (never loaded or stored).
    pub sftp_password: String,
    /// Remote folder.
    pub sftp_directory: String,
    /// Host-key pin.
    pub sftp_fingerprint: String,
    /// Public HTTPS links on.
    pub sftp_public_enabled: bool,
    /// Public folder URL.
    pub sftp_public_base_url: String,
    /// `immediate`, `interval` or `manual`.
    pub mode: String,
    /// Interval value (combo text).
    pub interval_value: String,
    /// `minutes`, `hours` or `days`.
    pub interval_unit: String,
    /// Compress oversized clips.
    pub auto_compress: bool,
    /// Delete after upload.
    pub auto_delete: bool,
    /// Status label text.
    pub status: String,
    /// Status label colour.
    pub status_kind: StatusKind,
    /// A connection test runs.
    pub testing: bool,
    status_reset_at: Option<f64>,
    pending: Option<(Job, Receiver<Reply>)>,
}

fn text(v: Option<&Value>, default: &str) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => default.to_owned(),
    }
}

impl UploadForm {
    /// `_load_settings`.
    pub fn load(s: &fthrust_core::upload::settings::UploaderSettings) -> UploadForm {
        let choice = match s.provider() {
            Provider::Sftp if s.str("sftp_provider_name") == "VaultScope" => Choice::VaultScope,
            Provider::Sftp => Choice::Sftp,
            Provider::Custom => Choice::Custom,
            Provider::Catbox => Choice::Catbox,
        };
        let interval = s
            .get("upload_interval_value")
            .and_then(Value::as_u64)
            .filter(|v| INTERVAL_VALUES.contains(&(*v as u32)))
            .unwrap_or(5);
        let unit = s.str("upload_interval_unit");
        UploadForm {
            choice,
            userhash: s.str("catbox_userhash").to_owned(),
            server_url: s.str("upload_server_url").to_owned(),
            auth_header: s.str("upload_auth_header").to_owned(),
            sftp_host: s.str("sftp_host").to_owned(),
            sftp_port: text(s.get("sftp_port"), "22"),
            sftp_username: s.str("sftp_username").to_owned(),
            sftp_password: String::new(),
            sftp_directory: match s.str("sftp_directory") {
                "" => ".".into(),
                d => d.to_owned(),
            },
            sftp_fingerprint: s.str("sftp_fingerprint").to_owned(),
            sftp_public_enabled: s.flag("sftp_public_enabled"),
            sftp_public_base_url: s.str("sftp_public_base_url").to_owned(),
            mode: match s.str("upload_mode") {
                m @ ("immediate" | "interval" | "manual") => m.to_owned(),
                _ => "manual".into(),
            },
            interval_value: interval.to_string(),
            interval_unit: if UNITS.contains(&unit) {
                unit.to_owned()
            } else {
                "minutes".into()
            },
            auto_compress: s.flag("upload_auto_compress"),
            auto_delete: s.flag("upload_auto_delete"),
            status: NOT_TESTED.into(),
            status_kind: StatusKind::Neutral,
            testing: false,
            status_reset_at: None,
            pending: None,
        }
    }

    /// `_sftp_config`: the SFTP fields, trimmed except the password.
    pub fn sftp_config(&self) -> Map<String, Value> {
        let mut m = Map::new();
        for (k, v) in [
            ("sftp_host", &self.sftp_host),
            ("sftp_port", &self.sftp_port),
            ("sftp_username", &self.sftp_username),
            ("sftp_directory", &self.sftp_directory),
            ("sftp_fingerprint", &self.sftp_fingerprint),
            ("sftp_public_base_url", &self.sftp_public_base_url),
        ] {
            m.insert(k.into(), json!(v.trim()));
        }
        m.insert("sftp_password".into(), json!(self.sftp_password));
        m.insert(
            "sftp_public_enabled".into(),
            json!(self.sftp_public_enabled),
        );
        m
    }

    /// The `test_connection` config.
    pub fn test_config(&self) -> Map<String, Value> {
        let mut m = self.sftp_config();
        m.insert("upload_provider".into(), json!(self.choice.stored()));
        m.insert("catbox_userhash".into(), json!(self.userhash.trim()));
        m.insert("upload_server_url".into(), json!(self.server_url.trim()));
        m.insert("upload_auth_header".into(), json!(self.auth_header.trim()));
        m
    }

    fn set_status(&mut self, text: &str, kind: StatusKind) {
        self.status = text.to_owned();
        self.status_kind = kind;
        self.status_reset_at = None;
    }
}

/// The form, loaded on first use.
pub fn form(state: &mut AppState) -> &mut UploadForm {
    if state.upload_form.is_none() {
        state.upload_form = Some(UploadForm::load(&state.uploader_settings));
    }
    state.upload_form.as_mut().expect("loaded")
}

/// A request is running.
pub fn busy(state: &AppState) -> bool {
    state
        .upload_form
        .as_ref()
        .is_some_and(|f| f.pending.is_some())
}

fn save_settings(state: &mut AppState, failure_title: &str) -> bool {
    match state.uploader_settings.save(&state.paths) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "uploader settings not saved");
            state.dialogs.push_message(
                failure_title,
                "The uploader setting could not be saved. Please try again.",
            );
            false
        }
    }
}

/// `_on_enabled_changed`: on asks for consent first (then Catbox's), off
/// saves at once.
pub fn set_enabled(state: &mut AppState, on: bool, _now: f64) {
    if !on {
        state.uploader_settings.set("upload_enabled", json!(false));
        save_settings(state, "Upload Extension");
        return;
    }
    if !state.uploader_settings.consent_current() {
        state.consent_checks = [false; 2];
        state.dialogs.push_modal(DialogKind::UploadConsent);
        return;
    }
    enable_now(state);
}

fn enable_now(state: &mut AppState) {
    state.uploader_settings.set("upload_enabled", json!(true));
    if !save_settings(state, "Upload Extension") {
        state.uploader_settings.set("upload_enabled", json!(false));
        return;
    }
    ensure_catbox(state, "enable");
}

/// `_ensure_provider_ready`: Catbox needs its consent; asks and returns
/// false when it is missing.
fn ensure_catbox(state: &mut AppState, context: &str) -> bool {
    if form(state).choice != Choice::Catbox || state.uploader_settings.catbox_consent_current() {
        return true;
    }
    state.consent_checks = [false; 2];
    state.dialogs.push_modal(DialogKind::CatboxConsent {
        context: context.to_owned(),
    });
    false
}

/// Picks a provider; switching to Catbox without its consent asks, and
/// cancelling reverts.
pub fn select_provider(state: &mut AppState, choice: Choice) {
    let previous = form(state).choice;
    if previous == choice {
        return;
    }
    form(state).choice = choice;
    if state.uploader_settings.enabled() {
        ensure_catbox(state, &format!("provider:{}", previous.id()));
    }
}

fn spawn(state: &mut AppState, job: Job, action: &str, config: Map<String, Value>) {
    let timeouts = Timeouts::default();
    let timeout = match job {
        Job::Test => timeouts.test,
        Job::Save | Job::Forget => timeouts.credentials,
    };
    let exe: Option<PathBuf> = state.uploader_exe.clone();
    let request = json!({"action": action, "config": config});
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let reply = match exe {
            Some(exe) => run_request(&exe, &request, timeout, &ChildSlot::default()),
            None => Reply::fail("The uploader component is missing from this installation."),
        };
        let _ = tx.send(reply);
    });
    form(state).pending = Some((job, rx));
}

/// `_on_test_connection`.
pub fn test_connection(state: &mut AppState) {
    if busy(state) || !ensure_catbox(state, "test") {
        return;
    }
    let f = form(state);
    if f.choice == Choice::Custom && f.server_url.trim().is_empty() {
        f.set_status("Enter a server URL first", StatusKind::Neutral);
        return;
    }
    f.set_status("Testing…", StatusKind::Neutral);
    f.testing = true;
    let config = f.test_config();
    spawn(state, Job::Test, "test_connection", config);
}

/// `_on_save`: SFTP credentials go through the uploader first.
pub fn save(state: &mut AppState, now: f64) {
    if busy(state) {
        return;
    }
    if state.uploader_settings.enabled() && !ensure_catbox(state, "save") {
        return;
    }
    if form(state).choice.is_sftp() {
        let config = form(state).sftp_config();
        spawn(state, Job::Save, "sftp_save", config);
        return;
    }
    write_all(state, now);
}

fn write_all(state: &mut AppState, now: f64) {
    let f = form(state);
    let choice = f.choice;
    f.server_url = validate::save_server_url(&f.server_url);
    let interval = f.interval_value.trim().parse::<u32>().unwrap_or(5);
    let values = [
        ("upload_provider", json!(choice.stored())),
        ("catbox_userhash", json!(f.userhash.trim())),
        ("upload_server_url", json!(f.server_url)),
        ("upload_auth_header", json!(f.auth_header.trim())),
        ("upload_mode", json!(f.mode)),
        ("upload_interval_value", json!(interval)),
        ("upload_interval_unit", json!(f.interval_unit)),
        ("upload_auto_compress", json!(f.auto_compress)),
        ("upload_auto_delete", json!(f.auto_delete)),
    ];
    let sftp = choice.is_sftp().then(|| f.sftp_config());
    if sftp.is_some() {
        f.sftp_password.clear();
    }
    for (k, v) in values {
        state.uploader_settings.set(k, v);
    }
    if let Some(cfg) = sftp {
        for (k, v) in cfg {
            if k != "sftp_password" {
                state.uploader_settings.set(&k, v);
            }
        }
        let name = if choice == Choice::VaultScope {
            "VaultScope"
        } else {
            ""
        };
        state
            .uploader_settings
            .set("sftp_provider_name", json!(name));
    }
    if save_settings(state, "Uploader Settings") {
        let f = form(state);
        f.set_status("Settings saved", StatusKind::Neutral);
        f.status_reset_at = Some(now + 3.0);
    }
}

/// `_forget_sftp_password`.
pub fn forget_password(state: &mut AppState) {
    if busy(state) {
        return;
    }
    let config = form(state).sftp_config();
    spawn(state, Job::Forget, "sftp_forget", config);
}

/// `_paste_vaultscope_login`: fills the SFTP fields from the copied JSON.
pub fn paste_vaultscope(state: &mut AppState, raw: &str) {
    match validate::vaultscope_paste(raw) {
        Ok(login) => {
            let f = form(state);
            f.sftp_host = login.host;
            f.sftp_port = login.port.to_string();
            f.sftp_username = login.username;
            f.sftp_password = login.password;
            f.sftp_directory = ".".into();
            f.sftp_fingerprint.clear();
            f.sftp_public_base_url.clear();
            f.sftp_public_enabled = false;
            f.set_status(
                "VaultScope login filled in. Test the connection, then save settings.",
                StatusKind::Neutral,
            );
        }
        Err(message) => state.dialogs.push_message("VaultScope login", message),
    }
}

/// Dialog answers, finished requests, the status reset and a pending
/// clipboard paste; runs every frame.
pub fn service(state: &mut AppState, now: f64) {
    let mut keep = Vec::new();
    for r in std::mem::take(&mut state.dialogs.results) {
        match r {
            DialogResult::UploadConsent(accepted) => {
                if accepted {
                    state
                        .uploader_settings
                        .accept_terms(&fthrust_core::upload::utc_now_iso());
                    enable_now(state);
                }
            }
            DialogResult::CatboxConsent { accepted, context } => {
                catbox_answer(state, accepted, &context, now)
            }
            other => keep.push(other),
        }
    }
    state.dialogs.results.extend(keep);
    if let Some(text) = state.clipboard_paste.take() {
        paste_vaultscope(state, &text);
    }
    let Some(f) = state.upload_form.as_mut() else {
        return;
    };
    if f.status_reset_at.is_some_and(|t| now >= t) {
        f.set_status(NOT_TESTED, StatusKind::Neutral);
    }
    let reply = match &f.pending {
        Some((_, rx)) => match rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Reply::fail("")),
        },
        None => None,
    };
    let Some(reply) = reply else {
        return;
    };
    let Some((job, _)) = f.pending.take() else {
        return;
    };
    match job {
        Job::Test => {
            f.testing = false;
            let mut message = reply.message.trim().to_owned();
            if !reply.ok && message.is_empty() {
                message = "The upload provider did not return a reason.".into();
            }
            let kind = if reply.ok {
                StatusKind::Ok
            } else {
                StatusKind::Error
            };
            f.set_status(&message, kind);
            if !reply.ok {
                state.error_bar.push_error(
                    "UPLOAD CONNECTION FAILED",
                    &message,
                    Level::Warning,
                    vec![Action {
                        label: "OPEN UPLOAD SETTINGS".into(),
                        id: OPEN_SETTINGS_ACTION.into(),
                    }],
                    Some("upload-connection".into()),
                    true,
                );
            }
        }
        Job::Save => {
            if reply.ok {
                write_all(state, now);
            } else {
                state.dialogs.push_message("SFTP settings", reply.message);
            }
        }
        Job::Forget => {
            if reply.ok {
                f.sftp_password.clear();
                f.set_status("Saved SFTP password removed", StatusKind::Neutral);
            } else {
                state.dialogs.push_message("SFTP password", reply.message);
            }
        }
    }
}

fn catbox_answer(state: &mut AppState, accepted: bool, context: &str, now: f64) {
    if accepted {
        state
            .uploader_settings
            .accept_catbox(&fthrust_core::upload::utc_now_iso());
        if !save_settings(state, "Uploader") {
            return;
        }
        match context {
            "save" => save(state, now),
            "test" => test_connection(state),
            _ => {}
        }
        return;
    }
    if context == "enable" {
        state.uploader_settings.set("upload_enabled", json!(false));
        save_settings(state, "Upload Extension");
    } else if let Some(previous) = context.strip_prefix("provider:") {
        form(state).choice = Choice::from_id(previous);
    }
}

fn field(
    ui: &mut Ui,
    state: &AppState,
    label: &str,
    value: &mut String,
    placeholder: &str,
    password: bool,
    enabled: bool,
) {
    let t = &state.tokens;
    section::label_wide(
        ui,
        t,
        label,
        size::LABEL,
        Weight::W600,
        Color::TextDim,
        None,
    );
    space(ui, 4.0);
    line_edit_with(
        ui,
        t,
        value,
        &format!("QLineEdit[{label}]"),
        &LineEditOpts {
            placeholder: placeholder.to_owned(),
            password,
            enabled,
            ..LineEditOpts::default()
        },
    );
    space(ui, 10.0);
}

fn button(ui: &mut Ui, state: &AppState, kind: Kind, text: &str, enabled: bool) -> egui::Response {
    buttons::button_with(
        ui,
        &state.tokens,
        None,
        kind,
        text,
        &format!("QPushButton[{text}]"),
        &Opts {
            enabled,
            ..Opts::default()
        },
    )
}

/// `Support Catbox hosting`, shared with the General tab's styling.
type SupportButton<'a> = &'a dyn Fn(&mut Ui, &crate::tokens::Tokens, &str, &str) -> egui::Response;

/// Draws the Upload section at width `w`.
pub fn ui(ui: &mut Ui, state: &mut AppState, w: f32) {
    ui_with(ui, state, w, &|ui, t, text, name| {
        buttons::button_with(ui, t, None, Kind::Secondary, text, name, &Opts::default())
    });
}

/// [`ui`] with the General tab's support-button painter.
pub fn ui_with(ui: &mut Ui, state: &mut AppState, w: f32, support: SupportButton<'_>) {
    let t = state.tokens.clone();
    let now = ui.ctx().input(|i| i.time);
    let _ = w;
    section::flat_header(ui, &t, "Upload", "Upload");
    space(ui, 12.0);
    let mut on = state.uploader_settings.enabled();
    if check::checkbox_with(
        ui,
        &t,
        &mut on,
        "Enable optional clip uploader",
        true,
        true,
        "QCheckBox[Enable optional clip uploader]",
    )
    .changed()
    {
        set_enabled(state, on, now);
    }
    space(ui, 16.0);
    let p = panels(form(state).choice);
    if p.support
        && support(
            ui,
            &t,
            "Support Catbox hosting",
            "QPushButton[Support Catbox hosting]",
        )
        .clicked()
    {
        crate::shell::open_url(CATBOX_SUPPORT_URL);
    }
    if !state.uploader_settings.enabled() {
        return;
    }
    space(ui, 24.0);
    section::flat_header(ui, &t, "Host", "Host");
    space(ui, 12.0);
    // Provider row.
    let current = PROVIDERS
        .iter()
        .position(|c| *c == form(state).choice)
        .unwrap_or(0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);
        section::label(
            ui,
            &t,
            "Provider",
            size::BODY,
            Weight::W400,
            Color::Text,
            None,
        );
        let mut combo = Combo::new(
            PROVIDERS.iter().map(|c| c.label().to_owned()).collect(),
            current,
        )
        .with_width(220.0);
        if combo
            .ui(
                ui,
                &t,
                &mut state.icons,
                &format!("QComboBox[{}]", PROVIDERS[current].label()),
            )
            .changed()
        {
            select_provider(state, PROVIDERS[combo.selected]);
        }
        if let Some(text) = p.open
            && button(ui, state, Kind::Outline, text, true).clicked()
        {
            crate::shell::open_url(if text == "OPEN CATBOX" {
                "https://catbox.moe/"
            } else {
                VAULTSCOPE_URL
            });
        }
    });
    space(ui, 14.0);
    let p = panels(form(state).choice);
    let mut f = state.upload_form.take().expect("loaded");
    if p.catbox {
        field(
            ui,
            state,
            "User hash",
            &mut f.userhash,
            "Optional Catbox account userhash",
            false,
            true,
        );
    }
    if p.custom {
        field(
            ui,
            state,
            "Server URL",
            &mut f.server_url,
            "https://your-server.example.com/upload",
            false,
            true,
        );
        field(
            ui,
            state,
            "Auth header",
            &mut f.auth_header,
            "Bearer token123  (optional)",
            false,
            true,
        );
    }
    let mut paste = false;
    let mut forget = false;
    if p.sftp {
        if p.vaultscope {
            section::label_wrapped(
                ui,
                &t,
                "FTHR exclusive: get one month free when you sign up through our VaultScope link.",
                size::LABEL,
                Weight::W400,
                Color::Text,
                ui.available_width(),
                0.0,
            );
            space(ui, 8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);
                if button(
                    ui,
                    state,
                    Kind::Primary,
                    "Get one month free at VaultScope",
                    true,
                )
                .clicked()
                {
                    crate::shell::open_url(VAULTSCOPE_URL);
                }
                if button(ui, state, Kind::Outline, "Paste VaultScope login", true).clicked() {
                    paste = true;
                }
            });
            space(ui, 12.0);
        }
        field(
            ui,
            state,
            "Host",
            &mut f.sftp_host,
            "sftp.example.com",
            false,
            true,
        );
        field(ui, state, "Port", &mut f.sftp_port, "22", false, true);
        field(
            ui,
            state,
            "Username",
            &mut f.sftp_username,
            "Your SFTP username",
            false,
            true,
        );
        field(
            ui,
            state,
            "Password",
            &mut f.sftp_password,
            "Blank keeps the saved password for this host and user",
            true,
            true,
        );
        field(
            ui,
            state,
            "Remote folder",
            &mut f.sftp_directory,
            ". (home directory) or /uploads",
            false,
            true,
        );
        field(
            ui,
            state,
            "Host key (optional)",
            &mut f.sftp_fingerprint,
            "Blank uses trusted known_hosts; SHA256:… pins a specific key",
            false,
            true,
        );
        field(
            ui,
            state,
            "Public folder URL",
            &mut f.sftp_public_base_url,
            "https://videos.example.com/clips/",
            false,
            f.sftp_public_enabled,
        );
        check::checkbox(
            ui,
            &t,
            &mut f.sftp_public_enabled,
            "Generate public HTTPS sharing links for SFTP uploads",
            "QCheckBox[Generate public HTTPS sharing links]",
        );
        space(ui, 10.0);
        section::label_wrapped(
            ui,
            &t,
            SFTP_NOTE,
            size::LABEL,
            Weight::W400,
            Color::TextDim,
            ui.available_width(),
            0.0,
        );
        space(ui, 10.0);
        forget = button(
            ui,
            state,
            Kind::Outline,
            "Forget saved password for this host and user",
            f.pending.is_none(),
        )
        .clicked();
        space(ui, 12.0);
    }
    // Test connection and status.
    let mut test = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(12.0, 0.0);
        test = button(ui, state, Kind::Outline, "Test connection", !f.testing).clicked();
        let color = match f.status_kind {
            StatusKind::Neutral => Color::TextDim,
            StatusKind::Ok => Color::Success,
            StatusKind::Error => Color::Error,
        };
        section::label_fit(
            ui,
            &t,
            &f.status,
            size::LABEL,
            Weight::W400,
            color,
            "uploadStatus",
        );
    });
    // Auto upload.
    space(ui, 24.0);
    section::flat_header(ui, &t, "Auto Upload", "Auto Upload");
    space(ui, 12.0);
    for (id, label) in MODES {
        if check::radio(ui, &t, f.mode == id, label, &format!("QRadioButton[{id}]")).clicked() {
            f.mode = id.to_owned();
        }
        space(ui, 6.0);
    }
    let live = interval_enabled(&f.mode);
    ui.add_enabled_ui(live, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);
            section::label(ui, &t, "Every", size::BODY, Weight::W400, Color::Text, None);
            let values: Vec<String> = INTERVAL_VALUES.iter().map(u32::to_string).collect();
            let sel = values
                .iter()
                .position(|v| *v == f.interval_value)
                .unwrap_or(2);
            let mut combo = Combo::new(values.clone(), sel).with_width(90.0);
            if combo
                .ui(ui, &t, &mut state.icons, "QComboBox[interval]")
                .changed()
            {
                f.interval_value = values[combo.selected].clone();
            }
            let usel = UNITS
                .iter()
                .position(|u| *u == f.interval_unit)
                .unwrap_or(0);
            let mut units =
                Combo::new(UNITS.iter().map(|u| u.to_string()).collect(), usel).with_width(130.0);
            if units
                .ui(ui, &t, &mut state.icons, "QComboBox[unit]")
                .changed()
            {
                f.interval_unit = UNITS[units.selected].to_owned();
            }
        });
    });
    space(ui, 24.0);
    section::flat_header(ui, &t, "Upload Processing", "Upload Processing");
    space(ui, 12.0);
    check::checkbox(
        ui,
        &t,
        &mut f.auto_compress,
        "Automatically compress oversized clips before upload",
        "QCheckBox[Automatically compress oversized clips]",
    );
    space(ui, 24.0);
    section::flat_header(ui, &t, "Post-Upload", "Post-Upload");
    space(ui, 12.0);
    check::checkbox(
        ui,
        &t,
        &mut f.auto_delete,
        "Auto-delete local clip after a confirmed successful upload",
        "QCheckBox[Auto-delete local clip after upload]",
    );
    space(ui, 20.0);
    let save_clicked = ui
        .with_layout(egui::Layout::left_to_right(Align::Min), |ui| {
            button(
                ui,
                state,
                Kind::Primary,
                "Save upload settings",
                f.pending.is_none(),
            )
            .clicked()
        })
        .inner;
    state.upload_form = Some(f);
    if paste {
        state.clipboard_read_requested = true;
    }
    if forget {
        forget_password(state);
    }
    if test {
        test_connection(state);
    }
    if save_clicked {
        save(state, now);
    }
    if busy(state) {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}
