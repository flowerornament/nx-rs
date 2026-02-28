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
    color_enabled(
        plain,
        ColorPolicyInput {
            no_color_set: env::var_os("NO_COLOR").is_some(),
            term_is_dumb: matches!(env::var("TERM").as_deref(), Ok("dumb")),
            stdout_is_terminal: std::io::stdout().is_terminal(),
        },
    )
}

#[derive(Debug, Clone, Copy)]
struct ColorPolicyInput {
    no_color_set: bool,
    term_is_dumb: bool,
    stdout_is_terminal: bool,
}

const fn color_enabled(plain: bool, input: ColorPolicyInput) -> bool {
    !plain && !input.no_color_set && !input.term_is_dumb && input.stdout_is_terminal
}

#[cfg(test)]
mod tests {
    use super::{ColorPolicyInput, IconSet, OutputStyle, color_enabled};

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

    #[test]
    fn color_policy_disables_when_no_color_is_set() {
        assert!(!color_enabled(
            false,
            ColorPolicyInput {
                no_color_set: true,
                term_is_dumb: false,
                stdout_is_terminal: true
            }
        ));
    }

    #[test]
    fn color_policy_disables_when_term_is_dumb() {
        assert!(!color_enabled(
            false,
            ColorPolicyInput {
                no_color_set: false,
                term_is_dumb: true,
                stdout_is_terminal: true
            }
        ));
    }

    #[test]
    fn color_policy_disables_when_plain_mode_is_enabled() {
        assert!(!color_enabled(
            true,
            ColorPolicyInput {
                no_color_set: false,
                term_is_dumb: false,
                stdout_is_terminal: true
            }
        ));
    }

    #[test]
    fn color_policy_requires_terminal_stdout() {
        assert!(color_enabled(
            false,
            ColorPolicyInput {
                no_color_set: false,
                term_is_dumb: false,
                stdout_is_terminal: true
            }
        ));
        assert!(!color_enabled(
            false,
            ColorPolicyInput {
                no_color_set: false,
                term_is_dumb: false,
                stdout_is_terminal: false
            }
        ));
    }
}
