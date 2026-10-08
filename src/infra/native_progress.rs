//! Bound native Nix redraws without rendering or discarding permanent output.

use regex::bytes::Regex;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const REDRAW_INTERVAL: Duration = Duration::from_millis(500);
const FRAGMENT_TIMEOUT: Duration = Duration::from_millis(100);
const FRAME_LIMIT: usize = 16 * 1024;
const CLEAR_LINE: &[u8] = b"\r\x1b[K";
// Only printable content and SGR styling may be replaced. Cursor movement,
// screen clearing and unknown terminal commands must never be discarded.
static REDRAW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?-u)^\r(?:[^\x00-\x1f\x7f]|\x1b\[[0-9;:]*m)*\x1b\[K\z")
        .expect("native redraw pattern should compile")
});

#[derive(Default)]
pub(super) struct NativeProgress {
    candidate: Vec<u8>,
    candidate_started: Option<Instant>,
    pending: Option<Vec<u8>>,
    last_draw: Option<Instant>,
}

impl NativeProgress {
    pub(super) fn push(&mut self, bytes: &[u8], now: Instant) -> Vec<u8> {
        let mut output = Vec::new();
        for &byte in bytes {
            if byte == b'\r' {
                if !self.candidate.is_empty() {
                    self.flush_candidate(&mut output, now);
                }
                self.candidate_started = Some(now);
            } else if self.candidate.is_empty() {
                self.flush_pending(&mut output, now);
                output.push(byte);
                continue;
            }

            self.candidate.push(byte);
            if byte == b'\n' || self.candidate.len() >= FRAME_LIMIT || self.candidate == CLEAR_LINE
            {
                self.flush_candidate(&mut output, now);
            } else if self.candidate.ends_with(b"\x1b[K") {
                if REDRAW.is_match(&self.candidate) {
                    self.pending = Some(std::mem::take(&mut self.candidate));
                    self.candidate_started = None;
                } else {
                    self.flush_candidate(&mut output, now);
                }
            }
        }
        output.extend(self.tick(now));
        output
    }

    pub(super) fn tick(&mut self, now: Instant) -> Vec<u8> {
        let mut output = Vec::new();
        if self
            .candidate_started
            .is_some_and(|started| now.saturating_duration_since(started) >= FRAGMENT_TIMEOUT)
        {
            self.flush_candidate(&mut output, now);
        }
        if self
            .last_draw
            .is_none_or(|last| now.saturating_duration_since(last) >= REDRAW_INTERVAL)
        {
            self.flush_pending(&mut output, now);
        }
        output
    }

    pub(super) fn finish(&mut self, now: Instant) -> Vec<u8> {
        let mut output = Vec::new();
        self.flush_candidate(&mut output, now);
        output
    }

    fn flush_candidate(&mut self, output: &mut Vec<u8>, now: Instant) {
        self.flush_pending(output, now);
        output.append(&mut self.candidate);
        self.candidate_started = None;
    }

    fn flush_pending(&mut self, output: &mut Vec<u8>, now: Instant) {
        if let Some(frame) = self.pending.take() {
            output.extend(frame);
            self.last_draw = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(label: &str) -> Vec<u8> {
        format!("\r\x1b[32m{label}\x1b[0m\x1b[K").into_bytes()
    }

    #[test]
    fn rapid_redraws_keep_latest_state_and_final_state() {
        let start = Instant::now();
        let mut relay = NativeProgress::default();
        assert_eq!(relay.push(&frame("first"), start), frame("first"));
        for millis in (50..500).step_by(50) {
            assert!(
                relay
                    .push(
                        &frame(&millis.to_string()),
                        start + Duration::from_millis(millis)
                    )
                    .is_empty()
            );
        }
        assert_eq!(relay.tick(start + REDRAW_INTERVAL), frame("450"));
        assert!(
            relay
                .push(&frame("final"), start + REDRAW_INTERVAL)
                .is_empty()
        );
        assert_eq!(relay.finish(start + REDRAW_INTERVAL), frame("final"));
    }

    #[test]
    fn split_utf8_and_escape_sequences_preserve_complete_frame() {
        let start = Instant::now();
        let mut relay = NativeProgress::default();
        let input = frame("✅ fetching");
        let output: Vec<_> = input
            .iter()
            .flat_map(|byte| relay.push(&[*byte], start))
            .collect();
        assert_eq!(output, input);
    }

    #[test]
    fn errors_and_clear_controls_are_immediate_and_ordered() {
        let start = Instant::now();
        let mut relay = NativeProgress::default();
        relay.push(&frame("first"), start);
        assert!(relay.push(&frame("latest"), start).is_empty());
        let message = b"\r\x1b[Kerror: build failed\r\n";
        let mut expected = frame("latest");
        expected.extend(message);
        assert_eq!(relay.push(message, start), expected);
    }

    #[test]
    fn unknown_terminal_controls_are_never_coalesced() {
        let start = Instant::now();
        let mut relay = NativeProgress::default();
        let unknown = b"\r\x1b[2Jscreen clear\x1b[K";
        assert_eq!(relay.push(unknown, start), unknown);
        assert_eq!(relay.push(unknown, start), unknown);
    }

    #[test]
    fn unmatched_prompt_is_visible_without_waiting_for_child_exit() {
        let start = Instant::now();
        let mut relay = NativeProgress::default();
        assert!(
            relay
                .push(b"\rTrust this configuration? ", start)
                .is_empty()
        );
        assert_eq!(
            relay.tick(start + FRAGMENT_TIMEOUT),
            b"\rTrust this configuration? "
        );
    }

    #[test]
    fn oversized_and_unfinished_output_is_not_dropped() {
        let start = Instant::now();
        let mut relay = NativeProgress::default();
        let mut input = vec![b'x'; FRAME_LIMIT + 30];
        input[0] = b'\r';
        assert_eq!(relay.push(&input, start), input);
        assert!(relay.push(b"\runfinished", start).is_empty());
        assert_eq!(relay.finish(start), b"\runfinished");
    }
}
