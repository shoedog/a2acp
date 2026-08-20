use std::ffi::{OsStr, OsString};
use std::path::Path;

use bridge_core::fs_custody::{BirthTimeV1, PinnedDirectoryV1};

use crate::custody::{
    is_custody_record_name, read_custody_record_in, CustodyReadRefusalV1, WorktreeCustodyRecordV1,
};
use crate::provider_path::{read_sidecar, WorktreeSidecar};

use super::ScannedWorktreeRecordV1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CheckedScanOpenRefusalV1 {
    CannotEnumerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckedScanEntryRefusalV1 {
    CannotReadEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckedScanRecordKindV1 {
    Legacy,
    Custody,
}

trait CheckedScanSourceV1 {
    fn open(
        &self,
        enumeration_root: &Path,
    ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1>;
}

trait CheckedScanRootSessionV1 {
    fn next_name(&mut self) -> Option<Result<OsString, CheckedScanEntryRefusalV1>>;

    fn read_legacy(&self, enumerated_name: &OsStr, record_display: &str)
        -> Option<WorktreeSidecar>;

    fn read_custody(
        &self,
        enumerated_name: &OsStr,
    ) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1>;

    fn finish(self: Box<Self>) -> RootObservationSetV1;
}

pub(super) trait CompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FilesystemCompatibilityPinOpenerV1;

impl CompatibilityPinOpenerV1 for FilesystemCompatibilityPinOpenerV1 {
    fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1> {
        PinnedDirectoryV1::open(enumeration_root, "worktree sweep root").ok()
    }
}

struct CompatibilityCheckedScanSourceV1<P> {
    pin_opener: P,
}

impl<P: CompatibilityPinOpenerV1> CompatibilityCheckedScanSourceV1<P> {
    const fn new(pin_opener: P) -> Self {
        Self { pin_opener }
    }
}

struct CompatibilityCheckedScanRootSessionV1 {
    names: std::fs::ReadDir,
    custody_root: Option<PinnedDirectoryV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RootIdentityCaptureV1 {
    #[allow(dead_code)]
    pub(super) dev: Option<u64>,
    #[allow(dead_code)]
    pub(super) ino: Option<u64>,
    #[allow(dead_code)]
    pub(super) birthtime: Option<BirthTimeV1>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RootObservationSetV1 {
    #[allow(dead_code)]
    pub(super) retained_enumeration_object: Option<RootIdentityCaptureV1>,
    #[allow(dead_code)]
    pub(super) pinned_custody_directory: Option<RootIdentityCaptureV1>,
    #[allow(dead_code)]
    pub(super) final_named_root: Option<RootIdentityCaptureV1>,
}

pub(super) struct CheckedScanRowV1 {
    record_path: String,
    enumerated_name: OsString,
    scanned: ScannedWorktreeRecordV1,
}

impl CheckedScanRowV1 {
    pub(super) fn parts(&self) -> (&str, &OsStr, &ScannedWorktreeRecordV1) {
        (&self.record_path, &self.enumerated_name, &self.scanned)
    }

    pub(super) fn record_path(&self) -> &str {
        &self.record_path
    }
}

pub(super) struct CheckedScanCompletedV1 {
    rows: Vec<CheckedScanRowV1>,
    iterator_error_count: usize,
    root_observations: RootObservationSetV1,
}

impl CheckedScanCompletedV1 {
    pub(super) fn into_action_rows(self) -> Vec<(String, ScannedWorktreeRecordV1)> {
        self.rows
            .into_iter()
            .map(|row| (row.record_path, row.scanned))
            .collect()
    }

    pub(super) fn into_exact_parts(self) -> (Vec<CheckedScanRowV1>, usize, RootObservationSetV1) {
        (self.rows, self.iterator_error_count, self.root_observations)
    }
}

impl<P: CompatibilityPinOpenerV1> CheckedScanSourceV1 for CompatibilityCheckedScanSourceV1<P> {
    fn open(
        &self,
        enumeration_root: &Path,
    ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1> {
        let names = std::fs::read_dir(enumeration_root)
            .map_err(|_| CheckedScanOpenRefusalV1::CannotEnumerate)?;
        let custody_root = self.pin_opener.open_pin(enumeration_root);
        Ok(Box::new(CompatibilityCheckedScanRootSessionV1 {
            names,
            custody_root,
        }))
    }
}

impl CheckedScanRootSessionV1 for CompatibilityCheckedScanRootSessionV1 {
    fn next_name(&mut self) -> Option<Result<OsString, CheckedScanEntryRefusalV1>> {
        self.names.next().map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|_| CheckedScanEntryRefusalV1::CannotReadEntry)
        })
    }

    fn read_legacy(
        &self,
        _enumerated_name: &OsStr,
        record_display: &str,
    ) -> Option<WorktreeSidecar> {
        read_sidecar(record_display)
    }

    fn read_custody(
        &self,
        enumerated_name: &OsStr,
    ) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1> {
        match self.custody_root.as_ref() {
            Some(root) => read_custody_record_in(root, enumerated_name),
            None => Err(CustodyReadRefusalV1::Unreadable(
                "sweep root is not pinnable".to_string(),
            )),
        }
    }

    fn finish(self: Box<Self>) -> RootObservationSetV1 {
        RootObservationSetV1::default()
    }
}

fn classify_record_display(record_display: &str) -> Option<CheckedScanRecordKindV1> {
    if record_display.ends_with(".meta.json") {
        Some(CheckedScanRecordKindV1::Legacy)
    } else if is_custody_record_name(record_display) {
        Some(CheckedScanRecordKindV1::Custody)
    } else {
        None
    }
}

fn scan_checked_rows_with_source(
    enumeration_root: &Path,
    source: &dyn CheckedScanSourceV1,
) -> Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1> {
    let mut session = source.open(enumeration_root)?;
    let mut rows = Vec::new();
    let mut iterator_error_count = 0usize;
    while let Some(name) = session.next_name() {
        let name = match name {
            Ok(name) => name,
            Err(CheckedScanEntryRefusalV1::CannotReadEntry) => {
                iterator_error_count += 1;
                continue;
            }
        };
        let record_path = enumeration_root.join(&name).to_string_lossy().into_owned();
        let scanned = match classify_record_display(&record_path) {
            Some(CheckedScanRecordKindV1::Legacy) => session
                .read_legacy(&name, &record_path)
                .map(ScannedWorktreeRecordV1::Legacy),
            Some(CheckedScanRecordKindV1::Custody) => Some(match session.read_custody(&name) {
                Ok(record) => ScannedWorktreeRecordV1::Custody(Box::new(record)),
                Err(refusal) => ScannedWorktreeRecordV1::UnreadableCustody(refusal),
            }),
            None => None,
        };
        if let Some(scanned) = scanned {
            rows.push(CheckedScanRowV1 {
                record_path,
                enumerated_name: name,
                scanned,
            });
        }
    }
    let root_observations = session.finish();
    Ok(CheckedScanCompletedV1 {
        rows,
        iterator_error_count,
        root_observations,
    })
}

pub(super) fn scan_compatibility_with_pin_opener<P>(
    enumeration_root: &Path,
    pin_opener: P,
) -> Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1>
where
    P: CompatibilityPinOpenerV1,
{
    let source = CompatibilityCheckedScanSourceV1::new(pin_opener);
    scan_checked_rows_with_source(enumeration_root, &source)
}

#[cfg(test)]
fn scan_checked_rows_for_test(
    enumeration_root: &Path,
    source: &dyn CheckedScanSourceV1,
) -> Result<CheckedScanCompletedV1, CheckedScanOpenRefusalV1> {
    scan_checked_rows_with_source(enumeration_root, source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_path::{sidecar_path, write_sidecar};
    use crate::sweep::{
        ExactAbsenceObservationV1, ExactAbsenceProbeV1, ExactAbsenceRootRefusalV1,
        UnusedCandidateDecisionV1,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::SessionCwd;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::SystemTime;

    type Log = Arc<Mutex<Vec<&'static str>>>;

    fn note(log: &Log, operation: &'static str) {
        log.lock().unwrap().push(operation);
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("a2a-checked-scan-{label}-{:?}", SystemTime::now()))
    }

    fn decoded_custody() -> WorktreeCustodyRecordV1 {
        WorktreeCustodyRecordV1::decode_canonical(br#"{"schema_version":1,"custody_id":"custody-3333333333333333333333333333333333333333333333333333333333333333","checkout_fingerprint":"6666666666666666666666666666666666666666666666666666666666666666","current_attempt":{"execution_id":"exec-11111111111111111111111111111111","attempt_id":"attempt-22222222222222222222222222222222","ordinal":0},"worktree":{"canonical_path":"/worktree","directory_identity":{"canonical_path":"/worktree","dev":null,"ino":null}},"state":{"state":"protection_prepared"},"claim":null}"#).unwrap()
    }

    fn sidecar(source: &Path, worktree: &Path) -> WorktreeSidecar {
        WorktreeSidecar {
            canonical_source: source.to_string_lossy().into_owned(),
            common_dir: source.join(".git").to_string_lossy().into_owned(),
            worktree_path: worktree.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "run".into(),
            host: "host".into(),
            lease: "lease".into(),
        }
    }

    #[derive(Clone)]
    struct Script {
        names: VecDeque<Result<OsString, CheckedScanEntryRefusalV1>>,
        legacy: Option<WorktreeSidecar>,
        custody: Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1>,
        log: Log,
    }

    fn script(log: Log, names: VecDeque<Result<OsString, CheckedScanEntryRefusalV1>>) -> Script {
        Script {
            names,
            legacy: None,
            custody: Ok(decoded_custody()),
            log,
        }
    }

    impl CheckedScanSourceV1 for Script {
        fn open(
            &self,
            _: &Path,
        ) -> Result<Box<dyn CheckedScanRootSessionV1>, CheckedScanOpenRefusalV1> {
            Ok(Box::new(self.clone()))
        }
    }

    impl CheckedScanRootSessionV1 for Script {
        fn next_name(&mut self) -> Option<Result<OsString, CheckedScanEntryRefusalV1>> {
            note(&self.log, "next");
            self.names.pop_front()
        }

        fn read_legacy(&self, _: &OsStr, _: &str) -> Option<WorktreeSidecar> {
            note(&self.log, "legacy");
            self.legacy.clone()
        }

        fn read_custody(&self, _: &OsStr) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1> {
            note(&self.log, "custody");
            self.custody.clone()
        }

        fn finish(self: Box<Self>) -> RootObservationSetV1 {
            note(&self.log, "finish");
            RootObservationSetV1::default()
        }
    }

    struct Probe {
        result: Option<ExactAbsenceObservationV1>,
        calls: Arc<AtomicUsize>,
        log: Log,
    }

    impl ExactAbsenceProbeV1 for Probe {
        fn observe_exact_absence(
            &self,
            _: &super::super::ExactAbsenceCandidateV1,
        ) -> Result<ExactAbsenceObservationV1, BridgeError> {
            note(&self.log, "probe");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.ok_or(BridgeError::StoreFailure)
        }
    }

    fn probe(result: Option<ExactAbsenceObservationV1>, log: Log) -> Probe {
        Probe {
            result,
            calls: Arc::new(AtomicUsize::new(0)),
            log,
        }
    }

    struct Pin(Arc<AtomicUsize>);

    impl CompatibilityPinOpenerV1 for Pin {
        fn open_pin(&self, _: &Path) -> Option<PinnedDirectoryV1> {
            self.0.fetch_add(1, Ordering::SeqCst);
            None
        }
    }

    #[test]
    fn compatibility_open_refusal_never_calls_pin_opener() {
        let root = temp_root("missing");
        let calls = Arc::new(AtomicUsize::new(0));
        assert!(!root.exists());
        let source = CompatibilityCheckedScanSourceV1::new(Pin(calls.clone()));
        assert!(scan_checked_rows_for_test(&root, &source).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn checked_scan_reads_each_selected_name_before_next_and_finishes_once() {
        let root = temp_root("ordered");
        let source = root.join("source");
        let worktree = root.join("legacy");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        assert!(std::process::Command::new("git")
            .args(["-C", source.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success());
        let legacy = sidecar(&source, &worktree);
        std::fs::write(sidecar_path(&legacy.worktree_path), b"injected").unwrap();
        let names = VecDeque::from([
            Ok(OsString::from("legacy.meta.json")),
            Err(CheckedScanEntryRefusalV1::CannotReadEntry),
            Ok(OsString::from("decoded.custody.v1.json")),
            Ok(OsString::from("ignored")),
            Err(CheckedScanEntryRefusalV1::CannotReadEntry),
        ]);
        let expected = decoded_custody();
        let mut action_source = script(Arc::new(Mutex::new(Vec::new())), names.clone());
        action_source.legacy = Some(legacy.clone());
        let action = super::super::project_action_scan_result(scan_checked_rows_for_test(
            &root,
            &action_source,
        ));
        assert_eq!(action.len(), 2);
        assert!(
            matches!(&action[1].1, ScannedWorktreeRecordV1::Custody(actual) if actual.as_ref() == &expected)
        );
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut exact_source = script(log.clone(), names);
        exact_source.legacy = Some(legacy);
        let outcome = super::super::project_exact_scan_result(
            SessionCwd::parse(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &exact_source),
            &probe(Some(ExactAbsenceObservationV1::BothAbsent), log.clone()),
        );
        let (_, errors, _observed, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(errors, 2usize);
        assert_eq!(rows[0].decision, UnusedCandidateDecisionV1::Authorized);
        assert!(
            matches!(rows[1].checked.parts().2, ScannedWorktreeRecordV1::Custody(actual) if actual.as_ref() == &expected)
        );
        assert_eq!(
            log.lock().unwrap().as_slice(),
            [
                "next", "legacy", "next", "next", "custody", "next", "next", "next", "finish",
                "probe"
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_route_pin_failure_preserves_legacy_and_refuses_custody() {
        let root = temp_root("pin-failure");
        let worktree = root.join("legacy");
        std::fs::create_dir_all(&worktree).unwrap();
        let legacy = sidecar(&root.join("source"), &worktree);
        write_sidecar(&legacy).unwrap();
        std::fs::write(root.join("bad.custody.v1.json"), b"ignored").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = Probe {
            result: None,
            calls: calls.clone(),
            log: Arc::new(Mutex::new(Vec::new())),
        };
        let outcome = super::super::sweep_orphans_with_exact_absence_with_pin_opener(
            &root.to_string_lossy(),
            &probe,
            Pin(Arc::new(AtomicUsize::new(0))),
        );
        let (_, _, _, rows) = outcome.into_exact_parts().unwrap();
        assert!(matches!(
            rows[0].checked.parts().2,
            ScannedWorktreeRecordV1::Legacy(_)
        ));
        assert!(
            matches!(rows[1].checked.parts().2, ScannedWorktreeRecordV1::UnreadableCustody(CustodyReadRefusalV1::Unreadable(message)) if message == "sweep root is not pinnable")
        );
        assert_eq!(rows[1].decision, UnusedCandidateDecisionV1::Refused);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_route_cannot_canonicalize_without_opening_pin() {
        let root = temp_root("cannot-canonicalize");
        let calls = Arc::new(AtomicUsize::new(0));
        let outcome = super::super::sweep_orphans_with_exact_absence_with_pin_opener(
            &root.to_string_lossy(),
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
            Pin(calls.clone()),
        );
        assert!(matches!(
            outcome.into_exact_parts(),
            Err((None, ExactAbsenceRootRefusalV1::CannotCanonicalize))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
