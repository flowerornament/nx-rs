use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::domain::source::SourcePreferences;
use crate::infra::cache::MultiSourceCache;
use crate::infra::sources::{
    CachedSearchOutcome, SourceSearchOutcome, cached_search_many_with_status,
    cached_search_with_status, search_all_sources, search_all_sources_quiet,
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
    query_package_with(name, prefs, repo_root, cache, search_all_sources)
}

pub fn query_package_quiet(
    name: &str,
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
) -> PackageQueryReport {
    query_package_with(name, prefs, repo_root, cache, search_all_sources_quiet)
}

fn query_package_with<F>(
    name: &str,
    prefs: &SourcePreferences,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    search: F,
) -> PackageQueryReport
where
    F: Fn(&str, &SourcePreferences, Option<&Path>) -> SourceSearchOutcome + Sync,
{
    let started = Instant::now();
    let cached = cached_search_with_status(name, prefs, repo_root, cache, search);
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
