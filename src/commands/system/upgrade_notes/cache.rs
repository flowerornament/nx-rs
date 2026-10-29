use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::sources::Evidence;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Note {
    pub evidence: Evidence,
    pub summary: Option<String>,
    pub saved: u64,
    #[serde(default)]
    pub summarized: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Cache {
    #[serde(default)]
    pub notes: BTreeMap<String, Note>,
    #[serde(default)]
    pub repositories: BTreeMap<String, (String, u64)>,
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
const WEEK: u64 = 7 * 24 * 60 * 60;

impl Cache {
    pub fn load(path: &Path) -> Self {
        let mut cache: Self = fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.len() <= 2 * 1024 * 1024)
            .and_then(|_| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let cutoff = now().saturating_sub(WEEK);
        cache.notes.retain(|_, note| note.saved >= cutoff);
        cache.repositories.retain(|_, (_, saved)| *saved >= cutoff);
        cache
    }

    pub fn save(&mut self, path: &Path) {
        while self.notes.len() > 50 {
            let Some(key) = self
                .notes
                .iter()
                .min_by_key(|(_, note)| note.saved)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.notes.remove(&key);
        }
        while self.repositories.len() > 100 {
            let Some(key) = self
                .repositories
                .iter()
                .min_by_key(|(_, (_, saved))| saved)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.repositories.remove(&key);
        }
        if let Some(parent) = path.parent()
            && fs::create_dir_all(parent).is_ok()
            && let Ok(bytes) = serde_json::to_vec(self)
            && bytes.len() <= 2 * 1024 * 1024
        {
            let _ = crate::infra::persistence::write_file_atomically(path, bytes);
        }
    }
}
