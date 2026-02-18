use std::fmt;
use std::path::{Path, PathBuf};

/// Package location with an optional line number.
///
/// Text parsing is isolated here so the rest of the command stack can use
/// typed path/line accessors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageLocation {
    path: PathBuf,
    line: Option<usize>,
}

impl PackageLocation {
    pub const fn new(path: PathBuf, line: Option<usize>) -> Self {
        Self { path, line }
    }

    pub fn parse(value: &str) -> Self {
        match value.rsplit_once(':') {
            Some((path, line)) if line.chars().all(|ch| ch.is_ascii_digit()) => {
                Self::new(PathBuf::from(path), line.parse::<usize>().ok())
            }
            _ => Self::new(PathBuf::from(value), None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn line(&self) -> Option<usize> {
        self.line
    }
}

impl fmt::Display for PackageLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            write!(f, "{}:{line}", self.path.display())
        } else {
            write!(f, "{}", self.path.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PackageLocation;
    use std::path::Path;

    #[test]
    fn parse_supports_colons_in_paths() {
        let location = PackageLocation::parse("a:12:34");

        assert_eq!(location.path(), Path::new("a:12"));
        assert_eq!(location.line(), Some(34));
        assert_eq!(location.to_string(), "a:12:34");
    }

    #[test]
    fn parse_missing_line_keeps_whole_path() {
        let location = PackageLocation::parse("packages/nix/cli.nix");

        assert_eq!(location.path(), Path::new("packages/nix/cli.nix"));
        assert_eq!(location.line(), None);
        assert_eq!(location.to_string(), "packages/nix/cli.nix");
    }

    #[test]
    fn parse_non_numeric_suffix_is_not_line() {
        let location = PackageLocation::parse("a:12:line");

        assert_eq!(location.path(), Path::new("a:12:line"));
        assert_eq!(location.line(), None);
        assert_eq!(location.to_string(), "a:12:line");
    }
}
