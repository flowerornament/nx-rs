use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Stdio};

use regex::Regex;
use serde_json::Value;

use crate::domain::config::ConfigFiles;
use crate::domain::plan::InstallPlan;
use crate::domain::source::PackageSource;
use crate::infra::shell::{CapturedCommand, run_captured_command};
use crate::output::printer::{ActivityKind, Printer};
use crate::output::style::OutputStyle;

pub const DEFAULT_CODEX_MODEL: &str = "gpt-5-codex-mini";

// --- Types

/// AI engine routing decision: which file to target and any warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub target_file: String,
    pub warning: Option<String>,
}

/// Outcome of an AI engine command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub success: bool,
    pub output: String,
}

/// Which pathway produced an edit outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditPathway {
    Deterministic,
    AiFallback,
}

/// Unified edit execution result: deterministic callback first, AI fallback second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditExecution {
    pub pathway: EditPathway,
    pub outcome: CommandOutcome,
}

// --- Trait

/// AI engine abstraction for package routing and fallback editing.
///
/// Engines are routing advisors: they pick the target file for general nix
/// packages. The deterministic `file_edit::apply_edit` handles actual insertion.
pub trait AiEngine: Send + Sync {
    /// Route a package to its target config file.
    fn route_package(
        &self,
        package: &str,
        description: &str,
        context: &str,
        candidates: &[String],
        fallback: &str,
        cwd: &Path,
    ) -> RouteDecision;

    /// Execute a freeform edit prompt (fallback for complex edits).
    fn run_edit(&self, prompt: &str, cwd: &Path) -> CommandOutcome;

    /// Whether this engine can handle flake.nix input modifications.
    fn supports_flake_input(&self) -> bool;

    /// Human-readable engine name.
    fn name(&self) -> &'static str;
}

// --- Concrete Adapters

/// Fast non-interactive engine via `codex exec`.
pub struct CodexEngine {
    pub model: String,
}

impl CodexEngine {
    pub fn new(model: Option<&str>) -> Self {
        Self {
            model: model.unwrap_or(DEFAULT_CODEX_MODEL).to_string(),
        }
    }
}

impl AiEngine for CodexEngine {
    fn route_package(
        &self,
        package: &str,
        description: &str,
        context: &str,
        candidates: &[String],
        fallback: &str,
        cwd: &Path,
    ) -> RouteDecision {
        let prompt =
            build_routing_prompt(package, description, context, Some(candidates), fallback);
        resolve_routing_run_result(
            package,
            run_captured_command(
                "codex",
                &["exec", "-m", &self.model, "--full-auto", &prompt],
                Some(cwd),
            ),
            candidates,
            fallback,
        )
    }

    fn run_edit(&self, prompt: &str, cwd: &Path) -> CommandOutcome {
        let result = run_captured_command(
            "codex",
            &["exec", "-m", &self.model, "--full-auto", prompt],
            Some(cwd),
        );
        match result {
            Ok(cmd) => CommandOutcome {
                success: cmd.code == 0,
                output: cmd.stdout,
            },
            Err(e) => CommandOutcome {
                success: false,
                output: e.to_string(),
            },
        }
    }

    fn supports_flake_input(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "codex"
    }
}

/// Interactive engine via `claude --print`.
pub struct ClaudeEngine {
    pub model: Option<String>,
}

impl ClaudeEngine {
    pub fn new(model: Option<&str>) -> Self {
        Self {
            model: model.map(String::from),
        }
    }
}

impl AiEngine for ClaudeEngine {
    fn route_package(
        &self,
        package: &str,
        description: &str,
        context: &str,
        candidates: &[String],
        fallback: &str,
        cwd: &Path,
    ) -> RouteDecision {
        let prompt =
            build_routing_prompt(package, description, context, Some(candidates), fallback);
        let mut args = vec!["--print", "-p", &prompt];
        let model_str;
        if let Some(ref m) = self.model {
            model_str = m.clone();
            args.extend_from_slice(&["-m", &model_str]);
        }
        resolve_routing_run_result(
            package,
            run_captured_command("claude", &args, Some(cwd)),
            candidates,
            fallback,
        )
    }

    fn run_edit(&self, prompt: &str, cwd: &Path) -> CommandOutcome {
        let mut args = vec!["--print", "-p", prompt];
        let model_str;
        if let Some(ref m) = self.model {
            model_str = m.clone();
            args.extend_from_slice(&["-m", &model_str]);
        }
        let result = run_captured_command("claude", &args, Some(cwd));
        match result {
            Ok(cmd) => CommandOutcome {
                success: cmd.code == 0,
                output: cmd.stdout,
            },
            Err(e) => CommandOutcome {
                success: false,
                output: e.to_string(),
            },
        }
    }

    fn supports_flake_input(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "claude"
    }
}

/// Streaming engine via `claude-codes` with activity display.
pub struct ClaudeCodeEngine {
    model: Option<String>,
    style: OutputStyle,
    max_auth: bool,
}

impl ClaudeCodeEngine {
    pub fn new(model: Option<&str>, style: OutputStyle) -> Self {
        let max_auth =
            std::env::var("NX_AI_BILLING").map_or(true, |v| !v.eq_ignore_ascii_case("api"));
        Self {
            model: model.map(String::from),
            style,
            max_auth,
        }
    }

    fn run_streaming(
        &self,
        prompt: &str,
        cwd: &Path,
        allowed_tools: Option<&[&str]>,
        max_turns: Option<u32>,
    ) -> anyhow::Result<String> {
        let printer = Printer::new(self.style);
        let session_id = uuid::Uuid::new_v4();

        let mut cmd = Command::new("claude");
        cmd.args([
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--session-id",
            &session_id.to_string(),
        ]);

        if let Some(ref m) = self.model {
            cmd.args(["-m", m.as_str()]);
        }

        if let Some(tools) = allowed_tools {
            let tool_list = tools.join(",");
            cmd.args(["--allowed-tools", &tool_list]);
        }

        if let Some(turns) = max_turns {
            let turns_str = turns.to_string();
            cmd.args(["--max-turns", &turns_str]);
        }

        cmd.args(["--permission-mode", "bypassPermissions"]);
        cmd.current_dir(cwd);

        if self.max_auth {
            cmd.env("ANTHROPIC_API_KEY", "");
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|_| anyhow::anyhow!("claude CLI not found — is it installed?"))?;

        // Send prompt via stdin as stream-json
        let input = claude_codes::ClaudeInput::user_message(prompt, session_id);
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let json = serde_json::to_string(&input)?;
            writeln!(stdin, "{json}")?;
            drop(stdin);
        }

        // Read streaming JSONL from stdout
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to capture claude stdout"))?;
        let reader = std::io::BufReader::new(stdout);
        let mut final_text = String::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(output) = claude_codes::ClaudeOutput::parse_json_tolerant(&line) else {
                continue;
            };
            match &output {
                claude_codes::ClaudeOutput::Assistant(msg) => {
                    for block in &msg.message.content {
                        match block {
                            claude_codes::ContentBlock::ToolUse(tool) => {
                                report_tool_activity(&printer, &tool.name, &tool.input);
                            }
                            claude_codes::ContentBlock::Text(t) => {
                                final_text.push_str(&t.text);
                            }
                            _ => {}
                        }
                    }
                }
                claude_codes::ClaudeOutput::Result(res) => {
                    if let Some(ref text) = res.result {
                        final_text.clone_from(text);
                    }
                }
                _ => {}
            }
        }

        let _ = child.wait();
        Ok(final_text)
    }
}

impl AiEngine for ClaudeCodeEngine {
    fn route_package(
        &self,
        package: &str,
        description: &str,
        context: &str,
        candidates: &[String],
        fallback: &str,
        cwd: &Path,
    ) -> RouteDecision {
        let prompt =
            build_routing_prompt(package, description, context, Some(candidates), fallback);
        match self.run_streaming(&prompt, cwd, None, Some(1)) {
            Ok(output) if !output.trim().is_empty() => {
                resolve_candidate_routing(package, &output, candidates, fallback)
            }
            _ => RouteDecision {
                target_file: fallback.to_string(),
                warning: Some(format!(
                    "Routing model unavailable for {package}; using fallback {fallback}"
                )),
            },
        }
    }

    fn run_edit(&self, prompt: &str, cwd: &Path) -> CommandOutcome {
        let tools = &["Read", "Edit", "Write", "Bash", "Glob", "Grep"];
        match self.run_streaming(prompt, cwd, Some(tools), None) {
            Ok(output) => CommandOutcome {
                success: true,
                output,
            },
            Err(e) => CommandOutcome {
                success: false,
                output: e.to_string(),
            },
        }
    }

    fn supports_flake_input(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "claude-code"
    }
}

/// Map tool use events to user-visible activity indicators.
fn report_tool_activity(printer: &Printer, tool_name: &str, input: &Value) {
    match tool_name {
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(Value::as_str)
                .and_then(|p| p.rsplit('/').next())
                .unwrap_or("file");
            printer.activity(ActivityKind::Reading, &format!("reading {path}"));
        }
        "Edit" | "Write" => {
            let path = input
                .get("file_path")
                .and_then(Value::as_str)
                .and_then(|p| p.rsplit('/').next())
                .unwrap_or("file");
            printer.activity(ActivityKind::Editing, &format!("editing {path}"));
        }
        "Bash" => {
            let cmd = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("command");
            let short = cmd.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
            printer.activity(ActivityKind::Running, &format!("running {short}"));
        }
        "Glob" | "Grep" => {
            let pattern = input
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or("files");
            printer.activity(ActivityKind::Searching, &format!("searching {pattern}"));
        }
        _ => {}
    }
}

// --- Factory

/// Select the appropriate AI engine based on CLI flags.
pub fn select_engine(
    engine: Option<&str>,
    model: Option<&str>,
    style: OutputStyle,
) -> Box<dyn AiEngine> {
    match engine.unwrap_or("claude-code") {
        "codex" => Box::new(CodexEngine::new(model)),
        "claude" => Box::new(ClaudeEngine::new(model)),
        _ => Box::new(ClaudeCodeEngine::new(model, style)),
    }
}

/// Execute an edit via deterministic callback when available, otherwise AI fallback.
pub fn run_edit_with_callback(
    engine: &dyn AiEngine,
    prompt: &str,
    cwd: &Path,
    callback: impl FnOnce() -> Option<CommandOutcome>,
) -> EditExecution {
    callback().map_or_else(
        || EditExecution {
            pathway: EditPathway::AiFallback,
            outcome: engine.run_edit(prompt, cwd),
        },
        |outcome| EditExecution {
            pathway: EditPathway::Deterministic,
            outcome,
        },
    )
}

fn resolve_routing_run_result(
    package: &str,
    result: anyhow::Result<CapturedCommand>,
    candidates: &[String],
    fallback: &str,
) -> RouteDecision {
    match result {
        Ok(cmd) if cmd.code == 0 && !cmd.stdout.trim().is_empty() => {
            resolve_candidate_routing(package, &cmd.stdout, candidates, fallback)
        }
        _ => RouteDecision {
            target_file: fallback.to_string(),
            warning: Some(format!(
                "Routing model unavailable for {package}; using fallback {fallback}"
            )),
        },
    }
}

// --- Routing Context Builder

/// Build a text context describing the nix config file structure for AI routing.
///
/// Scans `# nx:` tags from discovered config files and appends static routing rules.
pub fn build_routing_context(config: &ConfigFiles) -> String {
    let mut lines = vec!["Nix config file structure:".to_string()];
    let repo_root = config.repo_root();
    let taps_manifest = config.homebrew_taps();
    let taps_rel = taps_manifest
        .strip_prefix(repo_root)
        .unwrap_or(taps_manifest.as_path())
        .to_string_lossy()
        .to_string();

    for (purpose, path) in config.by_purpose() {
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy();
        lines.push(format!("- {rel} \u{2192} {purpose}"));
    }

    // Include untagged files
    for path in config.all_files() {
        let rel = path
            .strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        if !lines.iter().any(|l| l.contains(&rel)) {
            lines.push(format!("- {rel}"));
        }
    }

    lines.push(String::new());
    lines.push("Routing rules:".to_string());
    lines.push("- CLI tools go in packages/nix/cli.nix".to_string());
    lines.push("- Language runtimes/toolchains go in packages/nix/languages.nix".to_string());
    lines.push("- MCP tools (*-mcp, mcp-*) always go in packages/nix/cli.nix".to_string());
    lines.push("- Homebrew formulas go in packages/homebrew/brews.nix".to_string());
    lines.push("- GUI apps (casks) go in packages/homebrew/casks.nix".to_string());
    lines.push(format!("- Homebrew taps go in {taps_rel}"));
    lines.push("- When unsure, use the default install target".to_string());
    lines.push(String::new());
    lines.push("Language packages (add to withPackages, not as standalone):".to_string());
    lines.push(
        "- python3Packages.X \u{2192} add to python3.withPackages in the languages file"
            .to_string(),
    );
    lines
        .push("- luaPackages.X \u{2192} add to lua.withPackages in the languages file".to_string());
    lines.push("- nodePackages.X \u{2192} add to nodejs in the languages file".to_string());

    lines.join("\n")
}

// --- Output Parsing Helpers

/// Strip surrounding punctuation from a potential path token.
fn normalize_path_token(token: &str) -> String {
    token
        .trim()
        .trim_matches(|c: char| {
            matches!(
                c,
                '`' | '"'
                    | '\''
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '.'
                    | ','
                    | ':'
                    | ';'
            )
        })
        .to_string()
}

/// Extract file path tokens (things ending in `.nix`) from AI output text.
pub fn extract_path_tokens(text: &str) -> Vec<String> {
    let re = Regex::new(r"[A-Za-z0-9_./-]+\.nix").expect("valid regex");
    re.find_iter(text)
        .map(|m| normalize_path_token(m.as_str()))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Match a single extracted token against the candidate list.
///
/// Tries exact match, suffix match, then unique basename match.
pub fn match_candidate(token: &str, candidates: &[String]) -> Option<String> {
    // Exact match
    for c in candidates {
        if token == c || token.ends_with(&format!("/{c}")) {
            return Some(c.clone());
        }
    }

    // Basename-only fallback (only if unambiguous)
    let token_basename = Path::new(token)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(token);
    let basename_matches: Vec<&String> = candidates
        .iter()
        .filter(|c| {
            Path::new(c.as_str())
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == token_basename)
        })
        .collect();

    if basename_matches.len() == 1 {
        return Some(basename_matches[0].clone());
    }

    None
}

/// Select candidate files mentioned in AI output.
///
/// Combines regex-based token extraction with direct substring matching.
pub fn select_candidates_from_output(output: &str, candidates: &[String]) -> Vec<String> {
    let mut matches: Vec<String> = Vec::new();

    // Token-based matching
    for token in extract_path_tokens(output) {
        if let Some(matched) = match_candidate(&token, candidates)
            && !matches.contains(&matched)
        {
            matches.push(matched);
        }
    }

    // Direct substring fallback
    for candidate in candidates {
        if output.contains(candidate.as_str()) && !matches.contains(candidate) {
            matches.push(candidate.clone());
        }
    }

    matches
}

/// Resolve a routing decision from AI output against candidates.
///
/// Single match → success. Multiple → ambiguous warning. None → fallback warning.
pub fn resolve_candidate_routing(
    package: &str,
    output: &str,
    candidates: &[String],
    fallback: &str,
) -> RouteDecision {
    let matches = select_candidates_from_output(output, candidates);
    match matches.len() {
        1 => RouteDecision {
            target_file: matches.into_iter().next().expect("len checked"),
            warning: None,
        },
        n if n > 1 => {
            let choices = matches.join(", ");
            RouteDecision {
                target_file: fallback.to_string(),
                warning: Some(format!(
                    "ambiguous routing for {package} ({choices}); using fallback {fallback}"
                )),
            }
        }
        _ => RouteDecision {
            target_file: fallback.to_string(),
            warning: Some(format!(
                "unrecognized routing output for {package}; using fallback {fallback}"
            )),
        },
    }
}

// --- Prompt Builders

/// Build a routing prompt for the AI engine.
///
/// `description` is the human-readable package summary (e.g. "Yet another
/// language server for Nix") and `fallback` marks the default candidate.
pub fn build_routing_prompt(
    package: &str,
    description: &str,
    context: &str,
    candidates: Option<&[String]>,
    fallback: &str,
) -> String {
    let desc_suffix = if description.is_empty() {
        String::new()
    } else {
        format!(" ({description})")
    };
    candidates.map_or_else(
        || {
            format!(
                "{context}\n\nWhich packages/nix/*.nix file for '{package}'{desc_suffix}? Just the path (e.g., packages/nix/cli.nix)."
            )
        },
        |candidates| {
            let list = candidates
                .iter()
                .map(|c| {
                    if c == fallback {
                        format!("- {c}  (default install target)")
                    } else {
                        format!("- {c}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{context}\n\nChoose exactly one file for '{package}'{desc_suffix} from this allowed list:\n{list}\n\nReply with only one exact path from the list."
            )
        },
    )
}

/// Build a removal prompt for AI-based package removal.
pub fn build_remove_prompt(package: &str, file_path: &str) -> String {
    format!(
        "Remove the package \"{package}\" from {file_path}.\n\n\
         Remove the entire line including any inline comment.\n\
         If it was the only item in a section, you can remove the section header comment too.\n\n\
         Only make the edit, no explanation. Use the Edit tool."
    )
}

/// Build an edit prompt from a resolved install plan (fallback for complex edits).
pub fn build_edit_prompt(plan: &InstallPlan) -> String {
    let target = plan.target_file.to_string_lossy();

    if let Some(ref lang) = plan.language_info {
        return format!(
            "Add '{}' to the {}.withPackages list in {}.\n\
             Find the existing {}.withPackages block and add '{}' alphabetically inside the list.\n\
             Just make the edit, no explanation.",
            lang.bare_name, lang.runtime, target, lang.runtime, lang.bare_name,
        );
    }

    match plan.source_result.source {
        PackageSource::Mas => format!(
            "Add \"{}\" to the homebrew.masApps set in {}.\n\
             Look up the App Store ID if needed and add it as \"{}\" = <id>;.\n\
             Keep keys alphabetized. Just make the edit, no explanation.",
            plan.package_token, target, plan.package_token,
        ),
        PackageSource::Homebrew | PackageSource::Cask => {
            let list_name = match plan.source_result.source {
                PackageSource::Homebrew => "brews",
                _ => "casks",
            };
            format!(
                "Add \"{}\" to the homebrew.{} list in {}.\n\
                 Add it alphabetically within the {} list. Just make the edit, no explanation.",
                plan.package_token, list_name, target, list_name,
            )
        }
        _ => format!(
            "Add '{}' to {} in the appropriate section.\n\
             Add it alphabetically within its section. Just make the edit, no explanation.",
            plan.package_token, target,
        ),
    }
}

// --- Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::ConfigFiles;
    use crate::domain::plan::{InsertionMode, InstallPlan, LanguageInfo};
    use crate::domain::source::{PackageSource, SourceResult};
    use std::fs;
    use tempfile::TempDir;

    struct StubEngine {
        outcome: CommandOutcome,
    }

    impl AiEngine for StubEngine {
        fn route_package(
            &self,
            _package: &str,
            _description: &str,
            _context: &str,
            _candidates: &[String],
            fallback: &str,
            _cwd: &Path,
        ) -> RouteDecision {
            RouteDecision {
                target_file: fallback.to_string(),
                warning: None,
            }
        }

        fn run_edit(&self, _prompt: &str, _cwd: &Path) -> CommandOutcome {
            self.outcome.clone()
        }

        fn supports_flake_input(&self) -> bool {
            false
        }

        fn name(&self) -> &'static str {
            "stub"
        }
    }

    fn write_nix(dir: &std::path::Path, rel_path: &str, content: &str) {
        let full = dir.join(rel_path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, content).unwrap();
    }

    fn test_config() -> (TempDir, ConfigFiles) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            "# nx: cli tools and utilities\n[]",
        );
        write_nix(root, "packages/nix/dev.nix", "# nx: development tools\n[]");
        write_nix(
            root,
            "packages/nix/languages.nix",
            "# nx: language runtimes\n[]",
        );
        write_nix(
            root,
            "packages/homebrew/brews.nix",
            "# nx: formula manifest\n[]",
        );
        write_nix(
            root,
            "packages/homebrew/casks.nix",
            "# nx: cask manifest\n[]",
        );
        write_nix(root, "system/darwin.nix", "# nx: macos system\n{}");
        write_nix(root, "home/services.nix", "# nx: services\n{}");

        let cf = ConfigFiles::discover(root);
        (tmp, cf)
    }

    // --- extract_path_tokens ---

    #[test]
    fn extract_tokens_from_typical_output() {
        let output = "The package should go in packages/nix/cli.nix for CLI tools.";
        let tokens = extract_path_tokens(output);
        assert_eq!(tokens, vec!["packages/nix/cli.nix"]);
    }

    #[test]
    fn extract_tokens_handles_backtick_wrapping() {
        let output = "I'd put it in `packages/nix/dev.nix`.";
        let tokens = extract_path_tokens(output);
        assert_eq!(tokens, vec!["packages/nix/dev.nix"]);
    }

    #[test]
    fn extract_tokens_multiple() {
        let output = "Either packages/nix/cli.nix or packages/nix/dev.nix would work.";
        let tokens = extract_path_tokens(output);
        assert_eq!(tokens.len(), 2);
        assert!(tokens.contains(&"packages/nix/cli.nix".to_string()));
        assert!(tokens.contains(&"packages/nix/dev.nix".to_string()));
    }

    #[test]
    fn extract_tokens_no_nix_files() {
        let tokens = extract_path_tokens("I don't know where to put it.");
        assert!(tokens.is_empty());
    }

    // --- match_candidate ---

    #[test]
    fn match_candidate_exact() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        assert_eq!(
            match_candidate("packages/nix/cli.nix", &candidates),
            Some("packages/nix/cli.nix".to_string())
        );
    }

    #[test]
    fn match_candidate_suffix() {
        let candidates = vec!["packages/nix/cli.nix".to_string()];
        assert_eq!(
            match_candidate("/full/path/to/packages/nix/cli.nix", &candidates),
            Some("packages/nix/cli.nix".to_string())
        );
    }

    #[test]
    fn match_candidate_basename_unique() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        assert_eq!(
            match_candidate("dev.nix", &candidates),
            Some("packages/nix/dev.nix".to_string())
        );
    }

    #[test]
    fn match_candidate_basename_ambiguous() {
        // Two candidates share the same basename — should return None
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "home/nix/cli.nix".to_string(),
        ];
        assert_eq!(match_candidate("cli.nix", &candidates), None);
    }

    #[test]
    fn match_candidate_no_match() {
        let candidates = vec!["packages/nix/cli.nix".to_string()];
        assert_eq!(match_candidate("nonexistent.nix", &candidates), None);
    }

    // --- select_candidates_from_output ---

    #[test]
    fn select_single_candidate() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        let matches = select_candidates_from_output("Put it in packages/nix/cli.nix", &candidates);
        assert_eq!(matches, vec!["packages/nix/cli.nix"]);
    }

    #[test]
    fn select_candidates_deduplicates() {
        let candidates = vec!["packages/nix/cli.nix".to_string()];
        let matches = select_candidates_from_output(
            "packages/nix/cli.nix is the right place. Yes, packages/nix/cli.nix.",
            &candidates,
        );
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn select_candidates_direct_substring_fallback() {
        // Even if regex misses due to punctuation, direct substring should catch it
        let candidates = vec!["packages/nix/cli.nix".to_string()];
        let matches = select_candidates_from_output("packages/nix/cli.nix", &candidates);
        assert_eq!(matches.len(), 1);
    }

    // --- resolve_candidate_routing ---

    #[test]
    fn resolve_single_match() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        let decision = resolve_candidate_routing(
            "ripgrep",
            "packages/nix/cli.nix",
            &candidates,
            "packages/nix/cli.nix",
        );
        assert_eq!(decision.target_file, "packages/nix/cli.nix");
        assert!(decision.warning.is_none());
    }

    #[test]
    fn resolve_ambiguous_falls_back() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        let decision = resolve_candidate_routing(
            "ripgrep",
            "Either packages/nix/cli.nix or packages/nix/dev.nix",
            &candidates,
            "packages/nix/cli.nix",
        );
        assert_eq!(decision.target_file, "packages/nix/cli.nix");
        assert!(decision.warning.as_ref().unwrap().contains("ambiguous"));
    }

    #[test]
    fn resolve_no_match_falls_back() {
        let candidates = vec!["packages/nix/cli.nix".to_string()];
        let decision = resolve_candidate_routing(
            "ripgrep",
            "I have no idea",
            &candidates,
            "packages/nix/cli.nix",
        );
        assert_eq!(decision.target_file, "packages/nix/cli.nix");
        assert!(decision.warning.as_ref().unwrap().contains("unrecognized"));
    }

    #[test]
    fn routing_run_silent_fallback_when_command_unavailable() {
        let candidates = vec!["packages/nix/cli.nix".to_string()];
        let decision = resolve_routing_run_result(
            "ripgrep",
            Err(anyhow::anyhow!("command execution failed (codex)")),
            &candidates,
            "packages/nix/cli.nix",
        );
        assert_eq!(decision.target_file, "packages/nix/cli.nix");
        assert_eq!(
            decision.warning.as_deref(),
            Some("Routing model unavailable for ripgrep; using fallback packages/nix/cli.nix")
        );
    }

    #[test]
    fn routing_run_parses_successful_output() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        let decision = resolve_routing_run_result(
            "ripgrep",
            Ok(CapturedCommand {
                code: 0,
                stdout: "packages/nix/dev.nix".to_string(),
                stderr: String::new(),
            }),
            &candidates,
            "packages/nix/cli.nix",
        );
        assert_eq!(decision.target_file, "packages/nix/dev.nix");
        assert!(decision.warning.is_none());
    }

    // --- build_routing_context ---

    #[test]
    fn routing_context_contains_file_structure() {
        let (_tmp, config) = test_config();
        let context = build_routing_context(&config);
        assert!(context.contains("Nix config file structure:"));
        assert!(context.contains("cli.nix"));
        assert!(context.contains("cli tools and utilities"));
    }

    #[test]
    fn routing_context_contains_routing_rules() {
        let (_tmp, config) = test_config();
        let context = build_routing_context(&config);
        assert!(context.contains("Routing rules:"));
        assert!(context.contains("CLI tools go in packages/nix/cli.nix"));
        assert!(context.contains("MCP tools"));
        assert!(context.contains("Homebrew taps go in packages/homebrew/taps.nix"));
        assert!(context.contains("When unsure, use the default install target"));
    }

    #[test]
    fn routing_context_contains_language_guidance() {
        let (_tmp, config) = test_config();
        let context = build_routing_context(&config);
        assert!(context.contains("Language packages"));
        assert!(context.contains("python3Packages"));
    }

    // --- select_engine ---

    fn test_style() -> OutputStyle {
        OutputStyle {
            plain: true,
            icon_set: crate::output::style::IconSet::Minimal,
            color: false,
        }
    }

    #[test]
    fn select_engine_default_is_claude_code() {
        let engine = select_engine(None, None, test_style());
        assert_eq!(engine.name(), "claude-code");
        assert!(engine.supports_flake_input());
    }

    #[test]
    fn select_engine_codex_explicit() {
        let engine = select_engine(Some("codex"), None, test_style());
        assert_eq!(engine.name(), "codex");
    }

    #[test]
    fn select_engine_claude_raw() {
        let engine = select_engine(Some("claude"), None, test_style());
        assert_eq!(engine.name(), "claude");
        assert!(engine.supports_flake_input());
    }

    #[test]
    fn select_engine_unknown_defaults_to_claude_code() {
        let engine = select_engine(Some("unknown"), None, test_style());
        assert_eq!(engine.name(), "claude-code");
    }

    // --- Engine trait properties ---

    #[test]
    fn codex_does_not_support_flake_input() {
        let engine = CodexEngine::new(None);
        assert!(!engine.supports_flake_input());
        assert_eq!(engine.name(), "codex");
    }

    #[test]
    fn codex_engine_uses_default_model() {
        let engine = CodexEngine::new(None);
        assert_eq!(engine.model, DEFAULT_CODEX_MODEL);
    }

    #[test]
    fn claude_supports_flake_input() {
        let engine = ClaudeEngine::new(None);
        assert!(engine.supports_flake_input());
        assert_eq!(engine.name(), "claude");
    }

    #[test]
    fn codex_engine_custom_model() {
        let engine = CodexEngine::new(Some("gpt-4o"));
        assert_eq!(engine.model, "gpt-4o");
    }

    #[test]
    fn claude_engine_custom_model() {
        let engine = ClaudeEngine::new(Some("sonnet"));
        assert_eq!(engine.model, Some("sonnet".to_string()));
    }

    #[test]
    fn claude_code_supports_flake_input() {
        let engine = ClaudeCodeEngine::new(None, test_style());
        assert!(engine.supports_flake_input());
        assert_eq!(engine.name(), "claude-code");
    }

    #[test]
    fn claude_code_max_auth_default() {
        // Without NX_AI_BILLING set, max auth should be enabled
        let engine = ClaudeCodeEngine::new(None, test_style());
        assert!(engine.max_auth);
    }

    #[test]
    fn claude_code_custom_model() {
        let engine = ClaudeCodeEngine::new(Some("haiku"), test_style());
        assert_eq!(engine.model, Some("haiku".to_string()));
    }

    // --- report_tool_activity ---

    #[test]
    fn report_activity_read_extracts_basename() {
        let input: Value = serde_json::json!({"file_path": "/Users/me/repo/packages/nix/cli.nix"});
        // Smoke test: should not panic and should extract "cli.nix"
        let printer = Printer::new(test_style());
        report_tool_activity(&printer, "Read", &input);
    }

    #[test]
    fn report_activity_edit_extracts_basename() {
        let input: Value = serde_json::json!({"file_path": "packages/nix/languages.nix"});
        let printer = Printer::new(test_style());
        report_tool_activity(&printer, "Edit", &input);
    }

    #[test]
    fn report_activity_bash_truncates_command() {
        let input: Value =
            serde_json::json!({"command": "nix eval --raw nixpkgs#ripgrep.meta.description"});
        let printer = Printer::new(test_style());
        report_tool_activity(&printer, "Bash", &input);
    }

    #[test]
    fn report_activity_unknown_tool_is_silent() {
        let input: Value = serde_json::json!({"some": "data"});
        let printer = Printer::new(test_style());
        report_tool_activity(&printer, "UnknownTool", &input);
    }

    // --- claude streaming JSONL parsing ---

    #[test]
    fn parse_assistant_tool_use_from_jsonl() {
        let line = r#"{"type":"assistant","message":{"id":"msg_1","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"packages/nix/cli.nix"}}],"stop_reason":"tool_use"},"session_id":"test-session"}"#;
        let output = claude_codes::ClaudeOutput::parse_json_tolerant(line)
            .expect("should parse assistant message");
        let msg = output.as_assistant().expect("should be assistant");
        assert_eq!(msg.message.content.len(), 1);
        match &msg.message.content[0] {
            claude_codes::ContentBlock::ToolUse(tool) => {
                assert_eq!(tool.name, "Read");
                assert_eq!(
                    tool.input.get("file_path").and_then(Value::as_str),
                    Some("packages/nix/cli.nix")
                );
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_result_message_from_jsonl() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1200,"duration_api_ms":800,"num_turns":1,"result":"packages/nix/cli.nix","session_id":"test-session","total_cost_usd":0.002,"usage":null}"#;
        let output = claude_codes::ClaudeOutput::parse_json_tolerant(line)
            .expect("should parse result message");
        let res = output.as_result().expect("should be result");
        assert_eq!(res.result.as_deref(), Some("packages/nix/cli.nix"));
        assert!(!res.is_error);
    }

    #[test]
    fn parse_text_content_from_assistant() {
        let line = r#"{"type":"assistant","message":{"id":"msg_2","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"text","text":"packages/nix/cli.nix"}],"stop_reason":"end_turn"},"session_id":"test-session"}"#;
        let output = claude_codes::ClaudeOutput::parse_json_tolerant(line)
            .expect("should parse text content");
        let text = output.text_content().expect("should have text");
        assert_eq!(text, "packages/nix/cli.nix");
    }

    // --- build_routing_prompt ---

    #[test]
    fn routing_prompt_with_candidates() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        let prompt = build_routing_prompt(
            "nil",
            "Yet another language server for Nix",
            "context here",
            Some(&candidates),
            "packages/nix/cli.nix",
        );
        assert!(prompt.contains("nil"));
        assert!(prompt.contains("Yet another language server for Nix"));
        assert!(prompt.contains("packages/nix/cli.nix"));
        assert!(prompt.contains("(default install target)"));
        assert!(prompt.contains("packages/nix/dev.nix"));
        assert!(!prompt.contains("dev.nix  (default"));
        assert!(prompt.contains("Choose exactly one file"));
    }

    #[test]
    fn routing_prompt_without_candidates() {
        let prompt = build_routing_prompt("ripgrep", "fast grep", "context here", None, "");
        assert!(prompt.contains("ripgrep"));
        assert!(prompt.contains("(fast grep)"));
        assert!(prompt.contains("Which packages/nix/*.nix file"));
    }

    #[test]
    fn routing_prompt_empty_description() {
        let candidates = vec!["packages/nix/cli.nix".to_string()];
        let prompt = build_routing_prompt(
            "ripgrep",
            "",
            "context here",
            Some(&candidates),
            "packages/nix/cli.nix",
        );
        assert!(prompt.contains("for 'ripgrep' from"));
        assert!(!prompt.contains("()"));
    }

    // --- build_edit_prompt ---

    #[test]
    fn edit_prompt_language_package() {
        let plan = InstallPlan {
            source_result: SourceResult::new("pyyaml", PackageSource::Nxs),
            package_token: "python3Packages.pyyaml".to_string(),
            target_file: "/repo/packages/nix/languages.nix".into(),
            insertion_mode: InsertionMode::LanguageWithPackages,

            language_info: Some(LanguageInfo {
                bare_name: "pyyaml".to_string(),
                runtime: "python3".to_string(),
                method: "withPackages".to_string(),
            }),
            routing_warning: None,
        };
        let prompt = build_edit_prompt(&plan);
        assert!(prompt.contains("pyyaml"));
        assert!(prompt.contains("python3.withPackages"));
    }

    #[test]
    fn edit_prompt_brew_package() {
        let plan = InstallPlan {
            source_result: SourceResult::new("htop", PackageSource::Homebrew),
            package_token: "htop".to_string(),
            target_file: "/repo/packages/homebrew/brews.nix".into(),
            insertion_mode: InsertionMode::HomebrewManifest,

            language_info: None,
            routing_warning: None,
        };
        let prompt = build_edit_prompt(&plan);
        assert!(prompt.contains("htop"));
        assert!(prompt.contains("brews"));
    }

    #[test]
    fn edit_prompt_cask_package() {
        let plan = InstallPlan {
            source_result: SourceResult::new("firefox", PackageSource::Cask),
            package_token: "firefox".to_string(),
            target_file: "/repo/packages/homebrew/casks.nix".into(),
            insertion_mode: InsertionMode::HomebrewManifest,

            language_info: None,
            routing_warning: None,
        };
        let prompt = build_edit_prompt(&plan);
        assert!(prompt.contains("firefox"));
        assert!(prompt.contains("casks"));
    }

    #[test]
    fn edit_prompt_mas_package() {
        let plan = InstallPlan {
            source_result: SourceResult::new("Xcode", PackageSource::Mas),
            package_token: "Xcode".to_string(),
            target_file: "/repo/system/darwin.nix".into(),
            insertion_mode: InsertionMode::MasApps,

            language_info: None,
            routing_warning: None,
        };
        let prompt = build_edit_prompt(&plan);
        assert!(prompt.contains("Xcode"));
        assert!(prompt.contains("masApps"));
    }

    #[test]
    fn edit_prompt_general_nix() {
        let plan = InstallPlan {
            source_result: SourceResult::new("ripgrep", PackageSource::Nxs),
            package_token: "ripgrep".to_string(),
            target_file: "/repo/packages/nix/cli.nix".into(),
            insertion_mode: InsertionMode::NixManifest,

            language_info: None,
            routing_warning: None,
        };
        let prompt = build_edit_prompt(&plan);
        assert!(prompt.contains("ripgrep"));
        assert!(prompt.contains("cli.nix"));
        assert!(prompt.contains("alphabetically"));
    }

    // --- normalize_path_token ---

    #[test]
    fn normalize_strips_backticks_and_quotes() {
        assert_eq!(
            normalize_path_token("`packages/nix/cli.nix`"),
            "packages/nix/cli.nix"
        );
        assert_eq!(
            normalize_path_token("\"packages/nix/cli.nix\""),
            "packages/nix/cli.nix"
        );
    }

    #[test]
    fn normalize_strips_trailing_punctuation() {
        assert_eq!(
            normalize_path_token("packages/nix/cli.nix."),
            "packages/nix/cli.nix"
        );
        assert_eq!(
            normalize_path_token("packages/nix/cli.nix,"),
            "packages/nix/cli.nix"
        );
    }

    // --- build_remove_prompt ---

    #[test]
    fn remove_prompt_contains_package_and_path() {
        let prompt = build_remove_prompt("ripgrep", "packages/nix/cli.nix");
        assert!(prompt.contains("ripgrep"));
        assert!(prompt.contains("packages/nix/cli.nix"));
        assert!(prompt.contains("Remove"));
    }

    #[test]
    fn remove_prompt_instructs_edit_only() {
        let prompt = build_remove_prompt("htop", "packages/nix/cli.nix");
        assert!(prompt.contains("no explanation"));
        assert!(prompt.contains("Edit tool"));
    }

    // --- run_edit_with_callback ---

    #[test]
    fn edit_callback_path_uses_deterministic_outcome() {
        let engine = StubEngine {
            outcome: CommandOutcome {
                success: true,
                output: "ai".to_string(),
            },
        };

        let execution = run_edit_with_callback(&engine, "prompt", Path::new("/tmp"), || {
            Some(CommandOutcome {
                success: true,
                output: "deterministic".to_string(),
            })
        });

        assert_eq!(execution.pathway, EditPathway::Deterministic);
        assert!(execution.outcome.success);
        assert_eq!(execution.outcome.output, "deterministic");
    }

    #[test]
    fn edit_callback_path_falls_back_to_engine() {
        let engine = StubEngine {
            outcome: CommandOutcome {
                success: true,
                output: "ai fallback".to_string(),
            },
        };

        let execution = run_edit_with_callback(&engine, "prompt", Path::new("/tmp"), || None);

        assert_eq!(execution.pathway, EditPathway::AiFallback);
        assert!(execution.outcome.success);
        assert_eq!(execution.outcome.output, "ai fallback");
    }
}
