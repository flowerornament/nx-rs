use crate::infra::nix_runtime::{
    InstalledNix, NixDistribution, ReleaseVersion, detect_installed_nix,
};
use crate::infra::shell::CapturedCommand;
use crate::output::printer::Printer;

const DETERMINATE_FD_FIX: ReleaseVersion = ReleaseVersion::new(3, 16, 0);
const SOURCE_CACHE_SIGNATURES: [&str; 2] = [
    "failed to insert entry: invalid object specified",
    "object not found - no match for id",
];
const SOURCE_CACHE_ENTRIES: [&str; 5] = [
    "gitv3",
    "tarball-cache-v2",
    "fetcher-cache-v4.sqlite",
    "fetcher-cache-v4.sqlite-wal",
    "fetcher-cache-v4.sqlite-shm",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownNixIssue {
    TarballPackFileDescriptors,
    LazyTreeSourceCache,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileDescriptorGuidance {
    Upgrade,
    Report,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NixCacheHome {
    User,
    Root,
}

pub(super) fn diagnose_nix_failure(
    output: &CapturedCommand,
    cache_home: NixCacheHome,
    printer: &Printer,
) {
    if output.code == 0 {
        return;
    }
    let Some(issue) = classify(&output.stdout).or_else(|| classify(&output.stderr)) else {
        return;
    };
    let installed = detect_installed_nix().ok().flatten();

    match issue {
        KnownNixIssue::TarballPackFileDescriptors => {
            diagnose_file_descriptors(installed, printer);
        }
        KnownNixIssue::LazyTreeSourceCache => diagnose_source_cache(installed, cache_home, printer),
    }
}

fn classify(output: &str) -> Option<KnownNixIssue> {
    let file_descriptors_exhausted =
        output.contains("too many open files") || output.contains("Too many open files");
    let pack_indexer =
        output.contains("tarball-cache-v2") || output.contains("git packfile indexer");
    if file_descriptors_exhausted && pack_indexer {
        return Some(KnownNixIssue::TarballPackFileDescriptors);
    }
    SOURCE_CACHE_SIGNATURES
        .iter()
        .any(|signature| output.contains(signature))
        .then_some(KnownNixIssue::LazyTreeSourceCache)
}

fn diagnose_file_descriptors(installed: Option<InstalledNix>, printer: &Printer) {
    let Some(installed) = installed else {
        return;
    };
    match file_descriptor_guidance(&installed) {
        Some(FileDescriptorGuidance::Upgrade) => {
            printer.warn(&format!(
                "Determinate Nix {} has a file-descriptor bug fixed in {}",
                installed.version, DETERMINATE_FD_FIX
            ));
            Printer::detail("Run: sudo determinate-nixd upgrade");
        }
        Some(FileDescriptorGuidance::Report) => {
            printer.warn(&format!(
                "Determinate Nix {} hit a file-descriptor failure fixed in {}",
                installed.version, DETERMINATE_FD_FIX
            ));
            Printer::detail("Report: determinate-nixd bug \"Nix file-descriptor exhaustion\"");
        }
        None => {}
    }
}

fn file_descriptor_guidance(installed: &InstalledNix) -> Option<FileDescriptorGuidance> {
    (installed.distribution == NixDistribution::Determinate).then_some(
        if installed.version < DETERMINATE_FD_FIX {
            FileDescriptorGuidance::Upgrade
        } else {
            FileDescriptorGuidance::Report
        },
    )
}

fn diagnose_source_cache(
    installed: Option<InstalledNix>,
    cache_home: NixCacheHome,
    printer: &Printer,
) {
    printer.warn("Nix reported an inconsistent lazy-tree source cache");
    if installed.is_some_and(|nix| nix.distribution == NixDistribution::Determinate) {
        Printer::detail("Report: determinate-nixd bug \"Lazy-tree source-cache corruption\"");
    }
    Printer::detail(&repair_command(cache_home));
}

fn repair_command(cache_home: NixCacheHome) -> String {
    let (prefix, root) = match cache_home {
        NixCacheHome::User => ("Repair: rm -rf", "$HOME"),
        NixCacheHome::Root => ("Repair: sudo rm -rf", "/var/root"),
    };
    let paths = SOURCE_CACHE_ENTRIES.map(|entry| {
        let path = format!("{root}/.cache/nix/{entry}");
        if cache_home == NixCacheHome::User {
            format!("\"{path}\"")
        } else {
            path
        }
    });
    format!("{prefix} {}", paths.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_documented_issue_signatures() {
        assert_eq!(
            classify("creating git packfile indexer for tarball-cache-v2: Too many open files",),
            Some(KnownNixIssue::TarballPackFileDescriptors)
        );
        assert_eq!(classify("activation: Too many open files"), None);
        for signature in SOURCE_CACHE_SIGNATURES {
            assert_eq!(
                classify(signature),
                Some(KnownNixIssue::LazyTreeSourceCache)
            );
        }
        assert_eq!(
            classify("error: adding a file to a tree builder"),
            None,
            "tree-builder failures also occur for deterministic invalid archives"
        );
    }

    #[test]
    fn ignores_unrelated_failures() {
        assert_eq!(classify("error: attribute not found"), None);
    }

    #[test]
    fn fixed_versions_are_distribution_qualified() {
        let determinate = |version| InstalledNix {
            distribution: NixDistribution::Determinate,
            version,
        };
        assert_eq!(
            file_descriptor_guidance(&determinate(ReleaseVersion::new(3, 15, 1))),
            Some(FileDescriptorGuidance::Upgrade)
        );
        assert_eq!(
            file_descriptor_guidance(&determinate(
                ReleaseVersion::parse("3.16.0-rc.1").expect("valid fixture version"),
            )),
            Some(FileDescriptorGuidance::Upgrade)
        );
        assert_eq!(
            file_descriptor_guidance(&determinate(ReleaseVersion::new(3, 21, 8))),
            Some(FileDescriptorGuidance::Report)
        );
        assert_eq!(
            file_descriptor_guidance(&InstalledNix {
                distribution: NixDistribution::Upstream,
                version: ReleaseVersion::new(2, 34, 8),
            }),
            None
        );
    }

    #[test]
    fn repair_commands_share_one_complete_cache_inventory() {
        let user = repair_command(NixCacheHome::User);
        let root = repair_command(NixCacheHome::Root);
        for entry in SOURCE_CACHE_ENTRIES {
            assert!(user.contains(entry));
            assert!(root.contains(entry));
        }
        assert!(user.starts_with("Repair: rm -rf \"$HOME/"));
        assert!(root.starts_with("Repair: sudo rm -rf /var/root/"));
    }
}
