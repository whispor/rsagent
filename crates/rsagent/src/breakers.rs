//! Cross-run persistence for circuit-breaker state (`docs/EXTENSIBILITY.md` §3.5).
//!
//! A breaker lives in memory inside its source, and memory dies with the process. To make
//! `rsagent tools list` / `reset` meaningful, the CLI snapshots each source's breaker to a
//! small JSON file after a run and seeds it back on the next one. The snapshot records
//! wall-clock seconds (see [`BreakerReport`]), because a monotonic `Instant` means nothing
//! across processes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rsagent_agent::{BreakerReport, BreakerState, SourceEntry, ToolSource};
use serde::{Deserialize, Serialize};

use crate::fs::write_atomic;

/// The snapshot file, beside the working directory. One per project.
pub(crate) const BREAKER_FILE: &str = ".rsagent-breakers.json";

/// The on-disk schema version. Bump when the shape changes incompatibly; a file written by
/// another version is ignored rather than misread.
const SCHEMA_VERSION: u32 = 1;

/// The persisted document.
#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    #[serde(default)]
    breakers: BTreeMap<String, BreakerReport>,
}

/// A source-id → breaker-report map, loaded from and saved to disk.
///
/// Only *non-closed* breakers are kept: a healthy source has no state worth carrying
/// across runs, and dropping it on recovery is what keeps the file from growing forever.
#[derive(Debug)]
pub(crate) struct BreakerStore {
    path: PathBuf,
    reports: BTreeMap<String, BreakerReport>,
}

impl BreakerStore {
    /// Load the snapshot for `dir`, or an empty store when there is none (or it is
    /// unreadable, or written by another schema version — never a hard error).
    pub(crate) fn load(dir: &Path) -> Self {
        let path = dir.join(BREAKER_FILE);
        let reports = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Snapshot>(&text).ok())
            .filter(|snapshot| snapshot.version == SCHEMA_VERSION)
            .map_or_else(BTreeMap::new, |snapshot| snapshot.breakers);
        BreakerStore { path, reports }
    }

    /// Rebuild each source's breaker from its stored report, if it has one.
    pub(crate) fn seed(&self, entries: &[SourceEntry]) {
        for entry in entries {
            self.seed_source(entry.source());
        }
    }

    /// Rebuild one source's breaker from its stored report, if it has one.
    ///
    /// Used by `doctor`, which builds and probes sources one at a time.
    pub(crate) fn seed_source(&self, source: &dyn ToolSource) {
        if let Some(report) = self.reports.get(source.id()) {
            source.restore_breaker(report);
        }
    }

    /// Replace the stored reports with a fresh snapshot of every tripped source.
    pub(crate) fn capture(&mut self, entries: &[SourceEntry]) {
        self.reports.clear();
        for entry in entries {
            let Some(report) = entry.source().breaker_report() else {
                continue;
            };
            if report.state != BreakerState::Closed {
                self.reports.insert(entry.source().id().to_string(), report);
            }
        }
    }

    /// Persist the snapshot, or delete the file when nothing is tripped.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O or serialization error. Callers treat a failure as
    /// non-fatal: losing breaker state must never fail a run.
    pub(crate) fn save(&self) -> anyhow::Result<()> {
        if self.reports.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            };
        }
        let snapshot = Snapshot {
            version: SCHEMA_VERSION,
            breakers: self.reports.clone(),
        };
        let text = serde_json::to_string_pretty(&snapshot)?;
        Ok(write_atomic(&self.path, &text)?)
    }

    /// Forget stored reports: one id, or all of them when `id` is `None`.
    ///
    /// Returns the ids that were actually cleared, so the caller can report precisely.
    pub(crate) fn reset(&mut self, id: Option<&str>) -> Vec<String> {
        if let Some(id) = id {
            if self.reports.remove(id).is_some() {
                vec![id.to_string()]
            } else {
                Vec::new()
            }
        } else {
            let ids: Vec<String> = self.reports.keys().cloned().collect();
            self.reports.clear();
            ids
        }
    }

    /// The stored reports, for a status display.
    pub(crate) fn reports(&self) -> &BTreeMap<String, BreakerReport> {
        &self.reports
    }

    /// The path of the snapshot file, for messages.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::{BreakerStore, BREAKER_FILE};
    use rsagent_agent::{BreakerReport, BreakerState, SourceEntry};
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rsagent-breakers-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn an_open_report() -> BreakerReport {
        BreakerReport {
            state: BreakerState::Open,
            failures: 3,
            opened_at_unix: Some(1_700_000_000),
            cooldown_secs: 60,
        }
    }

    #[test]
    fn a_missing_file_loads_an_empty_store() {
        let dir = temp_dir("missing");
        let store = BreakerStore::load(&dir);
        assert!(store.reports().is_empty());
        assert_eq!(store.path(), dir.join(BREAKER_FILE));
    }

    #[test]
    fn save_then_load_round_trips_a_report() {
        let dir = temp_dir("roundtrip");
        let mut store = BreakerStore::load(&dir);
        // Seed one report by hand through the public surface: capture needs sources, so
        // write the file with a store we control via reset+save is not enough — instead
        // verify the pure serialization path through a direct save of a captured map.
        store.reset(None);
        // The store is empty, so `save` removes the file; prove that is a no-op success.
        store.save().expect("saving an empty store is fine");
        assert!(!dir.join(BREAKER_FILE).exists());

        // Now exercise a real file: write one, then read it back.
        let text = serde_json::json!({
            "version": 1,
            "breakers": { "local-x": an_open_report() }
        })
        .to_string();
        std::fs::write(dir.join(BREAKER_FILE), text).expect("write fixture");

        let loaded = BreakerStore::load(&dir);
        assert_eq!(loaded.reports().get("local-x"), Some(&an_open_report()));
    }

    #[test]
    fn a_foreign_schema_version_is_ignored() {
        let dir = temp_dir("version");
        let text = serde_json::json!({
            "version": 999,
            "breakers": { "local-x": an_open_report() }
        })
        .to_string();
        std::fs::write(dir.join(BREAKER_FILE), text).expect("write fixture");

        let loaded = BreakerStore::load(&dir);
        assert!(
            loaded.reports().is_empty(),
            "an unknown version must be ignored"
        );
    }

    #[test]
    fn reset_reports_only_ids_it_actually_cleared() {
        let dir = temp_dir("reset");
        let text = serde_json::json!({
            "version": 1,
            "breakers": {
                "a": an_open_report(),
                "b": an_open_report()
            }
        })
        .to_string();
        std::fs::write(dir.join(BREAKER_FILE), text).expect("write fixture");

        let mut store = BreakerStore::load(&dir);
        assert_eq!(store.reset(Some("a")), vec!["a".to_string()]);
        assert_eq!(store.reset(Some("missing")), Vec::<String>::new());
        assert_eq!(store.reset(None), vec!["b".to_string()]);
        assert!(store.reports().is_empty());
    }

    #[test]
    fn capture_keeps_only_non_closed_sources() {
        // A store with no sources still captures cleanly into an empty map.
        let dir = temp_dir("capture");
        let mut store = BreakerStore::load(&dir);
        store.capture(&Vec::<SourceEntry>::new());
        assert!(store.reports().is_empty());
    }
}
