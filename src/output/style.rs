use std::env;
use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum IconSet {
    Unicode,
    Minimal,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct OutputStyle {
    pub plain: bool,
    pub icon_set: IconSet,
    pub color: bool,
}

impl OutputStyle {
    pub fn from_flags(plain: bool, unicode: bool, minimal: bool) -> Self {
        let icon_set = if minimal || plain {
            IconSet::Minimal
        } else if unicode {
            IconSet::Unicode
        } else {
            IconSet::Minimal
        };

        Self {
            plain,
            icon_set,
            color: use_color(plain),
        }
    }
}

fn use_color(plain: bool) -> bool {
    if plain || env::var_os("NO_COLOR").is_some() {
        return false;
    }

    if matches!(env::var("TERM").as_deref(), Ok("dumb")) {
        return false;
    }

    std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::{IconSet, OutputStyle};

    #[test]
    fn minimal_wins_over_unicode() {
        let style = OutputStyle::from_flags(false, true, true);
        assert_eq!(style.icon_set, IconSet::Minimal);
    }

    #[test]
    fn plain_uses_minimal_set() {
        let style = OutputStyle::from_flags(true, true, false);
        assert_eq!(style.icon_set, IconSet::Minimal);
        assert!(!style.color);
    }

    #[test]
    fn unicode_selected_when_requested() {
        let style = OutputStyle::from_flags(false, true, false);
        assert_eq!(style.icon_set, IconSet::Unicode);
    }

    #[test]
    fn default_is_minimal() {
        let style = OutputStyle::from_flags(false, false, false);
        assert_eq!(style.icon_set, IconSet::Minimal);
    }
}
