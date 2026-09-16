#![cfg(unix)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use bridge_core::custody_inventory::{
    CustodyInventoryEntryV1, CustodyInventoryPlanV1, CustodyReasonCodeV1 as Reason,
    CustodyStateClassV1 as State, CustodyUnitKindV1 as Kind,
    HistoricalReconciliationDispositionV1 as Disposition, HistoricalReconciliationRowV1,
    LosslessPathV1,
};
use bridge_core::custody_inventory_collector::{
    collect_fixed_population_v1, CustodyCollectionResultV1, CustodyCollectionTargetV1,
    CustodyCollectorErrorV1, FixedCustodyPopulationV1, HistoricalCandidateV1,
    ObservedPathIdentityV1, ObservedPathKindV1 as PathKind, PathObservationV1 as Observation,
    ReadOnlyCustodyProbeV1, UnixMetadataCustodyProbeV1,
};
use bridge_core::fs_custody::BirthTimeV1;
use serde::Deserialize;
use tempfile::TempDir;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    current: Vec<ManifestCurrent>,
    historical: Vec<ManifestHistorical>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestCurrent {
    unit_id: String,
    kind: Kind,
    components: Vec<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestHistorical {
    historical_id: String,
    components: Vec<Vec<u8>>,
    identity_setup: IdentitySetup,
    expected_identity: Option<ObservedPathIdentityV1>,
    #[allow(dead_code)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum IdentitySetup {
    CaptureRuntime,
    LiteralChanged,
    None,
}

fn fixture_path(root: &Path, components: &[Vec<u8>]) -> LosslessPathV1 {
    assert!(!components.is_empty());
    let mut path = root.to_path_buf();
    for bytes in components {
        assert!(!bytes.is_empty());
        assert_ne!(bytes.as_slice(), b".");
        assert_ne!(bytes.as_slice(), b"..");
        assert!(!bytes.contains(&0));
        assert!(!bytes.contains(&b'/'));
        path.push(OsString::from_vec(bytes.clone()));
    }
    LosslessPathV1::from_bytes(path.as_os_str().as_bytes().to_vec())
}

fn as_path(path: &LosslessPathV1) -> PathBuf {
    PathBuf::from(OsString::from_vec(path.as_bytes().to_vec()))
}

// Independent test-only metadata capture: never calls the collector probe or its mapper.
fn capture_fixture_directory_identity(path: &LosslessPathV1) -> ObservedPathIdentityV1 {
    let metadata = fs::symlink_metadata(as_path(path)).unwrap();
    assert!(metadata.file_type().is_dir());
    ObservedPathIdentityV1 {
        kind: PathKind::Directory,
        dev: metadata.dev(),
        ino: metadata.ino(),
        birthtime: BirthTimeV1::from_metadata(&metadata),
    }
}

struct RecordingProbe {
    calls: RefCell<Vec<Vec<u8>>>,
    injected: BTreeMap<Vec<u8>, Observation>,
}

impl RecordingProbe {
    fn new(injected: BTreeMap<Vec<u8>, Observation>) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            injected,
        }
    }

    fn trace(&self) -> Vec<Vec<u8>> {
        self.calls.borrow().clone()
    }
}

impl ReadOnlyCustodyProbeV1 for RecordingProbe {
    fn observe(&self, path: &LosslessPathV1) -> Observation {
        self.calls.borrow_mut().push(path.as_bytes().to_vec());
        self.injected
            .get(path.as_bytes())
            .copied()
            .unwrap_or_else(|| UnixMetadataCustodyProbeV1.observe(path))
    }
}

struct Fixture {
    root: TempDir,
    population: FixedCustodyPopulationV1,
    ids: BTreeMap<String, LosslessPathV1>,
    probe: RecordingProbe,
    nonutf8_standin: Option<LosslessPathV1>,
}

#[rustfmt::skip]
impl Fixture {
    fn new() -> Self {
        let manifest: Manifest = serde_json::from_str(include_str!("fixtures/custody_inventory_slice1b/population.json")).unwrap();
        assert_eq!(manifest.current.len(), 8);
        assert_eq!(manifest.historical.len(), 8);
        let root = tempfile::tempdir().unwrap();
        let mut ids = BTreeMap::new();
        let mut current = Vec::new();
        let mut nonutf8_standin = None;
        for row in manifest.current {
            let path = fixture_path(root.path(), &row.components);
            ids.insert(row.unit_id.clone(), path.clone());
            match row.unit_id.as_str() {
                "current.ambiguous.b" | "current.symlink" => {}
                "current.only" => fs::write(as_path(&path), b"fixed regular-file bytes\n").unwrap(),
                "current.non_utf8" => match fs::create_dir(as_path(&path)) {
                    Ok(()) => {}
                    Err(error) if cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EILSEQ) => {
                        // APFS refuses invalid UTF-8 names. The raw population path stays unchanged;
                        // Linux runs the real-object control, while macOS injects stand-in metadata.
                        let standin = root.path().join("nonutf8-identity-standin");
                        fs::create_dir(&standin).unwrap();
                        nonutf8_standin = Some(LosslessPathV1::from_bytes(standin.as_os_str().as_bytes().to_vec()));
                    }
                    Err(error) => panic!("non-UTF-8 fixture creation failed: {error}"),
                },
                _ => fs::create_dir(as_path(&path)).unwrap(),
            }
            current.push(CustodyCollectionTargetV1 { unit_id: row.unit_id, kind: row.kind, path });
        }
        fs::create_dir(root.path().join("sentinel-target")).unwrap();
        fs::write(root.path().join("sibling-sentinel"), b"outside population").unwrap();
        std::os::unix::fs::symlink(root.path().join("sentinel-target"), as_path(ids.get("current.symlink").unwrap())).unwrap();

        let mut historical = Vec::new();
        for row in manifest.historical {
            let path = fixture_path(root.path(), &row.components);
            ids.insert(row.historical_id.clone(), path.clone());
            let expected_identity = match row.identity_setup {
                IdentitySetup::CaptureRuntime => Some(capture_fixture_directory_identity(if row.historical_id == "historical.06.non_utf8" {
                    nonutf8_standin.as_ref().unwrap_or(&path)
                } else {
                    &path
                })),
                IdentitySetup::LiteralChanged => {
                    let prior = row.expected_identity.unwrap();
                    assert_eq!(prior.kind, PathKind::Other);
                    Some(prior)
                }
                IdentitySetup::None => {
                    assert!(row.expected_identity.is_none());
                    None
                }
            };
            historical.push(HistoricalCandidateV1 {
                historical_id: row.historical_id,
                historical_path: path,
                expected_identity,
            });
        }
        let mut injected = BTreeMap::new();
        injected.insert(ids.get("current.probe_error").unwrap().as_bytes().to_vec(), Observation::Unreadable);
        if let Some(standin) = &nonutf8_standin {
            let observed = UnixMetadataCustodyProbeV1.observe(standin);
            assert!(matches!(observed, Observation::Present(_)));
            injected.insert(ids.get("current.non_utf8").unwrap().as_bytes().to_vec(), observed);
        }
        let population = FixedCustodyPopulationV1::new(current, historical).unwrap();
        Self {
            root,
            population,
            ids,
            probe: RecordingProbe::new(injected),
            nonutf8_standin,
        }
    }

    fn path(&self, id: &str) -> LosslessPathV1 {
        self.ids.get(id).unwrap().clone()
    }

    fn collect(&self) -> CustodyCollectionResultV1 {
        collect_fixed_population_v1(self.population.clone(), &self.probe).unwrap()
    }
}

#[rustfmt::skip]
fn entry(id: &str, kind: Kind, path: LosslessPathV1, reasons: Vec<Reason>) -> CustodyInventoryEntryV1 {
    CustodyInventoryEntryV1 {
        unit_id: id.to_owned(),
        kind,
        path,
        state: State::Unresolved,
        reasons,
    }
}

#[rustfmt::skip]
fn row(id: &str, path: LosslessPathV1, disposition: Disposition) -> HistoricalReconciliationRowV1 {
    HistoricalReconciliationRowV1 {
        historical_id: id.to_owned(),
        historical_path: path,
        disposition,
    }
}

#[test]
#[rustfmt::skip]
fn slice1b_fixed_population_collects_complete_expected_values() {
    let fixture = Fixture::new();
    let result = fixture.collect();
    let present = vec![Reason::ContentUnresolved, Reason::CustodyUnverified];
    let expected = vec![
        entry("current.ambiguous.a", Kind::EvidenceDirectory, fixture.path("current.ambiguous.a"), present.clone()),
        entry("current.ambiguous.b", Kind::EvidenceDirectory, fixture.path("current.ambiguous.b"), present.clone()),
        entry("current.clone.changed", Kind::LinkedWorktree, fixture.path("current.clone.changed"), present.clone()),
        entry("current.clone.resolved", Kind::StandaloneClone, fixture.path("current.clone.resolved"), present.clone()),
        entry("current.non_utf8", Kind::EvidenceDirectory, fixture.path("current.non_utf8"), present.clone()),
        entry("current.only", Kind::SourceLocalRefs, fixture.path("current.only"), present),
        entry(
            "current.probe_error",
            Kind::StandaloneClone,
            fixture.path("current.probe_error"),
            vec![Reason::ConsumerProbeFailed, Reason::ContentUnresolved, Reason::CustodyUnverified],
        ),
        entry(
            "current.symlink",
            Kind::SourceLocalRefs,
            fixture.path("current.symlink"),
            vec![Reason::MountBoundary, Reason::ContentUnresolved, Reason::CustodyUnverified],
        ),
    ];
    assert_eq!(result.plan.schema, "custody-plan.v1");
    assert_eq!(result.plan.entries, expected);
    let expected_rows = vec![
        row("historical.01.resolved", fixture.path("historical.01.resolved"), Disposition::PresentWithResolvedIdentity),
        row("historical.02.changed", fixture.path("historical.02.changed"), Disposition::PresentButChanged),
        row("historical.03.ambiguous", fixture.path("historical.03.ambiguous"), Disposition::AmbiguousMatch),
        row("historical.04.absent", fixture.path("historical.04.absent"), Disposition::AbsentWithoutSufficientDeletionOrCustodyEvidence),
        row(
            "historical.05.legacy_claim_only",
            fixture.path("historical.05.legacy_claim_only"),
            Disposition::AbsentWithoutSufficientDeletionOrCustodyEvidence,
        ),
        row("historical.06.non_utf8", fixture.path("historical.06.non_utf8"), Disposition::PresentWithResolvedIdentity),
        row("historical.07.symlink", fixture.path("historical.07.symlink"), Disposition::AmbiguousMatch),
        row("historical.08.probe_error", fixture.path("historical.08.probe_error"), Disposition::AmbiguousMatch),
    ];
    assert_eq!(result.historical_rows, expected_rows);
    assert_eq!(
        result.historical_rows.iter().map(HistoricalReconciliationRowV1::outcome_code).collect::<Vec<_>>(),
        vec![
            "outcome.present_resolved",
            "park.identity_changed",
            "park.history_ambiguous",
            "outcome.absent_unverified",
            "outcome.absent_unverified",
            "outcome.present_resolved",
            "park.history_ambiguous",
            "park.history_ambiguous",
        ]
    );
}

#[test]
#[rustfmt::skip]
fn slice1b_input_order_does_not_change_outputs() {
    let fixture = Fixture::new();
    let forward = fixture.collect();
    let mut current = fixture.population.current().to_vec();
    let mut historical = fixture.population.historical().to_vec();
    current.reverse();
    historical.reverse();
    let reversed = FixedCustodyPopulationV1::new(current, historical).unwrap();
    let probe = RecordingProbe::new(fixture.probe.injected.clone());
    let backward = collect_fixed_population_v1(reversed, &probe).unwrap();
    assert_eq!(forward.plan, backward.plan);
    assert_eq!(forward.plan.content_digest().unwrap(), backward.plan.content_digest().unwrap());
    assert_eq!(forward.historical_rows, backward.historical_rows);
    assert_eq!(fixture.probe.trace(), probe.trace());
}

#[test]
#[rustfmt::skip]
fn slice1b_fixture_join_and_non_utf8_paths_are_lossless() {
    let fixture = Fixture::new();
    let raw = fixture.path("current.non_utf8");
    let mut expected = fixture.root.path().as_os_str().as_bytes().to_vec();
    expected.extend_from_slice(b"/evidence-");
    expected.push(0xff);
    assert_eq!(raw.as_bytes(), expected);
    assert!(!raw.as_bytes().windows(3).any(|bytes| bytes == [0xef, 0xbf, 0xbd]));
    let result = fixture.collect();
    assert!(fixture.probe.trace().contains(&expected));
    assert_eq!(result.plan.entries.iter().find(|entry| entry.unit_id == "current.non_utf8").unwrap().path.as_bytes(), expected);
    assert_eq!(result.historical_rows.iter().find(|row| row.historical_id == "historical.06.non_utf8").unwrap().historical_path.as_bytes(), expected);
    let plan_bytes = serde_json::to_vec(&result.plan).unwrap();
    let rows_bytes = serde_json::to_vec(&result.historical_rows).unwrap();
    assert_eq!(serde_json::from_slice::<CustodyInventoryPlanV1>(&plan_bytes).unwrap(), result.plan);
    assert_eq!(serde_json::from_slice::<Vec<HistoricalReconciliationRowV1>>(&rows_bytes).unwrap(), result.historical_rows);
    assert!(!plan_bytes.windows(3).any(|bytes| bytes == [0xef, 0xbf, 0xbd]));
    if let Some(standin) = &fixture.nonutf8_standin {
        assert!(!fixture.probe.trace().contains(&standin.as_bytes().to_vec()));
        assert_eq!(UnixMetadataCustodyProbeV1.observe(&raw), Observation::Missing);
    } else {
        assert!(matches!(UnixMetadataCustodyProbeV1.observe(&raw), Observation::Present(identity) if identity.kind == PathKind::Directory));
    }
}

fn synthetic_identity(kind: PathKind) -> ObservedPathIdentityV1 {
    ObservedPathIdentityV1 {
        kind,
        dev: 7,
        ino: 11,
        birthtime: None,
    }
}

#[rustfmt::skip]
fn synthetic_case(observation: Observation, matches: usize, expected_identity: Option<ObservedPathIdentityV1>) -> CustodyCollectionResultV1 {
    let history_path = LosslessPathV1::from_bytes(b"/synthetic-historical".to_vec());
    let current = (0..matches.max(1))
        .map(|index| CustodyCollectionTargetV1 {
            unit_id: format!("current.{index}"),
            kind: Kind::StandaloneClone,
            path: if matches == 0 { LosslessPathV1::from_bytes(b"/different-current".to_vec()) } else { history_path.clone() },
        })
        .collect();
    let historical = vec![HistoricalCandidateV1 {
        historical_id: "historical.one".to_owned(),
        historical_path: history_path.clone(),
        expected_identity,
    }];
    let population = FixedCustodyPopulationV1::new(current, historical).unwrap();
    let probe = RecordingProbe::new(BTreeMap::from([(history_path.as_bytes().to_vec(), observation)]));
    collect_fixed_population_v1(population, &probe).unwrap()
}

#[test]
#[rustfmt::skip]
fn slice1b_reconciliation_is_total_and_missing_is_unverified() {
    let directory = synthetic_identity(PathKind::Directory);
    let cases = [
        ("missing", Observation::Missing, 1, Some(directory), Disposition::AbsentWithoutSufficientDeletionOrCustodyEvidence, "outcome.absent_unverified"),
        ("unreadable", Observation::Unreadable, 1, Some(directory), Disposition::AmbiguousMatch, "park.history_ambiguous"),
        ("zero_match", Observation::Present(directory), 0, Some(directory), Disposition::AmbiguousMatch, "park.history_ambiguous"),
        ("duplicate_match", Observation::Present(directory), 2, Some(directory), Disposition::AmbiguousMatch, "park.history_ambiguous"),
        ("no_identity", Observation::Present(directory), 1, None, Disposition::AmbiguousMatch, "park.history_ambiguous"),
        ("resolved", Observation::Present(directory), 1, Some(directory), Disposition::PresentWithResolvedIdentity, "outcome.present_resolved"),
        (
            "changed",
            Observation::Present(directory),
            1,
            Some(synthetic_identity(PathKind::Other)),
            Disposition::PresentButChanged,
            "park.identity_changed",
        ),
        (
            "symlink",
            Observation::Present(synthetic_identity(PathKind::Symlink)),
            1,
            Some(directory),
            Disposition::AmbiguousMatch,
            "park.history_ambiguous",
        ),
        ("other", Observation::Present(synthetic_identity(PathKind::Other)), 1, Some(directory), Disposition::AmbiguousMatch, "park.history_ambiguous"),
    ];
    for (name, observation, matches, identity, disposition, code) in cases {
        let result = synthetic_case(observation, matches, identity);
        assert_eq!(result.historical_rows, vec![row("historical.one", LosslessPathV1::from_bytes(b"/synthetic-historical".to_vec()), disposition)], "{name}");
        assert_eq!(result.historical_rows[0].outcome_code(), code, "{name}");
        assert_ne!(result.historical_rows[0].disposition, Disposition::AbsentWithSubstantiatedHistoricalDeletionEvidence, "{name}");
    }
    let fixture = Fixture::new();
    let result = fixture.collect();
    for id in ["historical.04.absent", "historical.05.legacy_claim_only"] {
        let row = result.historical_rows.iter().find(|row| row.historical_id == id).unwrap();
        assert_eq!(row.disposition, Disposition::AbsentWithoutSufficientDeletionOrCustodyEvidence);
        assert_eq!(row.outcome_code(), "outcome.absent_unverified");
    }
}

#[test]
#[rustfmt::skip]
fn slice1b_symlink_probe_failure_and_current_reasons_remain_conservative() {
    let fixture = Fixture::new();
    let result = fixture.collect();
    for (id, expected) in [
        ("current.clone.resolved", vec![Reason::ContentUnresolved, Reason::CustodyUnverified]),
        ("current.probe_error", vec![Reason::ConsumerProbeFailed, Reason::ContentUnresolved, Reason::CustodyUnverified]),
        ("current.symlink", vec![Reason::MountBoundary, Reason::ContentUnresolved, Reason::CustodyUnverified]),
    ] {
        let entry = result.plan.entries.iter().find(|entry| entry.unit_id == id).unwrap();
        assert_eq!(entry.state, State::Unresolved);
        assert_eq!(entry.reasons, expected, "{id}");
    }
    for id in ["historical.07.symlink", "historical.08.probe_error"] {
        assert_eq!(result.historical_rows.iter().find(|row| row.historical_id == id).unwrap().disposition, Disposition::AmbiguousMatch);
    }
    let target = fixture.root.path().join("sentinel-target").as_os_str().as_bytes().to_vec();
    assert!(!fixture.probe.trace().contains(&target));
    assert!(matches!(UnixMetadataCustodyProbeV1.observe(&fixture.path("current.symlink")), Observation::Present(identity) if identity.kind == PathKind::Symlink));
    let missing = synthetic_case(Observation::Missing, 1, None);
    assert_eq!(missing.plan.entries[0].reasons, vec![Reason::ActivityUnknown, Reason::ContentUnresolved, Reason::CustodyUnverified]);
    assert_eq!(missing.plan.entries[0].state, State::Unresolved);
}

#[test]
#[rustfmt::skip]
fn slice1b_population_constructor_enforces_private_cap_before_collection() {
    let fixture = Fixture::new();
    let current = fixture.population.current().to_vec();
    let historical = fixture.population.historical().to_vec();
    let mut cases = Vec::new();
    cases.push(("empty_current", vec![], historical.clone(), CustodyCollectorErrorV1::CurrentCount));
    cases.push(("empty_historical", current.clone(), vec![], CustodyCollectorErrorV1::HistoricalCount));
    let mut ninth_current = current.clone();
    ninth_current.push(CustodyCollectionTargetV1 {
        unit_id: "ninth".into(),
        ..current[0].clone()
    });
    cases.push(("ninth_current", ninth_current, historical.clone(), CustodyCollectorErrorV1::CurrentCount));
    let mut ninth_historical = historical.clone();
    ninth_historical.push(HistoricalCandidateV1 {
        historical_id: "ninth".into(),
        ..historical[0].clone()
    });
    cases.push(("ninth_historical", current.clone(), ninth_historical, CustodyCollectorErrorV1::HistoricalCount));
    let mut duplicate_current = current.clone();
    duplicate_current[1].unit_id = duplicate_current[0].unit_id.clone();
    cases.push(("duplicate_current", duplicate_current, historical.clone(), CustodyCollectorErrorV1::DuplicateCurrentId));
    let mut duplicate_historical = historical.clone();
    duplicate_historical[1].historical_id = duplicate_historical[0].historical_id.clone();
    cases.push(("duplicate_historical", current.clone(), duplicate_historical, CustodyCollectorErrorV1::DuplicateHistoricalId));
    let mut empty_current_id = current.clone();
    empty_current_id[0].unit_id.clear();
    cases.push(("empty_current_id", empty_current_id, historical.clone(), CustodyCollectorErrorV1::EmptyCurrentId));
    let mut empty_historical_id = historical.clone();
    empty_historical_id[0].historical_id.clear();
    cases.push(("empty_historical_id", current.clone(), empty_historical_id, CustodyCollectorErrorV1::EmptyHistoricalId));
    for bytes in [vec![], b"nul\0path".to_vec()] {
        let mut bad_current = current.clone();
        bad_current[0].path = LosslessPathV1::from_bytes(bytes.clone());
        cases.push(("bad_current_path", bad_current, historical.clone(), CustodyCollectorErrorV1::InvalidPath));
        let mut bad_historical = historical.clone();
        bad_historical[0].historical_path = LosslessPathV1::from_bytes(bytes);
        cases.push(("bad_historical_path", current.clone(), bad_historical, CustodyCollectorErrorV1::InvalidPath));
    }
    for (name, current, historical, expected) in cases {
        let unused_probe = RecordingProbe::new(BTreeMap::new());
        assert_eq!(FixedCustodyPopulationV1::new(current, historical).unwrap_err(), expected, "{name}");
        assert!(unused_probe.trace().is_empty(), "{name}");
    }
    assert_eq!(fixture.population.current().len(), 8);
    assert_eq!(fixture.population.historical().len(), 8);
    assert!(fixture.population.current().windows(2).all(|pair| pair[0].unit_id < pair[1].unit_id));
}

#[test]
#[rustfmt::skip]
fn slice1b_probes_only_the_sorted_unique_population() {
    let fixture = Fixture::new();
    let result = fixture.collect();
    let mut expected: Vec<Vec<u8>> = [
        "current.clone.resolved",
        "current.clone.changed",
        "current.ambiguous.a",
        "current.non_utf8",
        "current.symlink",
        "current.probe_error",
        "current.only",
        "historical.04.absent",
        "historical.05.legacy_claim_only",
    ]
    .iter()
    .map(|id| fixture.path(id).as_bytes().to_vec())
    .collect();
    expected.sort();
    assert_eq!(expected.len(), 9);
    assert_eq!(fixture.probe.trace(), expected);
    assert_eq!(result.plan.entries.len(), 8);
    assert_eq!(result.historical_rows.len(), 8);
    assert_eq!(fixture.probe.trace().iter().filter(|path| path.as_slice() == fixture.path("current.ambiguous.a").as_bytes()).count(), 1);
    for name in ["sibling-sentinel", "sentinel-target"] {
        let path = fixture.root.path().join(name).as_os_str().as_bytes().to_vec();
        assert!(!fixture.probe.trace().contains(&path));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TreeEntry {
    kind: u8,
    mode: u32,
    symlink: Vec<u8>,
    file: Vec<u8>,
}

#[rustfmt::skip]
fn tree_fingerprint(root: &Path) -> BTreeMap<Vec<u8>, TreeEntry> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<Vec<u8>, TreeEntry>) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let kind = if metadata.is_dir() {
                1
            } else if metadata.file_type().is_symlink() {
                2
            } else if metadata.is_file() {
                3
            } else {
                4
            };
            let entry = TreeEntry {
                kind,
                mode: metadata.mode(),
                symlink: if kind == 2 { fs::read_link(&path).unwrap().as_os_str().as_bytes().to_vec() } else { vec![] },
                file: if kind == 3 { fs::read(&path).unwrap() } else { vec![] },
            };
            out.insert(path.strip_prefix(root).unwrap().as_os_str().as_bytes().to_vec(), entry);
            if kind == 1 {
                visit(root, &path, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
#[rustfmt::skip]
fn slice1b_collector_leaves_fixture_tree_unchanged() {
    let fixture = Fixture::new();
    let before = tree_fingerprint(fixture.root.path());
    assert_eq!(before.get(b"new-current-only".as_slice()).unwrap().file, b"fixed regular-file bytes\n");
    assert!(!before.get(b"symlink-final".as_slice()).unwrap().symlink.is_empty());
    let root_mode = fs::symlink_metadata(fixture.root.path()).unwrap().mode();
    fixture.collect();
    assert_eq!(before, tree_fingerprint(fixture.root.path()));
    assert_eq!(root_mode, fs::symlink_metadata(fixture.root.path()).unwrap().mode());
}

#[test]
#[rustfmt::skip]
fn slice1b_concrete_probe_maps_kinds_and_identity_refinement() {
    let fixture = Fixture::new();
    for (id, expected_kind) in [("current.clone.resolved", PathKind::Directory), ("current.only", PathKind::RegularFile), ("current.symlink", PathKind::Symlink)] {
        assert!(matches!(UnixMetadataCustodyProbeV1.observe(&fixture.path(id)), Observation::Present(identity) if identity.kind == expected_kind), "{id}");
    }
    let fifo = fixture.root.path().join("other-fifo");
    let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    let fifo_path = LosslessPathV1::from_bytes(fifo.as_os_str().as_bytes().to_vec());
    assert!(matches!(UnixMetadataCustodyProbeV1.observe(&fifo_path), Observation::Present(identity) if identity.kind == PathKind::Other));
    assert_eq!(UnixMetadataCustodyProbeV1.observe(&fixture.path("historical.04.absent")), Observation::Missing);
    let mut not_a_directory = fixture.path("current.only").as_bytes().to_vec();
    not_a_directory.extend_from_slice(b"/child");
    assert_eq!(UnixMetadataCustodyProbeV1.observe(&LosslessPathV1::from_bytes(not_a_directory)), Observation::Unreadable);
    assert_eq!(UnixMetadataCustodyProbeV1.observe(&LosslessPathV1::from_bytes(b"nul\0path".to_vec())), Observation::Unreadable);

    let first = BirthTimeV1::new(100, 3).unwrap();
    let second = BirthTimeV1::new(101, 3).unwrap();
    let base = ObservedPathIdentityV1 {
        kind: PathKind::Directory,
        dev: 7,
        ino: 11,
        birthtime: Some(first),
    };
    let no_birth = ObservedPathIdentityV1 { birthtime: None, ..base };
    assert!(base.matches(&base));
    assert!(base.matches(&no_birth));
    assert!(no_birth.matches(&base));
    for (name, changed) in [
        ("birthtime", ObservedPathIdentityV1 { birthtime: Some(second), ..base }),
        ("kind", ObservedPathIdentityV1 { kind: PathKind::RegularFile, ..base }),
        ("device", ObservedPathIdentityV1 { dev: 8, ..base }),
        ("inode", ObservedPathIdentityV1 { ino: 12, ..base }),
    ] {
        assert!(!base.matches(&changed), "{name}");
    }
}
