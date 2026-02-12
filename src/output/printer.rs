use crate::output::style::{IconSet, OutputStyle};

struct GlyphSet {
    action: &'static str,
    success: &'static str,
    error: &'static str,
    dry_run: &'static str,
}

pub struct Printer {
    style: OutputStyle,
}

impl Printer {
    pub fn new(style: OutputStyle) -> Self {
        Self { style }
    }

    pub fn action(&self, text: &str) {
        println!("\n{} {text}", self.glyphs().action);
    }

    pub fn success(&self, text: &str) {
        println!("{} {text}", self.glyphs().success);
    }

    pub fn error(&self, text: &str) {
        eprintln!("{} {text}", self.glyphs().error);
    }

    pub fn dry_run_banner(&self) {
        println!(
            "\n{} Dry Run (no changes will be made)",
            self.glyphs().dry_run
        );
    }

    pub fn detail(&self, text: &str) {
        println!("  {text}");
    }

    pub fn stream_line(&self, text: &str, indent: &str, width: usize) {
        for segment in wrapped_segments(text, width.saturating_sub(indent.len()).max(20)) {
            println!("{indent}{segment}");
        }
    }

    fn glyphs(&self) -> GlyphSet {
        match self.style.icon_set {
            IconSet::Unicode => GlyphSet {
                action: "➜",
                success: "✔",
                error: "✘",
                dry_run: "~",
            },
            IconSet::Minimal => GlyphSet {
                action: ">",
                success: "+",
                error: "x",
                dry_run: "~",
            },
        }
    }
}

fn wrapped_segments(line: &str, max_content: usize) -> Vec<&str> {
    if line.chars().count() <= max_content {
        return vec![line];
    }

    let mut out = Vec::new();
    let mut remaining = line;
    while remaining.chars().count() > max_content {
        let candidate = nth_char_boundary(remaining, max_content);
        let split = remaining[..candidate]
            .rfind(' ')
            .unwrap_or(candidate)
            .max(1);
        out.push(&remaining[..split]);
        remaining = remaining[split..].trim_start();
        if remaining.is_empty() {
            return out;
        }
    }

    out.push(remaining);
    out
}

fn nth_char_boundary(input: &str, n: usize) -> usize {
    if input.chars().count() <= n {
        return input.len();
    }
    input
        .char_indices()
        .nth(n)
        .map(|(idx, _)| idx)
        .unwrap_or(input.len())
}

#[cfg(test)]
mod tests {
    use super::{Printer, wrapped_segments};
    use crate::output::style::{IconSet, OutputStyle};

    #[test]
    fn wrapped_segments_preserves_long_word_chunks() {
        let segments = wrapped_segments("alpha beta gamma delta", 8);
        assert_eq!(segments, vec!["alpha", "beta", "gamma", "delta"]);
    }

    #[test]
    fn printer_uses_unicode_glyphs_when_requested() {
        let printer = Printer::new(OutputStyle {
            plain: false,
            icon_set: IconSet::Unicode,
        });

        let glyphs = printer.glyphs();
        assert_eq!(glyphs.action, "➜");
        assert_eq!(glyphs.success, "✔");
        assert_eq!(glyphs.error, "✘");
    }

    #[test]
    fn printer_uses_minimal_glyphs_for_plain_mode() {
        let printer = Printer::new(OutputStyle {
            plain: true,
            icon_set: IconSet::Minimal,
        });

        let glyphs = printer.glyphs();
        assert_eq!(glyphs.action, ">");
        assert_eq!(glyphs.success, "+");
        assert_eq!(glyphs.error, "x");
    }
}
