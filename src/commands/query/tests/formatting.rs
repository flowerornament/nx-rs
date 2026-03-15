use super::*;

#[test]
fn info_source_json_serializes_required_metadata() {
    let source = source_result("mas-app", PackageSource::Mas, Some("mas-app"), 0.87);
    let entry = info_source_json_from_result(source);
    let value = serde_json::to_value(entry).expect("source json should serialize");

    assert_eq!(value.get("source").and_then(Value::as_str), Some("mas"));
    assert_eq!(value.get("version").and_then(Value::as_str), Some("1.2.3"));
    assert_eq!(
        value.get("description").and_then(Value::as_str),
        Some("desc")
    );
    assert!(value.get("homepage").is_some_and(Value::is_null));
    assert!(value.get("license").is_some_and(Value::is_null));
    assert!(value.get("dependencies").is_some_and(Value::is_null));
    assert!(value.get("build_dependencies").is_some_and(Value::is_null));
    assert!(value.get("caveats").is_some_and(Value::is_null));
    assert!(value.get("artifacts").is_some_and(Value::is_null));
    assert_eq!(value.get("broken").and_then(Value::as_bool), Some(false));
    assert_eq!(value.get("insecure").and_then(Value::as_bool), Some(false));
    assert_eq!(
        value.get("head_available").and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn info_source_label_uses_nix_attr_display() {
    let source = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"), 1.0);
    assert_eq!(info_source_label(&source), "nxs (pkgs.ripgrep)");
}

#[test]
fn info_status_text_matches_python_shape() {
    assert_eq!(info_status_text(false, None, None), "not installed");
    assert_eq!(info_status_text(true, Some("nxs"), None), "installed (nxs)");
    assert_eq!(
        info_status_text(true, Some("flake-input"), Some("fenix")),
        "installed via fenix"
    );
}
