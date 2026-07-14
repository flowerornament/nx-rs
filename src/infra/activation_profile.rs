use std::time::Instant;

use crate::infra::nix_output::NixActivityType;
use crate::infra::timing::TimingPhase;

#[derive(Debug)]
pub struct ActivationPhaseProfiler {
    started: Instant,
    active: Option<ActivePhase>,
    phases: Vec<TimingPhase>,
    saw_marker: bool,
}

#[derive(Debug)]
struct ActivePhase {
    name: String,
    started: Instant,
}

impl ActivationPhaseProfiler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            active: None,
            phases: Vec::new(),
            saw_marker: false,
        }
    }

    pub fn observe_stderr_line(&mut self, line: &str) {
        if let Some(marker) = activation_marker(line) {
            self.open_phase(marker.name, Instant::now());
        }
    }

    pub fn observe_nix_activity(&mut self, kind: NixActivityType) {
        let name = match kind {
            NixActivityType::CopyPath
            | NixActivityType::FileTransfer
            | NixActivityType::CopyPaths
            | NixActivityType::Substitute
            | NixActivityType::FetchTree => "fetches",
            NixActivityType::Realise
            | NixActivityType::Builds
            | NixActivityType::Build
            | NixActivityType::BuildWaiting => "nix-build",
            NixActivityType::Unknown
            | NixActivityType::OptimiseStore
            | NixActivityType::VerifyPaths
            | NixActivityType::QueryPathInfo
            | NixActivityType::PostBuildHook => return,
        };
        self.open_phase(name.to_string(), Instant::now());
    }

    #[must_use]
    pub fn finish(mut self) -> Vec<TimingPhase> {
        if !self.saw_marker {
            return Vec::new();
        }

        self.close_active(Instant::now());
        self.phases
    }

    fn open_phase(&mut self, name: String, now: Instant) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.name == name)
        {
            return;
        }

        if self.saw_marker {
            self.close_active(now);
        } else {
            let duration_ms = duration_ms(now.duration_since(self.started));
            if duration_ms > 0 {
                self.phases.push(phase("pre-activation", duration_ms));
            }
            self.saw_marker = true;
        }

        self.active = Some(ActivePhase { name, started: now });
    }

    fn close_active(&mut self, now: Instant) {
        if let Some(active) = self.active.take() {
            self.phases.push(phase(
                &active.name,
                duration_ms(now.duration_since(active.started)),
            ));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivationMarker {
    name: String,
}

impl ActivationMarker {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

fn phase(name: &str, duration_ms: u128) -> TimingPhase {
    TimingPhase {
        name: name.to_string(),
        duration_ms,
        status: "ok".to_string(),
        children: Vec::new(),
    }
}

fn duration_ms(duration: std::time::Duration) -> u128 {
    duration.as_millis()
}

fn activation_marker(line: &str) -> Option<ActivationMarker> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    if let Some(marker) = pre_activation_marker(line) {
        return Some(marker);
    }

    if line.starts_with("Activating home-manager configuration for ") {
        return Some(ActivationMarker::new("home-manager"));
    }

    if let Some(name) = line.strip_prefix("Activating ") {
        return Some(ActivationMarker::new(&format!("hm.{}", slug(name))));
    }

    nix_darwin_marker(line).map(ActivationMarker::new)
}

fn pre_activation_marker(line: &str) -> Option<ActivationMarker> {
    if line.starts_with("building the system configuration") {
        return Some(ActivationMarker::new("build"));
    }

    None
}

fn nix_darwin_marker(line: &str) -> Option<&'static str> {
    let markers = [
        ("setting up /Applications/Nix Apps", "nix-apps"),
        ("setting up pam", "pam"),
        ("applying patches", "patches"),
        ("setting up /etc", "etc"),
        ("setting up user defaults", "user-defaults"),
        ("user defaults", "user-defaults"),
        ("restarting Dock", "dock"),
        ("setting up launchd services", "launchd-services"),
        ("setting up user launchd services", "user-launchd-services"),
        ("configuring networking", "networking"),
        ("configuring application firewall", "application-firewall"),
        ("configuring power", "power"),
        ("setting up /Library/Fonts/Nix Fonts", "nix-fonts"),
        ("setting nvram variables", "nvram"),
        ("Homebrew bundle", "homebrew-bundle"),
    ];

    markers
        .into_iter()
        .find_map(|(prefix, name)| line.starts_with(prefix).then_some(name))
}

fn slug(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('.');
    let mut out = String::new();
    let mut previous_dash = false;

    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && !out.is_empty() && !previous_dash {
                out.push('-');
            }
            out.extend(ch.to_lowercase());
            previous_dash = false;
        } else if !previous_dash && !out.is_empty() {
            out.push('-');
            previous_dash = true;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_markers_detect_nix_darwin_and_home_manager_steps() {
        assert_eq!(
            activation_marker("Homebrew bundle...").map(|marker| marker.name),
            Some("homebrew-bundle".to_string())
        );
        assert_eq!(
            activation_marker("Activating home-manager configuration for morgan")
                .map(|marker| marker.name),
            Some("home-manager".to_string())
        );
        assert_eq!(
            activation_marker("Activating linkGeneration").map(|marker| marker.name),
            Some("hm.link-generation".to_string())
        );
        assert_eq!(activation_marker("Using ripgrep"), None);
    }

    #[test]
    fn activation_markers_detect_pre_activation_steps() {
        assert_eq!(
            activation_marker("building the system configuration...").map(|marker| marker.name),
            Some("build".to_string())
        );
    }

    #[test]
    fn profiler_records_marker_durations() {
        let mut profiler = ActivationPhaseProfiler::new();
        profiler.observe_stderr_line("setting up /etc...");
        profiler.observe_stderr_line("Homebrew bundle...");
        let phases = profiler.finish();

        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].name, "etc");
        assert_eq!(phases[1].name, "homebrew-bundle");
    }
}
