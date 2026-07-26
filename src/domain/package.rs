use std::collections::HashSet;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageSource {
    Nix,
    Homebrew,
    Cask,
    Mas,
    Service,
}

impl PackageSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nix => "nix",
            Self::Homebrew => "homebrew",
            Self::Cask => "cask",
            Self::Mas => "mas",
            Self::Service => "service",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DeclarationKind {
    Package,
    ExternalInput,
    GeneratedCommand,
    RuntimeEnvironment,
    RuntimeMember { runtime: String },
    Application,
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageDeclaration {
    pub name: String,
    pub source: PackageSource,
    pub sites: Vec<DeclarationSite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclarationSite {
    pub location: String,
    pub kind: DeclarationKind,
}

impl PackageDeclaration {
    #[must_use]
    pub fn runtime_member(&self) -> Option<&str> {
        let mut runtimes = self.sites.iter().map(|site| match &site.kind {
            DeclarationKind::RuntimeMember { runtime } => Some(runtime.as_str()),
            _ => None,
        });
        let runtime = runtimes.next().flatten()?;
        runtimes
            .all(|candidate| candidate == Some(runtime))
            .then_some(runtime)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InventoryIssue {
    pub location: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PackageInventory {
    pub declarations: Vec<PackageDeclaration>,
    pub issues: Vec<InventoryIssue>,
}

impl PackageInventory {
    pub fn push(&mut self, declaration: PackageDeclaration) {
        if let Some(existing) = self.declarations.iter_mut().find(|existing| {
            existing.name == declaration.name && existing.source == declaration.source
        }) {
            for site in declaration.sites {
                if !existing.sites.contains(&site) {
                    existing.sites.push(site);
                }
            }
        } else {
            self.declarations.push(declaration);
        }
    }

    #[must_use]
    pub fn buckets(&self) -> PackageBuckets {
        let mut buckets = PackageBuckets::default();
        let mut seen = SourceSeen::default();

        for declaration in &self.declarations {
            if declaration.runtime_member().is_some() {
                continue;
            }
            let (out, source_seen) = match declaration.source {
                PackageSource::Nix => (&mut buckets.nxs, &mut seen.nxs),
                PackageSource::Homebrew => (&mut buckets.brews, &mut seen.brews),
                PackageSource::Cask => (&mut buckets.casks, &mut seen.casks),
                PackageSource::Mas => (&mut buckets.mas, &mut seen.mas),
                PackageSource::Service => (&mut buckets.services, &mut seen.services),
            };
            if source_seen.insert(declaration.name.clone()) {
                out.push(declaration.name.clone());
            }
        }

        buckets
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PackageBuckets {
    pub nxs: Vec<String>,
    pub brews: Vec<String>,
    pub casks: Vec<String>,
    pub mas: Vec<String>,
    pub services: Vec<String>,
}

#[derive(Default)]
struct SourceSeen {
    nxs: HashSet<String>,
    brews: HashSet<String>,
    casks: HashSet<String>,
    mas: HashSet<String>,
    services: HashSet<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        DeclarationKind, DeclarationSite, PackageDeclaration, PackageInventory, PackageSource,
    };

    #[test]
    fn duplicate_packages_preserve_each_declaration_site() {
        let mut inventory = PackageInventory::default();
        for location in ["home/a.nix:3", "home/b.nix:7"] {
            inventory.push(PackageDeclaration {
                name: "ripgrep".to_string(),
                source: PackageSource::Nix,
                sites: vec![DeclarationSite {
                    location: location.to_string(),
                    kind: DeclarationKind::Package,
                }],
            });
        }

        assert_eq!(inventory.declarations.len(), 1);
        assert_eq!(inventory.declarations[0].sites.len(), 2);
        assert_eq!(inventory.buckets().nxs, vec!["ripgrep"]);
    }
}
