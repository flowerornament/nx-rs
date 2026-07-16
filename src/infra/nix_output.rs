use std::borrow::Cow;

use serde::Deserialize;

const NIX_JSON_PREFIX: &str = "@nix ";
const NIX_RECORD_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NixOutputMode {
    NativeBar,
    NativeBarWithLogs,
    Structured,
}

impl NixOutputMode {
    pub(crate) const fn for_terminal(verbose: bool, terminal_available: bool) -> Self {
        if terminal_available {
            if verbose {
                Self::NativeBarWithLogs
            } else {
                Self::NativeBar
            }
        } else {
            Self::Structured
        }
    }

    pub(crate) const fn log_format(self) -> NixLogFormat {
        match self {
            Self::NativeBar => NixLogFormat::Bar,
            Self::NativeBarWithLogs => NixLogFormat::BarWithLogs,
            Self::Structured => NixLogFormat::InternalJson,
        }
    }

    pub(crate) const fn is_native(self) -> bool {
        matches!(self, Self::NativeBar | Self::NativeBarWithLogs)
    }

    pub(crate) fn command_args(self, base_args: &[String]) -> Vec<String> {
        let mut args = Vec::with_capacity(base_args.len() + 2);
        args.extend([
            "--log-format".to_string(),
            self.log_format().as_arg().to_string(),
        ]);
        args.extend(without_log_format_args(base_args));
        args
    }
}

pub(crate) fn without_log_format_args(args: &[String]) -> impl Iterator<Item = String> + '_ {
    let mut skip_next = false;
    args.iter().filter_map(move |arg| {
        if skip_next {
            skip_next = false;
            return None;
        }
        if arg == "--log-format" {
            skip_next = true;
            return None;
        }
        (!arg.starts_with("--log-format=")).then(|| arg.clone())
    })
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

pub(crate) fn decode_nix_record(record: &[u8]) -> NixRecord {
    let text = String::from_utf8_lossy(record);
    let visible = visible_record(&text);
    if visible.is_empty() {
        return NixRecord::Ignored;
    }

    let Some(json) = visible.strip_prefix(NIX_JSON_PREFIX) else {
        return NixRecord::Diagnostic(NixDiagnostic::plain(visible.to_string()));
    };
    let Ok(record) = serde_json::from_str::<NixJsonRecord<'_>>(json) else {
        return NixRecord::Diagnostic(NixDiagnostic::plain(visible.to_string()));
    };

    match record.action.as_ref() {
        "start" => NixRecord::Activity(record.kind.map_or(NixActivityType::Unknown, Into::into)),
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

pub(crate) enum NixRecord {
    Activity(NixActivityType),
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
    #[serde(default, rename = "type")]
    kind: Option<u64>,
    #[serde(default, borrow)]
    msg: Cow<'a, str>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_message_becomes_decoded_diagnostic() {
        let record = decode_nix_record(
            r#"@nix {"action":"msg","level":0,"msg":"error: boom\nspecified: old\ngot: new"}"#
                .as_bytes(),
        );

        let NixRecord::Diagnostic(diagnostic) = record else {
            panic!("expected diagnostic");
        };
        assert_eq!(diagnostic.message, "error: boom\nspecified: old\ngot: new");
    }

    #[test]
    fn structured_start_becomes_typed_activity() {
        let record =
            decode_nix_record(br#"@nix {"action":"start","id":1,"level":0,"parent":0,"type":104}"#);
        assert!(matches!(
            record,
            NixRecord::Activity(NixActivityType::Builds)
        ));
    }

    #[test]
    fn non_protocol_text_remains_a_diagnostic() {
        let record = decode_nix_record(b"remote: Repository not found.");
        assert!(matches!(record, NixRecord::Diagnostic(_)));
    }

    #[test]
    fn command_output_mode_owns_log_format() {
        let base = vec![
            "flake".to_string(),
            "check".to_string(),
            "--log-format".to_string(),
            "raw".to_string(),
            "--log-format=raw".to_string(),
            "--show-trace".to_string(),
        ];

        assert_eq!(
            NixOutputMode::NativeBar.command_args(&base),
            ["--log-format", "bar", "flake", "check", "--show-trace"]
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
