//! In-crate controls for ADR-0041 slice 2B3c increment C2a: phase P's admission, prevalidation, and plan.
//!
//! Every row runs the phase-P planner over a REAL phase-G value: a real 2B2 export carried through the real
//! 2B3a reader (phase V) and the real 2B3b wrapper (phase G, with real Git). The export is the rich
//! plan-backed clone, a planned in-worktree separate Git directory, or the empty custom capsule with a
//! captured unborn HEAD beside hand-encoded phase-P frames, built by the real frame encoder, that stand where
//! a sealed capsule's staged payloads would (the 2B3b `stage_frame` fixture, applied before phase G runs).
//! Increment C2a writes nothing, so every row, a refusal or a success, also proves that the destination tree
//! and the one ledger are exactly what they were. Mounted beneath the 2B3b controls, this reuses their
//! fixtures without widening any of them. The mutation matrix in `.git/a2a-evidence/2b3c-c2a/` maps each
//! guard to the rows that turn red without it.

use super::super::payload::*;
use super::*;
use crate::custody_coverage::{
    plan_coverage_v1, CustodyCoverageRequestV1, CustodyCoverageSourcesV1, CustodyPlanningBudgetV1,
    NoExternalEvidenceRecordedV1,
};
use crate::custody_export::tests::{manifest_from_plan, planned_capability};
use crate::custody_frame::CustodyFrameComponentFaultV1;
use std::cell::{Cell as StdCell, RefCell as StdRefCell};
use std::rc::Rc as StdRc;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

const P_WORKTREE_V1: ClassV1 = CustodyCoverageClassV1::Worktree;
const P_INDEX_V1: ClassV1 = CustodyCoverageClassV1::Index;
const P_OPERATIONS_V1: ClassV1 = CustodyCoverageClassV1::InProgressGitOperations;
const P_CONFIG_V1: ClassV1 = CustodyCoverageClassV1::GitConfigurationAndHooks;
const P_BRIDGE_V1: ClassV1 = CustodyCoverageClassV1::BridgeEvidence;
/// The five phase-P classes, in step order.
const P_CLASSES_V1: [ClassV1; 5] = [
    P_WORKTREE_V1,
    P_INDEX_V1,
    P_OPERATIONS_V1,
    P_CONFIG_V1,
    P_BRIDGE_V1,
];
/// Every class outside the eight-class union (D5).
const P_OUTSIDE_UNION_V1: [ClassV1; 6] = [
    CustodyCoverageClassV1::LinkedWorktrees,
    CustodyCoverageClassV1::NestedRepositoriesAndSubmodules,
    CustodyCoverageClassV1::LfsAndExternalPayloads,
    CustodyCoverageClassV1::AlternatesAndSharedStores,
    CustodyCoverageClassV1::ExternalEvidence,
    CustodyCoverageClassV1::ReproducibleOutputs,
];
/// The plan table's production bound, restated.
const P_TABLE_BOUND_V1: u64 = 64 * 1024 * 1024;
const P_ENTRY_OVERHEAD_V1: u64 = 64;
/// A deadline for the rows that spend it: started only once the plane exists, and long enough for the
/// admission before the hook to finish on a loaded host.
const P_DEADLINE_V1: Duration = Duration::from_secs(3);

type TreeV1 = BTreeMap<String, Option<Vec<u8>>>;
/// The destination tree as a hook left it: a row whose hook changes the tree compares the tree after phase
/// P with this one instead of the tree before.
type DriftV1 = StdRc<StdRefCell<Option<TreeV1>>>;
type PlannedOutcomeV1 = Result<PlannedPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>>;

/// One real phase-G value with its destination.
struct PCaseV1 {
    dest: DestV1,
    plane: RestoredGitPlaneV1,
    drift: DriftV1,
}

impl PCaseV1 {
    /// The real wrapper (phase G) over a real phase-V value, as `run_plane` runs it, on the lane route: no
    /// child is tampered with, so the recording route (whose sealed anchor a non-root run cannot remove) is
    /// not needed.
    fn of(input: PlaneInV1) -> Self {
        let (_area, runner) = lane_runner();
        let PlaneInV1 {
            dest,
            verified,
            format,
            ..
        } = input;
        let outcome =
            restore_git_plane_v1(verified, &runner, format, frame_budget(), far_deadline());
        let plane = outcome.unwrap_or_else(|failure| {
            panic!(
                "phase G restores the plane: {:?} (completed {:?})",
                failure.cause, failure.last_completed
            )
        });
        Self {
            dest,
            plane,
            drift: StdRc::new(StdRefCell::new(None)),
        }
    }

    /// A sealed capsule through phase V and phase G.
    fn sealed(capsule: &Path) -> Self {
        let (dest, verified) = phase_v(capsule);
        let phase_v = ledger_used(&verified);
        Self::of(PlaneInV1 {
            dest,
            verified,
            format: GitObjectFormatV1::Sha1,
            phase_v,
        })
    }

    /// The empty custom capsule with a captured unborn HEAD, `edit`ed (frames staged, coverage changed)
    /// before phase G runs.
    fn unborn(capsule: &CustomCapsuleV1, edit: impl FnOnce(&mut VerifiedCapsuleV1)) -> Self {
        let mut input = PlaneInV1::unborn(capsule, CustodyGitObjectFormatV1::Sha1);
        edit(&mut input.verified);
        Self::of(input)
    }

    /// [`Self::unborn`] with each `(class, bytes)` staged as that class's one Captured payload.
    fn with_frames(capsule: &CustomCapsuleV1, frames: &[(ClassV1, Vec<u8>)]) -> Self {
        Self::unborn(capsule, |verified| {
            for (class, bytes) in frames {
                stage_frame(verified, *class, bytes);
            }
        })
    }

    /// A clone of the retained descriptor of `class`'s staged payload, for a row that changes it.
    fn staged_handle(&self, class: ClassV1) -> std::fs::File {
        let index = staged_index(self.plane.capsule(), class);
        self.plane.capsule().staged[index]
            .file
            .try_clone()
            .expect("a duplicate descriptor")
    }

    /// Plans under `deadline` and `bound`, and requires the run to leave no trace whatever it returns: the
    /// one ledger is unchanged, a refusal is in the no-materialization shape with the ledger's figure, and
    /// the destination tree is the one before (or the one a drifting hook left).
    fn run(self, row: &str, deadline: Instant, bound: u64) -> (DestV1, PlannedOutcomeV1) {
        let ledger = self.plane.ledger_used();
        let before = snapshot(&self.dest.path);
        let outcome = plan_payload_plane_bounded_v1(self.plane, frame_budget(), deadline, bound);
        let plane = match &outcome {
            Ok(planned) => &planned.plane,
            Err(failure) => {
                assert!(!failure.payload_materialization_began, "{row}");
                assert_eq!(failure.ledger_used, ledger, "{row}: the failure's figure");
                &failure.partial.plane
            }
        };
        assert_eq!(plane.ledger_used(), ledger, "{row}: the one ledger");
        let drifted = self.drift.borrow_mut().take();
        let unchanged = drifted.is_none();
        assert_eq!(
            snapshot(&self.dest.path),
            drifted.unwrap_or(before),
            "{row}: phase P created or changed nothing"
        );
        if unchanged {
            assert_eq!(
                self.dest.entries(),
                [".restore-work", "repository"],
                "{row}"
            );
            assert_eq!(
                listing(&stored(&self.dest, "repository")),
                [".git"],
                "{row}: no payload entry exists"
            );
        }
        (self.dest, outcome)
    }

    fn planned(self, row: &str) -> (DestV1, PlannedPayloadPlaneV1) {
        let (dest, outcome) = self.run(row, far_deadline(), P_TABLE_BOUND_V1);
        let planned =
            outcome.unwrap_or_else(|failure| panic!("{row}: must plan: {:?}", failure.cause));
        assert_eq!(planned.step, PayloadPlaneStepV1::Prevalidation, "{row}");
        assert!(
            planned
                .plane
                .capsule()
                .staged
                .iter()
                .all(|one| position(one) == 0),
            "{row}: every staged descriptor is back at offset 0"
        );
        (dest, planned)
    }

    fn refused(self, row: &str) -> Box<PayloadPlaneRestoreFailureV1> {
        self.refused_within(row, far_deadline(), P_TABLE_BOUND_V1)
    }

    fn refused_within(
        self,
        row: &str,
        deadline: Instant,
        bound: u64,
    ) -> Box<PayloadPlaneRestoreFailureV1> {
        match self.run(row, deadline, bound).1 {
            Ok(_) => panic!("{row}: must refuse"),
            Err(failure) => failure,
        }
    }
}

/// A hand-encoded frame of `class` for the custom capsules' generation.
fn p_frame(class: ClassV1, entries: &EntriesV1<'_>) -> Vec<u8> {
    encode_frame(class, GENERATION_V1, entries)
}

/// One small valid frame of each phase-P class.
fn p_minimal(class: ClassV1) -> Vec<u8> {
    match class {
        CustodyCoverageClassV1::Index => p_frame(class, &[file("index", b"DIRC")]),
        _ => p_frame(class, &[file("f", b"one")]),
    }
}

/// A frame entry as the test's own decode reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum OwnKindV1 {
    Dir(u32),
    File(u32, u64),
    Link(Vec<u8>),
}

fn own_kind(kind: &PlannedKindV1) -> OwnKindV1 {
    match kind {
        PlannedKindV1::Directory { mode, .. } => OwnKindV1::Dir(*mode),
        PlannedKindV1::Regular { mode, length } => OwnKindV1::File(*mode, *length),
        PlannedKindV1::Symlink { target, .. } => OwnKindV1::Link(target.clone()),
    }
}

fn own_entries_of(frame: &PlannedFrameV1) -> Vec<(Vec<u8>, OwnKindV1)> {
    frame
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), own_kind(&entry.kind)))
        .collect()
}

/// The test's own decode of one retained frame, independent of the planner. It leaves the descriptor at
/// offset 0.
fn own_decode(file: &std::fs::File, class: ClassV1, generation: &str) -> Vec<(Vec<u8>, OwnKindV1)> {
    use std::io::Seek as _;
    let mut handle = file.try_clone().expect("a duplicate descriptor");
    handle.seek(SeekFrom::Start(0)).expect("rewind");
    let header = CustodyFrameHeaderV1::new(class, generation).expect("a header");
    let mut entries = Vec::new();
    {
        let reader = BufReader::new(&handle);
        let mut decoder =
            CustodyFrameDecoderV1::new(reader, &header, frame_budget()).expect("a decoder");
        while let Some(entry) = decoder.next_entry().expect("a whole frame") {
            match entry {
                CustodyFrameEntryV1::Directory { path, mode } => {
                    entries.push((path.as_bytes().to_vec(), OwnKindV1::Dir(mode)));
                }
                CustodyFrameEntryV1::Regular {
                    path,
                    mode,
                    length,
                    mut content,
                } => {
                    std::io::copy(&mut content, &mut std::io::sink()).expect("its content");
                    entries.push((path.as_bytes().to_vec(), OwnKindV1::File(mode, length)));
                }
                CustodyFrameEntryV1::Symlink { path, target } => {
                    entries.push((path.as_bytes().to_vec(), OwnKindV1::Link(target)));
                }
            }
        }
    }
    handle.seek(SeekFrom::Start(0)).expect("rewind");
    entries
}

/// D6's table charge, by the test's own arithmetic: two copies of each path, its symlink target, and 64
/// bytes per entry.
fn own_charge<'a>(entries: impl IntoIterator<Item = (&'a [u8], Option<&'a [u8]>)>) -> u64 {
    entries
        .into_iter()
        .map(|(path, target)| {
            2 * path.len() as u64
                + target.map_or(0, |target| target.len() as u64)
                + P_ENTRY_OVERHEAD_V1
        })
        .sum()
}

fn hand_charge(entries: &EntriesV1<'_>) -> u64 {
    own_charge(entries.iter().map(|(path, entry)| {
        let target = match entry {
            EntryV1::Link(target) => Some(*target),
            _ => None,
        };
        (*path, target)
    }))
}

/// A frame's symlinks as `(exact path, external)`, in frame order.
fn planned_links(frame: &PlannedFrameV1) -> Vec<(Vec<u8>, bool)> {
    frame
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            PlannedKindV1::Symlink { external, .. } => Some((entry.path.clone(), *external)),
            _ => None,
        })
        .collect()
}

fn frame_fault(failure: &PayloadPlaneRestoreFailureV1) -> (ClassV1, PayloadFrameFaultV1) {
    match &failure.cause {
        PayloadPlaneErrorV1::PayloadFrameInvalid { class, fault } => (*class, fault.clone()),
        other => panic!("a frame refusal, not {other:?}"),
    }
}

/// The directory at `path` (the destination `root` or an entry beneath it) renamed out beside the
/// destination, where it stays (still the inode every retained pin holds) until the destination's area is
/// removed, and a new directory made at its name.
fn swap_out(root: &Path, path: &Path) {
    let beside = root.parent().expect("the destination's area");
    let away = tempfile::TempDir::new_in(beside)
        .expect("a place beside the destination")
        .keep();
    swap_directory(path, &away);
}

/// Runs `act` once from a hook and records the destination tree it left.
fn drifting(
    case: &PCaseV1,
    act: impl Fn(&Path) + 'static,
) -> impl Fn(&PayloadHookEventV1<'_>) + 'static {
    let (drift, root) = (case.drift.clone(), case.dest.path.clone());
    let done = StdCell::new(false);
    move |_| {
        if !done.replace(true) {
            act(&root);
            *drift.borrow_mut() = Some(snapshot(&root));
        }
    }
}

/// Plain Git for building a source, with a closed environment and a fixed identity.
fn source_git(cwd: &Path, args: &[&str]) {
    let output = Command::new(lane_git_path())
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("the source Git child runs");
    assert!(
        output.status.success(),
        "source git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The coverage control `separate_01`'s source (a worktree `repo/` whose root `.git` is a gitfile naming
/// the separate Git directory `repo/.gd` inside it) plus one more ordinary empty directory, `repo/empty`:
/// planned by the real 2B2b2b1 planner and sealed by the real 2B2 exporter.
struct SeparateGitDirV1 {
    _temp: tempfile::TempDir,
    capsule: PathBuf,
}

impl SeparateGitDirV1 {
    fn export() -> Self {
        let temp = tempfile::TempDir::new().expect("a fixture area");
        let area = temp.path().canonicalize().expect("a canonical area");
        let worktree = area.join("repo");
        let separate = worktree.join(".gd");
        std::fs::create_dir(&worktree).expect("the worktree");
        source_git(
            &area,
            &[
                "init",
                "-q",
                &format!("--separate-git-dir={}", separate.display()),
                worktree.to_str().expect("a UTF-8 fixture path"),
            ],
        );
        std::fs::write(worktree.join("README"), b"separate\n").expect("the README");
        source_git(&worktree, &["add", "README"]);
        source_git(&worktree, &["commit", "-q", "-m", "init"]);
        std::fs::create_dir(worktree.join("empty")).expect("an ordinary empty directory");
        assert!(worktree.join(".git").is_file(), "the root gitfile");
        let objects: Vec<CustodyOriginalObjectV1> = plumb(
            &separate,
            &[
                "cat-file",
                "--batch-all-objects",
                "--batch-check=%(objectname) %(objecttype)",
            ],
            None,
        )
        .lines()
        .map(|line| {
            let (object_id, kind) = line.split_once(' ').expect("an `id type` row");
            CustodyOriginalObjectV1::new(CustodyGitObjectFormatV1::Sha1, object_id, kind_of(kind))
                .expect("a listed object id")
        })
        .collect();
        let planning = area.join("planning");
        private_dir(&planning);
        let scratch = area.join("scratch");
        private_dir(&scratch);
        let request = CustodyCoverageRequestV1 {
            sources: CustodyCoverageSourcesV1::pin(&worktree, &separate, &separate.join("objects"))
                .expect("pin the source"),
            generation_id: GENERATION_V1.to_owned(),
            frame_budget: frame_budget(),
            entry_budget: 10_000,
            planning_budget: CustodyPlanningBudgetV1 {
                max_scratch_bytes: 64 * 1024 * 1024,
            },
            scratch_root: planning,
            external_evidence: NoExternalEvidenceRecordedV1::declare().into(),
            object_format: CustodyGitObjectFormatV1::Sha1,
            object_inventory: objects.clone(),
            git_route: lane_git_route(),
            deadline: far_deadline(),
        };
        let plan = plan_coverage_v1(&request).expect("the separate Git directory plans");
        let manifest = manifest_from_plan(&plan, GENERATION_V1, objects);
        let capability = planned_capability(&manifest, plan, request.sources);
        let outcome = export_capsule_v1(CustodyExportRequestV1 {
            manifest: &manifest,
            envelope_format: &fixture_envelope_format_v1(),
            recipients: &["fixture-recipient".to_owned()],
            budgets: CustodyExportBudgetsV1::v1_ceilings(),
            sealer: &FixtureSealerV1::honest(),
            capability,
            git_route: lane_git_route(),
            scratch_root: &scratch,
            deadline: far_deadline(),
        })
        .expect("the separate Git directory exports");
        let _ = expect_sealed(outcome);
        Self {
            _temp: temp,
            capsule: scratch.join("capsule"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The rich clone plans whole, and the plan is the frames
// ---------------------------------------------------------------------------------------------

/// The rich plan-backed clone (a dirty worktree with a file named `HEAD`, untracked and ignored files, an
/// index, an in-progress merge with rerere, an executable hook, and bridge evidence, all eight classes of
/// the union Captured) goes through V, G, and P. Every phase-P frame is planned in step order, bound to its
/// staged payload, and exactly the test's own decode of it; the root `.git` is the skipped connector; the
/// policy-excluded `target/` is absent; every frame mode is kept; nothing is external; the table charge is
/// the test's own arithmetic. Nothing is created or charged, every staged descriptor is back at offset 0,
/// and a second plan over the returned value is the same plan.
#[test]
fn c2a_the_rich_clones_real_frames_plan_whole_and_change_nothing() {
    let fixture = PlanFixtureV1::rich_clone();
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    let (_dest, planned) = PCaseV1::sealed(&fixture.capsule_dir()).planned("rich");
    let plan = &planned.plan;
    let capsule = planned.plane.capsule();
    let generation = capsule.control.manifest.generation_id().to_owned();

    // All eight classes of the union are Captured, so D5 admits a real capsule's whole union.
    let captured: Vec<ClassV1> = capsule
        .control
        .manifest
        .coverage()
        .iter()
        .filter(|row| row.state() == CustodyStateClassV1::Captured)
        .map(|row| row.class())
        .collect();
    assert_eq!(captured.len(), 8, "{captured:?}");
    assert_eq!(
        plan.classes.map(|(class, _)| class),
        P_CLASSES_V1,
        "the five classes in step order"
    );
    assert!(plan
        .classes
        .iter()
        .all(|(_, state)| matches!(state, PayloadClassStateV1::Captured { .. })));
    assert_eq!(
        plan.frames
            .iter()
            .map(|frame| frame.class)
            .collect::<Vec<_>>(),
        P_CLASSES_V1
    );
    assert_eq!(
        plan.frames
            .iter()
            .map(|frame| frame.plane)
            .collect::<Vec<_>>(),
        [
            PayloadPlaneV1::Worktree,
            PayloadPlaneV1::Index,
            PayloadPlaneV1::Evidence(P_OPERATIONS_V1),
            PayloadPlaneV1::Evidence(P_CONFIG_V1),
            PayloadPlaneV1::Evidence(P_BRIDGE_V1),
        ],
        "each class's plane"
    );

    let mut all = Vec::new();
    for (frame, (class, state)) in plan.frames.iter().zip(plan.classes) {
        assert_eq!(frame.class, class);
        assert_eq!(
            state,
            PayloadClassStateV1::Captured {
                staged: frame.staged
            }
        );
        assert_eq!(Some(frame.plane), PayloadPlaneV1::of(class));
        let staged = &capsule.staged[frame.staged];
        assert_eq!(
            staged.role,
            CustodyCapsuleArtifactRoleV1::CoveragePayload(class)
        );
        assert_eq!((frame.length, frame.sha256), (staged.length, staged.sha256));
        let own = own_decode(&staged.file, class, &generation);
        assert_eq!(
            own_entries_of(frame),
            own,
            "{class:?}: the plan is the frame"
        );
        let content: u64 = own
            .iter()
            .map(|(_, kind)| match kind {
                OwnKindV1::File(_, length) => *length,
                _ => 0,
            })
            .sum();
        assert_eq!(frame.content_bytes, content, "{class:?}");
        all.extend(own);
    }
    assert_eq!(
        plan.table_bytes,
        own_charge(all.iter().map(|(path, kind)| {
            let target = match kind {
                OwnKindV1::Link(target) => Some(target.as_slice()),
                _ => None,
            };
            (path.as_slice(), target)
        })),
        "the table charge"
    );

    let worktree = plan.frame(P_WORKTREE_V1).expect("the worktree frame");
    assert_eq!(
        plan.root_git,
        RootGitDispositionV1::ConnectorDirectorySkipped
    );
    let connector = worktree
        .entries
        .iter()
        .find(|entry| entry.path == b".git")
        .expect("the connector");
    assert_eq!(connector.action, PlannedActionV1::SkipConnector);
    assert!(matches!(
        connector.kind,
        PlannedKindV1::Directory {
            childless: true,
            ..
        }
    ));
    let names: Vec<&[u8]> = worktree
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    for name in [
        &b"HEAD"[..],
        b"README",
        b"f",
        b"untracked.txt",
        b".gitignore",
        b"build.log",
        b"Cargo.toml",
        b"Cargo.lock",
    ] {
        assert!(names.contains(&name), "{}", lossy(name));
    }
    assert!(
        !names.iter().any(|name| name.starts_with(b"target")),
        "the policy-excluded target/ is not planned"
    );
    assert!(worktree
        .entries
        .iter()
        .filter(|entry| entry.path != b".git")
        .all(|entry| entry.action == PlannedActionV1::Create));
    let own_childless: Vec<&[u8]> = worktree
        .entries
        .iter()
        .filter(|entry| {
            matches!(entry.kind, PlannedKindV1::Directory { .. })
                && entry.path != b".git"
                && !worktree.entries.iter().any(|other| {
                    other.path.len() > entry.path.len()
                        && other.path.starts_with(&entry.path)
                        && other.path[entry.path.len()] == b'/'
                })
        })
        .map(|entry| entry.path.as_slice())
        .collect();
    assert_eq!(plan.childless_directories(), own_childless);

    let index = plan.frame(P_INDEX_V1).expect("the index frame");
    assert_eq!(
        index
            .entries
            .iter()
            .map(|entry| entry.path.as_slice())
            .collect::<Vec<_>>(),
        [&b"index"[..]]
    );
    let config = plan.frame(P_CONFIG_V1).expect("the configuration frame");
    let hook = config
        .entries
        .iter()
        .find(|entry| entry.path == b"hooks/pre-commit")
        .expect("the hook");
    assert!(
        matches!(hook.kind, PlannedKindV1::Regular { mode: 0o755, .. }),
        "the frame mode is kept: {:?}",
        hook.kind
    );
    let bridge = plan.frame(P_BRIDGE_V1).expect("the bridge evidence frame");
    for name in [&b"a2a-bridge/evidence.json"[..], b"A2A_TASK.md"] {
        assert!(
            bridge.entries.iter().any(|entry| entry.path == name),
            "{}",
            lossy(name)
        );
    }
    assert!(plan.declared_external_references().is_empty());

    let first = plan.clone();
    let again = plan_payload_plane_v1(planned.plane, frame_budget(), far_deadline())
        .unwrap_or_else(|failure| panic!("a second plan: {:?}", failure.cause));
    assert_eq!(again.plan, first, "the plan is reproducible");
}

// ---------------------------------------------------------------------------------------------
// Admission: the eight-class union and the five classes' states
// ---------------------------------------------------------------------------------------------

/// Each of the five phase-P classes: Captured with its one staged payload plans it, Empty plans nothing for
/// it, and every other state or shape (ExcludedReproducible, Unresolved, Captured without a payload or with
/// two, Empty with one) refuses at admission with the class named, before the first rehash: nothing is
/// created or charged, and no step completed.
#[test]
fn c2a_each_phase_p_class_is_captured_with_one_payload_or_empty_and_nothing_else() {
    type EditV1 = fn(&mut VerifiedCapsuleV1, ClassV1);
    let capsule = frames_capsule();
    let excluded: EditV1 = |verified, class| {
        let rows = verified
            .control
            .manifest
            .coverage()
            .iter()
            .map(|row| {
                if row.class() == class {
                    CustodyCoverageEntryV1::new(
                        class,
                        CustodyStateClassV1::ExcludedReproducible,
                        vec![],
                        Some("exclusion-v1".to_owned()),
                    )
                    .expect("an excluded row")
                } else {
                    row.clone()
                }
            })
            .collect();
        verified.control.manifest = manifest_with_rows_unchecked(rows);
    };
    let captured_without: EditV1 = |verified, class| {
        verified.control.manifest =
            manifest_with_row(class, CustodyStateClassV1::Captured, verified);
    };
    let unresolved: EditV1 = |verified, class| {
        verified.control.manifest =
            manifest_with_row(class, CustodyStateClassV1::Unresolved, verified);
    };
    let empty_with: EditV1 = |verified, class| {
        stage_frame(verified, class, &p_minimal(class));
        verified.control.manifest = manifest_with_row(class, CustodyStateClassV1::Empty, verified);
    };
    let captured_twice: EditV1 = |verified, class| {
        stage_frame(verified, class, &p_minimal(class));
        let one = &verified.staged[staged_index(verified, class)];
        let twin = StagedPlaintextV1 {
            name: LosslessPathV1::from_bytes(b"test/twin.frame".to_vec()),
            role: one.role.clone(),
            file: one.file.try_clone().expect("a duplicate descriptor"),
            length: one.length,
            sha256: one.sha256,
        };
        verified.staged.push(twin);
    };
    let refusals: [(&str, EditV1, &str); 5] = [
        (
            "ExcludedReproducible",
            excluded,
            "neither Captured nor Empty",
        ),
        (
            "Captured without a payload",
            captured_without,
            "exactly one",
        ),
        ("Unresolved", unresolved, "neither Captured nor Empty"),
        ("Empty with a payload", empty_with, "Empty phase-P class"),
        ("Captured with two payloads", captured_twice, "exactly one"),
    ];
    for class in P_CLASSES_V1 {
        // Captured with its one payload.
        let row = format!("{class:?} Captured");
        let (_dest, planned) =
            PCaseV1::with_frames(&capsule, &[(class, p_minimal(class))]).planned(&row);
        let staged = staged_index(planned.plane.capsule(), class);
        for (listed, state) in planned.plan.classes {
            let want = if listed == class {
                PayloadClassStateV1::Captured { staged }
            } else {
                PayloadClassStateV1::Empty
            };
            assert_eq!(state, want, "{row}: {listed:?}");
        }
        assert_eq!(
            planned
                .plan
                .frames
                .iter()
                .map(|frame| frame.class)
                .collect::<Vec<_>>(),
            [class],
            "{row}"
        );

        for (name, edit, detail) in refusals {
            let row = format!("{class:?} {name}");
            let failure = PCaseV1::unborn(&capsule, |verified| edit(verified, class)).refused(&row);
            assert!(
                matches!(&failure.cause, PayloadPlaneErrorV1::UnsupportedCoverage { class: found, detail: why }
                    if *found == class && why.contains(detail)),
                "{row}: {:?}",
                failure.cause
            );
            assert_eq!(failure.last_completed, None, "{row}");
        }
    }

    // Every class Empty: admitted, nothing planned, no root `.git`.
    let (_dest, planned) = PCaseV1::with_frames(&capsule, &[]).planned("all Empty");
    assert!(planned.plan.frames.is_empty());
    assert!(planned
        .plan
        .classes
        .iter()
        .all(|(_, state)| *state == PayloadClassStateV1::Empty));
    assert_eq!(planned.plan.root_git, RootGitDispositionV1::Absent);
    assert_eq!(planned.plan.table_bytes, 0);
}

/// D5: a Captured class outside the eight-class union refuses at admission with that class, with its staged
/// frame and without one, before any phase-P class is examined: nothing is created or charged. (The rich
/// clone's row is the opposite branch: all eight Captured are admitted.)
#[test]
fn c2a_a_captured_class_outside_the_union_refuses_at_admission() {
    let capsule = frames_capsule();
    for class in P_OUTSIDE_UNION_V1 {
        for with_payload in [true, false] {
            let row = format!("{class:?} Captured, payload {with_payload}");
            let failure = PCaseV1::unborn(&capsule, |verified| {
                if with_payload {
                    stage_frame(verified, class, &p_frame(class, &[file("x", b"1")]));
                } else {
                    verified.control.manifest =
                        manifest_with_row(class, CustodyStateClassV1::Captured, verified);
                }
                // A broken phase-P class beside it: D5 is judged first.
                verified.control.manifest =
                    manifest_with_row(P_WORKTREE_V1, CustodyStateClassV1::Captured, verified);
            })
            .refused(&row);
            assert!(
                matches!(failure.cause, PayloadPlaneErrorV1::UnsupportedCapturedClass { class: found } if found == class),
                "{row}: {:?}",
                failure.cause
            );
            assert_eq!(failure.last_completed, None, "{row}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// D3: the worktree's `.git` connector
// ---------------------------------------------------------------------------------------------

/// D3's opposite branch on a REAL planned capsule: the coverage control's in-worktree separate Git
/// directory (`repo/.git` a gitfile naming `repo/.gd`) plus one more ordinary empty directory. The root
/// gitfile is planned to evidence and nothing is planned at `repository/.git`; both childless directories
/// (`.gd`, the connector coverage emitted, and `empty`) are planned as ordinary empty directories and both are
/// disclosed, with nothing claiming which one was a Git directory.
#[test]
fn c2a_a_root_gitfile_goes_to_evidence_and_every_childless_directory_is_disclosed() {
    let source = SeparateGitDirV1::export();
    let (_dest, planned) = PCaseV1::sealed(&source.capsule).planned("separate git dir");
    let plan = &planned.plan;
    assert_eq!(plan.root_git, RootGitDispositionV1::RootGitfileToEvidence);
    let worktree = plan.frame(P_WORKTREE_V1).expect("the worktree frame");
    let gitfile = worktree
        .entries
        .iter()
        .find(|entry| entry.path == b".git")
        .expect("the root gitfile");
    assert_eq!(gitfile.action, PlannedActionV1::RootGitfileToEvidence);
    assert!(matches!(gitfile.kind, PlannedKindV1::Regular { .. }));
    assert_eq!(plan.childless_directories(), [&b".gd"[..], b"empty"]);
    for name in [&b".gd"[..], b"empty"] {
        let entry = worktree
            .entries
            .iter()
            .find(|entry| entry.path == name)
            .expect("the childless directory");
        assert_eq!(entry.action, PlannedActionV1::Create, "{}", lossy(name));
    }
    assert!(
        !worktree
            .entries
            .iter()
            .any(|entry| entry.path.starts_with(b".gd/")),
        "the separate Git directory's contents are never planned in the worktree"
    );
}

/// D3's other rows on hand frames: a lone childless `.git` directory is the skipped connector; a root
/// gitfile goes to evidence; no root `.git` (or an Empty worktree) is `Absent`; a childless `.git` that is
/// not at the root (a separate Git directory coverage named `.git` beneath a subdirectory) is an ordinary
/// empty directory. A `.git` symlink, or a `.git` directory with any entry beneath it, refuses as an invalid
/// frame before anything is created; the refusal is held until the frame is whole.
#[test]
fn c2a_the_git_connector_is_skipped_or_sent_to_evidence_and_anything_else_refuses() {
    let capsule = frames_capsule();
    type AcceptedV1<'a> = (&'a str, Vec<u8>, RootGitDispositionV1, Vec<&'a [u8]>);
    let accepted: [AcceptedV1<'_>; 4] = [
        (
            "a childless .git directory",
            p_frame(P_WORKTREE_V1, &[dir(".git"), file("a", b"1")]),
            RootGitDispositionV1::ConnectorDirectorySkipped,
            vec![],
        ),
        (
            "a root gitfile",
            p_frame(P_WORKTREE_V1, &[file(".git", b"gitdir: ../elsewhere\n")]),
            RootGitDispositionV1::RootGitfileToEvidence,
            vec![],
        ),
        (
            "no root .git",
            p_frame(P_WORKTREE_V1, &[file("a", b"1")]),
            RootGitDispositionV1::Absent,
            vec![],
        ),
        (
            "a childless .git beneath a subdirectory",
            p_frame(P_WORKTREE_V1, &[dir("sub"), dir("sub/.git")]),
            RootGitDispositionV1::Absent,
            vec![&b"sub/.git"[..]],
        ),
    ];
    for (row, bytes, disposition, childless) in accepted {
        let (_dest, planned) =
            PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, bytes)]).planned(row);
        assert_eq!(planned.plan.root_git, disposition, "{row}");
        assert_eq!(planned.plan.childless_directories(), childless, "{row}");
    }

    let refused: [(&str, Vec<u8>, PayloadFrameFaultV1); 3] = [
        (
            "a .git symlink",
            p_frame(
                P_WORKTREE_V1,
                &[link(".git", b"elsewhere"), file("z", b"1")],
            ),
            PayloadFrameFaultV1::GitConnectorSymlink,
        ),
        (
            "a .git directory with a file",
            p_frame(
                P_WORKTREE_V1,
                &[
                    dir(".git"),
                    file(".git/config", b"[core]\n"),
                    file("z", b"1"),
                ],
            ),
            PayloadFrameFaultV1::BeneathGitConnector {
                path: ".git/config".into(),
            },
        ),
        (
            "a .git directory with a directory",
            p_frame(P_WORKTREE_V1, &[dir(".git"), dir(".git/hooks")]),
            PayloadFrameFaultV1::BeneathGitConnector {
                path: ".git/hooks".into(),
            },
        ),
    ];
    for (row, bytes, fault) in refused {
        let failure = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, bytes)]).refused(row);
        assert_eq!(frame_fault(&failure), (P_WORKTREE_V1, fault), "{row}");
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Admission),
            "{row}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// D6 and D7: names and targets
// ---------------------------------------------------------------------------------------------

/// D7: a target is external when it is absolute or when resolving it lexically against its link's parent
/// ever leaves the plane's root (`repository/` for the worktree, the class directory for evidence); every
/// other target is internal, including `dir/link -> ../inside`. Each external link is declared with its
/// exact plane and class, its path, and its target bytes (a non-UTF-8 target kept exactly), in plan order;
/// every target is kept verbatim either way.
#[test]
fn c2a_external_symlink_targets_are_declared_with_their_exact_plane() {
    let capsule = frames_capsule();
    let worktree = p_frame(
        P_WORKTREE_V1,
        &[
            link("abs", b"/etc/passwd"),
            dir("dir"),
            link("dir/away-and-back", b"../../repository/dir"),
            link("dir/deep", b"../../x"),
            link("dir/dot", b"./inside/../inside"),
            file("dir/inside", b"x"),
            link("dir/link", b"../inside"),
            link("dir/root", b".."),
            link("dir/up", b"../dir/inside"),
            link("escape", b"../outside"),
        ],
    );
    let config = p_frame(
        P_CONFIG_V1,
        &[
            link("esc", b"../bridge_evidence/x"),
            dir("hooks"),
            link("hooks/abs", b"/bin/sh"),
            link("hooks/inside", b"../config"),
            (b"hooks/raw", EntryV1::Link(b"/\xff\xfe")),
        ],
    );
    let (_dest, planned) = PCaseV1::with_frames(
        &capsule,
        &[(P_WORKTREE_V1, worktree), (P_CONFIG_V1, config)],
    )
    .planned("external targets");
    let plan = &planned.plan;
    let flags = |path: &str, external: bool| (path.as_bytes().to_vec(), external);
    assert_eq!(
        planned_links(plan.frame(P_WORKTREE_V1).expect("the worktree")),
        [
            flags("abs", true),
            flags("dir/away-and-back", true),
            flags("dir/deep", true),
            flags("dir/dot", false),
            flags("dir/link", false),
            flags("dir/root", false),
            flags("dir/up", false),
            flags("escape", true),
        ]
    );
    assert_eq!(
        planned_links(plan.frame(P_CONFIG_V1).expect("the configuration")),
        [
            flags("esc", true),
            flags("hooks/abs", true),
            flags("hooks/inside", false),
            flags("hooks/raw", true),
        ]
    );
    let declared = |plane, path: &[u8], target: &[u8]| ExternalSymlinkV1 {
        plane,
        path: path.to_vec(),
        target: target.to_vec(),
    };
    let evidence = PayloadPlaneV1::Evidence(P_CONFIG_V1);
    assert_eq!(
        plan.declared_external_references(),
        [
            declared(PayloadPlaneV1::Worktree, b"abs", b"/etc/passwd"),
            declared(
                PayloadPlaneV1::Worktree,
                b"dir/away-and-back",
                b"../../repository/dir"
            ),
            declared(PayloadPlaneV1::Worktree, b"dir/deep", b"../../x"),
            declared(PayloadPlaneV1::Worktree, b"escape", b"../outside"),
            declared(evidence, b"esc", b"../bridge_evidence/x"),
            declared(evidence, b"hooks/abs", b"/bin/sh"),
            declared(evidence, b"hooks/raw", b"/\xff\xfe"),
        ]
    );
    // Internal targets are kept verbatim too.
    let link_target = |path: &[u8]| {
        plan.frame(P_WORKTREE_V1)
            .expect("the worktree")
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| own_kind(&entry.kind))
    };
    assert_eq!(
        link_target(b"dir/link"),
        Some(OwnKindV1::Link(b"../inside".to_vec()))
    );
}

/// D6's lossless keys: names whose bytes differ but which render alike (two different invalid bytes and a
/// literal U+FFFD), as files and as directories with children, are all planned as distinct entries with their
/// exact bytes, and each child finds its own parent.
#[test]
fn c2a_byte_distinct_names_that_render_alike_stay_distinct() {
    let capsule = frames_capsule();
    let names: [&[u8]; 3] = [b"n\xff", b"n\xfe", "n\u{FFFD}".as_bytes()];
    assert!(
        names.iter().all(|name| lossy(name) == "n\u{FFFD}"),
        "the three names render alike"
    );
    let entries: [(&[u8], EntryV1<'_>); 7] = [
        (names[0], EntryV1::File(b"0")),
        (names[1], EntryV1::File(b"1")),
        (names[2], EntryV1::File(b"2")),
        (&b"d\xff"[..], EntryV1::Dir),
        (&b"d\xff/x"[..], EntryV1::File(b"a")),
        (&b"d\xfe"[..], EntryV1::Dir),
        (&b"d\xfe/x"[..], EntryV1::File(b"b")),
    ];
    let bytes = p_frame(P_WORKTREE_V1, &entries);
    let (_dest, planned) =
        PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, bytes)]).planned("lossless");
    let worktree = planned.plan.frame(P_WORKTREE_V1).expect("the worktree");
    let mut want: Vec<Vec<u8>> = entries.iter().map(|(path, _)| path.to_vec()).collect();
    let mut got: Vec<Vec<u8>> = worktree
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    want.sort();
    got.sort();
    assert_eq!(got, want, "seven exact names");
    for parent in [&b"d\xff"[..], b"d\xfe"] {
        let entry = worktree
            .entries
            .iter()
            .find(|entry| entry.path == parent)
            .expect("the directory");
        assert!(
            matches!(
                entry.kind,
                PlannedKindV1::Directory {
                    childless: false,
                    ..
                }
            ),
            "{parent:?} has its own child"
        );
    }
    assert!(planned.plan.childless_directories().is_empty());
}

/// The index frame holds only `index` and `sharedindex.*` regular files at its top level (byte-exact, case
/// included); any other name, a nested path, or another kind refuses as an invalid frame before anything is
/// created.
#[test]
fn c2a_the_index_frame_holds_only_index_and_shared_index_files() {
    let capsule = frames_capsule();
    let (_dest, planned) = PCaseV1::with_frames(
        &capsule,
        &[(
            P_INDEX_V1,
            p_frame(
                P_INDEX_V1,
                &[
                    file("index", b"DIRC"),
                    file("sharedindex.0123abcd", b"DIRC"),
                ],
            ),
        )],
    )
    .planned("index and a shared index");
    let index = planned.plan.frame(P_INDEX_V1).expect("the index frame");
    assert_eq!(
        own_entries_of(index),
        [
            (b"index".to_vec(), OwnKindV1::File(0o644, 4)),
            (b"sharedindex.0123abcd".to_vec(), OwnKindV1::File(0o644, 4)),
        ]
    );

    let name = |path: &str| PayloadFrameFaultV1::IndexName { path: path.into() };
    let kind = |path: &str| PayloadFrameFaultV1::IndexKind { path: path.into() };
    let rows: [(&str, Vec<u8>, PayloadFrameFaultV1); 5] = [
        (
            "another name",
            p_frame(P_INDEX_V1, &[file("other", b"x")]),
            name("other"),
        ),
        (
            "another case",
            p_frame(P_INDEX_V1, &[file("INDEX", b"x")]),
            name("INDEX"),
        ),
        (
            "a nested index",
            p_frame(P_INDEX_V1, &[dir("sub"), file("sub/index", b"x")]),
            name("sub"),
        ),
        (
            "a directory named index",
            p_frame(P_INDEX_V1, &[dir("index")]),
            kind("index"),
        ),
        (
            "a symlink named sharedindex.x",
            p_frame(P_INDEX_V1, &[link("sharedindex.x", b"index")]),
            kind("sharedindex.x"),
        ),
    ];
    for (row, bytes, fault) in rows {
        let failure = PCaseV1::with_frames(&capsule, &[(P_INDEX_V1, bytes)]).refused(row);
        assert_eq!(frame_fault(&failure), (P_INDEX_V1, fault), "{row}");
        assert_eq!(failure.last_completed, Some(PayloadPlaneStepV1::Admission));
    }
}

/// D6's rules in the plan table itself, which a decoder-valid frame never reaches (the decoder refuses
/// those shapes first): an empty, `.`, or `..` component; a parent that is missing or not a directory
/// entry; a path twice, in the same kind or another; an entry beneath `.git/` with no connector; a nested
/// index name. A refused entry is not admitted and charges nothing; the same path in two planes is two
/// entries; and the charge is checked arithmetic, so it refuses instead of wrapping.
#[test]
fn c2a_the_plan_table_refuses_every_path_rule_and_never_wraps() {
    use CustodyFrameComponentFaultV1::{Dot, DotDot, Empty};
    let regular = || FrameEntryKindV1::Regular {
        mode: 0o644,
        length: 1,
    };
    let directory = || FrameEntryKindV1::Directory { mode: 0o755 };
    let symlink = || FrameEntryKindV1::Symlink {
        target: b"t".to_vec(),
    };
    let mut table = PayloadPlanTableV1::new(u64::MAX);
    let component = |path: &str, kind| PayloadFrameFaultV1::InvalidComponent {
        path: path.into(),
        kind,
    };
    let worktree = PayloadPlaneV1::Worktree;
    for (path, fault) in [
        ("", component("", Empty)),
        ("a//b", component("a//b", Empty)),
        ("/a", component("/a", Empty)),
        ("a/", component("a/", Empty)),
        ("./a", component("./a", Dot)),
        ("a/..", component("a/..", DotDot)),
        (
            "missing/x",
            PayloadFrameFaultV1::ParentNotDirectory {
                path: "missing/x".into(),
            },
        ),
        (
            ".git/x",
            PayloadFrameFaultV1::BeneathGitConnector {
                path: ".git/x".into(),
            },
        ),
    ] {
        assert_eq!(
            table.admit(worktree, path.as_bytes(), regular()),
            Err(fault),
            "{path:?}"
        );
    }
    assert_eq!(
        table.admit(PayloadPlaneV1::Index, b"sub/index", regular()),
        Err(PayloadFrameFaultV1::IndexName {
            path: "sub/index".into()
        })
    );
    assert_eq!(table.retained(), 0, "a refused entry charges nothing");
    assert!(table.entries(worktree).is_empty());

    table.admit(worktree, b"f", regular()).expect("a file");
    table.admit(worktree, b"l", symlink()).expect("a symlink");
    table
        .admit(worktree, b"d", directory())
        .expect("a directory");
    let charged = table.retained();
    for (path, kind) in [("f/x", regular()), ("l/x", regular())] {
        assert_eq!(
            table.admit(worktree, path.as_bytes(), kind),
            Err(PayloadFrameFaultV1::ParentNotDirectory { path: path.into() }),
            "{path}"
        );
    }
    for kind in [directory(), regular(), symlink()] {
        assert_eq!(
            table.admit(worktree, b"d", kind),
            Err(PayloadFrameFaultV1::Duplicate { path: "d".into() })
        );
    }
    assert_eq!(table.retained(), charged);
    assert_eq!(table.entries(worktree).len(), 3);
    // One path in two planes is two entries.
    table
        .admit(PayloadPlaneV1::Evidence(P_CONFIG_V1), b"f", regular())
        .expect("another plane's f");
    table.admit(worktree, b"d/x", regular()).expect("a child");
    assert!(matches!(
        table.entries(worktree)[2].kind,
        PlannedKindV1::Directory {
            childless: false,
            ..
        }
    ));

    // Checked arithmetic: an unbounded table near the top of the range refuses instead of wrapping.
    let mut full = PayloadPlanTableV1::retaining_for_test(u64::MAX, u64::MAX - 10);
    assert_eq!(
        full.admit(worktree, b"a", regular()),
        Err(PayloadFrameFaultV1::PlanTableBound { bound: u64::MAX })
    );
    assert_eq!(full.retained(), u64::MAX - 10);
}

// ---------------------------------------------------------------------------------------------
// The table bound
// ---------------------------------------------------------------------------------------------

/// The plan table at exactly its bound plans, with the test's own charge, and one byte under it refuses as
/// the typed table bound before anything is created, for a path-dominated frame (long names, no link) and a
/// target-dominated one (short names, long link targets). Each charged term (two copies of the path, the
/// target, the 64-byte allowance) changes one of the two results if it changes.
#[test]
fn c2a_the_plan_table_bound_is_exact_for_paths_and_for_targets() {
    let capsule = frames_capsule();
    let long_names: Vec<Vec<u8>> = (0..8)
        .map(|at| format!("d/{}{at}", "p".repeat(200)).into_bytes())
        .collect();
    let mut path_dominated: Vec<(&[u8], EntryV1<'_>)> = vec![(&b"d"[..], EntryV1::Dir)];
    path_dominated.extend(
        long_names
            .iter()
            .map(|name| (name.as_slice(), EntryV1::File(b"1"))),
    );
    let long_a = vec![b'a'; 4000];
    let long_b = vec![b'b'; 3000];
    let target_dominated: Vec<(&[u8], EntryV1<'_>)> = vec![
        (&b"l1"[..], EntryV1::Link(&long_a)),
        (&b"l2"[..], EntryV1::Link(&long_b)),
        (&b"x"[..], EntryV1::File(b"1")),
    ];
    for (row, entries) in [
        ("path-dominated", path_dominated),
        ("target-dominated", target_dominated),
    ] {
        let charge = hand_charge(&entries);
        let bytes = p_frame(P_WORKTREE_V1, &entries);
        let (_dest, outcome) = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, bytes.clone())])
            .run(row, far_deadline(), charge);
        let planned =
            outcome.unwrap_or_else(|failure| panic!("{row}: fits exactly: {:?}", failure.cause));
        assert_eq!(planned.plan.table_bytes, charge, "{row}");

        let failure = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, bytes)]).refused_within(
            row,
            far_deadline(),
            charge - 1,
        );
        assert_eq!(
            frame_fault(&failure),
            (
                P_WORKTREE_V1,
                PayloadFrameFaultV1::PlanTableBound { bound: charge - 1 }
            ),
            "{row}: one byte over"
        );
        assert_eq!(failure.last_completed, Some(PayloadPlaneStepV1::Admission));
    }
}

// ---------------------------------------------------------------------------------------------
// The consumed-byte binding and the decode
// ---------------------------------------------------------------------------------------------

/// The prevalidation pass's own hook replaces the staged frame in place, after its rehash and before its
/// decode, with another VALID frame of exactly the same length: the decode reaches a verified trailer, so
/// only the binding of the consumed bytes to the staged digest can tell, and it refuses
/// `StagedInputChanged`. The replacement planned on its own is accepted, so the binding is the guard. A
/// replacement after phase G and before phase P (a changed byte, an appended byte, a truncation) is refused
/// by admission's rehash, before any step completes.
#[test]
fn c2a_a_frame_replaced_during_the_prevalidation_pass_refuses_as_changed() {
    let capsule = frames_capsule();
    let original = p_frame(P_WORKTREE_V1, &[file("a", b"1111")]);
    let replacement = p_frame(P_WORKTREE_V1, &[file("a", b"2222")]);
    assert_eq!(original.len(), replacement.len());
    assert_ne!(original, replacement);

    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, original.clone())]);
    let handle = case.staged_handle(P_WORKTREE_V1);
    let other = replacement.clone();
    let swapped = StdRc::new(StdCell::new(false));
    let seen = swapped.clone();
    let hook =
        install_payload_hook_for_test(PayloadHookPointV1::PrevalidationRehashed, move |event| {
            if event.class == Some(P_WORKTREE_V1) && !seen.replace(true) {
                handle
                    .write_all_at(&other, 0)
                    .expect("another frame in place");
            }
        });
    let failure = case.refused("replaced during prevalidation");
    assert!(swapped.get(), "the hook ran");
    assert!(
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::StagedInputChanged { .. }
        ),
        "{:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, Some(PayloadPlaneStepV1::Admission));
    drop(hook);

    let (_dest, planned) = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, replacement)])
        .planned("the replacement on its own");
    assert_eq!(planned.plan.frames.len(), 1);

    // Grown after admission and before prevalidation's own rehash: that rehash refuses it as changed (a
    // decode alone would meet a trailing byte instead).
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, original.clone())]);
    let handle = case.staged_handle(P_WORKTREE_V1);
    let length = original.len() as u64;
    let grow = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "payload-frame" {
            handle.write_all_at(b"x", length).expect("one more byte");
        }
    });
    let failure = case.refused("grown before prevalidation's rehash");
    assert!(
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::StagedInputChanged { .. }
        ),
        "{:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, Some(PayloadPlaneStepV1::Admission));
    drop(grow);

    type ChangeV1 = fn(&std::fs::File, &[u8]);
    let changes: [(&str, ChangeV1); 3] = [
        ("a changed byte", |file, bytes| {
            file.write_all_at(&[!bytes[20]], 20).expect("one byte");
        }),
        ("an appended byte", |file, bytes| {
            file.write_all_at(b"x", bytes.len() as u64)
                .expect("one more byte");
        }),
        ("a truncation", |file, bytes| {
            file.set_len(bytes.len() as u64 - 1)
                .expect("one byte fewer");
        }),
    ];
    for (row, change) in changes {
        let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, original.clone())]);
        change(&case.staged_handle(P_WORKTREE_V1), &original);
        let failure = case.refused(row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::StagedInputChanged { .. }
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, None, "{row}: admission's rehash");
    }
}

/// An entry refusal is held until the frame is whole and its bytes are bound: a same-length replacement whose
/// own entry would be refused (an index name) is refused as CHANGED, never as what the changed bytes say.
#[test]
fn c2a_a_held_entry_refusal_yields_to_the_byte_binding() {
    let capsule = frames_capsule();
    let original = p_frame(P_INDEX_V1, &[file("index", b"DIRC0001")]);
    let refusable = p_frame(P_INDEX_V1, &[file("indez", b"DIRC0001")]);
    assert_eq!(original.len(), refusable.len());

    // On its own, the refusable frame is refused for its name.
    let failure =
        PCaseV1::with_frames(&capsule, &[(P_INDEX_V1, refusable.clone())]).refused("refusable");
    assert_eq!(
        frame_fault(&failure),
        (
            P_INDEX_V1,
            PayloadFrameFaultV1::IndexName {
                path: "indez".into()
            }
        )
    );

    let case = PCaseV1::with_frames(&capsule, &[(P_INDEX_V1, original)]);
    let handle = case.staged_handle(P_INDEX_V1);
    let _hook =
        install_payload_hook_for_test(PayloadHookPointV1::PrevalidationRehashed, move |event| {
            if event.class == Some(P_INDEX_V1) {
                handle
                    .write_all_at(&refusable, 0)
                    .expect("the refusable frame in place");
            }
        });
    let failure = case.refused("held until the binding");
    assert!(
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::StagedInputChanged { .. }
        ),
        "{:?}",
        failure.cause
    );
}

/// A decode error is fatal: a frame for another generation, a truncated frame (its trailer cut), and a frame
/// of another class's header each refuse as an invalid frame of the class, after admission and before
/// anything is created.
#[test]
fn c2a_a_frame_that_does_not_decode_whole_refuses() {
    let capsule = frames_capsule();
    let whole = p_frame(P_WORKTREE_V1, &[file("a", b"1")]);
    let rows: [(&str, Vec<u8>); 3] = [
        (
            "another generation",
            encode_frame(P_WORKTREE_V1, "generation-elsewhere", &[file("a", b"1")]),
        ),
        ("a truncated trailer", whole[..whole.len() - 1].to_vec()),
        (
            "another class's header",
            p_frame(P_CONFIG_V1, &[file("a", b"1")]),
        ),
    ];
    for (row, bytes) in rows {
        let failure = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, bytes)]).refused(row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid {
                    class: P_WORKTREE_V1
                })
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, Some(PayloadPlaneStepV1::Admission));
    }
}

// ---------------------------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------------------------

/// The first gate (after admission's rehash) refuses any retained chain that changed since phase G: phase
/// G's metadata directory, an object-store directory, a restored metadata file or a copied object file
/// replaced by another inode, a bootstrap directory and an admitted bootstrap file (the object rows on a
/// Captured database), the staging and HOME directories, and the destination root itself (checked LAST).
/// Each refuses as an identity change with no step completed, and phase P creates nothing beside the drift.
#[test]
fn c2a_the_first_gate_refuses_a_changed_retained_chain() {
    type ActV1 = fn(&Path);
    let rows: [(&str, bool, ActV1); 9] = [
        ("the metadata refs directory", false, |root| {
            swap_out(root, &root.join("repository/.git/refs"));
        }),
        ("the object info directory", false, |root| {
            swap_out(root, &root.join("repository/.git/objects/info"));
        }),
        ("HEAD as another inode", false, |root| {
            let beside = root.parent().expect("the destination's area");
            replace_with_same_bytes(&root.join("repository/.git/HEAD"), beside);
        }),
        ("a copied object file as another inode", true, |root| {
            let beside = root.parent().expect("the destination's area");
            replace_with_same_bytes(&pack_file(&root.join("repository/.git")), beside);
        }),
        (
            "an admitted bootstrap file as another inode",
            true,
            |root| {
                let beside = root.parent().expect("the destination's area");
                replace_with_same_bytes(
                    &admitted_path(&root.join(".restore-work/git-bootstrap"), "pack"),
                    beside,
                );
            },
        ),
        ("a bootstrap directory", true, |root| {
            swap_out(root, &root.join(".restore-work/git-bootstrap/objects"));
        }),
        ("the plain staging directory", false, |root| {
            swap_out(root, &root.join(".restore-work/plain"));
        }),
        ("the HOME directory", false, |root| {
            swap_out(root, &root.join(".restore-work/home"));
        }),
        ("the destination root", false, |root| {
            swap_out(root, root);
        }),
    ];
    let capsule = frames_capsule();
    let captured_capsule = captured(CustodyGitObjectFormatV1::Sha1);
    for (row, captured_database, act) in rows {
        let case = if captured_database {
            PCaseV1::of(PlaneInV1::rich(
                &captured_capsule,
                CustodyGitObjectFormatV1::Sha1,
            ))
        } else {
            PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, p_minimal(P_WORKTREE_V1))])
        };
        let _hook =
            install_payload_hook_for_test(PayloadHookPointV1::AdmissionEntry, drifting(&case, act));
        let failure = case.refused(row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, None, "{row}");
    }
}

/// The mount census runs first in each gate: a mount appearing inside the destination at the first gate's
/// census refuses with no step completed, one at prevalidation's census refuses after admission, and a census
/// that cannot list the mount points fails closed. A clean run takes exactly the two censuses.
#[test]
fn c2a_a_mount_inside_the_destination_or_a_failing_census_refuses_at_its_gate() {
    let capsule = frames_capsule();
    let frames = [(P_WORKTREE_V1, p_minimal(P_WORKTREE_V1))];

    let census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec()]);
    let case = PCaseV1::with_frames(&capsule, &frames);
    let phase_g = census.calls();
    assert!(phase_g > 0, "phase G took its censuses through the seam");
    let _ = case.planned("clean");
    assert_eq!(census.calls() - phase_g, 2, "one census per gate");
    drop(census);

    for (row, at, completed) in [
        ("at admission's gate", 1, None),
        (
            "at prevalidation's gate",
            2,
            Some(PayloadPlaneStepV1::Admission),
        ),
    ] {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let inside = mount_inside(&case.dest.path);
        let _census = crate::custody_mounts::seam::install(Box::new(move |call| {
            if call == at {
                Ok(vec![b"/".to_vec(), inside.clone()])
            } else {
                Ok(vec![b"/".to_vec()])
            }
        }));
        let failure = case.refused(row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::MountBoundary(_))
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, completed, "{row}");
    }

    let case = PCaseV1::with_frames(&capsule, &frames);
    let _census = crate::custody_mounts::seam::install(Box::new(|_| {
        Err(crate::custody_mounts::CustodyMountErrorV1::MalformedLine { line: 1 })
    }));
    let failure = case.refused("a failing census");
    assert!(
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::MountBoundary(_))
        ),
        "{:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, None);
}

/// Prevalidation ends in the gate again: a retained chain changed after every frame is planned refuses as
/// an identity change with admission completed, and the plan is never returned.
#[test]
fn c2a_prevalidations_gate_refuses_a_chain_changed_after_the_plan() {
    let capsule = frames_capsule();
    type ActV1 = fn(&Path);
    let rows: [(&str, ActV1); 2] = [
        ("the metadata refs directory", |root| {
            swap_out(root, &root.join("repository/.git/refs"));
        }),
        ("the destination root", |root| {
            swap_out(root, root);
        }),
    ];
    for (row, act) in rows {
        let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, p_minimal(P_WORKTREE_V1))]);
        let _hook = install_payload_hook_for_test(
            PayloadHookPointV1::BeforePrevalidationGate,
            drifting(&case, act),
        );
        let failure = case.refused(row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Admission),
            "{row}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The one deadline
// ---------------------------------------------------------------------------------------------

/// The guards a run passes, in order, by their labels (phase G's shared guards included).
fn guard_labels(
    labels: &StdRc<StdRefCell<Vec<&'static str>>>,
) -> impl Fn(&GitHookEventV1<'_>) + 'static {
    let labels = labels.clone();
    move |event| labels.borrow_mut().push(event.label)
}

/// The guards a clean plan over `plane` with `frames` Captured phase-P frames passes, in order: the entry,
/// each frame's admission rehash (its two guards), the gate, then each frame (its own guard, its rehash, its
/// decode) and the gate again. Each gate: its own guard, one per object-store directory and copied file, per
/// metadata directory and file, per admitted bootstrap file, per staging chain, for HOME and XDG, per
/// bootstrap directory, and the root's own guard.
fn expected_guards(plane: &RestoredGitPlaneV1, frames: usize) -> Vec<&'static str> {
    let admitted = match &plane.bootstrap.objects {
        BootstrapObjectsV1::Captured { files, .. } => files.len(),
        BootstrapObjectsV1::Empty => 0,
    };
    let chains = 1 + plane.capsule().destination.staging.borrow().len() + 2;
    let mut gate = vec!["payload-gate"];
    gate.extend(std::iter::repeat_n(
        "objects-chain",
        5 + plane.objects.files.len(),
    ));
    gate.extend(std::iter::repeat_n(
        "payload-metadata-chain",
        plane.metadata.directories.len() + plane.metadata.files.len(),
    ));
    gate.extend(std::iter::repeat_n("objects-source", admitted));
    gate.extend(std::iter::repeat_n(
        "payload-chain",
        chains + plane.bootstrap.directories.len(),
    ));
    gate.push("payload-root");
    let mut guards = vec!["payload-entry"];
    for _ in 0..frames {
        guards.extend(["frame-rehash", "frame-rewind"]);
    }
    guards.extend(&gate);
    for _ in 0..frames {
        guards.extend([
            "payload-frame",
            "frame-rehash",
            "frame-rewind",
            "payload-frame-decoded",
        ]);
    }
    guards.extend(&gate);
    guards
}

/// The `nth` guard labelled `label` finds the deadline spent: the run halts there with the typed timeout,
/// never a success, with `completed` the last step completed.
fn halting_row(
    capsule: &CustomCapsuleV1,
    frames: &[(ClassV1, Vec<u8>)],
    label: &'static str,
    nth: usize,
    completed: Option<PayloadPlaneStepV1>,
) {
    let row = format!("{label} #{nth}");
    let case = PCaseV1::with_frames(capsule, frames);
    let deadline = Instant::now() + P_DEADLINE_V1;
    let count = StdCell::new(0_usize);
    let fired = StdRc::new(StdCell::new(false));
    let marked = fired.clone();
    let _guard = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == label {
            if count.get() == nth {
                marked.set(Instant::now() < deadline);
                pass_the_deadline(deadline);
            }
            count.set(count.get() + 1);
        }
    });
    let failure = case.refused_within(&row, deadline, P_TABLE_BOUND_V1);
    assert!(
        fired.get(),
        "{row}: INVALID (not behavioral evidence): the deadline passed before the guard was reached"
    );
    assert!(
        matches!(failure.cause, PayloadPlaneErrorV1::Timeout),
        "{row}: {:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, completed, "{row}");
}

/// A deadline already spent refuses at the entry guard, before anything is read: no later guard runs and
/// every staged descriptor stays where it was. A deadline that passes while one decoded entry is judged stops
/// AT that entry: no later entry is decoded. And each guard of a clean run (the entry, the first rehash, the
/// census, a chain, the root's own guard in each gate, each frame and its decode) halts the run alone with
/// the typed timeout, never a success, while brief delays at every guard leave the deadline enough headroom.
#[test]
fn c2a_the_one_deadline_is_kept_before_every_step_and_entry() {
    let capsule = frames_capsule();
    let frames = [(
        P_WORKTREE_V1,
        p_frame(
            P_WORKTREE_V1,
            &[
                file("a", b"1"),
                file("b", b"2"),
                file("c", b"3"),
                file("d", b"4"),
            ],
        ),
    )];

    // Spent at entry.
    let case = PCaseV1::with_frames(&capsule, &frames);
    let labels = StdRc::new(StdRefCell::new(Vec::new()));
    let spent = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, guard_labels(&labels));
    let failure = case.refused_within("spent", Instant::now(), P_TABLE_BOUND_V1);
    assert!(
        matches!(failure.cause, PayloadPlaneErrorV1::Timeout),
        "{:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, None);
    assert_eq!(*labels.borrow(), ["payload-entry"]);
    assert!(failure
        .partial
        .plane
        .capsule()
        .staged
        .iter()
        .all(|one| position(one) == 0));
    drop(spent);

    // Spent while one entry is judged.
    let case = PCaseV1::with_frames(&capsule, &frames);
    let deadline = Instant::now() + P_DEADLINE_V1;
    let entries = StdRc::new(StdRefCell::new(Vec::<String>::new()));
    let seen = entries.clone();
    let judged =
        install_payload_hook_for_test(PayloadHookPointV1::PrevalidationEntry, move |event| {
            seen.borrow_mut()
                .push(event.path.to_string_lossy().into_owned());
            if event.path == Path::new("b") {
                pass_the_deadline(deadline);
            }
        });
    let failure = case.refused_within("an entry", deadline, P_TABLE_BOUND_V1);
    assert!(
        matches!(failure.cause, PayloadPlaneErrorV1::Timeout),
        "{:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, Some(PayloadPlaneStepV1::Admission));
    assert_eq!(
        *entries.borrow(),
        ["a", "b"],
        "no entry after b was decoded"
    );
    drop(judged);

    // The guards of a clean run, exactly: one before each step and each retained chain, read off the
    // plane's own chains, and the root's own guard last in each gate (only its identity follows).
    let labels = StdRc::new(StdRefCell::new(Vec::new()));
    let case = PCaseV1::with_frames(&capsule, &frames);
    let expected = expected_guards(&case.plane, 1);
    {
        let _labels =
            install_git_hook_for_test(GitHookPointV1::DeadlineObserved, guard_labels(&labels));
        let _ = case.planned("clean guards");
    }
    assert_eq!(*labels.borrow(), expected);
    // Each row's seams are thread-local, so the rows run side by side.
    std::thread::scope(|scope| {
        let handles: Vec<_> = [
            ("payload-entry", 0, None),
            ("frame-rehash", 0, None),
            ("payload-gate", 0, None),
            ("payload-chain", 0, None),
            ("payload-root", 0, None),
            ("payload-frame", 0, Some(PayloadPlaneStepV1::Admission)),
            (
                "payload-frame-decoded",
                0,
                Some(PayloadPlaneStepV1::Admission),
            ),
            ("payload-gate", 1, Some(PayloadPlaneStepV1::Admission)),
            ("payload-root", 1, Some(PayloadPlaneStepV1::Admission)),
        ]
        .into_iter()
        .map(|(label, nth, completed)| {
            let (capsule, frames) = (&capsule, &frames);
            scope.spawn(move || halting_row(capsule, frames, label, nth, completed))
        })
        .collect();
        for handle in handles {
            handle.join().expect("a halting row's thread");
        }
    });

    let case = PCaseV1::with_frames(&capsule, &frames);
    let deadline = Instant::now() + P_DEADLINE_V1;
    let _brief = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, |_| {
        std::thread::sleep(Duration::from_millis(2));
    });
    let (_dest, outcome) = case.run("brief delays", deadline, P_TABLE_BOUND_V1);
    assert!(outcome.is_ok(), "brief delays leave the deadline headroom");
}

// ---------------------------------------------------------------------------------------------
// Structural controls
// ---------------------------------------------------------------------------------------------

/// STRUCTURAL ONLY (it reads the production source, so it is evidence of shape, never of behavior; the
/// behavioral evidence is every row above, each of which proves the tree and the ledger unchanged). The
/// production half of the phase-P module (its test seams excluded) holds no write, create, chmod, link, or
/// ledger-charging vocabulary, opens no path, runs no Git child, and holds no `unsafe`; neither does this
/// control file.
#[test]
fn c2a_the_planner_holds_no_write_vocabulary_and_no_unsafe() {
    let source = include_str!("custody_restore_payload.rs");
    assert_eq!(source.matches("// Test seams").count(), 1);
    assert_eq!(source.matches("// The gate\n").count(), 1);
    let (before, rest) = source.split_once("// Test seams").expect("the seams");
    let (_seams, after) = rest.split_once("// The gate\n").expect("the gate");
    let production: String = format!("{before}{after}")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "std::fs",
        "File::open(",
        "OpenOptions",
        "read_dir(",
        "create_new",
        "create_staging_directory",
        "symlink_child",
        "symlinkat",
        "set_exact",
        "set_permissions",
        "set_len",
        "write_all",
        "write(",
        "rename",
        "remove_",
        "borrow_mut",
        ".reserve",
        ".release(",
        "ScratchLedgerV1",
        "GitRunnerV1",
        "GitRunRequestV1",
        "Command",
        "unsafe",
    ] {
        assert!(
            !production.contains(forbidden),
            "the planner's production source mentions `{forbidden}`"
        );
    }
    for (file, source) in [
        ("custody_restore_payload.rs", source),
        (
            "custody_restore_payload_tests.rs",
            include_str!("custody_restore_payload_tests.rs"),
        ),
    ] {
        assert_eq!(unsafe_sites(source), 0, "{file} holds no unsafe");
    }
}

/// Every `unsafe` block, function, impl, and trait in `source`, and every `unsafe {` a macro's tokens hold.
fn unsafe_sites(source: &str) -> usize {
    use syn::visit::Visit;
    #[derive(Default)]
    struct SitesV1(usize);
    impl<'ast> Visit<'ast> for SitesV1 {
        fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
            self.0 += 1;
            syn::visit::visit_expr_unsafe(self, node);
        }
        fn visit_signature(&mut self, node: &'ast syn::Signature) {
            self.0 += usize::from(node.unsafety.is_some());
            syn::visit::visit_signature(self, node);
        }
        fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
            self.0 += usize::from(node.unsafety.is_some());
            syn::visit::visit_item_impl(self, node);
        }
        fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
            self.0 += usize::from(node.unsafety.is_some());
            syn::visit::visit_item_trait(self, node);
        }
        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            self.0 += node.tokens.to_string().matches("unsafe {").count();
            syn::visit::visit_macro(self, node);
        }
    }
    let mut sites = SitesV1::default();
    sites.visit_file(&syn::parse_file(source).expect("parse Rust source"));
    sites.0
}
