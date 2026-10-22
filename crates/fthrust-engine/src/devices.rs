//! Audio endpoint listing for `--list-playback-devices` and
//! `--list-microphones`, in the reference's JSON shape.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use pipewire as pw;
use pipewire::types::ObjectType;
use serde_json::json;

/// One sink or source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// PipeWire node name (the PulseAudio sink/source name).
    pub endpoint_id: String,
    /// Description for the UI; the engine substitutes the id when empty.
    pub display_name: String,
    /// Whether it is the session default.
    pub is_default: bool,
}

/// `{"schema_version":1,"microphones":[...]}` on one line (the key is
/// `microphones` for both listings, as in the reference).
pub fn listing_json(endpoints: &[Endpoint]) -> String {
    let items: Vec<_> = endpoints
        .iter()
        .map(|e| {
            json!({
                "endpoint_id": e.endpoint_id,
                "display_name": if e.display_name.is_empty() { &e.endpoint_id } else { &e.display_name },
                "device_state": 1,
                "is_default": e.is_default,
            })
        })
        .collect();
    json!({"schema_version": 1, "microphones": items}).to_string()
}

/// `{"schema_version":1,"error":"..."}`.
pub fn error_json(message: &str) -> String {
    json!({"schema_version": 1, "error": message}).to_string()
}

/// Enumeration time limit.
pub const ENUMERATE_TIMEOUT: Duration = Duration::from_secs(3);

static PW_INIT: OnceLock<()> = OnceLock::new();

/// Initialises libpipewire once per process.
pub fn init_pipewire() {
    PW_INIT.get_or_init(pw::init);
}

/// Playback sinks (`media.class = Audio/Sink`), default first-marked.
pub fn enumerate_sinks() -> Result<Vec<Endpoint>, String> {
    enumerate("Audio/Sink", "default.audio.sink")
}

/// Capture sources (`media.class = Audio/Source`).
pub fn enumerate_sources() -> Result<Vec<Endpoint>, String> {
    enumerate("Audio/Source", "default.audio.source")
}

#[derive(Default)]
struct Scan {
    nodes: BTreeMap<u32, (String, String)>,
    default_name: Option<String>,
    phase: u8,
    timed_out: bool,
}

fn enumerate(
    media_class: &'static str,
    default_key: &'static str,
) -> Result<Vec<Endpoint>, String> {
    init_pipewire();
    let main_loop =
        pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("pipewire main loop: {e}"))?;
    let context = pw::context::ContextRc::new(&main_loop, None)
        .map_err(|e| format!("pipewire context: {e}"))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("cannot connect to PipeWire: {e}"))?;
    let registry = core
        .get_registry_rc()
        .map_err(|e| format!("pipewire registry: {e}"))?;
    let scan = Rc::new(RefCell::new(Scan::default()));
    let metadata_proxies: Rc<
        RefCell<Vec<(pw::metadata::Metadata, pw::metadata::MetadataListener)>>,
    > = Rc::new(RefCell::new(Vec::new()));

    let registry_weak = registry.downgrade();
    let scan_for_registry = scan.clone();
    let proxies = metadata_proxies.clone();
    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            let Some(props) = global.props else { return };
            match global.type_ {
                ObjectType::Node => {
                    if props.get("media.class") == Some(media_class) {
                        let name = props.get("node.name").unwrap_or("").to_string();
                        let display = props
                            .get("node.description")
                            .or_else(|| props.get("node.nick"))
                            .unwrap_or("")
                            .to_string();
                        if !name.is_empty() {
                            scan_for_registry
                                .borrow_mut()
                                .nodes
                                .insert(global.id, (name, display));
                        }
                    }
                }
                ObjectType::Metadata => {
                    if props.get("metadata.name") == Some("default")
                        && let Some(registry) = registry_weak.upgrade()
                        && let Ok(metadata) = registry.bind::<pw::metadata::Metadata, _>(global)
                    {
                        let scan_for_meta = scan_for_registry.clone();
                        let listener = metadata
                            .add_listener_local()
                            .property(move |_subject, key, _type, value| {
                                if key == Some(default_key)
                                    && let Some(value) = value
                                {
                                    let name = serde_json::from_str::<serde_json::Value>(value)
                                        .ok()
                                        .and_then(|v| {
                                            v.get("name")
                                                .and_then(|n| n.as_str())
                                                .map(str::to_string)
                                        })
                                        .unwrap_or_else(|| value.to_string());
                                    scan_for_meta.borrow_mut().default_name = Some(name);
                                }
                                0
                            })
                            .register();
                        proxies.borrow_mut().push((metadata, listener));
                    }
                }
                _ => {}
            }
        })
        .register();

    // Two roundtrips: the first enumerates globals (and binds the default
    // metadata), the second lets the metadata properties arrive.
    let pending = Rc::new(RefCell::new(
        core.sync(0).map_err(|e| format!("pipewire sync: {e}"))?,
    ));
    let scan_for_done = scan.clone();
    let loop_for_done = main_loop.clone();
    let core_for_done = core.clone();
    let pending_for_done = pending.clone();
    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            if id != pw::core::PW_ID_CORE || seq != *pending_for_done.borrow() {
                return;
            }
            let mut scan = scan_for_done.borrow_mut();
            scan.phase += 1;
            if scan.phase == 1 {
                if let Ok(next) = core_for_done.sync(0) {
                    *pending_for_done.borrow_mut() = next;
                    return;
                }
            }
            loop_for_done.quit();
        })
        .error({
            let loop_for_error = main_loop.clone();
            move |id, _seq, _res, message| {
                if id == pw::core::PW_ID_CORE {
                    tracing::debug!("pipewire core error: {message}");
                    loop_for_error.quit();
                }
            }
        })
        .register();
    let scan_for_timer = scan.clone();
    let loop_for_timer = main_loop.clone();
    let timer = main_loop.loop_().add_timer(move |_| {
        scan_for_timer.borrow_mut().timed_out = true;
        loop_for_timer.quit();
    });
    timer
        .update_timer(Some(ENUMERATE_TIMEOUT), None)
        .into_result()
        .map_err(|e| format!("pipewire timer: {e}"))?;
    main_loop.run();

    let scan = scan.borrow();
    if scan.timed_out && scan.phase == 0 {
        return Err("PipeWire did not answer within 3 s".into());
    }
    let mut out: Vec<Endpoint> = scan
        .nodes
        .values()
        .map(|(name, display)| Endpoint {
            endpoint_id: name.clone(),
            display_name: display.clone(),
            is_default: scan.default_name.as_deref() == Some(name.as_str()),
        })
        .collect();
    if !out.is_empty() && !out.iter().any(|e| e.is_default) {
        out[0].is_default = true;
    }
    Ok(out)
}
