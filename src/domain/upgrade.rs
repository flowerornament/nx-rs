use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use regex::Regex;
use serde_json::Value;

// ─── Types ───────────────────────────────────────────────────────────────────

/// Source type for a flake.lock input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlakeSourceType {
    Github,
    Tarball,
    Other(String),
}

/// Parsed flake.lock input node.
#[derive(Debug, Clone)]
pub struct FlakeLockInput {
    #[allow(dead_code)] // retained for richer per-input reporting and diagnostics
    pub name: String,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub rev: String,
    pub last_modified: i64,
    #[allow(dead_code)] // retained for source-specific upgrade filtering/reporting
    pub source_type: FlakeSourceType,
}

/// A changed input between two flake.lock states.
#[derive(Debug, Clone)]
pub struct InputChange {
    pub name: String,
    pub owner: String,
    pub repo: String,
    pub old_rev: String,
    pub new_rev: String,
    #[allow(dead_code)] // retained for timestamp-aware changelog/report formatting
    pub old_modified: i64,
    #[allow(dead_code)] // retained for timestamp-aware changelog/report formatting
    pub new_modified: i64,
}

/// Result of comparing two flake.lock states.
#[derive(Debug, Clone)]
pub struct LockDiff {
    pub changed: Vec<InputChange>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

// ─── Lock Parsing ────────────────────────────────────────────────────────────

/// Load and parse `flake.lock` from the repository root.
///
/// Returns an empty map if the lock file doesn't exist.
pub fn load_flake_lock(repo_root: &Path) -> anyhow::Result<HashMap<String, FlakeLockInput>> {
    let lock_path = repo_root.join("flake.lock");
    if !lock_path.exists() {
        return Ok(HashMap::new());
    }
    parse_flake_lock(&lock_path)
}

/// Parse a `flake.lock` JSON file and extract root input information.
///
/// Handles github, tarball (`FlakeHub`), and unknown source types.
/// Skips `file` type inputs (binary artifacts, no changelog).
/// Skips `follows` references (list-valued inputs).
pub fn parse_flake_lock(path: &Path) -> anyhow::Result<HashMap<String, FlakeLockInput>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lock_data: Value =
        serde_json::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;

    let nodes = lock_data
        .get("nodes")
        .and_then(Value::as_object)
        .context("missing nodes in flake.lock")?;

    let root_inputs = nodes
        .get("root")
        .and_then(|r| r.get("inputs"))
        .and_then(Value::as_object);

    let Some(root_inputs) = root_inputs else {
        return Ok(HashMap::new());
    };

    let flakehub_re = Regex::new(r"/f/pinned/([^/]+)/([^/]+)/").expect("valid regex");
    let mut inputs = HashMap::new();

    for (input_name, node_ref) in root_inputs {
        // Skip follows references (list-valued)
        if node_ref.is_array() {
            continue;
        }

        let node_key = node_ref.as_str().unwrap_or(input_name.as_str());
        let Some(node) = nodes.get(node_key) else {
            continue;
        };
        let Some(locked) = node.get("locked").and_then(Value::as_object) else {
            continue;
        };

        let source_type_str = locked
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        // Skip file type (binary artifacts)
        if source_type_str == "file" {
            continue;
        }

        let rev = locked
            .get("rev")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let last_modified = locked
            .get("lastModified")
            .and_then(Value::as_i64)
            .unwrap_or(0);

        let (owner, repo, source_type) = match source_type_str {
            "github" => {
                let owner = locked
                    .get("owner")
                    .and_then(Value::as_str)
                    .map(String::from);
                let repo = locked.get("repo").and_then(Value::as_str).map(String::from);
                (owner, repo, FlakeSourceType::Github)
            }
            "tarball" => {
                let url = locked
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let (owner, repo) = flakehub_re.captures(url).map_or((None, None), |caps| {
                    (Some(caps[1].to_string()), Some(caps[2].to_string()))
                });
                (owner, repo, FlakeSourceType::Tarball)
            }
            other => {
                let owner = locked
                    .get("owner")
                    .and_then(Value::as_str)
                    .map(String::from);
                let repo = locked.get("repo").and_then(Value::as_str).map(String::from);
                (owner, repo, FlakeSourceType::Other(other.to_string()))
            }
        };

        inputs.insert(
            input_name.clone(),
            FlakeLockInput {
                name: input_name.clone(),
                owner,
                repo,
                rev,
                last_modified,
                source_type,
            },
        );
    }

    Ok(inputs)
}

// ─── Lock Diff ───────────────────────────────────────────────────────────────

/// Compare two flake.lock states and find changes.
///
/// Only tracks changed inputs that have owner/repo (GitHub-trackable sources).
pub fn diff_locks(
    old: &HashMap<String, FlakeLockInput>,
    new: &HashMap<String, FlakeLockInput>,
) -> LockDiff {
    let mut changed = Vec::new();
    let mut added: Vec<String> = new
        .keys()
        .filter(|k| !old.contains_key(k.as_str()))
        .cloned()
        .collect();
    let mut removed: Vec<String> = old
        .keys()
        .filter(|k| !new.contains_key(k.as_str()))
        .cloned()
        .collect();

    added.sort();
    removed.sort();

    for (name, new_input) in new {
        let Some(old_input) = old.get(name) else {
            continue;
        };

        if old_input.rev == new_input.rev {
            continue;
        }

        // Only track changes for inputs with GitHub info
        if let (Some(owner), Some(repo)) = (&new_input.owner, &new_input.repo) {
            changed.push(InputChange {
                name: name.clone(),
                owner: owner.clone(),
                repo: repo.clone(),
                old_rev: old_input.rev.clone(),
                new_rev: new_input.rev.clone(),
                old_modified: old_input.last_modified,
                new_modified: new_input.last_modified,
            });
        }
    }

    changed.sort_by(|a, b| a.name.cmp(&b.name));

    LockDiff {
        changed,
        added,
        removed,
    }
}

/// Shorten a git revision to 7 characters.
pub fn short_rev(rev: &str) -> &str {
    if rev.len() >= 7 { &rev[..7] } else { rev }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_lock(dir: &Path, content: &str) {
        fs::write(dir.join("flake.lock"), content).unwrap();
    }

    const FIXTURE_LOCK: &str = r#"{
  "nodes": {
    "home-manager": {
      "locked": {
        "lastModified": 1700000000,
        "owner": "nix-community",
        "repo": "home-manager",
        "rev": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "type": "github"
      }
    },
    "nixpkgs_2": {
      "locked": {
        "lastModified": 1700000001,
        "owner": "NixOS",
        "repo": "nixpkgs",
        "rev": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "type": "github"
      }
    },
    "flakehub-input": {
      "locked": {
        "lastModified": 1700000002,
        "rev": "cccccccccccccccccccccccccccccccccccccccc",
        "type": "tarball",
        "url": "https://api.flakehub.com/f/pinned/DeterminateSystems/nuenv/0.1.0/018c6d7e-3b22-7e3c-a3c6-00b72d7f6bed/source.tar.gz"
      }
    },
    "binary-artifact": {
      "locked": {
        "type": "file",
        "url": "https://example.com/binary.tar.gz"
      }
    },
    "root": {
      "inputs": {
        "home-manager": "home-manager",
        "nixpkgs": "nixpkgs_2",
        "flakehub-input": "flakehub-input",
        "binary-artifact": "binary-artifact",
        "follows-ref": ["nixpkgs"]
      }
    }
  }
}"#;

    const FIXTURE_LOCK_UPDATED: &str = r#"{
  "nodes": {
    "home-manager": {
      "locked": {
        "lastModified": 1700100000,
        "owner": "nix-community",
        "repo": "home-manager",
        "rev": "1111111111111111111111111111111111111111",
        "type": "github"
      }
    },
    "nixpkgs_2": {
      "locked": {
        "lastModified": 1700000001,
        "owner": "NixOS",
        "repo": "nixpkgs",
        "rev": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "type": "github"
      }
    },
    "new-input": {
      "locked": {
        "lastModified": 1700200000,
        "owner": "new-org",
        "repo": "new-repo",
        "rev": "dddddddddddddddddddddddddddddddddddddddd",
        "type": "github"
      }
    },
    "root": {
      "inputs": {
        "home-manager": "home-manager",
        "nixpkgs": "nixpkgs_2",
        "new-input": "new-input"
      }
    }
  }
}"#;

    // --- parse_flake_lock ---

    #[test]
    fn parse_extracts_github_inputs() {
        let tmp = TempDir::new().unwrap();
        write_lock(tmp.path(), FIXTURE_LOCK);

        let inputs = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();
        assert!(inputs.contains_key("home-manager"));

        let hm = &inputs["home-manager"];
        assert_eq!(hm.owner.as_deref(), Some("nix-community"));
        assert_eq!(hm.repo.as_deref(), Some("home-manager"));
        assert_eq!(hm.source_type, FlakeSourceType::Github);
        assert!(hm.rev.starts_with("aaaa"));
    }

    #[test]
    fn parse_handles_indirection() {
        let tmp = TempDir::new().unwrap();
        write_lock(tmp.path(), FIXTURE_LOCK);

        let inputs = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();
        // "nixpkgs" in root.inputs points to "nixpkgs_2" node
        assert!(inputs.contains_key("nixpkgs"));
        let np = &inputs["nixpkgs"];
        assert_eq!(np.owner.as_deref(), Some("NixOS"));
        assert_eq!(np.repo.as_deref(), Some("nixpkgs"));
    }

    #[test]
    fn parse_extracts_flakehub_tarball() {
        let tmp = TempDir::new().unwrap();
        write_lock(tmp.path(), FIXTURE_LOCK);

        let inputs = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();
        assert!(inputs.contains_key("flakehub-input"));

        let fh = &inputs["flakehub-input"];
        assert_eq!(fh.owner.as_deref(), Some("DeterminateSystems"));
        assert_eq!(fh.repo.as_deref(), Some("nuenv"));
        assert_eq!(fh.source_type, FlakeSourceType::Tarball);
    }

    #[test]
    fn parse_skips_file_type() {
        let tmp = TempDir::new().unwrap();
        write_lock(tmp.path(), FIXTURE_LOCK);

        let inputs = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();
        assert!(!inputs.contains_key("binary-artifact"));
    }

    #[test]
    fn parse_skips_follows_refs() {
        let tmp = TempDir::new().unwrap();
        write_lock(tmp.path(), FIXTURE_LOCK);

        let inputs = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();
        assert!(!inputs.contains_key("follows-ref"));
    }

    #[test]
    fn parse_missing_lock_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let inputs = load_flake_lock(tmp.path()).unwrap();
        assert!(inputs.is_empty());
    }

    // --- diff_locks ---

    #[test]
    fn diff_detects_changed_inputs() {
        let tmp = TempDir::new().unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK);
        let old = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK_UPDATED);
        let new = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        let diff = diff_locks(&old, &new);

        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].name, "home-manager");
        assert!(diff.changed[0].old_rev.starts_with("aaaa"));
        assert!(diff.changed[0].new_rev.starts_with("1111"));
    }

    #[test]
    fn diff_detects_added_inputs() {
        let tmp = TempDir::new().unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK);
        let old = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK_UPDATED);
        let new = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        let diff = diff_locks(&old, &new);
        assert!(diff.added.contains(&"new-input".to_string()));
    }

    #[test]
    fn diff_detects_removed_inputs() {
        let tmp = TempDir::new().unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK);
        let old = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK_UPDATED);
        let new = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        let diff = diff_locks(&old, &new);
        assert!(diff.removed.contains(&"flakehub-input".to_string()));
    }

    #[test]
    fn diff_unchanged_inputs_not_in_changed() {
        let tmp = TempDir::new().unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK);
        let old = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        write_lock(tmp.path(), FIXTURE_LOCK_UPDATED);
        let new = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();

        let diff = diff_locks(&old, &new);
        // nixpkgs didn't change
        assert!(
            !diff.changed.iter().any(|c| c.name == "nixpkgs"),
            "unchanged inputs should not appear in changed"
        );
    }

    #[test]
    fn diff_identical_locks_empty() {
        let tmp = TempDir::new().unwrap();
        write_lock(tmp.path(), FIXTURE_LOCK);

        let inputs = parse_flake_lock(&tmp.path().join("flake.lock")).unwrap();
        let diff = diff_locks(&inputs, &inputs);

        assert!(diff.changed.is_empty());
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
    }

    // --- short_rev ---

    #[test]
    fn short_rev_truncates_to_seven() {
        assert_eq!(
            short_rev("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "aaaaaaa"
        );
    }

    #[test]
    fn short_rev_short_input_unchanged() {
        assert_eq!(short_rev("abc"), "abc");
    }
}
