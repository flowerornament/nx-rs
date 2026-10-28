use crate::infra::shell::CapturedCommand;
use crate::output::printer::Printer;
use anyhow::{Result, bail};

#[derive(Default)]
pub(super) struct Reply {
    pending: Vec<u8>,
    text: String,
    seen: bool,
    failed: bool,
}

impl Reply {
    pub fn observe(&mut self, bytes: &[u8], printer: &Printer) {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=end).collect::<Vec<_>>();
            let line = String::from_utf8_lossy(&line);
            let Ok(output) = claude_codes::ClaudeOutput::parse_json_tolerant(&line) else {
                continue;
            };
            match output {
                claude_codes::ClaudeOutput::Assistant(message) => {
                    for block in message.message.content {
                        match block {
                            claude_codes::ContentBlock::ToolUse(tool) => {
                                super::report_tool_activity(printer, &tool.name, &tool.input);
                            }
                            claude_codes::ContentBlock::Text(text) => {
                                self.text.push_str(&text.text);
                            }
                            _ => {}
                        }
                    }
                }
                claude_codes::ClaudeOutput::Result(result) => {
                    self.seen = true;
                    self.failed |= result.is_error
                        || !matches!(result.subtype, claude_codes::io::ResultSubtype::Success);
                    if let Some(text) = result.result {
                        self.text = text;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn finish(mut self, output: &CapturedCommand, printer: &Printer) -> Result<String> {
        if !self.pending.is_empty() {
            self.observe(b"\n", printer);
        }
        if output.code != 0 || self.failed || !self.seen {
            let detail = if output.stderr.trim().is_empty() {
                self.text.trim()
            } else {
                output.stderr.trim()
            };
            bail!(
                "Claude did not complete the edit (exit {}): {}",
                output.code,
                crate::infra::text::truncate_with_ellipsis(detail, 300)
            );
        }
        Ok(self.text)
    }
}
