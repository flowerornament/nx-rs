use super::*;

#[test]
fn collect_info_sources_uses_cache_before_search() {
    let (tmp, mut cache) = cache_fixture();
    let root = tmp.path();
    cache
        .as_mut()
        .expect("cache should exist")
        .set_many(&[source_result(
            "ripgrep",
            PackageSource::Nxs,
            Some("ripgrep"),
            0.95,
        )])
        .expect("cache set should succeed");

    let args = info_args();
    let searches = Cell::new(0usize);

    let results = collect_info_sources_with(
        package_from_args(&args),
        &args,
        root,
        &mut cache,
        |_, _, _| {
            searches.set(searches.get() + 1);
            SourceSearchOutcome::default()
        },
    );

    assert_eq!(searches.get(), 0);
    assert_eq!(results.results.len(), 1);
    assert_eq!(results.results[0].source, PackageSource::Nxs);
}

#[test]
fn collect_info_sources_falls_back_to_search_and_updates_cache() {
    let (tmp, mut cache) = cache_fixture();
    let root = tmp.path();

    let args = info_args();
    let search_calls = Cell::new(0usize);

    let searched_result = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"), 0.9);
    let results = collect_info_sources_with(
        package_from_args(&args),
        &args,
        root,
        &mut cache,
        |_, _, _| {
            search_calls.set(search_calls.get() + 1);
            SourceSearchOutcome {
                results: vec![searched_result.clone()],
                unavailable_sources: Vec::new(),
            }
        },
    );

    assert_eq!(search_calls.get(), 1);
    assert_eq!(results.results.len(), 1);
    assert_eq!(results.results[0].attr.as_deref(), Some("ripgrep"));

    let cached = cache
        .as_ref()
        .expect("cache should exist")
        .get_all("ripgrep");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].attr.as_deref(), Some("ripgrep"));
}

#[test]
fn collect_info_sources_searches_on_cache_miss() {
    let (tmp, mut cache) = cache_fixture();
    let root = tmp.path();
    let args = info_args();
    let searches = Cell::new(0usize);

    let results = collect_info_sources_with(
        package_from_args(&args),
        &args,
        root,
        &mut cache,
        |_, _, _| {
            searches.set(searches.get() + 1);
            SourceSearchOutcome {
                results: vec![source_result(
                    "ripgrep",
                    PackageSource::Nxs,
                    Some("ripgrep"),
                    1.0,
                )],
                unavailable_sources: Vec::new(),
            }
        },
    );

    assert_eq!(results.results.len(), 1);
    assert_eq!(searches.get(), 1);
}
