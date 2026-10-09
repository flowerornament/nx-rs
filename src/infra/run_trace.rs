//! Opt-in process-wide run trace. One CLI run owns the sink; workers inherit it.
//! Phase context is explicitly propagated to workers, never inferred from timing.

use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
thread_local! { static PARENT: Cell<Option<u64>> = const { Cell::new(None) }; }

struct Sink {
    file: File,
    started: Instant,
    next_id: u64,
    failed: bool,
}

#[derive(Serialize)]
struct Row<'a> {
    version: u8,
    event: &'a str,
    id: u64,
    parent: Option<u64>,
    offset_ns: u128,
    label: &'a str,
    code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

fn emit(event: &str, id: u64, parent: Option<u64>, label: &str, code: Option<i32>) {
    emit_data(event, id, parent, label, code, None);
}

fn emit_data(
    event: &str,
    id: u64,
    parent: Option<u64>,
    label: &str,
    code: Option<i32>,
    data: Option<serde_json::Value>,
) {
    let mut sink = SINK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(sink) = sink.as_mut() {
        let row = Row {
            version: 1,
            event,
            id,
            parent,
            offset_ns: sink.started.elapsed().as_nanos(),
            label,
            code,
            data,
        };
        if serde_json::to_writer(&mut sink.file, &row).is_err()
            || sink.file.write_all(b"\n").is_err()
        {
            sink.failed = true;
        }
    }
}

pub(crate) struct Run {
    finished: bool,
}

impl Run {
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var_os("NX_TRACE_PATH")?;
        let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("Could not create NX_TRACE_PATH: {error}");
                return None;
            }
        };
        *SINK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Sink {
            file,
            started: Instant::now(),
            next_id: 1,
            failed: false,
        });
        PARENT.set(Some(0));
        ACTIVE.store(true, Ordering::Relaxed);
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        emit_data(
            "run-start",
            0,
            None,
            &format!("nx {} epoch_ms={epoch}", env!("CARGO_PKG_VERSION")),
            None,
            Some(
                serde_json::json!({ "binary_path": std::env::current_exe().ok(), "terminal_available": crate::infra::shell::terminal_stdio_available() }),
            ),
        );
        Some(Self { finished: false })
    }

    pub(crate) fn finish(mut self, code: i32) {
        let complete = SINK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|sink| !sink.failed);
        emit_data(
            "run-end",
            0,
            None,
            "nx",
            Some(code),
            Some(serde_json::json!({"complete":complete})),
        );
        self.finished = true;
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if !self.finished {
            emit("run-end", 0, None, "nx", None);
        }
        ACTIVE.store(false, Ordering::Relaxed);
        PARENT.set(None);
        let sink = SINK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut sink) = sink
            && (sink.file.flush().is_err() || sink.failed)
        {
            eprintln!("NX_TRACE_PATH write failed; trace is incomplete");
        }
    }
}

pub(crate) struct Span {
    id: u64,
    parent: Option<u64>,
    label: String,
    kind: &'static str,
    code: Option<i32>,
}

impl Span {
    pub(crate) fn start(kind: &'static str, label: String) -> Option<Self> {
        if !ACTIVE.load(Ordering::Relaxed) {
            return None;
        }
        let id = {
            let mut sink = SINK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let sink = sink.as_mut()?;
            let id = sink.next_id;
            sink.next_id += 1;
            id
        };
        let parent = PARENT.get();
        emit(&format!("{kind}-start"), id, parent, &label, None);
        PARENT.set(Some(id));
        Some(Self {
            id,
            parent,
            label,
            kind,
            code: None,
        })
    }

    pub(crate) fn finish(&mut self, code: i32) {
        self.code = Some(code);
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        emit(
            &format!("{}-end", self.kind),
            self.id,
            self.parent,
            &self.label,
            self.code,
        );
        PARENT.set(self.parent);
    }
}

pub(crate) fn command(program: &str, args: &[&str]) -> Option<Span> {
    if !ACTIVE.load(Ordering::Relaxed) {
        return None;
    }
    let name = Path::new(program).file_name()?.to_string_lossy();
    // Record a bounded operation label, not arbitrary arguments, prompts or env.
    let words: Vec<_> = args
        .iter()
        .copied()
        .filter(|arg| {
            matches!(
                *arg,
                "flake"
                    | "update"
                    | "check"
                    | "build"
                    | "derivation"
                    | "show"
                    | "path-info"
                    | "--dry-run"
                    | "--version"
                    | "--help"
                    | "upgrade"
                    | "outdated"
                    | "bundle"
                    | "diff"
                    | "status"
                    | "rev-parse"
                    | "HEAD"
                    | "commit"
                    | "add"
                    | "--set"
                    | "bar"
                    | "bar-with-logs"
                    | "internal-json"
                    | "auth"
                    | "api"
                    | "token"
                    | "version"
            )
        })
        .take(4)
        .collect();
    Span::start(
        "command",
        format!("{name} {}", words.join(" ")).trim_end().to_string(),
    )
}

pub(crate) fn with_parent<T>(run: impl FnOnce() -> T) -> impl FnOnce() -> T {
    let parent = PARENT.get();
    move || {
        struct Restore(Option<u64>);
        impl Drop for Restore {
            fn drop(&mut self) {
                PARENT.set(self.0);
            }
        }
        let _restore = Restore(PARENT.replace(parent));
        run()
    }
}

/// Preserve typed Nix activity IDs, parents and targets. Diagnostics and build
/// output remain outside the measurement trace.
pub(crate) fn nix_activity(record: &[u8]) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let Some(raw) = record.strip_prefix(b"@nix ") else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(raw) else {
        return;
    };
    if !matches!(value["action"].as_str(), Some("start" | "stop" | "result")) {
        return;
    }
    let data = ["action", "id", "type", "parent", "fields"]
        .into_iter()
        .filter_map(|key| value.get(key).map(|value| (key.to_string(), value.clone())))
        .collect::<serde_json::Map<_, _>>();
    emit_data(
        "nix-activity",
        0,
        PARENT.get(),
        "nix",
        None,
        Some(data.into()),
    );
}
