use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::domain::source::SourcePreferences;
use crate::infra::cache::MultiSourceCache;
use crate::infra::sources::{
    CachedSearchOutcome, SourceSearchOutcome, cached_search_many_with_status,
    cached_search_many_with_status_quiet,
};

#[derive(Debug, Clone)]
pub struct PackageQueryReport {
    pub outcome: SourceSearchOutcome,
    pub cache_hit: bool,
    pub elapsed: Duration,
}

pub fn query_package(
    name: &str,
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
) -> PackageQueryReport {
    query_package_cached(name, prefs, repo_root, cache, false)
}

pub fn query_package_quiet(
    name: &str,
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
) -> PackageQueryReport {
    query_package_cached(name, prefs, repo_root, cache, true)
}

fn query_package_cached(
    name: &str,
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    quiet: bool,
) -> PackageQueryReport {
    let started = Instant::now();
    let names = [name.to_string()];
    let mut outcomes = if quiet {
        cached_search_many_with_status_quiet(&names, prefs, repo_root, cache)
    } else {
        cached_search_many_with_status(&names, prefs, repo_root, cache)
    };
    let cached = outcomes
        .remove(name)
        .unwrap_or_else(|| CachedSearchOutcome {
            outcome: SourceSearchOutcome::default(),
            cache_hit: false,
        });
    PackageQueryReport {
        outcome: cached.outcome,
        cache_hit: cached.cache_hit,
        elapsed: started.elapsed(),
    }
}

pub fn query_packages(
    names: &[String],
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
) -> HashMap<String, PackageQueryReport> {
    let started = Instant::now();
    let outcomes = cached_search_many_with_status(names, prefs, repo_root, cache);

    if outcomes.is_empty() {
        return HashMap::new();
    }

    let elapsed = started.elapsed();
    let query_count = u32::try_from(outcomes.len()).unwrap_or(u32::MAX);
    let per_query = elapsed.div_f64(f64::from(query_count));

    outcomes
        .into_iter()
        .map(|(name, CachedSearchOutcome { outcome, cache_hit })| {
            (
                name,
                PackageQueryReport {
                    outcome,
                    cache_hit,
                    elapsed: per_query,
                },
            )
        })
        .collect()
}
