//! Subscription-only CLI boundary. Never reads or changes credential files.
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde_json::Value;

use super::bounded_command;
use super::shell::CapturedCommand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Provider {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    ReadOnly,
    Edit,
}

const CODEX_PROVIDER: &str = "model_provider=\"openai\"";
const CODEX_LOGIN: &str = "forced_login_method=\"chatgpt\"";
const BILLING_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_AWS_API_KEY",
    "ANTHROPIC_AWS_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_SIMPLE",
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "OPENAI_BASE_URL",
    "CODEX_APP_SERVER_CHATGPT_BASE_URL",
];
static CODEX_MCP: OnceLock<Result<Vec<String>, String>> = OnceLock::new();
static CLAUDE_AUTH: OnceLock<Result<(), String>> = OnceLock::new();
static CODEX_AUTH: OnceLock<Result<(), String>> = OnceLock::new();

impl Provider {
    pub(crate) const fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn command(self, cwd: Option<&Path>, purpose: Option<Purpose>) -> Command {
        let mut command = super::shell::operation_command(self.program());
        for key in BILLING_ENV {
            command.env_remove(key);
        }
        match self {
            Self::Claude => {
                let env = BILLING_ENV
                    .iter()
                    .map(|key| ((*key).to_owned(), serde_json::Value::String(String::new())))
                    .collect::<serde_json::Map<_, _>>();
                let mut policy = serde_json::json!({"forceLoginMethod":"claudeai", "env": env});
                if purpose == Some(Purpose::ReadOnly) {
                    policy["disableAllHooks"] = Value::Bool(true);
                }
                command.args(["--settings", &policy.to_string()]);
                if purpose == Some(Purpose::ReadOnly) {
                    command.args([
                        "--tools",
                        "",
                        "--strict-mcp-config",
                        "--mcp-config",
                        r#"{"mcpServers":{}}"#,
                        "--disable-slash-commands",
                        "--no-session-persistence",
                        "--effort",
                        "low",
                    ]);
                }
            }
            Self::Codex => {
                command.args([
                    "-c",
                    CODEX_PROVIDER,
                    "-c",
                    "openai_base_url=\"https://chatgpt.com/backend-api/codex\"",
                    "-c",
                    "chatgpt_base_url=\"https://chatgpt.com/backend-api/\"",
                ]);
                if purpose.is_some() {
                    command.args(["-c", CODEX_LOGIN]);
                }
                if purpose == Some(Purpose::ReadOnly) {
                    command.args([
                        "--config",
                        "features.plugins=false",
                        "--config",
                        "features.hooks=false",
                        "--config",
                        "notify=[]",
                        "--config",
                        "web_search=\"disabled\"",
                        "--config",
                        "features.shell_tool=false",
                        "--config",
                        "model_reasoning_effort=\"low\"",
                    ]);
                }
            }
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
    }

    pub(crate) fn ensure_login(
        self,
        cwd: Option<&Path>,
        deadline: Instant,
        cancel: Option<&AtomicBool>,
    ) -> Result<()> {
        let cache = match self {
            Self::Claude => &CLAUDE_AUTH,
            Self::Codex => &CODEX_AUTH,
        };
        let result = cache.get_or_init(|| {
            let mut command = self.command(cwd, None);
            let args: &[&str] = match self {
                Self::Claude => &["auth", "status", "--json"],
                Self::Codex => &["login", "status"],
            };
            command.args(args);
            let output = bounded_command::run_prepared(
                command, self.program(), &["auth-status"],
                deadline.min(Instant::now() + Duration::from_secs(2)), cancel, |_| {},
            ).map_err(|_| format!("{} subscription login could not be checked", self.program()))?;
            if output.code == 0 && subscription_status(self, &output.stdout, &output.stderr) {
                Ok(())
            } else {
                let login = if self == Self::Claude { "claude auth login" } else { "codex login (ChatGPT)" };
                Err(format!("{} subscription login unavailable; sign in with {login}. API billing is disabled in nx", self.program()))
            }
        });
        result
            .as_ref()
            .map_err(|message| anyhow::anyhow!(message.clone()))
            .copied()
    }

    fn disabled_mcp(
        self,
        cwd: Option<&Path>,
        deadline: Instant,
        cancel: Option<&AtomicBool>,
    ) -> Result<&'static [String]> {
        let settings = CODEX_MCP.get_or_init(|| {
            let mut command = self.command(cwd, None);
            command.args(["mcp", "list", "--json"]);
            let output = bounded_command::run_prepared(
                command,
                self.program(),
                &["mcp", "list"],
                deadline.min(Instant::now() + Duration::from_secs(2)),
                cancel,
                |_| {},
            )
            .map_err(|_| "Codex tool configuration could not be checked".to_owned())?;
            if output.code != 0 {
                return Err(
                    "Codex tool configuration unavailable; skipped read-only inference".into(),
                );
            }
            let entries: Vec<Value> = serde_json::from_str(&output.stdout)
                .map_err(|_| "Invalid Codex tool configuration".to_owned())?;
            if entries.len() > 100
                || entries
                    .iter()
                    .any(|entry| entry["enabled"].as_bool().is_none())
            {
                return Err("Invalid or oversized Codex tool configuration".into());
            }
            entries
                .iter()
                .filter(|entry| entry["enabled"].as_bool() == Some(true))
                .map(|entry| {
                    let name = entry["name"]
                        .as_str()
                        .ok_or_else(|| "Invalid Codex tool name".to_owned())?;
                    if name.is_empty()
                        || !name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    {
                        return Err(
                            "Unsupported Codex tool name; skipped read-only inference".into()
                        );
                    }
                    // Plugin-provided entries need a valid transport even when disabled.
                    let transport = match entry["transport"]["type"].as_str() {
                        Some("stdio") => "command=\"false\"",
                        Some("streamable_http") => "url=\"http://127.0.0.1:9\"",
                        _ => {
                            return Err(
                                "Unsupported Codex tool transport; skipped read-only inference"
                                    .into(),
                            );
                        }
                    };
                    Ok(format!("mcp_servers.{name}={{{transport},enabled=false}}"))
                })
                .collect()
        });
        settings
            .as_ref()
            .map(Vec::as_slice)
            .map_err(|error| anyhow::anyhow!(error.clone()))
    }

    pub(crate) fn run(
        self,
        purpose: Purpose,
        args: &[&str],
        cwd: Option<&Path>,
        deadline: Instant,
        cancel: Option<&AtomicBool>,
        observe: impl FnMut(&[u8]),
    ) -> Result<CapturedCommand> {
        self.ensure_login(cwd, deadline, cancel)?;
        if std::env::var("NX_AI_BILLING").is_ok_and(|value| value.eq_ignore_ascii_case("api")) {
            bail!("NX_AI_BILLING=api is unsupported; nx only uses subscription logins");
        }
        let mut command = self.command(cwd, Some(purpose));
        command.args(args);
        if self == Self::Codex && purpose == Purpose::ReadOnly {
            for setting in self.disabled_mcp(cwd, deadline, cancel)? {
                command.args(["-c", setting]);
            }
        }
        bounded_command::run_prepared(
            command,
            self.program(),
            &["inference"],
            deadline,
            cancel,
            observe,
        )
    }
}

fn subscription_status(provider: Provider, stdout: &str, stderr: &str) -> bool {
    match provider {
        Provider::Claude => serde_json::from_str::<Value>(stdout)
            .ok()
            .is_some_and(|value| {
                value["loggedIn"] == true
                    && value["authMethod"] == "claude.ai"
                    && value
                        .get("apiKeySource")
                        .is_none_or(|source| source.is_null() || source == "none")
            }),
        Provider::Codex => stdout
            .lines()
            .chain(stderr.lines())
            .any(|line| line.trim() == "Logged in using ChatGPT"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_subscription_authentication_is_accepted() {
        assert!(subscription_status(
            Provider::Claude,
            r#"{"loggedIn":true,"authMethod":"claude.ai","apiKeySource":null}"#,
            ""
        ));
        for status in [
            r#"{"loggedIn":true,"authMethod":"api_key","apiKeySource":"ANTHROPIC_API_KEY"}"#,
            r#"{"loggedIn":true,"authMethod":"claude.ai","apiKeySource":"helper"}"#,
            r#"{"loggedIn":false,"authMethod":"claude.ai"}"#,
            "invalid",
        ] {
            assert!(!subscription_status(Provider::Claude, status, ""));
        }
        assert!(subscription_status(
            Provider::Codex,
            "",
            "Logged in using ChatGPT\n"
        ));
        assert!(!subscription_status(
            Provider::Codex,
            "",
            "Logged in using an API key: hidden"
        ));
        assert!(!subscription_status(Provider::Codex, "Not logged in", ""));
    }

    #[test]
    fn child_policy_clears_billing_inputs_but_does_not_pin_a_model() {
        for provider in [Provider::Claude, Provider::Codex] {
            let command = provider.command(None, Some(Purpose::Edit));
            for key in BILLING_ENV {
                assert!(
                    command
                        .get_envs()
                        .any(|(name, value)| name == *key && value.is_none()),
                    "{key}"
                );
            }
            let args = command
                .get_args()
                .map(|arg| arg.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            assert!(!args.iter().any(|arg| arg == "--model" || arg == "-m"));
            if provider == Provider::Codex {
                assert!(args.iter().any(|arg| arg == CODEX_LOGIN));
                assert!(args.iter().any(|arg| arg == CODEX_PROVIDER));
            } else {
                let policy = args
                    .windows(2)
                    .find(|pair| pair[0] == "--settings")
                    .map(|pair| serde_json::from_str::<Value>(&pair[1]).unwrap())
                    .unwrap();
                assert_eq!(policy["forceLoginMethod"], "claudeai");
                for key in BILLING_ENV {
                    assert_eq!(policy["env"][*key], "");
                }
            }
        }
    }

    #[test]
    fn codex_status_check_cannot_force_a_mismatched_login_or_log_the_user_out() {
        let command = Provider::Codex.command(None, None);
        assert!(!command.get_args().any(|arg| arg == CODEX_LOGIN));
    }
}

#[cfg(test)]
mod subprocess_tests {
    use super::*;
    use crate::infra::ai_engine::{AiEngine, ClaudeCodeEngine};
    use crate::output::style::{IconSet, OutputStyle};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn all_adapters_enforce_auth_without_using_host_credentials() {
        for provider in ["claude", "codex"] {
            for scenario in ["subscription", "api", "missing"] {
                probe(provider, scenario);
            }
        }
        probe("codex", "mcp_bad");
        for scenario in [
            "stream_good",
            "stream_error",
            "stream_bad_subtype",
            "stream_exit",
            "stream_missing",
        ] {
            probe("claude", scenario);
        }
    }

    fn probe(provider: &str, scenario: &str) {
        let temp = tempfile::tempdir().unwrap();
        for name in ["claude", "codex"] {
            let path = temp.path().join(name);
            fs::write(&path, STUB).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "infra::ai_cli::subprocess_tests::child",
                "--ignored",
                "--nocapture",
            ])
            .env("NX_AI_TEST_PROVIDER", provider)
            .env("NX_AI_TEST_SCENARIO", scenario)
            .env("NX_AI_TEST_DIR", temp.path())
            .env("PATH", temp.path())
            .env("HOME", temp.path())
            .env_remove("NX_AI_BILLING")
            .env("ANTHROPIC_API_KEY", "dummy-must-not-reach-cli")
            .env("OPENAI_API_KEY", "dummy-must-not-reach-cli")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{provider}/{scenario}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            temp.path().join("inference").exists(),
            !matches!(scenario, "api" | "missing" | "mcp_bad")
        );
    }

    #[test]
    #[ignore = "subprocess entry point"]
    fn child() {
        let Ok(scenario) = std::env::var("NX_AI_TEST_SCENARIO") else {
            return;
        };
        if scenario.starts_with("stream_") {
            let engine = ClaudeCodeEngine::new(
                None,
                OutputStyle {
                    plain: true,
                    icon_set: IconSet::Minimal,
                    color: false,
                },
            );
            let outcome = engine.run_edit("test prompt", Path::new("."));
            assert_eq!(outcome.success, scenario == "stream_good");
        } else {
            let provider = if std::env::var("NX_AI_TEST_PROVIDER").unwrap() == "codex" {
                Provider::Codex
            } else {
                Provider::Claude
            };
            let result = provider.run(
                Purpose::ReadOnly,
                if provider == Provider::Claude {
                    &["--print", "-p", "test prompt"]
                } else {
                    &["exec", "--sandbox", "read-only", "test prompt"]
                },
                None,
                Instant::now() + Duration::from_secs(5),
                None,
                |_| {},
            );
            assert_eq!(result.is_ok(), scenario == "subscription");
            if let Ok(result) = result {
                assert_eq!(result.code, 0);
                assert_eq!(result.stdout.trim(), "done");
            }
        }
    }

    const STUB: &str = r#"#!/bin/sh
set -eu
[ -z "${ANTHROPIC_API_KEY:-}" ] && [ -z "${OPENAI_API_KEY:-}" ] || exit 71
case " $* " in
  *" auth status "*)
    case "$NX_AI_TEST_SCENARIO" in
      api) printf '%s\n' '{"loggedIn":true,"authMethod":"api_key","apiKeySource":"helper"}' ;;
      missing) printf '%s\n' '{"loggedIn":false,"authMethod":"none"}' ;;
      *) printf '%s\n' '{"loggedIn":true,"authMethod":"claude.ai","apiKeySource":null}' ;;
    esac
    exit 0 ;;
  *" mcp list "*)
    if [ "$NX_AI_TEST_SCENARIO" = mcp_bad ]; then printf '%s\n' '[{"name":"unknown"}]';
    else printf '%s\n' '[{"name":"example_tool","enabled":true,"transport":{"type":"stdio"}}]'; fi
    exit 0 ;;
  *" login status "*)
    case " $* " in *forced_login_method*) exit 72 ;; esac
    case "$NX_AI_TEST_SCENARIO" in
      api) printf '%s\n' 'Logged in using an API key: hidden' >&2 ;;
      missing) printf '%s\n' 'Not logged in' >&2 ;;
      *) printf '%s\n' 'Logged in using ChatGPT' >&2 ;;
    esac
    exit 0 ;;
esac
case " $* " in
  *" exec "*) case " $* " in *"mcp_servers.example_tool={command=\"false\",enabled=false}"*) : ;; *) exit 73 ;; esac ;;
esac
: > "$NX_AI_TEST_DIR/inference"
case "$NX_AI_TEST_SCENARIO" in
  stream_good|stream_exit)
    printf '%s' '{"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"result":"updated","session_id":"test","total_cost_usd":0,"usage":null}'
    if [ "$NX_AI_TEST_SCENARIO" = stream_exit ]; then exit 7; fi ;;
  stream_error)
    printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"result":"limit reached","session_id":"test","total_cost_usd":0,"usage":null}' ;;
  stream_bad_subtype)
    printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"result":"partial","session_id":"test","total_cost_usd":0,"usage":null}' ;;
  stream_missing) printf '%s\n' '{"type":"unknown"}' ;;
  *) printf '%s\n' done ;;
esac
"#;
}

#[cfg(test)]
mod live_smoke {
    use super::*;

    #[test]
    #[ignore = "explicit subscription-only CLI smoke; NX_AI_SMOKE_PROVIDER required"]
    fn subscription_cli() {
        let provider = match std::env::var("NX_AI_SMOKE_PROVIDER").as_deref() {
            Ok("claude") => Provider::Claude,
            Ok("codex") => Provider::Codex,
            _ => panic!("set NX_AI_SMOKE_PROVIDER=claude or codex explicitly"),
        };
        let prompt = "Reply with exactly OK. Do not use tools.";
        let mut args = if provider == Provider::Claude {
            vec!["--print", "--max-turns", "1", "-p", prompt]
        } else {
            crate::infra::ai_engine::codex_exec_args(prompt, None, "read-only")
        };
        if provider == Provider::Codex {
            args.extend(["--skip-git-repo-check", "--ephemeral"]);
        }
        let cwd = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let output = provider
            .run(
                Purpose::ReadOnly,
                &args,
                Some(cwd.path()),
                started + Duration::from_secs(20),
                None,
                |_| {},
            )
            .unwrap();
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert_eq!(output.stdout.trim(), "OK");
        eprintln!(
            "{} subscription smoke: {:.2}s",
            provider.program(),
            started.elapsed().as_secs_f64()
        );
    }
}
