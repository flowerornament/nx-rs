use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;

const NIX_JSON_PREFIX: &str = "@nix ";
const NIX_RECORD_LIMIT: usize = 16 * 1024;
const RENDER_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NixLogFormat {
    InternalJson,
    Bar,
    BarWithLogs,
}

impl NixLogFormat {
    pub(crate) const fn for_native_terminal(verbose: bool) -> Self {
        if verbose {
            Self::BarWithLogs
        } else {
            Self::Bar
        }
    }

    pub(crate) const fn as_arg(self) -> &'static str {
        match self {
            Self::InternalJson => "internal-json",
            Self::Bar => "bar",
            Self::BarWithLogs => "bar-with-logs",
        }
    }

    pub(crate) const fn as_config(self) -> &'static str {
        match self {
            Self::InternalJson => "log-format = internal-json",
            Self::Bar => "log-format = bar",
            Self::BarWithLogs => "log-format = bar-with-logs",
        }
    }

    pub(crate) const fn as_env_assignment(self) -> &'static str {
        match self {
            Self::InternalJson => "NIX_CONFIG=log-format = internal-json",
            Self::Bar => "NIX_CONFIG=log-format = bar",
            Self::BarWithLogs => "NIX_CONFIG=log-format = bar-with-logs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NixActivityType {
    Unknown,
    CopyPath,
    FileTransfer,
    Realise,
    CopyPaths,
    Builds,
    Build,
    OptimiseStore,
    VerifyPaths,
    Substitute,
    QueryPathInfo,
    PostBuildHook,
    BuildWaiting,
    FetchTree,
}

impl From<u64> for NixActivityType {
    fn from(value: u64) -> Self {
        match value {
            100 => Self::CopyPath,
            101 => Self::FileTransfer,
            102 => Self::Realise,
            103 => Self::CopyPaths,
            104 => Self::Builds,
            105 => Self::Build,
            106 => Self::OptimiseStore,
            107 => Self::VerifyPaths,
            108 => Self::Substitute,
            109 => Self::QueryPathInfo,
            110 => Self::PostBuildHook,
            111 => Self::BuildWaiting,
            112 => Self::FetchTree,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NixResultType {
    FileLinked,
    BuildLogLine,
    UntrustedPath,
    CorruptedPath,
    SetPhase,
    Progress,
    SetExpected,
    PostBuildLogLine,
    FetchStatus,
    Unknown,
}

impl From<u64> for NixResultType {
    fn from(value: u64) -> Self {
        match value {
            100 => Self::FileLinked,
            101 => Self::BuildLogLine,
            102 => Self::UntrustedPath,
            103 => Self::CorruptedPath,
            104 => Self::SetPhase,
            105 => Self::Progress,
            106 => Self::SetExpected,
            107 => Self::PostBuildLogLine,
            108 => Self::FetchStatus,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Progress {
    done: u64,
    expected: u64,
    running: u64,
    failed: u64,
}

#[derive(Debug)]
struct Activity {
    kind: NixActivityType,
    progress: Progress,
    expected_by_type: HashMap<NixActivityType, u64>,
    label: Option<String>,
    phase: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ActivityTotals {
    done: u64,
    expected: u64,
    failed: u64,
}

#[derive(Default)]
pub(crate) struct NixProgress {
    activities: HashMap<u64, Activity>,
    totals: HashMap<NixActivityType, ActivityTotals>,
    current_activity: Option<u64>,
    files_linked: u64,
    bytes_linked: u64,
    corrupted_paths: u64,
    untrusted_paths: u64,
    active: bool,
    rendered_summary: Option<String>,
    last_rendered_at: Option<Instant>,
}

impl NixProgress {
    pub(crate) fn observe_record(&mut self, record: &[u8]) -> NixRecord {
        let text = String::from_utf8_lossy(record);
        let visible = visible_record(&text);
        if visible.is_empty() {
            return NixRecord::Ignored;
        }

        if let Some(json) = visible.strip_prefix(NIX_JSON_PREFIX) {
            return self.observe_json_record(json, visible);
        }

        NixRecord::Diagnostic(visible.to_string())
    }

    fn observe_json_record(&mut self, json: &str, original: &str) -> NixRecord {
        let Ok(record) = serde_json::from_str::<NixJsonRecord>(json) else {
            return NixRecord::Diagnostic(original.to_string());
        };

        match record.action.as_str() {
            "start" => {
                let Some(id) = record.id else {
                    return NixRecord::Ignored;
                };
                let kind = record.kind.map_or(NixActivityType::Unknown, Into::into);
                let label = activity_label(kind, &record.text, &record.fields);
                self.activities.insert(
                    id,
                    Activity {
                        kind,
                        progress: Progress::default(),
                        expected_by_type: HashMap::new(),
                        label,
                        phase: None,
                        status: None,
                    },
                );
                self.current_activity = Some(id);
                NixRecord::Progress(Some(kind))
            }
            "stop" => {
                if let Some(id) = record.id {
                    self.stop_activity(id);
                }
                NixRecord::Progress(None)
            }
            "result" => {
                if let (Some(id), Some(kind)) = (record.id, record.kind) {
                    self.observe_result(id, kind, &record.fields);
                }
                NixRecord::Progress(None)
            }
            "msg" => {
                if record.msg.is_empty() {
                    NixRecord::Ignored
                } else {
                    NixRecord::Diagnostic(record.msg)
                }
            }
            _ => NixRecord::Ignored,
        }
    }

    fn stop_activity(&mut self, id: u64) {
        let Some(activity) = self.activities.remove(&id) else {
            return;
        };
        let totals = self.totals.entry(activity.kind).or_default();
        totals.done = totals.done.saturating_add(activity.progress.done);
        totals.failed = totals.failed.saturating_add(activity.progress.failed);

        for (kind, expected) in activity.expected_by_type {
            let totals = self.totals.entry(kind).or_default();
            totals.expected = totals.expected.saturating_sub(expected);
        }

        if self.current_activity == Some(id) {
            self.current_activity = self.activities.keys().max().copied();
        }
    }

    fn observe_result(&mut self, id: u64, kind: u64, fields: &[Value]) {
        match NixResultType::from(kind) {
            NixResultType::FileLinked => {
                self.files_linked = self.files_linked.saturating_add(1);
                self.bytes_linked = self.bytes_linked.saturating_add(integer_field(fields, 0));
            }
            NixResultType::UntrustedPath => {
                self.untrusted_paths = self.untrusted_paths.saturating_add(1);
            }
            NixResultType::CorruptedPath => {
                self.corrupted_paths = self.corrupted_paths.saturating_add(1);
            }
            NixResultType::Progress => {
                let Some(activity) = self.activities.get_mut(&id) else {
                    return;
                };
                activity.progress = Progress {
                    done: integer_field(fields, 0),
                    expected: integer_field(fields, 1),
                    running: integer_field(fields, 2),
                    failed: integer_field(fields, 3),
                };
            }
            NixResultType::SetExpected => {
                let expected_kind = NixActivityType::from(integer_field(fields, 0));
                let expected = integer_field(fields, 1);
                let Some(activity) = self.activities.get_mut(&id) else {
                    return;
                };
                let previous = activity
                    .expected_by_type
                    .insert(expected_kind, expected)
                    .unwrap_or(0);
                let totals = self.totals.entry(expected_kind).or_default();
                totals.expected = totals
                    .expected
                    .saturating_sub(previous)
                    .saturating_add(expected);
            }
            NixResultType::SetPhase => {
                let Some(status) = string_field(fields, 0) else {
                    return;
                };
                if let Some(activity) = self.activities.get_mut(&id) {
                    activity.phase = Some(status.to_string());
                    self.current_activity = Some(id);
                }
            }
            NixResultType::FetchStatus => {
                let Some(status) = string_field(fields, 0) else {
                    return;
                };
                if let Some(activity) = self.activities.get_mut(&id) {
                    activity.status = Some(status.to_string());
                    self.current_activity = Some(id);
                }
            }
            NixResultType::BuildLogLine
            | NixResultType::PostBuildLogLine
            | NixResultType::Unknown => {}
        }
    }

    pub(crate) fn render(&mut self, stderr: &mut impl Write) -> anyhow::Result<()> {
        let summary = self.summary();
        if self.rendered_summary.as_deref() == Some(summary.as_str()) {
            return Ok(());
        }
        let now = Instant::now();
        if self
            .last_rendered_at
            .is_some_and(|last| now.duration_since(last) < RENDER_INTERVAL)
        {
            return Ok(());
        }

        self.active = true;
        self.rendered_summary = Some(summary.clone());
        self.last_rendered_at = Some(now);
        write!(stderr, "\r\x1b[2K  nix: {summary}").context("writing child stderr")?;
        stderr.flush().context("flushing child stderr")
    }

    pub(crate) fn clear(&mut self, stderr: &mut impl Write) -> anyhow::Result<()> {
        if self.active {
            write!(stderr, "\r\x1b[2K").context("clearing child stderr progress")?;
            stderr.flush().context("flushing child stderr")?;
            self.active = false;
            self.rendered_summary = None;
            self.last_rendered_at = None;
        }
        Ok(())
    }

    pub(crate) fn summary(&self) -> String {
        let mut parts = Vec::new();
        push_activity_summary(
            &mut parts,
            self.activity_progress(NixActivityType::Builds),
            "built",
        );
        push_activity_summary(
            &mut parts,
            self.activity_progress(NixActivityType::CopyPaths),
            "copied",
        );
        push_size_summary(
            &mut parts,
            self.activity_progress(NixActivityType::CopyPath),
            "copied",
        );
        push_size_summary(
            &mut parts,
            self.activity_progress(NixActivityType::FileTransfer),
            "DL",
        );
        push_activity_summary(
            &mut parts,
            self.activity_progress(NixActivityType::OptimiseStore),
            "paths optimised",
        );
        if self.files_linked > 0 || self.bytes_linked > 0 {
            parts.push(format!(
                "{} freed in {} inodes",
                format_bytes(self.bytes_linked),
                self.files_linked
            ));
        }
        push_activity_summary(
            &mut parts,
            self.activity_progress(NixActivityType::VerifyPaths),
            "paths verified",
        );
        if self.corrupted_paths > 0 {
            parts.push(format!("{} corrupted", self.corrupted_paths));
        }
        if self.untrusted_paths > 0 {
            parts.push(format!("{} untrusted", self.untrusted_paths));
        }

        if let Some(status) = self.current_label() {
            parts.push(status);
        }

        if parts.is_empty() {
            "preparing build plan".to_string()
        } else {
            truncate(&parts.join(", "), 120)
        }
    }

    fn activity_progress(&self, kind: NixActivityType) -> Progress {
        let totals = self.totals.get(&kind).copied().unwrap_or_default();
        let mut progress = Progress {
            done: totals.done,
            expected: totals.done,
            running: 0,
            failed: totals.failed,
        };

        for activity in self
            .activities
            .values()
            .filter(|activity| activity.kind == kind)
        {
            progress.done = progress.done.saturating_add(activity.progress.done);
            progress.expected = progress.expected.saturating_add(activity.progress.expected);
            progress.running = progress.running.saturating_add(activity.progress.running);
            progress.failed = progress.failed.saturating_add(activity.progress.failed);
        }
        progress.expected = progress.expected.max(totals.expected);
        progress
    }

    fn current_label(&self) -> Option<String> {
        let activity = self
            .current_activity
            .and_then(|id| self.activities.get(&id))?;
        let mut label = activity.label.clone().unwrap_or_default();
        if let Some(phase) = &activity.phase {
            if label.is_empty() {
                label.push_str(phase);
            } else {
                label.push_str(" (");
                label.push_str(phase);
                label.push(')');
            }
        }
        if let Some(status) = &activity.status {
            if !label.is_empty() {
                label.push_str(": ");
            }
            label.push_str(status);
        }
        (!label.is_empty()).then_some(label)
    }
}

pub(crate) enum NixRecord {
    Progress(Option<NixActivityType>),
    Diagnostic(String),
    Ignored,
}

#[derive(Debug, Deserialize)]
struct NixJsonRecord {
    action: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default, rename = "type")]
    kind: Option<u64>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    fields: Vec<Value>,
}

pub(crate) fn feed_nix_output(
    chunk: &[u8],
    pending: &mut Vec<u8>,
    mut observe: impl FnMut(&[u8]) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    for byte in chunk {
        if matches!(byte, b'\n' | b'\r') {
            if !pending.is_empty() {
                observe(pending)?;
                pending.clear();
            }
            continue;
        }

        pending.push(*byte);
        if pending.len() >= NIX_RECORD_LIMIT {
            observe(pending)?;
            pending.clear();
        }
    }
    Ok(())
}

fn activity_label(kind: NixActivityType, text: &str, fields: &[Value]) -> Option<String> {
    match kind {
        NixActivityType::Build => string_field(fields, 0)
            .map(store_path_name)
            .map(|name| format!("building {name}")),
        NixActivityType::Substitute => {
            let name = string_field(fields, 0).map(store_path_name)?;
            let source = string_field(fields, 1).unwrap_or("cache");
            Some(format!("fetching {name} from {source}"))
        }
        NixActivityType::FileTransfer => string_field(fields, 0)
            .map(short_source)
            .map(|source| format!("downloading {source}")),
        _ => (!text.is_empty()).then(|| text.to_string()),
    }
}

fn integer_field(fields: &[Value], index: usize) -> u64 {
    fields.get(index).and_then(Value::as_u64).unwrap_or(0)
}

fn string_field(fields: &[Value], index: usize) -> Option<&str> {
    fields.get(index).and_then(Value::as_str)
}

fn push_activity_summary(parts: &mut Vec<String>, progress: Progress, noun: &str) {
    if progress.running == 0 && progress.done == 0 && progress.expected == 0 && progress.failed == 0
    {
        return;
    }

    let counts = if progress.running > 0 && progress.expected > 0 {
        format!(
            "{}/{}/{}",
            progress.running, progress.done, progress.expected
        )
    } else if progress.running > 0 {
        format!("{}/{}", progress.running, progress.done)
    } else if progress.expected != progress.done {
        format!("{}/{}", progress.done, progress.expected)
    } else {
        progress.done.to_string()
    };
    let failed = (progress.failed > 0).then(|| format!(" ({} failed)", progress.failed));
    parts.push(format!(
        "{counts} {noun}{}",
        failed.as_deref().unwrap_or("")
    ));
}

fn push_size_summary(parts: &mut Vec<String>, progress: Progress, noun: &str) {
    if progress.running == 0 && progress.done == 0 && progress.expected == 0 && progress.failed == 0
    {
        return;
    }

    let (scale, unit) = size_unit(
        progress
            .running
            .max(progress.done)
            .max(progress.expected)
            .max(progress.failed),
    );
    let display = |bytes| format_bytes_at(bytes, scale);
    let sizes = if progress.running > 0 && progress.expected > 0 {
        format!(
            "{}/{}/{} {unit}",
            display(progress.running),
            display(progress.done),
            display(progress.expected)
        )
    } else if progress.running > 0 {
        format!(
            "{}/{} {unit}",
            display(progress.running),
            display(progress.done)
        )
    } else if progress.expected != progress.done && progress.expected > 0 {
        format!(
            "{}/{} {unit}",
            display(progress.done),
            display(progress.expected)
        )
    } else {
        format!("{} {unit}", display(progress.done))
    };
    let failed =
        (progress.failed > 0).then(|| format!(" ({} {unit} failed)", display(progress.failed)));
    parts.push(format!("{sizes} {noun}{}", failed.as_deref().unwrap_or("")));
}

fn size_unit(bytes: u64) -> (u64, &'static str) {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut divisor = 1;
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024 && unit < UNITS.len() - 1 {
        value /= 1024;
        divisor *= 1024;
        unit += 1;
    }
    (divisor, UNITS[unit])
}

fn format_bytes_at(bytes: u64, divisor: u64) -> String {
    if divisor == 1 {
        bytes.to_string()
    } else {
        let whole = bytes / divisor;
        let decimal = (bytes % divisor).saturating_mul(10) / divisor;
        format!("{whole}.{decimal}")
    }
}

fn format_bytes(bytes: u64) -> String {
    let (divisor, unit) = size_unit(bytes);
    format!("{} {unit}", format_bytes_at(bytes, divisor))
}

fn visible_record(text: &str) -> &str {
    text.rsplit('\r')
        .next()
        .unwrap_or_default()
        .trim_end_matches(['\n', '\r'])
}

fn store_path_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let name = base.split_once('-').map_or(base, |(_, name)| name);
    name.strip_suffix(".drv").unwrap_or(name).to_string()
}

fn short_source(source: &str) -> String {
    source
        .strip_prefix("https://")
        .or_else(|| source.strip_prefix("http://"))
        .unwrap_or(source)
        .to_string()
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observe(progress: &mut NixProgress, line: &str) -> NixRecord {
        progress.observe_record(line.as_bytes())
    }

    #[test]
    fn structured_progress_matches_nix_activity_protocol() {
        let mut progress = NixProgress::default();
        observe(
            &mut progress,
            r#"@nix {"action":"start","id":1,"level":0,"parent":0,"text":"","type":104}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"result","fields":[2,5,1,0],"id":1,"type":105}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"start","fields":["https://cache.nixos.org/nar"],"id":2,"level":0,"parent":0,"text":"","type":101}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"result","fields":[1048576,2097152,0,0],"id":2,"type":105}"#,
        );

        assert_eq!(
            progress.summary(),
            "1/2/5 built, 1.0/2.0 MiB DL, downloading cache.nixos.org/nar"
        );
    }

    #[test]
    fn structured_message_becomes_decoded_diagnostic() {
        let mut progress = NixProgress::default();
        let record = observe(
            &mut progress,
            r#"@nix {"action":"msg","level":0,"msg":"error: boom\nspecified: old\ngot: new"}"#,
        );

        let NixRecord::Diagnostic(message) = record else {
            panic!("expected diagnostic");
        };
        assert_eq!(message, "error: boom\nspecified: old\ngot: new");
    }

    #[test]
    fn parent_expected_total_is_not_added_twice() {
        let mut progress = NixProgress::default();
        observe(
            &mut progress,
            r#"@nix {"action":"start","id":1,"level":0,"parent":0,"text":"realising","type":102}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"result","fields":[104,5],"id":1,"type":106}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"start","id":2,"level":0,"parent":1,"text":"","type":104}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"result","fields":[2,5,1,0],"id":2,"type":105}"#,
        );

        assert!(progress.summary().starts_with("1/2/5 built"));
    }

    #[test]
    fn activity_phase_and_fetch_status_enrich_current_operation() {
        let mut progress = NixProgress::default();
        observe(
            &mut progress,
            r#"@nix {"action":"start","id":1,"level":0,"parent":0,"text":"building package","type":105,"fields":["/nix/store/hash-package.drv","",1,1]}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"result","fields":["configurePhase"],"id":1,"type":104}"#,
        );
        observe(
            &mut progress,
            r#"@nix {"action":"result","fields":["checking dependencies"],"id":1,"type":108}"#,
        );

        assert_eq!(
            progress.summary(),
            "building package (configurePhase): checking dependencies"
        );
    }

    #[test]
    fn non_protocol_text_remains_a_diagnostic() {
        let mut progress = NixProgress::default();
        let record = observe(&mut progress, "remote: Repository not found.");
        assert!(matches!(record, NixRecord::Diagnostic(_)));
    }

    #[test]
    fn renderer_updates_and_clears_one_terminal_line() {
        let mut progress = NixProgress::default();
        observe(
            &mut progress,
            r#"@nix {"action":"start","id":1,"level":0,"parent":0,"text":"building derivations","type":104}"#,
        );
        let mut stderr = Vec::new();

        progress.render(&mut stderr).expect("render progress");
        progress.clear(&mut stderr).expect("clear progress");

        assert_eq!(
            String::from_utf8(stderr).expect("terminal output is utf-8"),
            "\r\x1b[2K  nix: building derivations\r\x1b[2K"
        );
    }

    #[test]
    fn feed_output_splits_newline_and_carriage_return_records() {
        let mut pending = Vec::new();
        let mut records = Vec::new();
        feed_nix_output(b"one\rtwo\nthree", &mut pending, |record| {
            records.push(String::from_utf8_lossy(record).into_owned());
            Ok(())
        })
        .expect("feed output");

        assert_eq!(records, ["one", "two"]);
        assert_eq!(pending, b"three");
    }
}
