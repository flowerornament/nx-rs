use std::io::Write;

use anyhow::Context;

const QUIET_NIX_PENDING_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NixStatusLine {
    SourceFetch,
    StoreCopy,
    Build,
    Plan,
    StorePath,
    BarProgress,
}

#[derive(Default)]
pub(crate) struct CompactNixProgress {
    sources: usize,
    copied_paths: usize,
    builds: usize,
    active: bool,
    rendered_summary: Option<String>,
}

impl CompactNixProgress {
    pub(crate) fn observe(
        &mut self,
        line: NixStatusLine,
        stderr: &mut impl Write,
    ) -> anyhow::Result<()> {
        match line {
            NixStatusLine::SourceFetch => self.sources += 1,
            NixStatusLine::StoreCopy => self.copied_paths += 1,
            NixStatusLine::Build => self.builds += 1,
            NixStatusLine::Plan | NixStatusLine::StorePath | NixStatusLine::BarProgress => {}
        }
        let summary = self.summary();
        if self.rendered_summary.as_deref() == Some(summary.as_str()) {
            return Ok(());
        }

        self.active = true;
        self.rendered_summary = Some(summary.clone());
        write!(stderr, "\r\x1b[2K  nix: {summary}").context("writing child stderr")?;
        stderr.flush().context("flushing child stderr")
    }

    pub(crate) fn clear(&mut self, stderr: &mut impl Write) -> anyhow::Result<()> {
        if self.active {
            write!(stderr, "\r\x1b[2K").context("clearing child stderr progress")?;
            stderr.flush().context("flushing child stderr")?;
            self.active = false;
            self.rendered_summary = None;
        }
        Ok(())
    }

    pub(crate) fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.sources > 0 {
            parts.push(format!("realized {} sources", self.sources));
        }
        if self.copied_paths > 0 {
            parts.push(format!("copied {} paths", self.copied_paths));
        }
        if self.builds > 0 {
            parts.push(format!("building {} derivations", self.builds));
        }
        if parts.is_empty() {
            "preparing build plan".to_string()
        } else {
            parts.join(", ")
        }
    }
}

pub(crate) fn tee_quiet_nix_chunk(
    chunk: &[u8],
    pending: &mut Vec<u8>,
    progress: &mut CompactNixProgress,
    stderr: &mut impl Write,
) -> anyhow::Result<()> {
    for byte in chunk {
        if matches!(byte, b'\n' | b'\r') {
            if !pending.is_empty() {
                tee_quiet_nix_record(pending, progress, stderr)?;
                pending.clear();
            }
            continue;
        }

        pending.push(*byte);
        if pending.len() >= QUIET_NIX_PENDING_LIMIT {
            tee_quiet_nix_record(pending, progress, stderr)?;
            pending.clear();
        }
    }
    Ok(())
}

pub(crate) fn tee_quiet_nix_record(
    record: &[u8],
    progress: &mut CompactNixProgress,
    stderr: &mut impl Write,
) -> anyhow::Result<()> {
    let text = String::from_utf8_lossy(record);
    let visible = visible_record(&text);
    if visible.is_empty() {
        return Ok(());
    }

    if let Some(kind) = classify_nix_chatter_line(visible) {
        progress.observe(kind, stderr)
    } else {
        progress.clear(stderr)?;
        writeln!(stderr, "{visible}").context("writing child stderr")?;
        stderr.flush().context("flushing child stderr")
    }
}

pub(crate) fn classify_nix_chatter_line(line: &str) -> Option<NixStatusLine> {
    let line = line.trim_start();
    if line.starts_with("copying path ") {
        return Some(NixStatusLine::StoreCopy);
    }
    if line.starts_with("building '/nix/store/")
        || line.starts_with("building /nix/store/")
        || line.starts_with("building path(s) ")
    {
        return Some(NixStatusLine::Build);
    }
    if line.starts_with("unpacking ") && line.contains(" into the Git cache") {
        return Some(NixStatusLine::SourceFetch);
    }
    if (line.starts_with("these ") && line.contains(" paths will be fetched"))
        || (line.starts_with("these ") && line.contains(" derivations will be built"))
        || line.starts_with("this derivation will be built")
    {
        return Some(NixStatusLine::Plan);
    }
    if is_plain_store_path_line(line) {
        return Some(NixStatusLine::StorePath);
    }
    if is_bar_progress_line(line) {
        return Some(NixStatusLine::BarProgress);
    }
    None
}

fn visible_record(text: &str) -> &str {
    text.rsplit('\r')
        .next()
        .unwrap_or_default()
        .trim_end_matches(['\n', '\r'])
}

fn is_plain_store_path_line(line: &str) -> bool {
    line.starts_with("/nix/store/") && !line.contains(char::is_whitespace) && !line.contains(':')
}

fn is_bar_progress_line(line: &str) -> bool {
    line.starts_with('[')
        && (line.contains(" built")
            || line.contains(" copied")
            || line.contains(" MiB DL")
            || line.contains(" KiB DL")
            || line.contains(" DL]"))
}
