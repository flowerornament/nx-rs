use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const FETCHER_CACHE_RELATIVE: &str = ".cache/nix/fetcher-cache-v4.sqlite";

pub fn fetcher_cache_path(home_dir: &Path) -> PathBuf {
    home_dir.join(FETCHER_CACHE_RELATIVE)
}

pub fn changed_paths(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut changed = BTreeSet::new();

    for (path, before_content) in before {
        match after.get(path) {
            Some(after_content) => {
                if after_content != before_content {
                    changed.insert(path.clone());
                }
            }
            None => {
                changed.insert(path.clone());
            }
        }
    }

    for path in after.keys() {
        if !before.contains_key(path) {
            changed.insert(path.clone());
        }
    }

    changed.into_iter().collect()
}
