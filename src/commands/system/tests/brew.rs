use super::*;

#[test]
fn brew_parse_extracts_formulae() {
    let json = r#"{
            "formulae": [
                {
                    "name": "git",
                    "installed_versions": ["2.43.0"],
                    "current_version": "2.44.0"
                },
                {
                    "name": "jq",
                    "installed_versions": ["1.6"],
                    "current_version": "1.7.1"
                }
            ],
            "casks": []
        }"#;

    let result = parse_brew_outdated_json(json);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].name, "git");
    assert_eq!(result[0].installed_version, "2.43.0");
    assert_eq!(result[0].current_version, "2.44.0");
    assert!(!result[0].is_cask);
    assert_eq!(result[1].name, "jq");
    assert_eq!(result[1].installed_version, "1.6");
    assert_eq!(result[1].current_version, "1.7.1");
    assert!(!result[1].is_cask);
}

#[test]
fn brew_parse_extracts_casks() {
    let json = r#"{
            "formulae": [],
            "casks": [
                {
                    "name": "firefox",
                    "installed_versions": "120.0",
                    "current_version": "121.0"
                }
            ]
        }"#;

    let result = parse_brew_outdated_json(json);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "firefox");
    assert_eq!(result[0].installed_version, "120.0");
    assert_eq!(result[0].current_version, "121.0");
    assert!(result[0].is_cask);
}

#[test]
fn brew_parse_mixed_formulae_and_casks_sorted() {
    let json = r#"{
            "formulae": [
                {
                    "name": "zsh",
                    "installed_versions": ["5.9"],
                    "current_version": "5.9.1"
                }
            ],
            "casks": [
                {
                    "name": "alacritty",
                    "installed_versions": "0.12",
                    "current_version": "0.13"
                }
            ]
        }"#;

    let result = parse_brew_outdated_json(json);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].name, "alacritty");
    assert!(result[0].is_cask);
    assert_eq!(result[1].name, "zsh");
    assert!(!result[1].is_cask);
}

#[test]
fn brew_parse_skips_incomplete_entries() {
    let json = r#"{
            "formulae": [
                {
                    "name": "",
                    "installed_versions": ["1.0"],
                    "current_version": "2.0"
                },
                {
                    "name": "valid",
                    "installed_versions": ["1.0"],
                    "current_version": "2.0"
                }
            ],
            "casks": []
        }"#;

    let result = parse_brew_outdated_json(json);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "valid");
}

#[test]
fn brew_parse_invalid_json_returns_empty() {
    let result = parse_brew_outdated_json("not json at all");
    assert!(result.is_empty());
}

#[test]
fn brew_parse_empty_json_returns_empty() {
    let result = parse_brew_outdated_json("{}");
    assert!(result.is_empty());
}

#[test]
fn brew_parse_empty_arrays_returns_empty() {
    let json = r#"{"formulae": [], "casks": []}"#;
    let result = parse_brew_outdated_json(json);
    assert!(result.is_empty());
}

#[test]
fn brew_info_parse_extracts_formula_metadata() {
    let json = r#"{
            "formulae": [
                {
                    "name": "git",
                    "homepage": "https://github.com/git/git",
                    "desc": "Distributed revision control system"
                }
            ]
        }"#;

    let result = parse_brew_info_json(json, false);
    let metadata = result.get("git").expect("git metadata should exist");
    assert_eq!(
        metadata.homepage.as_deref(),
        Some("https://github.com/git/git")
    );
    assert_eq!(
        metadata.description.as_deref(),
        Some("Distributed revision control system")
    );
}

#[test]
fn brew_info_parse_extracts_cask_metadata() {
    let json = r#"{
            "casks": [
                {
                    "token": "firefox",
                    "homepage": "https://www.mozilla.org/firefox/",
                    "desc": "Web browser"
                }
            ]
        }"#;

    let result = parse_brew_info_json(json, true);
    let metadata = result
        .get("firefox")
        .expect("firefox metadata should exist");
    assert_eq!(
        metadata.homepage.as_deref(),
        Some("https://www.mozilla.org/firefox/")
    );
    assert_eq!(metadata.description.as_deref(), Some("Web browser"));
}

#[test]
fn brew_info_parse_invalid_json_returns_empty() {
    let result = parse_brew_info_json("oops", false);
    assert!(result.is_empty());
}

#[test]
fn brew_compare_url_for_github_homepage() {
    let url = brew_compare_url(
        Some("https://github.com/BurntSushi/ripgrep"),
        "v14.1.0",
        "14.1.1",
    );
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/BurntSushi/ripgrep/compare/14.1.0...14.1.1")
    );
}

#[test]
fn brew_compare_url_non_github_returns_none() {
    let url = brew_compare_url(Some("https://example.com/project"), "1.0.0", "1.1.0");
    assert!(url.is_none());
}
