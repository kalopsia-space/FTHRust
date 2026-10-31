//! The portal flow against a scripted ScreenCast portal on a private D-Bus
//! daemon: every failure closes the session, the token Start returns is
//! saved before anything else can fail, and a declined dialog keeps the
//! old token.

#[path = "../../fthrust-engine/tests/private_bus/mod.rs"]
mod private_bus;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fthrust_core::FthrPaths;
use fthrust_portal::capture::PortalCapture;
use fthrust_portal::screencast::{PortalFailure, Timeouts};
use fthrust_portal::token;
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, interface};

/// What the fake portal does with `Start`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Start {
    /// The user dismisses the dialog.
    Declined,
    /// The dialog never answers.
    Silent,
    /// Accepted, with a new restore token.
    Accepted,
}

struct Script {
    start: Start,
    log: Vec<String>,
}

type Shared = Arc<Mutex<Script>>;

fn option(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options
        .get(key)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_default()
}

fn handle(header: &Header<'_>, kind: &str, token: &str) -> String {
    let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
    let id = sender.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/{kind}/{id}/{token}")
}

async fn respond(
    conn: &Connection,
    header: &Header<'_>,
    request: &str,
    code: u32,
    results: HashMap<&str, Value<'_>>,
) {
    let sender = header.sender().map(|s| s.to_owned());
    conn.emit_signal(
        sender,
        request,
        "org.freedesktop.portal.Request",
        "Response",
        &(code, results),
    )
    .await
    .unwrap();
}

struct FakeScreenCast(Shared);

#[interface(name = "org.freedesktop.portal.ScreenCast")]
impl FakeScreenCast {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        5
    }

    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let request = handle(&header, "request", &option(&options, "handle_token"));
        let session = handle(
            &header,
            "session",
            &option(&options, "session_handle_token"),
        );
        self.0.lock().unwrap().log.push("CreateSession".into());
        conn.object_server()
            .at(session.as_str(), FakeSession(self.0.clone()))
            .await?;
        let results = HashMap::from([("session_handle", Value::from(session.as_str()))]);
        respond(conn, &header, &request, 0, results).await;
        Ok(OwnedObjectPath::try_from(request).unwrap())
    }

    async fn select_sources(
        &self,
        _session: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let request = handle(&header, "request", &option(&options, "handle_token"));
        let presented = option(&options, "restore_token");
        self.0
            .lock()
            .unwrap()
            .log
            .push(format!("SelectSources token={presented}"));
        respond(conn, &header, &request, 0, HashMap::new()).await;
        Ok(OwnedObjectPath::try_from(request).unwrap())
    }

    async fn start(
        &self,
        _session: OwnedObjectPath,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let request = handle(&header, "request", &option(&options, "handle_token"));
        let start = {
            let mut script = self.0.lock().unwrap();
            script.log.push("Start".into());
            script.start
        };
        match start {
            Start::Declined => respond(conn, &header, &request, 1, HashMap::new()).await,
            Start::Silent => {}
            Start::Accepted => {
                let streams = vec![(42u32, HashMap::<String, Value>::new())];
                let results = HashMap::from([
                    ("streams", Value::from(streams)),
                    ("restore_token", Value::from("issued-by-start")),
                ]);
                respond(conn, &header, &request, 0, results).await;
            }
        }
        Ok(OwnedObjectPath::try_from(request).unwrap())
    }

    async fn open_pipe_wire_remote(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        self.0.lock().unwrap().log.push("OpenPipeWireRemote".into());
        Err(zbus::fdo::Error::Failed("no PipeWire here".into()))
    }
}

struct FakeSession(Shared);

#[interface(name = "org.freedesktop.portal.Session")]
impl FakeSession {
    async fn close(&self) {
        self.0.lock().unwrap().log.push("Close".into());
    }
}

/// Runs one open with `start` scripted and returns the result and the log.
fn open_with(
    script: &Shared,
    start: Start,
    paths: &FthrPaths,
    cancel: Arc<AtomicBool>,
) -> (Result<(), PortalFailure>, Vec<String>) {
    *script.lock().unwrap() = Script {
        start,
        log: Vec::new(),
    };
    let timeouts = Timeouts {
        dialog: Duration::from_secs(5),
        method: Duration::from_secs(5),
    };
    let result = PortalCapture::open(paths, 30, cancel, timeouts).map(|_| ());
    // The close runs before open returns; give the fake a moment to log it.
    std::thread::sleep(Duration::from_millis(100));
    let log = script.lock().unwrap().log.clone();
    (result, log)
}

#[test]
fn failed_opens_close_the_session_and_keep_the_right_token() {
    let Some(_bus) = private_bus::PrivateBus::start() else {
        return eprintln!("skipping: dbus-daemon is not available");
    };
    let script: Shared = Arc::new(Mutex::new(Script {
        start: Start::Declined,
        log: Vec::new(),
    }));
    let portal_runtime = tokio::runtime::Runtime::new().unwrap();
    let _portal = portal_runtime
        .block_on(async {
            zbus::connection::Builder::session()?
                .name("org.freedesktop.portal.Desktop")?
                .serve_at(
                    "/org/freedesktop/portal/desktop",
                    FakeScreenCast(script.clone()),
                )?
                .build()
                .await
        })
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(home.path());
    let token_path = token::path(&paths);
    let no_cancel = || Arc::new(AtomicBool::new(false));

    // The user dismisses the dialog: the session is closed and the token
    // that was presented stays for next time.
    token::save(&token_path, "saved").unwrap();
    let (result, log) = open_with(&script, Start::Declined, &paths, no_cancel());
    assert_eq!(result, Err(PortalFailure::UserDeclined), "{log:?}");
    assert!(
        log.contains(&"SelectSources token=saved".to_string()),
        "{log:?}"
    );
    assert_eq!(log.last().map(String::as_str), Some("Close"), "{log:?}");
    assert_eq!(token::load(&token_path).unwrap().as_deref(), Some("saved"));

    // Start accepts and issues a new token (the presented one is spent),
    // then OpenPipeWireRemote fails: the new token is already saved, and
    // the session is closed.
    let (result, log) = open_with(&script, Start::Accepted, &paths, no_cancel());
    assert!(
        matches!(result, Err(PortalFailure::Failed(_))),
        "{result:?}"
    );
    assert!(log.contains(&"OpenPipeWireRemote".to_string()), "{log:?}");
    assert_eq!(log.last().map(String::as_str), Some("Close"), "{log:?}");
    assert_eq!(
        token::load(&token_path).unwrap().as_deref(),
        Some("issued-by-start")
    );

    // Capture is cancelled while the dialog is up: the session is closed
    // (which dismisses the dialog).
    let cancel = no_cancel();
    let flag = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::SeqCst);
    });
    let (result, log) = open_with(&script, Start::Silent, &paths, cancel);
    canceller.join().unwrap();
    assert_eq!(result, Err(PortalFailure::Cancelled), "{log:?}");
    assert_eq!(log.last().map(String::as_str), Some("Close"), "{log:?}");
    assert_eq!(
        token::load(&token_path).unwrap().as_deref(),
        Some("issued-by-start")
    );
}
