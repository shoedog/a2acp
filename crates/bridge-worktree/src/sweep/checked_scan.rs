use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use bridge_core::fs_custody::{BirthTimeV1, PinnedDirectoryV1, RetainedDirectoryEnumerationV1};

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
    names: RetainedDirectoryEnumerationV1,
    enumeration_root: PathBuf,
    custody_root: Option<PinnedDirectoryV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RootIdentityCaptureV1 {
    pub(super) dev: Option<u64>,
    pub(super) ino: Option<u64>,
    pub(super) birthtime: Option<BirthTimeV1>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RootObservationSetV1 {
    pub(super) retained_enumeration_object: Option<RootIdentityCaptureV1>,
    pub(super) pinned_custody_directory: Option<RootIdentityCaptureV1>,
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
        let names = RetainedDirectoryEnumerationV1::open(enumeration_root)
            .map_err(|_| CheckedScanOpenRefusalV1::CannotEnumerate)?;
        let custody_root = self.pin_opener.open_pin(enumeration_root);
        Ok(Box::new(CompatibilityCheckedScanRootSessionV1 {
            names,
            enumeration_root: enumeration_root.to_path_buf(),
            custody_root,
        }))
    }
}

impl CheckedScanRootSessionV1 for CompatibilityCheckedScanRootSessionV1 {
    fn next_name(&mut self) -> Option<Result<OsString, CheckedScanEntryRefusalV1>> {
        self.names
            .next_name()
            .map(|entry| entry.map_err(|_| CheckedScanEntryRefusalV1::CannotReadEntry))
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
        let retained_enumeration_object =
            self.names
                .retained_object_identity()
                .map(|identity| RootIdentityCaptureV1 {
                    dev: Some(identity.dev),
                    ino: Some(identity.ino),
                    birthtime: identity.birthtime,
                });
        let pinned_custody_directory = self.custody_root.as_ref().map(|root| {
            let identity = root.identity();
            RootIdentityCaptureV1 {
                dev: identity.dev,
                ino: identity.ino,
                birthtime: identity.btime,
            }
        });
        #[cfg(unix)]
        let final_named_root = std::fs::metadata(&self.enumeration_root)
            .ok()
            .map(|metadata| {
                use std::os::unix::fs::MetadataExt as _;
                RootIdentityCaptureV1 {
                    dev: Some(metadata.dev()),
                    ino: Some(metadata.ino()),
                    birthtime: BirthTimeV1::from_metadata(&metadata),
                }
            });
        #[cfg(not(unix))]
        let final_named_root = {
            let _ = std::fs::metadata(&self.enumeration_root);
            None
        };
        RootObservationSetV1 {
            retained_enumeration_object,
            pinned_custody_directory,
            final_named_root,
        }
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
    use crate::custody::{
        custody_record_path, PreservationReasonV1, PreservedWorktreeClaimV1, RecoveryLocatorV1,
        WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
    };
    use crate::provider_path::{read_sidecar, sidecar_path, write_sidecar};
    use crate::sweep::{
        CustodyRootObservationV1, ExactAbsenceEnumerationV1, ExactAbsenceObservationV1,
        ExactAbsenceProbeV1, ExactAbsenceRootRefusalV1, UnusedCandidateDecisionV1,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::execution_policy::{PolicyNodeRefV1, Sha256HexV1, WorktreeObjectIdentityV1};
    use bridge_core::fs_custody::verify_payload_directory_identity;
    use std::collections::{HashMap, VecDeque};
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
        let record: WorktreeCustodyRecordV1 = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "custody_id": format!("custody-{}", "3".repeat(64)),
            "checkout_fingerprint": "6".repeat(64),
            "current_attempt": {"execution_id": format!("exec-{}", "1".repeat(32)), "attempt_id": format!("attempt-{}", "2".repeat(32)), "ordinal": 0},
            "worktree": {"canonical_path": "/worktree", "directory_identity": {"canonical_path": "/worktree"}},
            "state": {"state": "protection_prepared"},
            "claim": null,
        }))
        .unwrap();
        WorktreeCustodyRecordV1::decode_canonical(&record.encode_canonical().unwrap()).unwrap()
    }

    fn identity(path: &Path) -> WorktreeObjectIdentityV1 {
        let directory_identity =
            verify_payload_directory_identity(&std::fs::canonicalize(path).unwrap()).unwrap();
        WorktreeObjectIdentityV1 {
            canonical_path: directory_identity.canonical_path.clone(),
            directory_identity,
        }
    }

    fn valid_records(root: &Path) -> (WorktreeSidecar, WorktreeCustodyRecordV1) {
        let source = root.join("source");
        let legacy_worktree = root.join("legacy");
        let custody_worktree = root.join("custody");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&legacy_worktree).unwrap();
        std::fs::create_dir_all(&custody_worktree).unwrap();
        assert!(std::process::Command::new("git")
            .args(["-C", source.to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap()
            .success());
        let legacy = sidecar(&source, &legacy_worktree);
        write_sidecar(&legacy).unwrap();
        let mut custody = decoded_custody();
        custody.worktree = identity(&custody_worktree);
        let attempt = custody.current_attempt.clone();
        custody.claim = Some(PreservedWorktreeClaimV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id: custody.custody_id.clone(),
            execution_id: custody.current_attempt.execution_id.clone(),
            origin_attempt_id: custody.current_attempt.attempt_id.clone(),
            current_attempt: attempt,
            node: PolicyNodeRefV1 {
                sorted_ordinal: 0,
                id_sha256: Sha256HexV1::parse("5".repeat(64)).unwrap(),
            },
            checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
            source: identity(&source),
            root: identity(root),
            worktree: custody.worktree.clone(),
            common_dir: identity(&source.join(".git")),
            reason: PreservationReasonV1::NodeFailure,
            created_wall_ms: 1_700_000_000_000,
            recovery_locator: RecoveryLocatorV1::RegisteredWorktree {},
        });
        std::fs::write(
            custody_record_path(&custody.worktree.canonical_path),
            custody.encode_canonical().unwrap(),
        )
        .unwrap();
        (legacy, custody)
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
        custody_by_name: HashMap<OsString, Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1>>,
        observations: RootObservationSetV1,
        log: Log,
    }

    fn script(log: Log, names: VecDeque<Result<OsString, CheckedScanEntryRefusalV1>>) -> Script {
        Script {
            names,
            legacy: None,
            custody: Ok(decoded_custody()),
            custody_by_name: HashMap::new(),
            observations: RootObservationSetV1::default(),
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

        fn read_custody(
            &self,
            enumerated_name: &OsStr,
        ) -> Result<WorktreeCustodyRecordV1, CustodyReadRefusalV1> {
            note(&self.log, "custody");
            self.custody_by_name
                .get(enumerated_name)
                .unwrap_or(&self.custody)
                .clone()
        }

        fn finish(self: Box<Self>) -> RootObservationSetV1 {
            note(&self.log, "finish");
            self.observations
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

    struct RootPin(Arc<Mutex<Option<PathBuf>>>);

    impl CompatibilityPinOpenerV1 for RootPin {
        fn open_pin(&self, enumeration_root: &Path) -> Option<PinnedDirectoryV1> {
            *self.0.lock().unwrap() = Some(enumeration_root.to_path_buf());
            None
        }
    }

    fn pin_failure_records(root: &Path) -> WorktreeSidecar {
        let legacy = sidecar(&root.join("source"), &root.join("legacy"));
        write_sidecar(&legacy).unwrap();
        std::fs::write(root.join("bad.custody.v1.json"), b"ignored").unwrap();
        legacy
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn production_scan_populates_all_three_root_captures() {
        let root = temp_root("production-root-captures");
        std::fs::create_dir_all(&root).unwrap();

        let (_, _, observations) =
            scan_compatibility_with_pin_opener(&root, FilesystemCompatibilityPinOpenerV1)
                .unwrap()
                .into_exact_parts();

        for capture in [
            observations.retained_enumeration_object,
            observations.pinned_custody_directory,
            observations.final_named_root,
        ] {
            let capture = capture.expect("a healthy Unix root must produce every capture");
            assert!(capture.dev.is_some());
            assert!(capture.ino.is_some());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn retained_capture_is_not_the_pin_and_not_path_metadata() {
        use std::os::unix::fs::MetadataExt as _;

        let root = temp_root("retained-root-capture");
        let replacement = root.with_file_name("retained-root-capture-replacement");
        let original = root.with_file_name("retained-root-capture-original");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("original-name"), b"original").unwrap();
        std::fs::create_dir(&replacement).unwrap();
        std::fs::write(replacement.join("replacement-name"), b"replacement").unwrap();
        let original_metadata = std::fs::metadata(&root).unwrap();
        let replacement_metadata = std::fs::metadata(&replacement).unwrap();
        assert_ne!(
            (original_metadata.dev(), original_metadata.ino()),
            (replacement_metadata.dev(), replacement_metadata.ino()),
            "fixture directories must have distinct identities before the replacement"
        );

        let source = CompatibilityCheckedScanSourceV1::new(FilesystemCompatibilityPinOpenerV1);
        let session = source.open(&root).unwrap();
        std::fs::rename(&root, &original).unwrap();
        std::fs::rename(&replacement, &root).unwrap();
        let observations = session.finish();

        let retained = observations.retained_enumeration_object.unwrap();
        let pinned = observations.pinned_custody_directory.unwrap();
        let final_named = observations.final_named_root.unwrap();
        assert_eq!(retained.dev, Some(original_metadata.dev()));
        assert_eq!(retained.ino, Some(original_metadata.ino()));
        assert_eq!(pinned.dev, Some(original_metadata.dev()));
        assert_eq!(pinned.ino, Some(original_metadata.ino()));
        assert_eq!(final_named.dev, Some(replacement_metadata.dev()));
        assert_eq!(final_named.ino, Some(replacement_metadata.ino()));
        assert_eq!(
            super::super::classify_root_observations(observations),
            CustodyRootObservationV1::IdentityChanged
        );

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(original).unwrap();
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn pin_failure_leaves_the_root_observation_unavailable() {
        let root = temp_root("unavailable-root-capture");
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
        write_sidecar(&legacy).unwrap();
        let canonical = crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap();

        let success = super::super::project_exact_scan_result(
            canonical.clone(),
            scan_compatibility_with_pin_opener(&root, FilesystemCompatibilityPinOpenerV1),
            &probe(
                Some(ExactAbsenceObservationV1::BothAbsent),
                Arc::new(Mutex::new(Vec::new())),
            ),
        );
        let failure = super::super::project_exact_scan_result(
            canonical,
            scan_compatibility_with_pin_opener(&root, Pin(Arc::new(AtomicUsize::new(0)))),
            &probe(
                Some(ExactAbsenceObservationV1::BothAbsent),
                Arc::new(Mutex::new(Vec::new())),
            ),
        );
        let (_, _, successful_observations, successful_rows) = success.into_exact_parts().unwrap();
        let (_, _, failed_observations, failed_rows) = failure.into_exact_parts().unwrap();

        assert_eq!(
            super::super::classify_root_observations(successful_observations),
            CustodyRootObservationV1::Pinned
        );
        assert_eq!(
            super::super::classify_root_observations(failed_observations),
            CustodyRootObservationV1::Unavailable
        );
        assert!(failed_observations.retained_enumeration_object.is_some());
        assert!(failed_observations.pinned_custody_directory.is_none());
        assert!(failed_observations.final_named_root.is_some());
        assert_eq!(
            successful_rows
                .iter()
                .map(|row| (
                    row.checked.parts().0,
                    row.checked.parts().1,
                    row.assessment.decision()
                ))
                .collect::<Vec<_>>(),
            failed_rows
                .iter()
                .map(|row| (
                    row.checked.parts().0,
                    row.checked.parts().1,
                    row.assessment.decision()
                ))
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    #[test]
    fn root_capture_birthtime_capability_is_homogeneous_across_the_three_captures() {
        let root = temp_root("birthtime-capability");
        std::fs::create_dir_all(&root).unwrap();

        let (_, _, observations) =
            scan_compatibility_with_pin_opener(&root, FilesystemCompatibilityPinOpenerV1)
                .unwrap()
                .into_exact_parts();
        let retained = observations.retained_enumeration_object.unwrap();
        let pinned = observations.pinned_custody_directory.unwrap();
        let final_named = observations.final_named_root.unwrap();
        let result = super::super::classify_root_observations(observations);
        let availability = |capture: RootIdentityCaptureV1| {
            if capture.birthtime.is_some() {
                "some"
            } else {
                "none"
            }
        };
        eprintln!(
            "SLICE-B-F8 fixture_dev={} fixture_ino={} retained_birthtime={} pinned_birthtime={} final_named_birthtime={} result={result:?}",
            retained.dev.unwrap(),
            retained.ino.unwrap(),
            availability(retained),
            availability(pinned),
            availability(final_named),
        );
        assert_eq!(retained.birthtime.is_some(), pinned.birthtime.is_some());
        assert_eq!(pinned.birthtime.is_some(), final_named.birthtime.is_some());
        assert_eq!(result, CustodyRootObservationV1::Pinned);

        std::fs::remove_dir_all(root).unwrap();
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
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &exact_source),
            &probe(Some(ExactAbsenceObservationV1::BothAbsent), log.clone()),
        );
        let (_, errors, _observed, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(errors, 2usize);
        assert_eq!(
            rows[0].assessment.decision(),
            UnusedCandidateDecisionV1::Authorized
        );
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
        let source = root.join("source");
        let worktree = root.join("legacy");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let legacy = sidecar(&source, &worktree);
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = Probe {
            result: None,
            calls: calls.clone(),
            log: Arc::new(Mutex::new(Vec::new())),
        };
        let mut source = script(
            Arc::new(Mutex::new(Vec::new())),
            VecDeque::from([
                Ok(OsString::from("legacy.meta.json")),
                Ok(OsString::from("bad.custody.v1.json")),
            ]),
        );
        source.legacy = Some(legacy);
        source.custody = Err(CustodyReadRefusalV1::Unreadable(
            "sweep root is not pinnable".to_string(),
        ));
        let outcome = super::super::project_exact_scan_result(
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &source),
            &probe,
        );
        let (_, _, _, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(matches!(
            rows[0].checked.parts().2,
            ScannedWorktreeRecordV1::Legacy(_)
        ));
        assert!(matches!(
            rows[1].checked.parts().2,
            ScannedWorktreeRecordV1::UnreadableCustody(CustodyReadRefusalV1::Unreadable(message)) if message == "sweep root is not pinnable"
        ));
        assert_eq!(
            rows[1].assessment.decision(),
            UnusedCandidateDecisionV1::Refused
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_route_cannot_canonicalize_without_opening_pin() {
        let calls = Arc::new(AtomicUsize::new(0));
        let outcome = super::super::sweep_orphans_with_exact_absence_with_pin_opener(
            "",
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
            Pin(calls.clone()),
        );
        assert!(matches!(
            outcome.into_exact_parts(),
            Err((None, ExactAbsenceRootRefusalV1::CannotCanonicalize))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    // Evidence: infrastructure mechanism; catches dropped or incorrectly retained decisions.
    #[test]
    fn exact_projection_retains_production_computed_decisions() {
        let root = temp_root("retained-decisions");
        let (legacy, custody) = valid_records(&root);
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut source = script(
            log.clone(),
            VecDeque::from([
                Ok(OsString::from("legacy.meta.json")),
                Ok(OsString::from("custody.custody.v1.json")),
            ]),
        );
        source.legacy = Some(legacy);
        source.custody = Ok(custody);
        let probe = probe(Some(ExactAbsenceObservationV1::BothAbsent), log);
        let outcome = super::super::project_exact_scan_result(
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &source),
            &probe,
        );
        let (_, _, _, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.assessment.decision())
                .collect::<Vec<_>>(),
            [
                UnusedCandidateDecisionV1::Authorized,
                UnusedCandidateDecisionV1::Authorized,
            ]
        );
        assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: refactor characterization; catches an incorrect valid-record decision mapping.
    #[test]
    fn exact_projection_preserves_legacy_and_custody_decision_matrix() {
        let root = temp_root("decision-matrix");
        let (legacy, custody) = valid_records(&root);
        for (observation, expected) in [
            (
                Some(ExactAbsenceObservationV1::BothAbsent),
                UnusedCandidateDecisionV1::Authorized,
            ),
            (
                Some(ExactAbsenceObservationV1::TargetPresent),
                UnusedCandidateDecisionV1::Refused,
            ),
            (
                Some(ExactAbsenceObservationV1::RegisteredButAbsent),
                UnusedCandidateDecisionV1::Refused,
            ),
            (None, UnusedCandidateDecisionV1::Refused),
        ] {
            for (name, legacy_record, custody_record) in [
                (
                    "legacy.meta.json",
                    Some(legacy.clone()),
                    Ok(decoded_custody()),
                ),
                ("custody.custody.v1.json", None, Ok(custody.clone())),
            ] {
                let log = Arc::new(Mutex::new(Vec::new()));
                let mut source = script(log.clone(), VecDeque::from([Ok(OsString::from(name))]));
                source.legacy = legacy_record;
                source.custody = custody_record;
                let outcome = super::super::project_exact_scan_result(
                    crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
                    scan_checked_rows_for_test(&root, &source),
                    &probe(observation, log),
                );
                let (_, _, _, rows) = outcome.into_exact_parts().unwrap();
                assert_eq!(
                    rows[0].assessment.decision(),
                    expected,
                    "{name}: {observation:?}"
                );
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: refactor characterization; catches probing an unreadable custody record.
    #[test]
    fn unreadable_custody_refuses_without_probe() {
        let root = temp_root("unreadable-custody");
        std::fs::create_dir_all(&root).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut source = script(
            log.clone(),
            VecDeque::from([Ok(OsString::from("bad.custody.v1.json"))]),
        );
        source.custody = Err(CustodyReadRefusalV1::Unreadable("test refusal".to_string()));
        let probe = probe(Some(ExactAbsenceObservationV1::BothAbsent), log);
        let outcome = super::super::project_exact_scan_result(
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &source),
            &probe,
        );
        let (_, _, _, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(
            rows[0].assessment.decision(),
            UnusedCandidateDecisionV1::Refused
        );
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: canonical-root characterization plus compiler-only report-return shape.
    #[test]
    fn exact_route_preserves_canonical_scan_root_and_report_return() {
        let root = temp_root("canonical-root");
        std::fs::create_dir_all(&root).unwrap();
        let canonical = std::fs::canonicalize(&root).unwrap();
        let opened = Arc::new(Mutex::new(None));
        let supplied = root.join(".");
        let outcome = super::super::sweep_orphans_with_exact_absence_with_pin_opener(
            supplied.to_str().unwrap(),
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
            RootPin(opened.clone()),
        );
        let (scan_root, _, _, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(scan_root.as_str(), canonical.to_str().unwrap());
        assert!(rows.is_empty());
        assert_eq!(opened.lock().unwrap().as_deref(), Some(canonical.as_path()));
        let _: super::super::ExactAbsenceSweepReportV1 =
            super::super::sweep_orphans_with_exact_absence(
                supplied.to_str().unwrap(),
                &probe(None, Arc::new(Mutex::new(Vec::new()))),
            );
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: characterization; catches classifier precedence or suffix-boundary changes.
    #[test]
    fn checked_scan_classifier_preserves_full_path_precedence_and_boundaries() {
        let root = temp_root("classifier");
        let classify = |name| classify_record_display(&root.join(name).to_string_lossy());
        assert_eq!(
            classify("legacy.meta.json"),
            Some(CheckedScanRecordKindV1::Legacy)
        );
        assert_eq!(classify(".custody.v1.json"), None);
        assert_eq!(classify("dir/.custody.v1.json"), None);
        assert_eq!(
            classify(r"dir\.custody.v1.json"),
            Some(CheckedScanRecordKindV1::Custody)
        );
    }

    // Evidence: characterization; catches malformed-legacy retention or lost custody refusals.
    #[test]
    fn checked_scan_silently_omits_bad_legacy_and_retains_bad_custody() {
        let root = temp_root("bad-records");
        std::fs::create_dir_all(&root).unwrap();
        let malformed = root.join("bad.meta.json");
        std::fs::write(&malformed, b"not json").unwrap();
        assert_eq!(read_sidecar(&malformed.to_string_lossy()), None);
        let refusal = CustodyReadRefusalV1::Unreadable("preserved refusal".to_string());
        let mut source = script(
            Arc::new(Mutex::new(Vec::new())),
            VecDeque::from([
                Ok(OsString::from("bad.meta.json")),
                Ok(OsString::from("bad.custody.v1.json")),
            ]),
        );
        source.custody = Err(refusal.clone());
        let rows =
            super::super::project_action_scan_result(scan_checked_rows_for_test(&root, &source));
        assert_eq!(rows.len(), 1);
        assert!(
            matches!(&rows[0].1, ScannedWorktreeRecordV1::UnreadableCustody(actual) if actual == &refusal)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: characterization; catches skipped iterator errors or reordered injected traversal.
    #[test]
    fn checked_scan_counts_iterator_errors_and_continues_in_injected_order() {
        let root = temp_root("iterator-errors");
        std::fs::create_dir_all(&root).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut source = script(
            log.clone(),
            VecDeque::from([
                Ok(OsString::from("second.custody.v1.json")),
                Err(CheckedScanEntryRefusalV1::CannotReadEntry),
                Ok(OsString::from("first.custody.v1.json")),
                Err(CheckedScanEntryRefusalV1::CannotReadEntry),
            ]),
        );
        source.custody = Err(CustodyReadRefusalV1::Unreadable("refusal".to_string()));
        let outcome = super::super::project_exact_scan_result(
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &source),
            &probe(None, log.clone()),
        );
        let (_, errors, _, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(errors, 2);
        let paths = rows
            .iter()
            .map(|row| row.checked.record_path().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                root.join("second.custody.v1.json")
                    .to_string_lossy()
                    .into_owned(),
                root.join("first.custody.v1.json")
                    .to_string_lossy()
                    .into_owned(),
            ]
        );
        assert_eq!(
            log.lock().unwrap().as_slice(),
            ["next", "custody", "next", "next", "custody", "next", "next", "finish"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: report projection; the injected scan forces otherwise hard-to-produce iterator failures.
    #[test]
    fn exact_projection_reports_forced_iterator_errors() {
        let root = temp_root("reported-iterator-errors");
        std::fs::create_dir_all(&root).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let source = script(
            log.clone(),
            VecDeque::from([
                Err(CheckedScanEntryRefusalV1::CannotReadEntry),
                Err(CheckedScanEntryRefusalV1::CannotReadEntry),
            ]),
        );

        let report = super::super::project_exact_scan_result(
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &source),
            &probe(None, log),
        )
        .into_report(root.to_string_lossy().into_owned());

        assert!(matches!(
            report.scan().enumeration(),
            ExactAbsenceEnumerationV1::Incomplete { skipped_entries: 2 }
        ));
        assert!(report.entries().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: injected-seam mechanism; catches loss of exact-only root observations.
    #[test]
    fn nondefault_root_observations_survive_exact_without_changing_rows_or_decisions() {
        let root = temp_root("observations");
        std::fs::create_dir_all(&root).unwrap();
        let legacy = sidecar(&root.join("source"), &root.join("legacy"));
        let observations = RootObservationSetV1 {
            retained_enumeration_object: Some(RootIdentityCaptureV1 {
                dev: Some(1),
                ino: Some(2),
                birthtime: None,
            }),
            pinned_custody_directory: None,
            final_named_root: Some(RootIdentityCaptureV1 {
                dev: Some(3),
                ino: Some(4),
                birthtime: None,
            }),
        };
        let names = VecDeque::from([Ok(OsString::from("legacy.meta.json"))]);
        let mut default_source = script(Arc::new(Mutex::new(Vec::new())), names.clone());
        default_source.legacy = Some(legacy.clone());
        let mut observed_source = script(Arc::new(Mutex::new(Vec::new())), names);
        observed_source.legacy = Some(legacy);
        observed_source.observations = observations;
        let canonical = crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap();
        let default = super::super::project_exact_scan_result(
            canonical.clone(),
            scan_checked_rows_for_test(&root, &default_source),
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
        );
        let observed = super::super::project_exact_scan_result(
            canonical,
            scan_checked_rows_for_test(&root, &observed_source),
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
        );
        let (_, default_errors, default_observations, default_rows) =
            default.into_exact_parts().unwrap();
        let (_, observed_errors, observed_observations, observed_rows) =
            observed.into_exact_parts().unwrap();
        assert_eq!(default_errors, observed_errors);
        assert_eq!(default_observations, RootObservationSetV1::default());
        assert_eq!(observed_observations, observations);
        assert_eq!(default_rows.len(), 1);
        for (before, after) in default_rows.iter().zip(observed_rows.iter()) {
            assert_eq!(before.checked.parts().0, after.checked.parts().0);
            assert_eq!(before.checked.parts().1, after.checked.parts().1);
            assert_eq!(before.assessment.decision(), after.assessment.decision());
            assert!(
                matches!((before.checked.parts().2, after.checked.parts().2), (ScannedWorktreeRecordV1::Legacy(left), ScannedWorktreeRecordV1::Legacy(right)) if left == right)
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: characterization; catches loss of a canonical root on enumeration refusal.
    #[test]
    fn enumeration_refusal_retains_canonical_root_and_skips_assessment() {
        let root = temp_root("enumeration-refusal");
        std::fs::write(&root, b"not a directory").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let log = Arc::new(Mutex::new(Vec::new()));
        let probe = probe(None, log.clone());
        let outcome = super::super::sweep_orphans_with_exact_absence_with_pin_opener(
            root.to_str().unwrap(),
            &probe,
            Pin(calls.clone()),
        );
        assert!(matches!(
            outcome.into_exact_parts(),
            Err((Some(canonical), ExactAbsenceRootRefusalV1::CannotEnumerate))
                if canonical.as_str() == std::fs::canonicalize(&root).unwrap().to_str().unwrap()
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
        assert!(log.lock().unwrap().is_empty());
        std::fs::remove_file(root).unwrap();
    }

    // Evidence: projection characterization; catches action-only metadata retention or exact loss.
    #[test]
    fn action_projection_erases_only_action_metadata() {
        let root = temp_root("action-erasure");
        std::fs::create_dir_all(&root).unwrap();
        let legacy = sidecar(&root.join("source"), &root.join("legacy"));
        let names = VecDeque::from([
            Ok(OsString::from("legacy.meta.json")),
            Err(CheckedScanEntryRefusalV1::CannotReadEntry),
            Ok(OsString::from("custody.custody.v1.json")),
        ]);
        let mut action_source = script(Arc::new(Mutex::new(Vec::new())), names.clone());
        action_source.legacy = Some(legacy.clone());
        let action: Vec<(String, ScannedWorktreeRecordV1)> =
            scan_checked_rows_for_test(&root, &action_source)
                .unwrap()
                .into_action_rows();
        let mut exact_source = script(Arc::new(Mutex::new(Vec::new())), names);
        exact_source.legacy = Some(legacy);
        exact_source.observations = RootObservationSetV1 {
            retained_enumeration_object: None,
            pinned_custody_directory: Some(RootIdentityCaptureV1 {
                dev: Some(5),
                ino: Some(6),
                birthtime: None,
            }),
            final_named_root: None,
        };
        let (rows, errors, observations) = scan_checked_rows_for_test(&root, &exact_source)
            .unwrap()
            .into_exact_parts();
        let action_paths = action
            .iter()
            .map(|(path, _)| path.to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            action_paths,
            vec![
                root.join("legacy.meta.json").to_string_lossy().into_owned(),
                root.join("custody.custody.v1.json")
                    .to_string_lossy()
                    .into_owned(),
            ]
        );
        assert_eq!(
            rows.iter().map(|row| row.parts().1).collect::<Vec<_>>(),
            [
                OsStr::new("legacy.meta.json"),
                OsStr::new("custody.custody.v1.json"),
            ]
        );
        assert_eq!(errors, 1);
        assert!(observations.pinned_custody_directory.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: injected-seam mechanism; catches test-only projection reimplementation.
    #[test]
    fn injected_sources_use_production_action_and_exact_projections() {
        let root = temp_root("production-projections");
        std::fs::create_dir_all(&root).unwrap();
        let names = VecDeque::from([Ok(OsString::from("bad.custody.v1.json"))]);
        let action = super::super::project_action_scan_result(scan_checked_rows_for_test(
            &root,
            &script(Arc::new(Mutex::new(Vec::new())), names.clone()),
        ));
        let exact = super::super::project_exact_scan_result(
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &script(Arc::new(Mutex::new(Vec::new())), names)),
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
        );
        let (_, _, _, rows) = exact.into_exact_parts().unwrap();
        assert_eq!(action[0].0, rows[0].checked.record_path());
        assert!(matches!(
            (&action[0].1, rows[0].checked.parts().2),
            (ScannedWorktreeRecordV1::Custody(left), ScannedWorktreeRecordV1::Custody(right)) if left == right
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: projection characterization; catches divergence between action and exact records.
    #[test]
    fn injected_sources_prove_action_and_exact_projection_equivalence() {
        let root = temp_root("projection-equivalence");
        std::fs::create_dir_all(&root).unwrap();
        let names = VecDeque::from([
            Ok(OsString::from("legacy.meta.json")),
            Ok(OsString::from("decoded.custody.v1.json")),
            Ok(OsString::from("unreadable.custody.v1.json")),
        ]);
        let legacy = sidecar(&root.join("source"), &root.join("legacy"));
        let custody = decoded_custody();
        let refusal = CustodyReadRefusalV1::Unreadable("same refusal".to_string());
        let mut source = script(Arc::new(Mutex::new(Vec::new())), names);
        source.legacy = Some(legacy);
        source
            .custody_by_name
            .insert(OsString::from("unreadable.custody.v1.json"), Err(refusal));
        source.custody = Ok(custody);
        let action =
            super::super::project_action_scan_result(scan_checked_rows_for_test(&root, &source));
        let exact = super::super::project_exact_scan_result(
            crate::provider_path::canonicalize_lenient(root.to_str().unwrap()).unwrap(),
            scan_checked_rows_for_test(&root, &source),
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
        );
        let (_, _, _, rows) = exact.into_exact_parts().unwrap();
        assert_eq!(
            action.iter().map(|(path, _)| path).collect::<Vec<_>>(),
            rows.iter()
                .map(|row| row.checked.record_path())
                .collect::<Vec<_>>()
        );
        for ((_, action_record), exact_row) in action.iter().zip(rows.iter()) {
            match (action_record, exact_row.checked.parts().2) {
                (ScannedWorktreeRecordV1::Legacy(left), ScannedWorktreeRecordV1::Legacy(right)) => {
                    assert_eq!(left, right)
                }
                (
                    ScannedWorktreeRecordV1::Custody(left),
                    ScannedWorktreeRecordV1::Custody(right),
                ) => {
                    assert_eq!(left, right)
                }
                (
                    ScannedWorktreeRecordV1::UnreadableCustody(left),
                    ScannedWorktreeRecordV1::UnreadableCustody(right),
                ) => {
                    assert_eq!(left, right)
                }
                _ => panic!("action and exact record kinds diverged"),
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: post-canonicalization opener seam; catches pin failure before canonicalization.
    #[test]
    fn report_side_pin_failure_uses_post_canonicalization_opener_seam() {
        let root = temp_root("report-pin-failure");
        std::fs::create_dir_all(&root).unwrap();
        let legacy = pin_failure_records(&root);
        let opened = Arc::new(Mutex::new(None));
        let outcome = super::super::sweep_orphans_with_exact_absence_with_pin_opener(
            root.join(".").to_str().unwrap(),
            &probe(None, Arc::new(Mutex::new(Vec::new()))),
            RootPin(opened.clone()),
        );
        let (canonical, _, _, rows) = outcome.into_exact_parts().unwrap();
        assert_eq!(
            canonical.as_str(),
            std::fs::canonicalize(&root).unwrap().to_str().unwrap()
        );
        assert_eq!(
            opened.lock().unwrap().as_deref(),
            Some(std::path::Path::new(canonical.as_str()))
        );
        assert!(rows.iter().any(|row| matches!(
            row.checked.parts().2,
            ScannedWorktreeRecordV1::Legacy(actual) if actual == &legacy
        )));
        assert!(rows.iter().any(|row| matches!(
            row.checked.parts().2,
            ScannedWorktreeRecordV1::UnreadableCustody(CustodyReadRefusalV1::Unreadable(message)) if message == "sweep root is not pinnable"
        )));
        std::fs::remove_dir_all(root).unwrap();
    }

    // Evidence: action-projection seam; catches loss of legacy rows after deterministic pin failure.
    #[test]
    fn compatibility_pin_failure_preserves_legacy_and_refuses_custody() {
        let root = temp_root("action-pin-failure");
        std::fs::create_dir_all(&root).unwrap();
        let legacy = pin_failure_records(&root);
        let calls = Arc::new(AtomicUsize::new(0));
        let rows = super::super::project_action_scan_result(scan_compatibility_with_pin_opener(
            &root,
            Pin(calls.clone()),
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(rows.iter().any(|(_, record)| matches!(
            record,
            ScannedWorktreeRecordV1::Legacy(actual) if actual == &legacy
        )));
        assert!(rows.iter().any(|(_, record)| matches!(
            record,
            ScannedWorktreeRecordV1::UnreadableCustody(CustodyReadRefusalV1::Unreadable(message)) if message == "sweep root is not pinnable"
        )));
        std::fs::remove_dir_all(root).unwrap();
    }
}
