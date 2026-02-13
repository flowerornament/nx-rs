// No consumers yet — downstream commands wire in via .12/.13
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::domain::source::{SourceResult, normalize_name};

const CACHE_SCHEMA_VERSION: u64 = 1;
const CACHE_FILENAME: &str = "packages_v4.json";
const SOURCE_PRIORITY: &[&str] = &["nxs", "nur", "homebrew", "cask"];

/// Maps source names to flake.lock input names for revision lookup.
fn source_to_input(source: &str) -> &str {
    match source {
        "nxs" => "nxs",
        "nur" => "nur",
        "homebrew" | "cask" => "homebrew",
        "mas" => "mas",
        other => other,
    }
}

/// JSON cache for package lookups, keyed to source revision.
///
/// SPEC §5: schema envelope, revision-aware keys, alias-normalized lookups,
/// source-priority retrieval, homebrew-only guardrail.
pub struct MultiSourceCache {
    cache_path: PathBuf,
    revisions: HashMap<String, String>,
    entries: HashMap<String, Value>,
}

impl MultiSourceCache {
    /// Load (or initialize) the cache for a given repo root.
    ///
    /// Silently degrades to empty state on any I/O or schema error.
    pub fn load(repo_root: &Path) -> Self {
        let cache_dir = dirs_cache().join("nx");
        Self::load_with_cache_dir(repo_root, &cache_dir)
    }

    /// Load with an explicit cache directory (used by tests to avoid touching `$HOME`).
    pub fn load_with_cache_dir(repo_root: &Path, cache_dir: &Path) -> Self {
        let _ = fs::create_dir_all(cache_dir);
        let cache_path = cache_dir.join(CACHE_FILENAME);

        let revisions = load_revisions(repo_root);
        let entries = load_entries(&cache_path);

        Self {
            cache_path,
            revisions,
            entries,
        }
    }

    /// Get the flake.lock revision for a source (12-char truncated hash).
    pub fn get_revision(&self, source: &str) -> &str {
        let input = source_to_input(source);
        self.revisions
            .get(input)
            .map(String::as_str)
            .unwrap_or("unknown")
    }

    /// Look up a single cached result by name + source.
    pub fn get(&self, name: &str, source: &str) -> Option<SourceResult> {
        let key = self.cache_key(name, source);
        let entry = self.entries.get(&key)?;
        let obj = entry.as_object()?;

        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        Some(SourceResult {
            name: name.to_string(),
            source: source.to_string(),
            attr: obj.get("attr").and_then(Value::as_str).map(String::from),
            version: obj.get("version").and_then(Value::as_str).map(String::from),
            confidence: obj.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
            description,
            requires_flake_mod: obj
                .get("requires_flake_mod")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            flake_url: obj
                .get("flake_url")
                .and_then(Value::as_str)
                .map(String::from),
        })
    }

    /// Get all cached results for a package name, in source priority order.
    ///
    /// SPEC §5 guardrail: if only homebrew/cask results exist (no nxs/nur),
    /// returns empty to force a fresh search.
    pub fn get_all(&self, name: &str) -> Vec<SourceResult> {
        let mut results = Vec::new();
        let mut has_nix_source = false;

        for &source in SOURCE_PRIORITY {
            if let Some(result) = self.get(name, source) {
                results.push(result);
                if matches!(source, "nxs" | "nur") {
                    has_nix_source = true;
                }
            }
        }

        // Homebrew-only guardrail: force fresh search when no nix sources cached
        if !results.is_empty() && !has_nix_source {
            return Vec::new();
        }

        results
    }

    /// Cache a single search result (writes to disk immediately).
    ///
    /// Skips results with no `attr`.
    pub fn set(&mut self, result: &SourceResult) {
        if result.attr.is_none() {
            return;
        }
        let key = self.cache_key(&result.name, &result.source);
        self.entries.insert(key, entry_to_value(result));
        self.save();
    }

    /// Cache multiple results, keeping only the highest confidence per (name, source).
    ///
    /// Single disk write at the end.
    pub fn set_many(&mut self, results: &[SourceResult]) {
        let mut best: HashMap<(&str, &str), &SourceResult> = HashMap::new();
        for result in results {
            let key = (result.name.as_str(), result.source.as_str());
            if best
                .get(&key)
                .is_none_or(|prev| result.confidence > prev.confidence)
            {
                best.insert(key, result);
            }
        }

        for result in best.values() {
            if result.attr.is_none() {
                continue;
            }
            let key = self.cache_key(&result.name, &result.source);
            self.entries.insert(key, entry_to_value(result));
        }

        self.save();
    }

    /// Remove cached entries for a package, optionally filtered by source.
    pub fn invalidate(&mut self, name: &str, source: Option<&str>) {
        let normalized = normalize_name(name);
        let before = self.entries.len();

        self.entries.retain(|k, _| {
            let mut parts = k.splitn(3, '|');
            let (Some(cached_name), Some(cached_source)) = (parts.next(), parts.next()) else {
                return true;
            };
            !(cached_name == normalized && source.is_none_or(|s| cached_source == s))
        });

        if self.entries.len() < before {
            self.save();
        }
    }

    /// Clear entire cache.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.save();
    }

    // -- Internal --

    fn cache_key(&self, name: &str, source: &str) -> String {
        let normalized = normalize_name(name);
        let rev = self.get_revision(source);
        format!("{normalized}|{source}|{rev}")
    }

    fn save(&self) {
        let envelope = serde_json::json!({
            "schema_version": CACHE_SCHEMA_VERSION,
            "entries": self.entries,
        });
        let json = serde_json::to_string_pretty(&envelope).unwrap_or_default();
        let _ = fs::write(&self.cache_path, json);
    }
}

/// Build a JSON value from a SourceResult for cache storage.
fn entry_to_value(result: &SourceResult) -> Value {
    serde_json::json!({
        "attr": result.attr,
        "version": result.version,
        "description": result.description,
        "confidence": result.confidence,
        "requires_flake_mod": result.requires_flake_mod,
        "flake_url": result.flake_url,
    })
}

/// Parse flake.lock to extract source revisions (12-char truncated).
fn load_revisions(repo_root: &Path) -> HashMap<String, String> {
    let lock_path = repo_root.join("flake.lock");
    let Ok(content) = fs::read_to_string(&lock_path) else {
        return HashMap::new();
    };
    let Ok(lock) = serde_json::from_str::<Value>(&content) else {
        return HashMap::from([("nxs".to_string(), "unknown".to_string())]);
    };

    let mut revisions = HashMap::new();
    let Some(nodes) = lock.get("nodes").and_then(Value::as_object) else {
        return revisions;
    };

    // nixpkgs → "nxs"
    if let Some(rev) = nodes
        .get("nixpkgs")
        .and_then(|n| n.get("locked"))
        .and_then(|l| l.get("rev"))
        .and_then(Value::as_str)
    {
        revisions.insert("nxs".to_string(), truncate_rev(rev));
    }

    // All other inputs (except root)
    for (name, data) in nodes {
        if name == "root" {
            continue;
        }
        if let Some(rev) = data
            .get("locked")
            .and_then(|l| l.get("rev"))
            .and_then(Value::as_str)
        {
            revisions.insert(name.clone(), truncate_rev(rev));
        }
    }

    revisions
}

/// Load and validate cache entries from disk.
///
/// Returns empty map on missing file, parse error, or schema mismatch.
fn load_entries(cache_path: &Path) -> HashMap<String, Value> {
    let Ok(content) = fs::read_to_string(cache_path) else {
        return HashMap::new();
    };
    let Ok(raw) = serde_json::from_str::<Value>(&content) else {
        return HashMap::new();
    };
    let Some(obj) = raw.as_object() else {
        return HashMap::new();
    };

    if obj.get("schema_version").and_then(Value::as_u64) != Some(CACHE_SCHEMA_VERSION) {
        return HashMap::new();
    }

    let Some(entries) = obj.get("entries").and_then(Value::as_object) else {
        return HashMap::new();
    };

    entries
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Truncate a revision hash to 12 characters (git short hash convention).
fn truncate_rev(rev: &str) -> String {
    rev[..rev.len().min(12)].to_string()
}

fn dirs_cache() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".cache")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_flake_lock(repo: &Path) {
        let lock = serde_json::json!({
            "nodes": {
                "root": {"inputs": {"nixpkgs": "nixpkgs", "nur": "nur"}},
                "nixpkgs": {"locked": {"rev": "abcdef1234567890"}},
                "nur": {"locked": {"rev": "0123456789abcdef"}},
            }
        });
        fs::write(
            repo.join("flake.lock"),
            serde_json::to_string(&lock).unwrap(),
        )
        .unwrap();
    }

    fn make_cache(repo: &Path, home: &Path) -> MultiSourceCache {
        let cache_dir = home.join(".cache").join("nx");
        MultiSourceCache::load_with_cache_dir(repo, &cache_dir)
    }

    fn result(name: &str, source: &str, attr: &str, confidence: f64) -> SourceResult {
        SourceResult {
            name: name.to_string(),
            source: source.to_string(),
            attr: Some(attr.to_string()),
            version: None,
            confidence,
            description: String::new(),
            requires_flake_mod: false,
            flake_url: None,
        }
    }

    #[test]
    fn revision_keying() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let cache = make_cache(&repo, &home);
        assert_eq!(cache.get_revision("nxs"), "abcdef123456");
        assert_eq!(cache.get_revision("nur"), "0123456789ab");
        assert_eq!(cache.get_revision("missing"), "unknown");
    }

    #[test]
    fn homebrew_only_is_ignored() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        cache.set(&result("ripgrep", "homebrew", "ripgrep", 0.8));

        // Homebrew-only should return empty (guardrail)
        assert!(cache.get_all("ripgrep").is_empty());
    }

    #[test]
    fn nxs_present_returns_results() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        cache.set(&result("ripgrep", "nxs", "ripgrep", 0.9));

        let results = cache.get_all("ripgrep");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "nxs");
    }

    #[test]
    fn schema_mismatch_invalidates() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        // Write a cache with wrong schema version
        let cache_dir = home.join(".cache").join("nx");
        fs::create_dir_all(&cache_dir).unwrap();
        let bad_cache = serde_json::json!({
            "schema_version": -1,
            "entries": {
                "ripgrep|nxs|abcdef123456": {
                    "attr": "ripgrep",
                    "version": null,
                    "description": "fast search",
                    "confidence": 0.9,
                    "requires_flake_mod": false,
                    "flake_url": null,
                }
            }
        });
        fs::write(
            cache_dir.join(CACHE_FILENAME),
            serde_json::to_string(&bad_cache).unwrap(),
        )
        .unwrap();

        let cache = make_cache(&repo, &home);
        assert!(cache.get_all("ripgrep").is_empty());
    }

    #[test]
    fn normalizes_alias_keys() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        cache.set(&SourceResult {
            name: "py-yaml".to_string(),
            source: "nxs".to_string(),
            attr: Some("python3Packages.pyyaml".to_string()),
            version: None,
            confidence: 0.9,
            description: "YAML parser".to_string(),
            requires_flake_mod: false,
            flake_url: None,
        });

        // Look up with the canonical name — should find the aliased entry
        let results = cache.get_all("pyyaml");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].attr.as_deref(), Some("python3Packages.pyyaml"));
    }

    #[test]
    fn set_many_keeps_highest_confidence() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        let results = vec![
            result("ripgrep", "nxs", "ripgrep", 0.5),
            result("ripgrep", "nxs", "ripgrep-all", 0.9),
        ];
        cache.set_many(&results);

        let r = cache.get("ripgrep", "nxs").unwrap();
        assert_eq!(r.attr.as_deref(), Some("ripgrep-all"));
        assert!((r.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn invalidate_by_name() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        cache.set(&result("ripgrep", "nxs", "ripgrep", 0.9));
        cache.set(&result("ripgrep", "homebrew", "ripgrep", 0.8));

        cache.invalidate("ripgrep", None);
        assert!(cache.get("ripgrep", "nxs").is_none());
        assert!(cache.get("ripgrep", "homebrew").is_none());
    }

    #[test]
    fn invalidate_by_source() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        cache.set(&result("ripgrep", "nxs", "ripgrep", 0.9));
        cache.set(&result("ripgrep", "homebrew", "ripgrep", 0.8));

        cache.invalidate("ripgrep", Some("homebrew"));
        assert!(cache.get("ripgrep", "nxs").is_some());
        assert!(cache.get("ripgrep", "homebrew").is_none());
    }

    #[test]
    fn clear_empties_cache() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        cache.set(&result("ripgrep", "nxs", "ripgrep", 0.9));
        cache.clear();
        assert!(cache.get_all("ripgrep").is_empty());
    }

    #[test]
    fn set_skips_result_without_attr() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cache = make_cache(&repo, &home);
        cache.set(&SourceResult::new("ripgrep", "nxs")); // attr is None
        assert!(cache.get("ripgrep", "nxs").is_none());
    }

    #[test]
    fn missing_flake_lock_uses_unknown() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        // No flake.lock written

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let cache = make_cache(&repo, &home);
        assert_eq!(cache.get_revision("nxs"), "unknown");
    }

    #[test]
    fn cache_persists_to_disk() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        write_flake_lock(&repo);

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        // Write via one instance
        let mut cache1 = make_cache(&repo, &home);
        cache1.set(&result("ripgrep", "nxs", "ripgrep", 0.9));

        // Read via a fresh instance
        let cache2 = make_cache(&repo, &home);
        let r = cache2.get("ripgrep", "nxs").unwrap();
        assert_eq!(r.attr.as_deref(), Some("ripgrep"));
    }

    #[test]
    fn cask_shares_homebrew_revision() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();

        // flake.lock with homebrew input
        let lock = serde_json::json!({
            "nodes": {
                "root": {"inputs": {}},
                "nixpkgs": {"locked": {"rev": "aaaa1234567890"}},
                "homebrew": {"locked": {"rev": "bbbb1234567890"}},
            }
        });
        fs::write(
            repo.join("flake.lock"),
            serde_json::to_string(&lock).unwrap(),
        )
        .unwrap();

        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let cache = make_cache(&repo, &home);
        assert_eq!(cache.get_revision("homebrew"), "bbbb12345678");
        assert_eq!(cache.get_revision("cask"), "bbbb12345678"); // shares homebrew
    }
}
