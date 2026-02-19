use std::path::Path;

use regex::Regex;

use crate::domain::config::ConfigFiles;
use crate::domain::plan::InstallPlan;
use crate::domain::source::PackageSource;
use crate::infra::shell::{CapturedCommand, run_captured_command};

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
    #[allow(dead_code)] // tested; consumed by install logging in future phase
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
        context: &str,
        candidates: &[String],
        fallback: &str,
        cwd: &Path,
    ) -> RouteDecision {
        let prompt = build_routing_prompt(package, context, Some(candidates));
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
        context: &str,
        candidates: &[String],
        fallback: &str,
        cwd: &Path,
    ) -> RouteDecision {
        let prompt = build_routing_prompt(package, context, Some(candidates));
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

// --- Factory

/// Select the appropriate AI engine based on CLI flags.
pub fn select_engine(engine: Option<&str>, model: Option<&str>) -> Box<dyn AiEngine> {
    match engine.unwrap_or("codex") {
        "claude" => Box::new(ClaudeEngine::new(model)),
        _ => Box::new(CodexEngine::new(model)),
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
    lines.push("- Homebrew taps go in packages/homebrew/taps.nix".to_string());
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
pub fn build_routing_prompt(package: &str, context: &str, candidates: Option<&[String]>) -> String {
    candidates.map_or_else(
        || {
            format!(
                "{context}\n\nWhich packages/nix/*.nix file for '{package}'? Just the path (e.g., packages/nix/cli.nix)."
            )
        },
        |candidates| {
            let list = candidates
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{context}\n\nChoose exactly one file for '{package}' from this allowed list:\n{list}\n\nReply with only one exact path from the list."
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
#[allow(dead_code)] // tested; consumed by install AI edit path in future phase
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
    }

    #[test]
    fn routing_context_contains_language_guidance() {
        let (_tmp, config) = test_config();
        let context = build_routing_context(&config);
        assert!(context.contains("Language packages"));
        assert!(context.contains("python3Packages"));
    }

    // --- select_engine ---

    #[test]
    fn select_engine_default_is_codex() {
        let engine = select_engine(None, None);
        assert_eq!(engine.name(), "codex");
        assert!(!engine.supports_flake_input());
    }

    #[test]
    fn select_engine_codex_explicit() {
        let engine = select_engine(Some("codex"), None);
        assert_eq!(engine.name(), "codex");
    }

    #[test]
    fn select_engine_claude() {
        let engine = select_engine(Some("claude"), None);
        assert_eq!(engine.name(), "claude");
        assert!(engine.supports_flake_input());
    }

    #[test]
    fn select_engine_unknown_defaults_to_codex() {
        let engine = select_engine(Some("unknown"), None);
        assert_eq!(engine.name(), "codex");
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

    // --- build_routing_prompt ---

    #[test]
    fn routing_prompt_with_candidates() {
        let candidates = vec![
            "packages/nix/cli.nix".to_string(),
            "packages/nix/dev.nix".to_string(),
        ];
        let prompt = build_routing_prompt("ripgrep", "context here", Some(&candidates));
        assert!(prompt.contains("ripgrep"));
        assert!(prompt.contains("packages/nix/cli.nix"));
        assert!(prompt.contains("packages/nix/dev.nix"));
        assert!(prompt.contains("Choose exactly one file"));
    }

    #[test]
    fn routing_prompt_without_candidates() {
        let prompt = build_routing_prompt("ripgrep", "context here", None);
        assert!(prompt.contains("ripgrep"));
        assert!(prompt.contains("Which packages/nix/*.nix file"));
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
