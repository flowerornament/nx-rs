use super::*;

#[test]
fn collect_info_flakehub_skips_lookup_when_disabled() {
    let searches = Cell::new(0usize);
    let results = collect_info_flakehub("ripgrep", false, |_| {
        searches.set(searches.get() + 1);
        vec![FlakeHubInfo {
            name: "Org/ripgrep".to_string(),
            description: "desc".to_string(),
            version: Some("1.0.0".to_string()),
        }]
    });
    assert!(results.is_empty());
    assert_eq!(searches.get(), 0);
}

#[test]
fn collect_info_flakehub_limits_results_to_three() {
    let results = collect_info_flakehub("ripgrep", true, |_| {
        vec![
            FlakeHubInfo {
                name: "Org/a".to_string(),
                description: String::new(),
                version: None,
            },
            FlakeHubInfo {
                name: "Org/b".to_string(),
                description: String::new(),
                version: None,
            },
            FlakeHubInfo {
                name: "Org/c".to_string(),
                description: String::new(),
                version: None,
            },
            FlakeHubInfo {
                name: "Org/d".to_string(),
                description: String::new(),
                version: None,
            },
        ]
    });
    assert_eq!(results.len(), 3);
    assert_eq!(results[2].name, "Org/c");
}
