use serde::Serialize;

/// Serialize to compact JSON matching Python's `json.dumps` default separators
/// (`, ` between items, `: ` between key and value).
pub fn to_string_compact<T: Serialize>(value: &T) -> serde_json::Result<String> {
    let mut buf = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, PythonCompactFormatter);
    value.serialize(&mut serializer)?;
    // Safety: serde_json always produces valid UTF-8
    Ok(String::from_utf8(buf).expect("serde_json produced invalid UTF-8"))
}

/// Matches Python's default `json.dumps` separators: `", "` and `": "`.
struct PythonCompactFormatter;

impl serde_json::ser::Formatter for PythonCompactFormatter {
    fn begin_object_key<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if !first {
            writer.write_all(b", ")?;
        }
        Ok(())
    }

    fn begin_object_value<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        writer.write_all(b": ")
    }

    fn begin_array_value<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if !first {
            writer.write_all(b", ")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::to_string_compact;
    use serde_json::json;

    #[test]
    fn compact_matches_python_separators() {
        let value = json!({"name": "ripgrep", "location": "packages/nix/cli.nix:5"});
        let result = to_string_compact(&value).unwrap();
        assert_eq!(
            result,
            r#"{"name": "ripgrep", "location": "packages/nix/cli.nix:5"}"#
        );
    }

    #[test]
    fn compact_nested_objects() {
        let value = json!({"ripgrep": {"match": "ripgrep", "location": "path:5"}});
        let result = to_string_compact(&value).unwrap();
        assert_eq!(
            result,
            r#"{"ripgrep": {"match": "ripgrep", "location": "path:5"}}"#
        );
    }
}
