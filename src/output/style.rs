#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum IconSet {
    Unicode,
    Minimal,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct OutputStyle {
    pub plain: bool,
    pub icon_set: IconSet,
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

        Self { plain, icon_set }
    }
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
