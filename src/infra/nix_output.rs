use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::infra::text::truncate_with_ellipsis;
use crate::output::printer::DETAIL_INDENT;

const NIX_JSON_PREFIX: &str = "@nix ";
const NIX_RECORD_LIMIT: usize = 16 * 1024;
const RENDER_INTERVAL: Duration = Duration::from_millis(50);
const MAX_ACTIVE_ACTIVITIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NixOutputMode {
    Structured,
    Verbose,
}

impl NixOutputMode {
    pub(crate) const fn from_verbose(verbose: bool) -> Self {
        if verbose {
            Self::Verbose
        } else {
            Self::Structured
        }
    }

    pub(crate) const fn log_format(self) -> NixLogFormat {
        match self {
            Self::Structured => NixLogFormat::InternalJson,
            Self::Verbose => NixLogFormat::BarWithLogs,
        }
    }

    pub(crate) const fn is_verbose(self) -> bool {
        matches!(self, Self::Verbose)
    }

    pub(crate) fn command_args(self, base_args: &[String]) -> Vec<String> {
        let mut args = Vec::with_capacity(base_args.len() + 2);
        args.extend([
            "--log-format".to_string(),
            self.log_format().as_arg().to_string(),
        ]);
        args.extend(base_args.iter().cloned());
        args
    }
}

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

impl NixActivityType {
    pub(crate) const fn timing_phase(self) -> Option<&'static str> {
        match self {
            Self::CopyPath
            | Self::FileTransfer
            | Self::CopyPaths
            | Self::Substitute
            | Self::FetchTree => Some("fetches"),
            Self::Realise | Self::Builds | Self::Build | Self::BuildWaiting => Some("nix-build"),
            Self::Unknown
            | Self::OptimiseStore
            | Self::VerifyPaths
            | Self::QueryPathInfo
            | Self::PostBuildHook => None,
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
pub(crate) struct NixOutputRenderer {
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

impl NixOutputRenderer {
    pub(crate) fn observe_record(&mut self, record: &[u8]) -> NixRecord {
        let text = String::from_utf8_lossy(record);
        let visible = visible_record(&text);
        if visible.is_empty() {
            return NixRecord::Ignored;
        }

        if let Some(json) = visible.strip_prefix(NIX_JSON_PREFIX) {
            return self.observe_json_record(json, visible);
        }

        NixRecord::Diagnostic(NixDiagnostic::plain(visible.to_string()))
    }

    fn observe_json_record(&mut self, json: &str, original: &str) -> NixRecord {
        let Ok(record) = serde_json::from_str::<NixJsonRecord<'_>>(json) else {
            return NixRecord::Diagnostic(NixDiagnostic::plain(original.to_string()));
        };

        match record.action.as_ref() {
            "start" => {
                let Some(id) = record.id else {
                    return NixRecord::Ignored;
                };
                let kind = record.kind.map_or(NixActivityType::Unknown, Into::into);
                let label = activity_label(kind, &record.text, record.fields);
                if self.activities.len() >= MAX_ACTIVE_ACTIVITIES
                    && let Some(oldest) = self.activities.keys().min().copied()
                {
                    self.stop_activity(oldest);
                }
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
                    self.observe_result(id, kind, record.fields);
                }
                NixRecord::Progress(None)
            }
            "msg" => {
                if record.msg.is_empty() {
                    NixRecord::Ignored
                } else {
                    NixRecord::Diagnostic(NixDiagnostic {
                        message: record.msg.into_owned(),
                    })
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

    fn observe_result(&mut self, id: u64, kind: u64, fields: Option<&RawValue>) {
        match NixResultType::from(kind) {
            NixResultType::FileLinked => {
                let bytes = parse_fields::<(u64,)>(fields).map_or(0, |fields| fields.0);
                self.files_linked = self.files_linked.saturating_add(1);
                self.bytes_linked = self.bytes_linked.saturating_add(bytes);
            }
            NixResultType::UntrustedPath => {
                self.untrusted_paths = self.untrusted_paths.saturating_add(1);
            }
            NixResultType::CorruptedPath => {
                self.corrupted_paths = self.corrupted_paths.saturating_add(1);
            }
            NixResultType::Progress => {
                let Some((done, expected, running, failed)) = parse_fields(fields) else {
                    return;
                };
                let Some(activity) = self.activities.get_mut(&id) else {
                    return;
                };
                activity.progress = Progress {
                    done,
                    expected,
                    running,
                    failed,
                };
            }
            NixResultType::SetExpected => {
                let Some((expected_kind, expected)) = parse_fields::<(u64, u64)>(fields) else {
                    return;
                };
                let expected_kind = NixActivityType::from(expected_kind);
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
                let Some((status,)) = parse_fields::<(Cow<'_, str>,)>(fields) else {
                    return;
                };
                if let Some(activity) = self.activities.get_mut(&id) {
                    activity.phase = Some(status.into_owned());
                    self.current_activity = Some(id);
                }
            }
            NixResultType::FetchStatus => {
                let Some((status,)) = parse_fields::<(Cow<'_, str>,)>(fields) else {
                    return;
                };
                if let Some(activity) = self.activities.get_mut(&id) {
                    activity.status = Some(status.into_owned());
                    self.current_activity = Some(id);
                }
            }
            NixResultType::BuildLogLine
            | NixResultType::PostBuildLogLine
            | NixResultType::Unknown => {}
        }
    }

    pub(crate) fn render(&mut self, stderr: &mut impl Write) -> anyhow::Result<()> {
        let now = Instant::now();
        if self
            .last_rendered_at
            .is_some_and(|last| now.duration_since(last) < RENDER_INTERVAL)
        {
            return Ok(());
        }
        let summary = self.summary();
        self.last_rendered_at = Some(now);
        if self.rendered_summary.as_deref() == Some(summary.as_str()) {
            return Ok(());
        }

        self.active = true;
        self.rendered_summary = Some(summary.clone());
        write!(stderr, "\r\x1b[2K{DETAIL_INDENT}nix: {summary}").context("writing child stderr")?;
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

    pub(crate) fn render_diagnostic(
        &mut self,
        stderr: &mut impl Write,
        diagnostic: &NixDiagnostic,
    ) -> anyhow::Result<()> {
        self.clear(stderr)?;
        write_detail_lines(stderr, diagnostic.message.as_bytes())?;
        if !diagnostic.message.ends_with('\n') {
            writeln!(stderr).context("terminating Nix diagnostic")?;
        }
        stderr.flush().context("flushing Nix diagnostic")
    }

    pub(crate) fn render_captured_diagnostics(
        stderr: &mut impl Write,
        diagnostics: &[u8],
    ) -> anyhow::Result<()> {
        write_detail_lines(stderr, diagnostics)?;
        stderr.flush().context("flushing Nix diagnostics")
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
            truncate_with_ellipsis(&parts.join(", "), 120)
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

fn write_detail_lines(stderr: &mut impl Write, output: &[u8]) -> anyhow::Result<()> {
    for line in output.split_inclusive(|byte| *byte == b'\n') {
        stderr
            .write_all(DETAIL_INDENT.as_bytes())
            .context("writing Nix output indent")?;
        stderr.write_all(line).context("writing Nix output")?;
    }
    Ok(())
}

pub(crate) enum NixRecord {
    Progress(Option<NixActivityType>),
    Diagnostic(NixDiagnostic),
    Ignored,
}

pub(crate) struct NixDiagnostic {
    pub(crate) message: String,
}

impl NixDiagnostic {
    fn plain(message: String) -> Self {
        Self { message }
    }
}

#[derive(Deserialize)]
struct NixJsonRecord<'a> {
    #[serde(borrow)]
    action: Cow<'a, str>,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default, rename = "type")]
    kind: Option<u64>,
    #[serde(default, borrow)]
    text: Cow<'a, str>,
    #[serde(default, borrow)]
    msg: Cow<'a, str>,
    #[serde(default, rename = "level")]
    _level: Option<u64>,
    #[serde(default, borrow)]
    fields: Option<&'a RawValue>,
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

fn activity_label(kind: NixActivityType, text: &str, fields: Option<&RawValue>) -> Option<String> {
    match kind {
        NixActivityType::Build => {
            let (path, machine, _, _) =
                parse_fields::<(Cow<'_, str>, Cow<'_, str>, u64, u64)>(fields)?;
            let mut label = format!("building {}", store_path_display_name(&path));
            if !machine.is_empty() {
                label.push_str(" on ");
                label.push_str(&machine);
            }
            Some(label)
        }
        NixActivityType::Substitute => {
            let (path, source) = parse_fields::<(Cow<'_, str>, Cow<'_, str>)>(fields)?;
            let verb = if source.starts_with("local") {
                "copying"
            } else {
                "fetching"
            };
            Some(format!(
                "{verb} {} from {source}",
                store_path_display_name(&path)
            ))
        }
        NixActivityType::FileTransfer => {
            let (source,) = parse_fields::<(Cow<'_, str>,)>(fields)?;
            Some(format!("downloading {}", short_source(&source)))
        }
        NixActivityType::PostBuildHook => {
            let (path,) = parse_fields::<(Cow<'_, str>,)>(fields)?;
            Some(format!("post-build {}", store_path_display_name(&path)))
        }
        NixActivityType::QueryPathInfo => {
            let (path, source) = parse_fields::<(Cow<'_, str>, Cow<'_, str>)>(fields)?;
            Some(format!(
                "querying {} on {source}",
                store_path_display_name(&path)
            ))
        }
        _ => (!text.is_empty()).then(|| text.to_string()),
    }
}

fn parse_fields<'a, T>(fields: Option<&'a RawValue>) -> Option<T>
where
    T: Deserialize<'a>,
{
    serde_json::from_str(fields?.get()).ok()
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

pub(crate) fn store_path_display_name(path: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn observe(progress: &mut NixOutputRenderer, line: &str) -> NixRecord {
        progress.observe_record(line.as_bytes())
    }

    #[test]
    fn structured_progress_matches_nix_activity_protocol() {
        let mut progress = NixOutputRenderer::default();
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
        let mut progress = NixOutputRenderer::default();
        let record = observe(
            &mut progress,
            r#"@nix {"action":"msg","level":0,"msg":"error: boom\nspecified: old\ngot: new"}"#,
        );

        let NixRecord::Diagnostic(diagnostic) = record else {
            panic!("expected diagnostic");
        };
        assert_eq!(diagnostic.message, "error: boom\nspecified: old\ngot: new");
    }

    #[test]
    fn parent_expected_total_is_not_added_twice() {
        let mut progress = NixOutputRenderer::default();
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
        let mut progress = NixOutputRenderer::default();
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
        let mut progress = NixOutputRenderer::default();
        let record = observe(&mut progress, "remote: Repository not found.");
        assert!(matches!(record, NixRecord::Diagnostic(_)));
    }

    #[test]
    fn anomalous_activity_stream_remains_bounded() {
        let mut progress = NixOutputRenderer::default();
        for id in 0..=MAX_ACTIVE_ACTIVITIES as u64 {
            observe(
                &mut progress,
                &format!(
                    r#"@nix {{"action":"start","id":{id},"level":0,"parent":0,"text":"build","type":105}}"#
                ),
            );
        }

        assert_eq!(progress.activities.len(), MAX_ACTIVE_ACTIVITIES);
    }

    #[test]
    fn renderer_updates_and_clears_one_terminal_line() {
        let mut progress = NixOutputRenderer::default();
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
    fn renderer_indents_every_diagnostic_line() {
        let mut renderer = NixOutputRenderer::default();
        let diagnostic =
            NixDiagnostic::plain("warning: check this\nderivation evaluated".to_string());
        let mut stderr = Vec::new();

        renderer
            .render_diagnostic(&mut stderr, &diagnostic)
            .expect("render live diagnostic");
        NixOutputRenderer::render_captured_diagnostics(
            &mut stderr,
            b"warning: replayed\nderivation replayed\n",
        )
        .expect("render captured diagnostics");

        assert_eq!(
            stderr,
            b"  warning: check this\n  derivation evaluated\n  warning: replayed\n  derivation replayed\n"
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
