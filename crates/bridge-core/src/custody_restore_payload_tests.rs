//! In-crate controls for ADR-0041 slice 2B3c increments C2a (phase P's admission, prevalidation, and plan)
//! and C2b (materialization: the worktree, the index, and the evidence).
//!
//! Every row runs phase P over a REAL phase-G value: a real 2B2 export carried through the real 2B3a reader
//! (phase V) and the real 2B3b wrapper (phase G, with real Git). The export is the rich plan-backed clone
//! (with a real split index for C2b), a planned in-worktree separate Git directory, or the empty custom
//! capsule with a captured unborn HEAD beside hand-encoded phase-P frames, built by the real frame encoder,
//! that stand where a sealed capsule's staged payloads would (the 2B3b `stage_frame` fixture, applied before
//! phase G runs). Planning writes nothing, so every C2a row, a refusal or a success, also proves that the
//! destination tree and the one ledger are exactly what they were; every C2b row proves that the one ledger
//! moved by exactly what phase P reports it charged and that nothing phase V or phase G made changed.
//! Mounted beneath the 2B3b controls, this reuses their fixtures without widening any of them. The mutation
//! matrices in `.git/a2a-evidence/2b3c-c2a/` and `.git/a2a-evidence/2b3c-c2b/` map each guard to the rows
//! that turn red without it.

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
    let gate = gate_guards(plane);
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

/// One gate's guards over `plane` before anything is created: its own, one per object-store directory and
/// copied file, per metadata directory and file, per admitted bootstrap file, per staging chain, for HOME
/// and XDG, per bootstrap directory, and the root's own guard.
fn gate_guards(plane: &RestoredGitPlaneV1) -> Vec<&'static str> {
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
    gate
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
/// planner's production half of the phase-P module (its test seams and the C2b materialization section,
/// which follows it, excluded) holds no write, create, chmod, link, or ledger-charging vocabulary, opens no
/// path, runs no Git child, and holds no `unsafe`; neither module file holds `unsafe` at all.
#[test]
fn c2a_the_planner_holds_no_write_vocabulary_and_no_unsafe() {
    let source = include_str!("custody_restore_payload.rs");
    assert_eq!(source.matches("// Test seams").count(), 1);
    assert_eq!(source.matches("// The gate\n").count(), 1);
    assert_eq!(source.matches(P_WRITE_MARKER_V1).count(), 1);
    let (planner, _materialization) = source.split_once(P_WRITE_MARKER_V1).expect("the marker");
    let (before, rest) = planner.split_once("// Test seams").expect("the seams");
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

// =============================================================================================
// Increment C2b: materialization (the worktree, the index, and the evidence)
//
// Every row drives `materialize_payload_plane_v1` over a REAL phase-G value (see the module doc). The
// runner below holds, on EVERY outcome, that the one ledger moved by exactly what phase P reports it
// charged, that a failure reports that ledger's figure, that every entry phase V and phase G made is
// unchanged (unless the row's own hook drifted the tree), and that every entry phase P added lies in its
// own planes: beneath `repository/` (and beneath `repository/.git` only `index` and `sharedindex.*`) or
// beneath `evidence/`. Rows that need it also hold that what phase P recorded is exactly what it added.
// =============================================================================================

/// The materialization section's header in the production source.
const P_WRITE_MARKER_V1: &str =
    "// Materialization (increment C2b): the worktree, the index, and the evidence";
/// A deadline for the C2b rows that spend it: started only once the plane exists (every fixture built and
/// phase G done), and long enough for planning and the writes before the hook on a loaded host. A row whose
/// hook is reached after it reports itself INVALID instead of passing or failing on the deadline's account.
const P_WRITE_DEADLINE_V1: Duration = Duration::from_secs(10);

type WrittenOutcomeV1 = Result<MaterializedPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>>;

/// One destination entry as the test reads it with `std::fs`, never following a link: a directory's mode,
/// a file's mode and bytes, a link's target.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DiskV1 {
    Dir(u32),
    File(u32, Vec<u8>),
    Link(Vec<u8>),
}

/// A destination tree by exact relative path bytes: never a rendering, so byte-distinct names stay apart.
type DiskTreeV1 = BTreeMap<Vec<u8>, DiskV1>;

fn disk_tree(root: &Path) -> DiskTreeV1 {
    use std::os::unix::ffi::OsStringExt as _;
    use std::os::unix::fs::PermissionsExt as _;
    let mut tree = BTreeMap::new();
    let mut frontier = vec![root.to_path_buf()];
    while let Some(directory) = frontier.pop() {
        for entry in std::fs::read_dir(&directory).expect("a destination directory") {
            let path = entry.expect("a destination entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("beneath the root")
                .as_os_str()
                .as_bytes()
                .to_vec();
            let metadata = std::fs::symlink_metadata(&path).expect("its metadata");
            let mode = metadata.permissions().mode() & 0o7777;
            let found = if metadata.file_type().is_symlink() {
                DiskV1::Link(
                    std::fs::read_link(&path)
                        .expect("a link target")
                        .into_os_string()
                        .into_vec(),
                )
            } else if metadata.is_dir() {
                frontier.push(path);
                DiskV1::Dir(mode)
            } else {
                DiskV1::File(mode, std::fs::read(&path).expect("a destination file"))
            };
            tree.insert(relative, found);
        }
    }
    tree
}

/// Every entry of `after` that `before` does not name.
fn added(before: &DiskTreeV1, after: &DiskTreeV1) -> DiskTreeV1 {
    after
        .iter()
        .filter(|(name, _)| !before.contains_key(*name))
        .map(|(name, entry)| (name.clone(), entry.clone()))
        .collect()
}

/// The one ledger's charge for `tree` by the test's own measure: one allowance per entry, and every regular
/// file's bytes (a link's target lies within its allowance).
fn disk_charge(tree: &DiskTreeV1) -> u64 {
    tree.values()
        .map(|entry| match entry {
            DiskV1::File(_, bytes) => ENTRY_V1 + bytes.len() as u64,
            DiskV1::Dir(_) | DiskV1::Link(_) => ENTRY_V1,
        })
        .sum()
}

/// `name` lies in one of phase P's planes: beneath `evidence/`, beneath `repository/` but outside its `.git`,
/// or directly beneath `repository/.git` as `index` or `sharedindex.*`.
fn in_phase_p_planes(name: &[u8]) -> bool {
    if name == b"evidence" || name.starts_with(b"evidence/") {
        return true;
    }
    match name.strip_prefix(b"repository/.git/") {
        Some(leaf) => {
            !leaf.contains(&b'/') && (leaf == b"index" || leaf.starts_with(b"sharedindex."))
        }
        None => name.starts_with(b"repository/") && name != b"repository/.git",
    }
}

/// `class`'s wire name, as serde writes it in the manifest: D1's evidence directory, by the test's own route.
fn wire_name(class: ClassV1) -> Vec<u8> {
    serde_json::to_value(class)
        .expect("a class serializes")
        .as_str()
        .expect("a class is a string")
        .as_bytes()
        .to_vec()
}

fn raw<'a>(path: &'a [u8], entry: EntryV1<'a>) -> (&'a [u8], EntryV1<'a>) {
    (path, entry)
}

/// Every entry of `class`'s staged frame with its content, by the test's own decode, which leaves the
/// descriptor at offset 0.
fn own_contents(plane: &RestoredGitPlaneV1, class: ClassV1) -> Vec<(Vec<u8>, OwnKindV1, Vec<u8>)> {
    use std::io::{Read as _, Seek as _};
    let capsule = plane.capsule();
    let generation = capsule.control.manifest.generation_id().to_owned();
    let mut handle = capsule.staged[staged_index(capsule, class)]
        .file
        .try_clone()
        .expect("a duplicate descriptor");
    handle.seek(SeekFrom::Start(0)).expect("rewind");
    let header = CustodyFrameHeaderV1::new(class, &generation).expect("a header");
    let mut entries = Vec::new();
    {
        let reader = BufReader::new(&handle);
        let mut decoder =
            CustodyFrameDecoderV1::new(reader, &header, frame_budget()).expect("a decoder");
        while let Some(entry) = decoder.next_entry().expect("a whole frame") {
            match entry {
                CustodyFrameEntryV1::Directory { path, mode } => {
                    entries.push((path.as_bytes().to_vec(), OwnKindV1::Dir(mode), Vec::new()));
                }
                CustodyFrameEntryV1::Regular {
                    path,
                    mode,
                    length,
                    mut content,
                } => {
                    let mut bytes = Vec::new();
                    content.read_to_end(&mut bytes).expect("its content");
                    entries.push((
                        path.as_bytes().to_vec(),
                        OwnKindV1::File(mode, length),
                        bytes,
                    ));
                }
                CustodyFrameEntryV1::Symlink { path, target } => {
                    entries.push((
                        path.as_bytes().to_vec(),
                        OwnKindV1::Link(target),
                        Vec::new(),
                    ));
                }
            }
        }
    }
    handle.seek(SeekFrom::Start(0)).expect("rewind");
    entries
}

/// Exactly what phase P must add over `plane`, by the test's OWN decode of every Captured phase-P frame:
/// each worktree entry beneath `repository/` (the childless `.git` connector skipped, a root gitfile at
/// `evidence/worktree_root_gitfile/gitfile`), each index file beneath `repository/.git/`, and each evidence
/// entry beneath `evidence/<the class's serde wire name>/`, with `evidence/` and a slot directory only where
/// an entry needs them. Every directory is `0700`, every file `0600` with the frame's bytes, and every link
/// its target.
fn expected_added(plane: &RestoredGitPlaneV1) -> DiskTreeV1 {
    let mut tree = BTreeMap::new();
    let captured = P_CLASSES_V1.into_iter().filter(|class| {
        plane
            .capsule()
            .staged
            .iter()
            .any(|one| one.role == CustodyCapsuleArtifactRoleV1::CoveragePayload(*class))
    });
    for class in captured {
        for (path, kind, content) in own_contents(plane, class) {
            let slot = match class {
                P_WORKTREE_V1 if path == b".git" => match kind {
                    OwnKindV1::Dir(_) => continue,
                    OwnKindV1::File(..) => Some(b"worktree_root_gitfile".to_vec()),
                    OwnKindV1::Link(_) => panic!("a .git symlink is never planned"),
                },
                P_WORKTREE_V1 | P_INDEX_V1 => None,
                _ => Some(wire_name(class)),
            };
            let name = match (&slot, class) {
                (Some(slot), P_WORKTREE_V1) => joined(&joined(b"evidence", slot), b"gitfile"),
                (Some(slot), _) => joined(&joined(b"evidence", slot), &path),
                (None, P_WORKTREE_V1) => joined(b"repository", &path),
                (None, _) => joined(b"repository/.git", &path),
            };
            if let Some(slot) = &slot {
                tree.insert(b"evidence".to_vec(), DiskV1::Dir(0o700));
                tree.insert(joined(b"evidence", slot), DiskV1::Dir(0o700));
            }
            let entry = match kind {
                OwnKindV1::Dir(_) => DiskV1::Dir(0o700),
                OwnKindV1::File(..) => DiskV1::File(0o600, content),
                OwnKindV1::Link(target) => DiskV1::Link(target),
            };
            tree.insert(name, entry);
        }
    }
    tree
}

/// One materialization's outcome, with the destination tree before it.
struct WrittenV1 {
    dest: DestV1,
    before: DiskTreeV1,
    outcome: WrittenOutcomeV1,
}

impl PCaseV1 {
    /// Materializes under `deadline` and requires what EVERY outcome keeps (see this section's head).
    fn write_within(self, row: &str, deadline: Instant) -> WrittenV1 {
        let ledger = self.plane.ledger_used();
        let before = disk_tree(&self.dest.path);
        let outcome = materialize_payload_plane_v1(self.plane, frame_budget(), deadline);
        let (plane, created) = match &outcome {
            Ok(done) => (&done.plane, &done.created),
            Err(failure) => {
                assert_eq!(
                    failure.ledger_used,
                    failure.partial.plane.ledger_used(),
                    "{row}: the failure's figure is the one ledger's"
                );
                (&failure.partial.plane, &failure.partial.created)
            }
        };
        assert_eq!(
            plane.ledger_used(),
            ledger + charge_of(created.charged),
            "{row}: the one ledger moved by exactly what phase P charged"
        );
        let after = disk_tree(&self.dest.path);
        if self.drift.borrow_mut().take().is_none() {
            for (name, entry) in &before {
                assert_eq!(
                    after.get(name),
                    Some(entry),
                    "{row}: {} is unchanged",
                    lossy(name)
                );
            }
            for name in added(&before, &after).keys() {
                assert!(
                    in_phase_p_planes(name),
                    "{row}: {} lies outside phase P's planes",
                    lossy(name)
                );
            }
        }
        WrittenV1 {
            dest: self.dest,
            before,
            outcome,
        }
    }

    fn write(self, row: &str) -> WrittenV1 {
        self.write_within(row, far_deadline())
    }
}

impl WrittenV1 {
    fn success(&self, row: &str) -> &MaterializedPayloadPlaneV1 {
        match &self.outcome {
            Ok(done) => done,
            Err(failure) => panic!(
                "{row}: must materialize: {:?} (completed {:?})",
                failure.cause, failure.last_completed
            ),
        }
    }

    fn failure(&self, row: &str) -> &PayloadPlaneRestoreFailureV1 {
        match &self.outcome {
            Ok(_) => panic!("{row}: must refuse"),
            Err(failure) => failure,
        }
    }

    fn created(&self) -> &PayloadCreatedV1 {
        match &self.outcome {
            Ok(done) => &done.created,
            Err(failure) => &failure.partial.created,
        }
    }

    fn added(&self) -> DiskTreeV1 {
        added(&self.before, &disk_tree(&self.dest.path))
    }

    /// Everything phase P recorded is on disk as recorded (a directory's pin and mode, a file's mode,
    /// bytes, length and SHA-256, a link's target, and each entry's device and inode), the file in flight
    /// holds exactly its written bytes, and nothing else was added but `unrecorded` (entries phase P created
    /// and a post-create guard refused) and `planted` (a row's own entries). The disk's own charge of
    /// everything phase P added is what phase P reports.
    fn assert_created_is_the_disk(&self, row: &str, unrecorded: &[&[u8]], planted: &[&[u8]]) {
        use std::os::unix::fs::MetadataExt as _;
        let created = self.created();
        let added = self.added();
        let identity = |name: &[u8]| {
            let metadata = std::fs::symlink_metadata(self.dest.path.join(OsStr::from_bytes(name)))
                .expect("a recorded entry exists");
            (metadata.dev(), metadata.ino())
        };
        let mut recorded = DiskTreeV1::new();
        let tree_rows = |recorded: &mut DiskTreeV1, at: TreeAtV1, tree: &CreatedTreeV1| {
            assert!(
                tree.directories.keys().all(|path| tree
                    .entries
                    .iter()
                    .any(|one| one.path == *path && one.kind == CreatedKindV1::Directory)),
                "{row}: every retained pin is a recorded directory"
            );
            for entry in &tree.entries {
                let name = at.name(&entry.path);
                assert_eq!(
                    identity(&name),
                    (entry.dev, entry.ino),
                    "{row}: {}",
                    lossy(&name)
                );
                let disk = match &entry.kind {
                    CreatedKindV1::Directory => {
                        let pin = tree.directories.get(&entry.path).expect("its retained pin");
                        assert_eq!(
                            (pin.identity().dev, pin.identity().ino),
                            (Some(entry.dev), Some(entry.ino)),
                            "{row}: {}'s pin",
                            lossy(&name)
                        );
                        DiskV1::Dir(entry.created_mode.expect("a directory's created mode"))
                    }
                    CreatedKindV1::Regular { length, sha256 } => {
                        let bytes = match added.get(&name) {
                            Some(DiskV1::File(_, bytes)) => bytes.clone(),
                            other => panic!("{row}: {} is {other:?}", lossy(&name)),
                        };
                        assert_eq!(bytes.len() as u64, *length, "{row}: {}", lossy(&name));
                        assert_eq!(sha256_bytes(&bytes), *sha256, "{row}: {}", lossy(&name));
                        DiskV1::File(entry.created_mode.expect("a file's created mode"), bytes)
                    }
                    CreatedKindV1::Symlink { target } => {
                        assert_eq!(entry.created_mode, None, "{row}");
                        DiskV1::Link(target.clone())
                    }
                };
                recorded.insert(name, disk);
            }
        };
        tree_rows(&mut recorded, TreeAtV1::Worktree, &created.worktree);
        tree_rows(&mut recorded, TreeAtV1::Index, &created.index);
        if let Some(root) = &created.evidence.root {
            assert_eq!(
                identity(b"evidence"),
                (
                    root.identity().dev.expect("a device"),
                    root.identity().ino.expect("an inode")
                ),
                "{row}: evidence/"
            );
            recorded.insert(b"evidence".to_vec(), DiskV1::Dir(0o700));
        }
        for slot in &created.evidence.slots {
            let at = TreeAtV1::Evidence(slot.slot);
            assert_eq!(
                identity(&at.prefix()),
                (
                    slot.pin.identity().dev.expect("a device"),
                    slot.pin.identity().ino.expect("an inode")
                ),
                "{row}: {}",
                lossy(&at.prefix())
            );
            recorded.insert(at.prefix(), DiskV1::Dir(0o700));
            tree_rows(&mut recorded, at, &slot.tree);
        }
        if let Some(flight) = &created.in_flight {
            match added.get(&flight.name) {
                Some(DiskV1::File(mode, bytes)) => {
                    assert_eq!(bytes.len() as u64, flight.written, "{row}: in flight");
                    recorded.insert(flight.name.clone(), DiskV1::File(*mode, bytes.clone()));
                }
                other => panic!("{row}: the file in flight is {other:?}"),
            }
        }
        let mut by_phase_p = added.clone();
        for name in planted {
            assert!(
                by_phase_p.remove(*name).is_some(),
                "{row}: {} was planted",
                lossy(name)
            );
        }
        assert_eq!(
            disk_charge(&by_phase_p),
            charge_of(created.charged),
            "{row}: the charge is the disk's own measure"
        );
        let mut on_disk = by_phase_p;
        for name in unrecorded {
            assert!(
                on_disk.remove(*name).is_some(),
                "{row}: {} was added",
                lossy(name)
            );
        }
        assert_eq!(
            on_disk, recorded,
            "{row}: what phase P recorded is what it added"
        );
    }
}

/// Runs `act` once from a hook whose event `when` selects, and records the destination tree it left.
fn drifting_when(
    case: &PCaseV1,
    when: impl Fn(&PayloadHookEventV1<'_>) -> bool + 'static,
    act: impl Fn(&Path) + 'static,
) -> impl Fn(&PayloadHookEventV1<'_>) + 'static {
    let (drift, root) = (case.drift.clone(), case.dest.path.clone());
    let done = StdCell::new(false);
    move |event| {
        if when(event) && !done.replace(true) {
            act(&root);
            *drift.borrow_mut() = Some(snapshot(&root));
        }
    }
}

/// A step gate's hook point, for that step only.
fn at_step(step: PayloadPlaneStepV1) -> impl Fn(&PayloadHookEventV1<'_>) -> bool + 'static {
    move |event| event.step == Some(step)
}

/// A frame of each phase-P step: a worktree with a directory, files, and a link; an index; and one bridge
/// evidence file.
fn three_step_frames() -> Vec<(ClassV1, Vec<u8>)> {
    vec![
        (
            P_WORKTREE_V1,
            p_frame(
                P_WORKTREE_V1,
                &[
                    file("a", b"alpha"),
                    dir("d"),
                    file("d/x", b"inside"),
                    link("l", b"a"),
                ],
            ),
        ),
        (
            P_INDEX_V1,
            p_frame(P_INDEX_V1, &[file("index", b"DIRC0001")]),
        ),
        (P_BRIDGE_V1, p_frame(P_BRIDGE_V1, &[file("e", b"evidence")])),
    ]
}

fn sharedindex_names(git: &Path) -> Vec<Vec<u8>> {
    std::fs::read_dir(git)
        .expect("the source Git directory")
        .map(|entry| entry.expect("an entry").file_name().as_bytes().to_vec())
        .filter(|name| name.starts_with(b"sharedindex."))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Byte-exact restoration
// ---------------------------------------------------------------------------------------------

/// The rich plan-backed clone, given a REAL split index (`git update-index --split-index` writes `index` and
/// one `sharedindex.<hash>`) and a worktree symlink, goes V to G to P. Exactly what the test's own decode of
/// every phase-P frame says is added, byte-exact, and nothing else: every worktree entry beneath
/// `repository/`, `index` and the shared index beneath `repository/.git/`, and each evidence class beneath
/// `evidence/<its serde wire name>/`; the connector is skipped and nothing is at
/// `evidence/worktree_root_gitfile`; `target/` is absent; the bridge's `a2a-bridge/evidence.json` and
/// `A2A_TASK.md` exist only beneath `evidence/bridge_evidence/`. Every file is `0600` and every directory
/// `0700`, the frame and final modes are recorded (the executable hook's `0755` frame mode, its `0400`
/// final mode), every recorded identity is the disk's, and the source's own bytes agree.
#[test]
fn c2b_the_rich_clone_restores_byte_exact_with_a_split_index() {
    let fixture = PlanFixtureV1::rich_clone();
    let clone = fixture
        .capsule_dir()
        .parent()
        .and_then(Path::parent)
        .expect("the fixture's area")
        .join("clone");
    assert!(
        clone.join(".git/MERGE_RR").is_file(),
        "the rich clone's source, found by its layout"
    );
    source_git(&clone, &["update-index", "--split-index"]);
    let shared = sharedindex_names(&clone.join(".git"));
    assert_eq!(shared.len(), 1, "real Git wrote one shared index");
    std::os::unix::fs::symlink("README", clone.join("readme-link")).expect("a worktree link");
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    let written = PCaseV1::sealed(&fixture.capsule_dir()).write("rich");
    let done = written.success("rich");
    assert_eq!(done.step, PayloadPlaneStepV1::Evidence);
    assert!(
        done.plane
            .capsule()
            .staged
            .iter()
            .all(|one| position(one) == 0),
        "every staged descriptor is back at offset 0"
    );
    let added = written.added();
    assert_eq!(
        added,
        expected_added(&done.plane),
        "byte-exact: what the frames say, and nothing else"
    );
    written.assert_created_is_the_disk("rich", &[], &[]);

    // The split index, byte-exact, and the source's own bytes where Git does not rewrite them.
    let index_names: Vec<&[u8]> = done
        .created
        .index
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    assert_eq!(index_names, [&b"index"[..], shared[0].as_slice()]);
    for (name, source) in [
        (
            joined(b"repository/.git", &shared[0]),
            clone.join(".git").join(OsStr::from_bytes(&shared[0])),
        ),
        (b"repository/README".to_vec(), clone.join("README")),
        (
            b"evidence/git_configuration_and_hooks/hooks/pre-commit".to_vec(),
            clone.join(".git/hooks/pre-commit"),
        ),
        (
            b"evidence/bridge_evidence/a2a-bridge/evidence.json".to_vec(),
            clone.join(".git/a2a-bridge/evidence.json"),
        ),
        (
            b"evidence/bridge_evidence/A2A_TASK.md".to_vec(),
            clone.join(".git/A2A_TASK.md"),
        ),
    ] {
        assert_eq!(
            added.get(&name),
            Some(&DiskV1::File(
                0o600,
                std::fs::read(&source).expect("the source file")
            )),
            "{} is the source's bytes",
            lossy(&name)
        );
    }
    assert_eq!(
        added.get(&b"repository/readme-link"[..]),
        Some(&DiskV1::Link(b"README".to_vec()))
    );

    // The bridge's evidence exists only beneath evidence/bridge_evidence/, anywhere in the destination.
    let everything = disk_tree(&written.dest.path);
    for leaf in [&b"evidence.json"[..], b"A2A_TASK.md"] {
        let found: Vec<&Vec<u8>> = everything
            .keys()
            .filter(|name| name.ends_with(leaf) && name[..name.len() - leaf.len()].ends_with(b"/"))
            .collect();
        assert_eq!(found.len(), 1, "{}: {found:?}", lossy(leaf));
        assert!(
            found[0].starts_with(b"evidence/bridge_evidence/"),
            "{}",
            lossy(found[0])
        );
    }
    assert!(!everything.contains_key(&b"repository/.git/a2a-bridge"[..]));
    assert!(!everything.contains_key(&b"evidence/worktree_root_gitfile"[..]));
    assert!(added
        .keys()
        .all(|name| !name.starts_with(b"repository/target")));
    assert_eq!(
        done.plan.root_git,
        RootGitDispositionV1::ConnectorDirectorySkipped
    );

    // The slots, in creation order, are the three evidence classes', by their wire names.
    assert_eq!(
        done.created
            .evidence
            .slots
            .iter()
            .map(|slot| slot.slot)
            .collect::<Vec<_>>(),
        [
            EvidenceSlotV1::InProgressGitOperations,
            EvidenceSlotV1::GitConfigurationAndHooks,
            EvidenceSlotV1::BridgeEvidence,
        ]
    );
    for (slot, class) in [
        (EvidenceSlotV1::InProgressGitOperations, P_OPERATIONS_V1),
        (EvidenceSlotV1::GitConfigurationAndHooks, P_CONFIG_V1),
        (EvidenceSlotV1::BridgeEvidence, P_BRIDGE_V1),
    ] {
        assert_eq!(slot.directory_name(), wire_name(class).as_slice());
    }

    // Each row's frame mode is the frame's; the final mode is the frame's for the worktree, 0600 for the
    // index, and 0400/0500 for evidence; a link has none.
    let frame_modes = |class: ClassV1| -> BTreeMap<Vec<u8>, Option<u32>> {
        own_contents(&done.plane, class)
            .into_iter()
            .map(|(path, kind, _)| {
                let mode = match kind {
                    OwnKindV1::Dir(mode) | OwnKindV1::File(mode, _) => Some(mode),
                    OwnKindV1::Link(_) => None,
                };
                (path, mode)
            })
            .collect()
    };
    let check = |tree: &CreatedTreeV1, class: ClassV1, at: TreeAtV1| {
        let modes = frame_modes(class);
        for entry in &tree.entries {
            assert_eq!(
                Some(&entry.frame_mode),
                modes.get(&entry.path),
                "{}",
                lossy(&entry.path)
            );
            let want = match (&entry.kind, at) {
                (CreatedKindV1::Symlink { .. }, _) => None,
                (_, TreeAtV1::Worktree) => entry.frame_mode,
                (_, TreeAtV1::Index) => Some(0o600),
                (CreatedKindV1::Directory, _) => Some(0o500),
                (_, _) => Some(0o400),
            };
            assert_eq!(entry.final_mode, want, "{}", lossy(&entry.path));
        }
    };
    check(&done.created.worktree, P_WORKTREE_V1, TreeAtV1::Worktree);
    check(&done.created.index, P_INDEX_V1, TreeAtV1::Index);
    for slot in &done.created.evidence.slots {
        let class = match slot.slot {
            EvidenceSlotV1::InProgressGitOperations => P_OPERATIONS_V1,
            EvidenceSlotV1::GitConfigurationAndHooks => P_CONFIG_V1,
            EvidenceSlotV1::BridgeEvidence => P_BRIDGE_V1,
            EvidenceSlotV1::WorktreeRootGitfile => panic!("no root gitfile"),
        };
        check(&slot.tree, class, TreeAtV1::Evidence(slot.slot));
    }
    let hook = done
        .created
        .evidence
        .slot(EvidenceSlotV1::GitConfigurationAndHooks)
        .and_then(|slot| {
            slot.tree
                .entries
                .iter()
                .find(|entry| entry.path == b"hooks/pre-commit")
        })
        .expect("the hook");
    assert_eq!(
        (hook.frame_mode, hook.created_mode, hook.final_mode),
        (Some(0o755), Some(0o600), Some(0o400)),
        "the hook is recorded executable and written inert"
    );
}

/// D3 on a REAL planned capsule: the worktree's root gitfile (`repo/.git`, naming the in-worktree separate Git
/// directory `repo/.gd`) is written to `evidence/worktree_root_gitfile/gitfile` with the source's own bytes,
/// and nothing is created at `repository/.git`, which is still phase G's directory; both childless
/// directories are restored as empty directories. On hand frames, the gitfile is written in the worktree step
/// (before its gate) beside a later evidence class.
#[test]
fn c2b_a_root_gitfile_lands_in_evidence_and_nothing_at_repository_git() {
    use std::os::unix::fs::MetadataExt as _;
    let source = SeparateGitDirV1::export();
    let repo = source
        .capsule
        .parent()
        .and_then(Path::parent)
        .expect("the fixture's area")
        .join("repo");
    let gitfile = std::fs::read(repo.join(".git")).expect("the source gitfile");
    let written = PCaseV1::sealed(&source.capsule).write("separate git dir");
    let done = written.success("separate git dir");
    let added = written.added();
    assert_eq!(added, expected_added(&done.plane));
    written.assert_created_is_the_disk("separate git dir", &[], &[]);
    assert_eq!(
        added.get(&b"evidence/worktree_root_gitfile/gitfile"[..]),
        Some(&DiskV1::File(0o600, gitfile))
    );
    let git = std::fs::symlink_metadata(written.dest.path.join("repository/.git"))
        .expect("repository/.git");
    assert!(git.is_dir(), "repository/.git is still a directory");
    assert_eq!(
        (Some(git.dev()), Some(git.ino())),
        (
            done.plane.objects.pins.git.identity().dev,
            done.plane.objects.pins.git.identity().ino
        ),
        "repository/.git is still phase G's directory"
    );
    for name in [&b"repository/.gd"[..], b"repository/empty"] {
        assert_eq!(added.get(name), Some(&DiskV1::Dir(0o700)));
        assert!(
            !added
                .keys()
                .any(|other| other.starts_with(name) && other.len() > name.len()),
            "{} stays empty",
            lossy(name)
        );
    }
    assert_eq!(
        done.plan.root_git,
        RootGitDispositionV1::RootGitfileToEvidence
    );
    let slot = done
        .created
        .evidence
        .slot(EvidenceSlotV1::WorktreeRootGitfile)
        .expect("the gitfile's slot");
    assert_eq!(
        done.created.evidence.slots[0].slot,
        EvidenceSlotV1::WorktreeRootGitfile,
        "the gitfile's slot is made in the worktree step, before any evidence class's"
    );
    let rows: Vec<_> = slot
        .tree
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.final_mode))
        .collect();
    assert_eq!(rows, [(b"gitfile".to_vec(), Some(0o400))]);

    // Hand frames: the gitfile is in evidence before the worktree step's gate, and a later class's slot
    // follows it.
    let capsule = frames_capsule();
    let case = PCaseV1::with_frames(
        &capsule,
        &[
            (
                P_WORKTREE_V1,
                p_frame(
                    P_WORKTREE_V1,
                    &[file(".git", b"gitdir: ../elsewhere\n"), file("a", b"1")],
                ),
            ),
            (P_BRIDGE_V1, p_frame(P_BRIDGE_V1, &[file("e", b"2")])),
        ],
    );
    let root = case.dest.path.clone();
    let seen = StdRc::new(StdCell::new(None));
    let saw = seen.clone();
    let _hook = install_payload_hook_for_test(PayloadHookPointV1::BeforeStepGate, move |event| {
        if event.step == Some(PayloadPlaneStepV1::Worktree) {
            saw.set(Some(
                std::fs::read(root.join("evidence/worktree_root_gitfile/gitfile")).ok()
                    == Some(b"gitdir: ../elsewhere\n".to_vec()),
            ));
        }
    });
    let written = case.write("hand gitfile");
    let done = written.success("hand gitfile");
    assert_eq!(seen.get(), Some(true), "written in the worktree step");
    assert_eq!(written.added(), expected_added(&done.plane));
    assert_eq!(
        done.created
            .evidence
            .slots
            .iter()
            .map(|slot| slot.slot)
            .collect::<Vec<_>>(),
        [
            EvidenceSlotV1::WorktreeRootGitfile,
            EvidenceSlotV1::BridgeEvidence
        ]
    );
    assert!(!written.added().contains_key(&b"repository/.git"[..]));
}

/// Symlinks in the worktree and in an evidence class are written verbatim (absolute, `..`, dangling,
/// non-UTF-8, to a directory) and never followed: the outside file they name keeps its bytes, inode, mode,
/// and times, the outside directory stays empty, and no dangling or escaping target comes into being. Each
/// is recorded with its target, no mode, and its identity; the plan declares exactly the external ones.
#[test]
fn c2b_symlinks_are_written_verbatim_and_never_followed() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let outside = tempfile::TempDir::new().expect("an outside area");
    let outside = outside.path().canonicalize().expect("a canonical area");
    let victim = outside.join("victim");
    std::fs::write(&victim, b"victim\n").expect("the victim");
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o640)).expect("its mode");
    let elsewhere = outside.join("elsewhere");
    std::fs::create_dir(&elsewhere).expect("an outside directory");
    let observe = |path: &Path| {
        let metadata = std::fs::symlink_metadata(path).expect("an outside entry");
        (
            metadata.ino(),
            metadata.mode(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    let victim_before = observe(&victim);
    let victim_target = victim.as_os_str().as_bytes().to_vec();
    let elsewhere_target = elsewhere.as_os_str().as_bytes().to_vec();
    let worktree = [
        raw(b"README", EntryV1::File(b"readme\n")),
        raw(b"abs", EntryV1::Link(&victim_target)),
        raw(b"d", EntryV1::Dir),
        raw(b"d/up", EntryV1::Link(b"../README")),
        raw(b"dangling", EntryV1::Link(b"no-such-name")),
        raw(b"escape", EntryV1::Link(b"../escaped")),
        raw(b"raw", EntryV1::Link(b"\xff\xfe/x")),
        raw(b"to-dir", EntryV1::Link(&elsewhere_target)),
    ];
    let config = [
        raw(b"config", EntryV1::File(b"[core]\n")),
        raw(b"hooks", EntryV1::Dir),
        raw(b"hooks/pre-commit", EntryV1::Link(&victim_target)),
        raw(b"hooks/up", EntryV1::Link(b"../config")),
    ];
    let capsule = frames_capsule();
    let written = PCaseV1::with_frames(
        &capsule,
        &[
            (P_WORKTREE_V1, p_frame(P_WORKTREE_V1, &worktree)),
            (P_CONFIG_V1, p_frame(P_CONFIG_V1, &config)),
        ],
    )
    .write("links");
    let done = written.success("links");
    written.assert_created_is_the_disk("links", &[], &[]);
    let added = written.added();
    assert_eq!(added, expected_added(&done.plane));
    for (name, target) in [
        (&b"repository/abs"[..], victim_target.as_slice()),
        (b"repository/d/up", b"../README"),
        (b"repository/dangling", b"no-such-name"),
        (b"repository/escape", b"../escaped"),
        (b"repository/raw", b"\xff\xfe/x"),
        (b"repository/to-dir", elsewhere_target.as_slice()),
        (
            b"evidence/git_configuration_and_hooks/hooks/pre-commit",
            victim_target.as_slice(),
        ),
        (
            b"evidence/git_configuration_and_hooks/hooks/up",
            b"../config",
        ),
    ] {
        assert_eq!(
            added.get(name),
            Some(&DiskV1::Link(target.to_vec())),
            "{}",
            lossy(name)
        );
    }
    assert_eq!(
        observe(&victim),
        victim_before,
        "the outside file is untouched"
    );
    assert_eq!(std::fs::read(&victim).expect("the victim"), b"victim\n");
    assert_eq!(
        std::fs::read_dir(&elsewhere)
            .expect("the outside directory")
            .count(),
        0
    );
    let everything = disk_tree(&written.dest.path);
    for absent in [
        &b"repository/no-such-name"[..],
        b"escaped",
        b"repository/\xff\xfe",
    ] {
        assert!(!everything.contains_key(absent), "{}", lossy(absent));
    }
    for entry in done
        .created
        .worktree
        .entries
        .iter()
        .chain(&done.created.evidence.slots[0].tree.entries)
    {
        if let CreatedKindV1::Symlink { .. } = entry.kind {
            assert_eq!(
                (entry.frame_mode, entry.created_mode, entry.final_mode),
                (None, None, None),
                "{}",
                lossy(&entry.path)
            );
        }
    }
    let external: Vec<(PayloadPlaneV1, Vec<u8>)> = done
        .plan
        .declared_external_references()
        .into_iter()
        .map(|reference| (reference.plane, reference.path))
        .collect();
    assert_eq!(
        external,
        [
            (PayloadPlaneV1::Worktree, b"abs".to_vec()),
            (PayloadPlaneV1::Worktree, b"escape".to_vec()),
            (PayloadPlaneV1::Worktree, b"to-dir".to_vec()),
            (
                PayloadPlaneV1::Evidence(P_CONFIG_V1),
                b"hooks/pre-commit".to_vec()
            ),
        ]
    );
}

/// A symlink target the frame admits but the destination file system cannot hold (4095 bytes: macOS refuses
/// it, Linux holds it; probed at run time) refuses typed, with nothing created at its name and its allowance
/// given back; where the destination holds it, it is restored byte-exact.
#[test]
fn c2b_a_symlink_the_destination_cannot_hold_refuses_typed() {
    let capsule = frames_capsule();
    let long = vec![b'x'; 4095];
    let frame = p_frame(
        P_WORKTREE_V1,
        &[
            raw(b"a", EntryV1::File(b"1")),
            raw(b"long", EntryV1::Link(&long)),
        ],
    );
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, frame)]);
    let probe = tempfile::TempDir::new_in(case.dest.path.parent().expect("the area"))
        .expect("a probe beside the destination");
    let holds =
        std::os::unix::fs::symlink(OsStr::from_bytes(&long), probe.path().join("p")).is_ok();
    println!("PAYLOAD-LONG-SYMLINK target_len=4095 holds={holds}");
    let written = case.write("a 4095-byte target");
    if holds {
        let _ = written.success("a 4095-byte target");
        assert_eq!(
            written.added().get(&b"repository/long"[..]),
            Some(&DiskV1::Link(long.clone()))
        );
    } else {
        let failure = written.failure("a 4095-byte target");
        assert!(
            matches!(&failure.cause, PayloadPlaneErrorV1::DestinationUnsupported { name, target_len: 4095 } if name == "repository/long"),
            "{:?}",
            failure.cause
        );
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Prevalidation)
        );
        assert!(failure.payload_materialization_began);
        assert!(!written.added().contains_key(&b"repository/long"[..]));
    }
    written.assert_created_is_the_disk("a 4095-byte target", &[], &[]);
}

// ---------------------------------------------------------------------------------------------
// The one ledger
// ---------------------------------------------------------------------------------------------

/// The one ledger at exactly phase P's charge materializes and ends full; one byte less refuses as the
/// typed budget refusal at the last reservation, with everything before it kept and charged: for a
/// content-dominated frame the last chunk (its file exists, empty, in flight), and for an entry-dominated one
/// the last entry (nothing at its name). A ledger with no room refuses the first entry before anything
/// exists.
#[test]
fn c2b_the_ledger_exactly_fits_and_one_byte_over_refuses() {
    let capsule = frames_capsule();
    let a = vec![b'a'; 300];
    let z = vec![b'z'; 500];
    let content = p_frame(
        P_WORKTREE_V1,
        &[
            raw(b"d", EntryV1::Dir),
            raw(b"d/a", EntryV1::File(&a)),
            raw(b"z", EntryV1::File(&z)),
        ],
    );
    let entries = p_frame(P_WORKTREE_V1, &[file("a", b"x"), link("l", b"a")]);
    let budget_refused = |failure: &PayloadPlaneRestoreFailureV1| {
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::Budget(_))
        )
    };
    for (row, frame, charge, last) in [
        ("content-dominated", &content, 3 * ENTRY_V1 + 800, 500),
        ("entry-dominated", &entries, 2 * ENTRY_V1 + 1, ENTRY_V1),
    ] {
        let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, frame.clone())]);
        leave_headroom(case.plane.capsule(), charge);
        let written = case.write(row);
        let done = written.success(row);
        assert_eq!(
            done.plane.ledger_used(),
            RESTORE_BYTES_CEILING_V1,
            "{row}: full"
        );
        assert_eq!(charge_of(done.created.charged), charge, "{row}");
        written.assert_created_is_the_disk(row, &[], &[]);

        let row = format!("{row}: one byte over");
        let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, frame.clone())]);
        leave_headroom(case.plane.capsule(), charge - 1);
        let written = case.write(&row);
        let failure = written.failure(&row);
        assert!(budget_refused(failure), "{row}: {:?}", failure.cause);
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Prevalidation)
        );
        assert!(failure.payload_materialization_began);
        assert_eq!(
            failure.ledger_used,
            RESTORE_BYTES_CEILING_V1 + 1 - last,
            "{row}: all but the last reservation"
        );
        assert_eq!(charge_of(failure.partial.created.charged), charge - last);
        written.assert_created_is_the_disk(&row, &[], &[]);
    }
    // The content-dominated row's last file exists, empty, in flight; the entry-dominated row's link does not.
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, content)]);
    leave_headroom(case.plane.capsule(), 3 * ENTRY_V1 + 799);
    let written = case.write("the last chunk");
    assert_eq!(
        written.created().in_flight,
        Some(InFlightPayloadFileV1 {
            name: b"repository/z".to_vec(),
            expected_length: 500,
            written: 0
        })
    );
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, entries.clone())]);
    leave_headroom(case.plane.capsule(), 2 * ENTRY_V1);
    let written = case.write("the last entry");
    assert!(!written.added().contains_key(&b"repository/l"[..]));
    assert_eq!(written.created().in_flight, None);

    // No room at all: the first entry is refused before anything exists, and materialization never began.
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, entries)]);
    leave_headroom(case.plane.capsule(), 0);
    let written = case.write("no room");
    let failure = written.failure("no room");
    assert!(budget_refused(failure), "{:?}", failure.cause);
    assert!(!failure.payload_materialization_began);
    assert!(written.added().is_empty());
}

/// A short write followed by a failed (or a zero-length) write keeps exactly the accepted bytes on disk and
/// charged, gives back exactly the unwritten part of the chunk, and records the file in flight with them;
/// short writes alone, with headroom, write the whole file and charge exactly its bytes.
#[test]
fn c2b_a_short_write_then_an_error_charges_exactly_the_accepted_bytes() {
    let capsule = frames_capsule();
    let frame = p_frame(P_WORKTREE_V1, &[file("f", b"0123456789")]);
    for (row, fault) in [
        ("a short write, then an error", ObjectFaultV1::WriteAfter(6)),
        (
            "a short write, then a zero write",
            ObjectFaultV1::WriteZeroAfter(6),
        ),
    ] {
        let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, frame.clone())]);
        let _chunk = override_object_copy_chunk_for_test(4);
        let _fault = override_object_fault_for_test(fault);
        let written = case.write(row);
        let failure = written.failure(row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::Io(_))
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Prevalidation)
        );
        assert!(failure.payload_materialization_began);
        assert_eq!(
            failure.partial.created.in_flight,
            Some(InFlightPayloadFileV1 {
                name: b"repository/f".to_vec(),
                expected_length: 10,
                written: 6
            }),
            "{row}"
        );
        assert_eq!(
            (
                failure.partial.created.charged.entries,
                failure.partial.created.charged.bytes
            ),
            (1, 6),
            "{row}: exactly the accepted bytes"
        );
        assert_eq!(
            written.added().get(&b"repository/f"[..]),
            Some(&DiskV1::File(0o600, b"012345".to_vec()))
        );
        written.assert_created_is_the_disk(row, &[], &[]);
    }
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, frame)]);
    let _chunk = override_object_copy_chunk_for_test(4);
    let _fault = override_object_fault_for_test(ObjectFaultV1::ShortWrites(3));
    let written = case.write("short writes alone");
    let done = written.success("short writes alone");
    assert_eq!(
        (done.created.charged.entries, done.created.charged.bytes),
        (1, 10)
    );
    assert_eq!(
        written.added().get(&b"repository/f"[..]),
        Some(&DiskV1::File(0o600, b"0123456789".to_vec()))
    );
}

// ---------------------------------------------------------------------------------------------
// The one deadline
// ---------------------------------------------------------------------------------------------

/// Whether a hook that spends the deadline was reached before it: `None` until it fires.
type FiredV1 = StdRc<StdCell<Option<bool>>>;

/// Sleeps past `deadline` the first time `when` holds, recording whether that was before it.
fn spend(deadline: Instant, fired: &FiredV1) -> impl Fn() + 'static {
    let fired = fired.clone();
    move || {
        if fired.get().is_none() {
            fired.set(Some(Instant::now() < deadline));
            pass_the_deadline(deadline);
        }
    }
}

/// The deadline expires at one point of materialization: the run halts there with the typed timeout and the
/// right last completed step, never a success, keeping exactly what was created before: at an entry in the
/// middle of the worktree frame (the entries before it exist, none after), between two chunks of one file
/// (its first chunk on disk and charged, in flight), before the index frame's decode, before an evidence
/// frame's decode, and inside the worktree's and the evidence's step gates. Brief delays at every guard
/// leave the deadline headroom. Each row's deadline starts after its plane exists, and each row asserts its
/// hook fired before it (INVALID otherwise).
#[test]
fn c2b_the_deadline_expiring_mid_frame_or_mid_step_halts_with_the_partial_kept() {
    #[derive(Clone, Copy, Debug)]
    enum SpendV1 {
        Entry(&'static [u8]),
        Chunk(usize),
        Rehashed(ClassV1),
        StepGate(PayloadPlaneStepV1),
    }
    let capsule = frames_capsule();
    let frames = vec![
        (
            P_WORKTREE_V1,
            p_frame(
                P_WORKTREE_V1,
                &[
                    file("a", b"1"),
                    file("b", b"2"),
                    file("big", b"0123456789"),
                    file("c", b"3"),
                    file("d", b"4"),
                ],
            ),
        ),
        (
            P_INDEX_V1,
            p_frame(P_INDEX_V1, &[file("index", b"DIRC0001")]),
        ),
        (P_BRIDGE_V1, p_frame(P_BRIDGE_V1, &[file("e", b"evidence")])),
    ];
    type RowV1 = (
        SpendV1,
        Option<PayloadPlaneStepV1>,
        &'static [&'static [u8]],
    );
    let rows: [RowV1; 6] = [
        (
            SpendV1::Entry(b"c"),
            Some(PayloadPlaneStepV1::Prevalidation),
            &[b"repository/a", b"repository/b", b"repository/big"],
        ),
        (
            SpendV1::Chunk(1),
            Some(PayloadPlaneStepV1::Prevalidation),
            &[b"repository/a", b"repository/b"],
        ),
        (
            SpendV1::Rehashed(P_INDEX_V1),
            Some(PayloadPlaneStepV1::Worktree),
            &[
                b"repository/a",
                b"repository/b",
                b"repository/big",
                b"repository/c",
                b"repository/d",
            ],
        ),
        (
            SpendV1::Rehashed(P_BRIDGE_V1),
            Some(PayloadPlaneStepV1::Index),
            &[
                b"repository/a",
                b"repository/b",
                b"repository/big",
                b"repository/c",
                b"repository/d",
                b"repository/.git/index",
            ],
        ),
        (
            SpendV1::StepGate(PayloadPlaneStepV1::Worktree),
            Some(PayloadPlaneStepV1::Prevalidation),
            &[
                b"repository/a",
                b"repository/b",
                b"repository/big",
                b"repository/c",
                b"repository/d",
            ],
        ),
        (
            SpendV1::StepGate(PayloadPlaneStepV1::Evidence),
            Some(PayloadPlaneStepV1::Index),
            &[
                b"repository/a",
                b"repository/b",
                b"repository/big",
                b"repository/c",
                b"repository/d",
                b"repository/.git/index",
                b"evidence",
                b"evidence/bridge_evidence",
                b"evidence/bridge_evidence/e",
            ],
        ),
    ];
    // Each row's seams are thread-local, so the rows run side by side.
    std::thread::scope(|scope| {
        let handles: Vec<_> = rows
            .into_iter()
            .map(|(spent, completed, kept)| {
                let (capsule, frames) = (&capsule, &frames);
                scope.spawn(move || {
                    let row = format!("{spent:?}");
                    let case = PCaseV1::with_frames(capsule, frames);
                    let _chunk = override_object_copy_chunk_for_test(4);
                    let deadline = Instant::now() + P_WRITE_DEADLINE_V1;
                    let fired: FiredV1 = StdRc::new(StdCell::new(None));
                    let act = spend(deadline, &fired);
                    let _guard: Box<dyn std::any::Any> = match spent {
                        SpendV1::Entry(path) => Box::new(install_payload_hook_for_test(
                            PayloadHookPointV1::MaterializationEntry,
                            move |event| {
                                if event.path == Path::new(OsStr::from_bytes(path)) {
                                    act();
                                }
                            },
                        )),
                        SpendV1::Chunk(nth) => {
                            let seen = StdCell::new(0_usize);
                            Box::new(install_git_hook_for_test(
                                GitHookPointV1::BeforeObjectChunk,
                                move |event| {
                                    if event.path.ends_with("big") {
                                        if seen.get() == nth {
                                            act();
                                        }
                                        seen.set(seen.get() + 1);
                                    }
                                },
                            ))
                        }
                        SpendV1::Rehashed(class) => Box::new(install_payload_hook_for_test(
                            PayloadHookPointV1::MaterializationRehashed,
                            move |event| {
                                if event.class == Some(class) {
                                    act();
                                }
                            },
                        )),
                        SpendV1::StepGate(step) => Box::new(install_payload_hook_for_test(
                            PayloadHookPointV1::BeforeStepGate,
                            move |event| {
                                if event.step == Some(step) {
                                    act();
                                }
                            },
                        )),
                    };
                    let written = case.write_within(&row, deadline);
                    assert_eq!(
                        fired.get(),
                        Some(true),
                        "{row}: INVALID (not behavioral evidence): the hook was not reached before the deadline"
                    );
                    let failure = written.failure(&row);
                    assert!(
                        matches!(failure.cause, PayloadPlaneErrorV1::Timeout),
                        "{row}: {:?}",
                        failure.cause
                    );
                    assert_eq!(failure.last_completed, completed, "{row}");
                    let mut kept: Vec<Vec<u8>> = kept.iter().map(|name| name.to_vec()).collect();
                    if let SpendV1::Chunk(_) = spent {
                        kept.push(b"repository/big".to_vec());
                        assert_eq!(
                            failure.partial.created.in_flight,
                            Some(InFlightPayloadFileV1 {
                                name: b"repository/big".to_vec(),
                                expected_length: 10,
                                written: 4
                            }),
                            "{row}: one chunk on disk"
                        );
                    }
                    kept.sort();
                    let added: Vec<Vec<u8>> = written.added().into_keys().collect();
                    assert_eq!(added, kept, "{row}: exactly what was created before");
                    written.assert_created_is_the_disk(&row, &[], &[]);
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("a deadline row's thread");
        }
    });

    let case = PCaseV1::with_frames(&capsule, &frames);
    let deadline = Instant::now() + P_WRITE_DEADLINE_V1;
    let _brief = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, |_| {
        std::thread::sleep(Duration::from_millis(2));
    });
    let written = case.write_within("brief delays", deadline);
    let _ = written.success("brief delays leave the deadline headroom");
}

/// The `nth` guard labelled `label` of a materialization finds the deadline spent: the run halts there with
/// the typed timeout, never a success, with `completed` the last step completed.
fn halting_write(
    capsule: &CustomCapsuleV1,
    frames: &[(ClassV1, Vec<u8>)],
    label: &'static str,
    nth: usize,
    completed: PayloadPlaneStepV1,
) {
    let row = format!("{label} #{nth}");
    let case = PCaseV1::with_frames(capsule, frames);
    let deadline = Instant::now() + P_WRITE_DEADLINE_V1;
    let fired: FiredV1 = StdRc::new(StdCell::new(None));
    let act = spend(deadline, &fired);
    let count = StdCell::new(0_usize);
    let _guard = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == label {
            if count.get() == nth {
                act();
            }
            count.set(count.get() + 1);
        }
    });
    let written = case.write_within(&row, deadline);
    assert_eq!(
        fired.get(),
        Some(true),
        "{row}: INVALID (not behavioral evidence): the guard was not reached before the deadline"
    );
    let failure = written.failure(&row);
    assert!(
        matches!(failure.cause, PayloadPlaneErrorV1::Timeout),
        "{row}: {:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, Some(completed), "{row}");
}

/// Every guard of a clean materialization is where it is documented, in order, read off the plane's own
/// chains and the frames: planning's, then for each step each frame's own guard, its rehash, an item guard
/// before each create, a created guard as each create returns (`evidence/` and its slot included), a read
/// guard after each chunk, a verified guard once a written file is verified and recorded, and its decoded
/// guard, and the step's gate, which holds one more guard per entry created so far before the root's. Each
/// materialization guard halts the run alone with the typed timeout and the right last completed step.
#[test]
fn c2b_every_materialization_guard_is_in_its_place_and_halts_the_run_alone() {
    let capsule = frames_capsule();
    let frames = three_step_frames();
    let labels = StdRc::new(StdRefCell::new(Vec::new()));
    let case = PCaseV1::with_frames(&capsule, &frames);
    let mut expected = expected_guards(&case.plane, frames.len());
    let gate_with = |created: usize| {
        let mut gate = gate_guards(&case.plane);
        let root = gate.pop().expect("the root's guard");
        gate.extend(std::iter::repeat_n("payload-output-chain", created));
        gate.push(root);
        gate
    };
    let frame = |items: &[&'static str]| {
        let mut guards = vec!["payload-write-frame", "frame-rehash", "frame-rewind"];
        guards.extend(items);
        guards.push("payload-write-decoded");
        guards
    };
    let file = ["payload-created", "metadata-read", "payload-verified"];
    // The worktree: `a` (a file), `d`, `d/x` (a file), `l`; then its gate over those four.
    let mut worktree = vec!["payload-write-item"];
    worktree.extend(file);
    worktree.extend([
        "payload-write-item",
        "payload-created",
        "payload-write-item",
    ]);
    worktree.extend(file);
    worktree.extend(["payload-write-item", "payload-created"]);
    expected.extend(frame(&worktree));
    expected.extend(gate_with(4));
    // The index: `index`; then its gate over five.
    let mut index = vec!["payload-write-item"];
    index.extend(file);
    expected.extend(frame(&index));
    expected.extend(gate_with(5));
    // The bridge evidence: `evidence/`, its slot, and `e`; then its gate over eight.
    let mut evidence = vec!["payload-write-item", "payload-created", "payload-created"];
    evidence.extend(file);
    expected.extend(frame(&evidence));
    expected.extend(gate_with(8));
    {
        let _labels =
            install_git_hook_for_test(GitHookPointV1::DeadlineObserved, guard_labels(&labels));
        let _ = case.write("clean guards").success("clean guards");
    }
    assert_eq!(*labels.borrow(), expected);

    std::thread::scope(|scope| {
        let handles: Vec<_> = [
            ("payload-write-frame", 0, PayloadPlaneStepV1::Prevalidation),
            ("payload-write-item", 2, PayloadPlaneStepV1::Prevalidation),
            ("metadata-read", 0, PayloadPlaneStepV1::Prevalidation),
            ("payload-write-decoded", 1, PayloadPlaneStepV1::Worktree),
            ("payload-output-chain", 0, PayloadPlaneStepV1::Prevalidation),
            ("payload-gate", 2, PayloadPlaneStepV1::Prevalidation),
            ("payload-write-frame", 2, PayloadPlaneStepV1::Index),
            ("payload-root", 4, PayloadPlaneStepV1::Index),
            ("payload-created", 0, PayloadPlaneStepV1::Prevalidation),
            ("payload-created", 5, PayloadPlaneStepV1::Index),
            ("payload-verified", 0, PayloadPlaneStepV1::Prevalidation),
        ]
        .into_iter()
        .map(|(label, nth, completed)| {
            let (capsule, frames) = (&capsule, &frames);
            scope.spawn(move || halting_write(capsule, frames, label, nth, completed))
        })
        .collect();
        for handle in handles {
            handle.join().expect("a halting row's thread");
        }
    });
}

/// A create that returns after the deadline is never followed by another create: the object it made is kept
/// (recorded and charged, or in flight), and the run halts with the typed timeout before the next
/// filesystem call. The guard each create is followed by is made slow, as a create blocked in its own
/// directory sync or a slow file system would be: `evidence/` (its slot and the entry beneath it never
/// exist, `evidence/`'s pin and its one allowance are in the partial value), the slot (the entry never
/// exists), a directory entry (its child never exists), a file's create (the file is in flight with no byte,
/// nothing after it exists, also for the frame's last entry), a symlink, and a written file's verification.
#[test]
fn c2b_a_create_returning_after_the_deadline_is_never_followed_by_another() {
    let capsule = frames_capsule();
    let evidence_only = vec![(P_BRIDGE_V1, p_frame(P_BRIDGE_V1, &[file("e", b"evidence")]))];
    let worktree = vec![(
        P_WORKTREE_V1,
        p_frame(
            P_WORKTREE_V1,
            &[
                dir("d"),
                file("d/x", b"inside"),
                link("l", b"d"),
                file("z", b"z"),
            ],
        ),
    )];
    type RowV1<'a> = (
        &'static str,
        &'a [(ClassV1, Vec<u8>)],
        &'static str,
        usize,
        PayloadPlaneStepV1,
        &'static [&'static [u8]],
    );
    let rows: [RowV1<'_>; 7] = [
        (
            "a slow evidence/",
            &evidence_only,
            "payload-created",
            0,
            PayloadPlaneStepV1::Index,
            &[b"evidence"],
        ),
        (
            "a slow slot",
            &evidence_only,
            "payload-created",
            1,
            PayloadPlaneStepV1::Index,
            &[b"evidence", b"evidence/bridge_evidence"],
        ),
        (
            "a slow directory",
            &worktree,
            "payload-created",
            0,
            PayloadPlaneStepV1::Prevalidation,
            &[b"repository/d"],
        ),
        (
            "a slow file create",
            &worktree,
            "payload-created",
            1,
            PayloadPlaneStepV1::Prevalidation,
            &[b"repository/d", b"repository/d/x"],
        ),
        (
            "a slow file verification",
            &worktree,
            "payload-verified",
            0,
            PayloadPlaneStepV1::Prevalidation,
            &[b"repository/d", b"repository/d/x"],
        ),
        (
            "a slow symlink",
            &worktree,
            "payload-created",
            2,
            PayloadPlaneStepV1::Prevalidation,
            &[b"repository/d", b"repository/d/x", b"repository/l"],
        ),
        (
            "a slow last file create",
            &worktree,
            "payload-created",
            3,
            PayloadPlaneStepV1::Prevalidation,
            &[
                b"repository/d",
                b"repository/d/x",
                b"repository/l",
                b"repository/z",
            ],
        ),
    ];
    std::thread::scope(|scope| {
        let handles: Vec<_> = rows
            .into_iter()
            .map(|(row, frames, label, nth, completed, kept)| {
                let capsule = &capsule;
                scope.spawn(move || {
                    let case = PCaseV1::with_frames(capsule, frames);
                    let deadline = Instant::now() + P_WRITE_DEADLINE_V1;
                    let fired: FiredV1 = StdRc::new(StdCell::new(None));
                    let act = spend(deadline, &fired);
                    let count = StdCell::new(0_usize);
                    let _guard =
                        install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
                            if event.label == label {
                                if count.get() == nth {
                                    act();
                                }
                                count.set(count.get() + 1);
                            }
                        });
                    let written = case.write_within(row, deadline);
                    assert_eq!(
                        fired.get(),
                        Some(true),
                        "{row}: INVALID (not behavioral evidence): the guard was not reached before the deadline"
                    );
                    let failure = written.failure(row);
                    assert!(
                        matches!(failure.cause, PayloadPlaneErrorV1::Timeout),
                        "{row}: {:?}",
                        failure.cause
                    );
                    assert_eq!(failure.last_completed, Some(completed), "{row}");
                    assert_eq!(
                        written.added().into_keys().collect::<Vec<_>>(),
                        kept.iter().map(|name| name.to_vec()).collect::<Vec<_>>(),
                        "{row}: nothing is created after the slow create"
                    );
                    written.assert_created_is_the_disk(row, &[], &[]);
                    let created = &failure.partial.created;
                    match row {
                        "a slow evidence/" => {
                            assert!(created.evidence.root.is_some(), "{row}: the pin is kept");
                            assert!(created.evidence.slots.is_empty(), "{row}");
                            assert_eq!(
                                (created.charged.entries, created.charged.bytes),
                                (1, 0),
                                "{row}: its one allowance stays charged"
                            );
                        }
                        "a slow file create" => assert_eq!(
                            created.in_flight,
                            Some(InFlightPayloadFileV1 {
                                name: b"repository/d/x".to_vec(),
                                expected_length: 6,
                                written: 0
                            }),
                            "{row}: in flight, with no byte"
                        ),
                        "a slow last file create" => assert_eq!(
                            created.in_flight,
                            Some(InFlightPayloadFileV1 {
                                name: b"repository/z".to_vec(),
                                expected_length: 1,
                                written: 0
                            }),
                            "{row}: in flight, with no byte"
                        ),
                        _ => assert_eq!(created.in_flight, None, "{row}"),
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("a slow-create row's thread");
        }
    });
}

/// Every regular file is created `0600` and every directory `0700`, and each must keep that mode until the
/// terminal gate. A mode changed (and nothing else: the same inode, kind, and length) right after an entry is
/// created, or before a step's gate, refuses as an identity change at that gate with the right last completed
/// step: a worktree file made `0777` or `0400`, a worktree directory made `0755`, the index file, an evidence
/// file, `evidence/`, and a slot directory. A file whose mode is not `0600` as its create returns refuses at
/// its own create, in flight with no byte, before any later entry.
#[test]
fn c2b_the_created_modes_are_required_at_creation_and_at_every_gate() {
    use std::os::unix::fs::PermissionsExt as _;
    let chmod = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("a mode change");
    };
    let capsule = frames_capsule();
    let frames = three_step_frames();
    type ActV1 = fn(&Path);
    let after_creation: [(&str, &str, ActV1); 2] = [
        ("a file made 0777", "repository/a", |root| {
            std::fs::set_permissions(
                root.join("repository/a"),
                std::fs::Permissions::from_mode(0o777),
            )
            .expect("a mode change");
        }),
        ("a directory made 0755", "repository/d", |root| {
            std::fs::set_permissions(
                root.join("repository/d"),
                std::fs::Permissions::from_mode(0o755),
            )
            .expect("a mode change");
        }),
    ];
    let identity_changed = |failure: &PayloadPlaneRestoreFailureV1| {
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
        )
    };
    for (row, name, act) in after_creation {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let _hook = install_payload_hook_for_test(
            PayloadHookPointV1::EntryCreated,
            drifting_when(&case, move |event| event.path.ends_with(name), act),
        );
        let written = case.write(row);
        let failure = written.failure(row);
        assert!(identity_changed(failure), "{row}: {:?}", failure.cause);
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Prevalidation),
            "{row}"
        );
        for later in [&b"repository/d/x"[..], b"repository/l"] {
            assert!(
                written.added().contains_key(later),
                "{row}: {} was created",
                lossy(later)
            );
        }
    }

    let before_gate: [(&str, PayloadPlaneStepV1, &str, u32); 6] = [
        (
            "a file made 0400",
            PayloadPlaneStepV1::Worktree,
            "repository/a",
            0o400,
        ),
        (
            "the index made 0644",
            PayloadPlaneStepV1::Index,
            "repository/.git/index",
            0o644,
        ),
        (
            "an evidence file made 0400",
            PayloadPlaneStepV1::Evidence,
            "evidence/bridge_evidence/e",
            0o400,
        ),
        (
            "evidence/ made 0755",
            PayloadPlaneStepV1::Evidence,
            "evidence",
            0o755,
        ),
        (
            "a slot made 0711",
            PayloadPlaneStepV1::Evidence,
            "evidence/bridge_evidence",
            0o711,
        ),
        (
            "a directory made 0750",
            PayloadPlaneStepV1::Worktree,
            "repository/d",
            0o750,
        ),
    ];
    for (row, step, name, mode) in before_gate {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let target = case.dest.path.join(name);
        let _hook = install_payload_hook_for_test(
            PayloadHookPointV1::BeforeStepGate,
            drifting_when(&case, at_step(step), move |_| chmod(&target, mode)),
        );
        let written = case.write(row);
        let failure = written.failure(row);
        assert!(identity_changed(failure), "{row}: {:?}", failure.cause);
        let completed = match step {
            PayloadPlaneStepV1::Worktree => PayloadPlaneStepV1::Prevalidation,
            PayloadPlaneStepV1::Index => PayloadPlaneStepV1::Worktree,
            _ => PayloadPlaneStepV1::Index,
        };
        assert_eq!(failure.last_completed, Some(completed), "{row}");
    }

    // At creation: the mode as the create returns (made 0644 at the guard that follows the create).
    let case = PCaseV1::with_frames(&capsule, &frames);
    let target = case.dest.path.join("repository/a");
    let done = StdCell::new(false);
    let _guard = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "payload-created" && !done.replace(true) {
            chmod(&target, 0o644);
        }
    });
    let written = case.write("a file not created 0600");
    let failure = written.failure("a file not created 0600");
    assert!(identity_changed(failure), "{:?}", failure.cause);
    assert_eq!(
        failure.last_completed,
        Some(PayloadPlaneStepV1::Prevalidation)
    );
    assert_eq!(
        failure
            .partial
            .created
            .in_flight
            .as_ref()
            .map(|file| file.written),
        Some(0),
        "refused before its first byte"
    );
    assert_eq!(
        written.added().into_keys().collect::<Vec<_>>(),
        [b"repository/a".to_vec()],
        "nothing after it is created"
    );
}

// ---------------------------------------------------------------------------------------------
// The consumed-byte binding of the materialization pass
// ---------------------------------------------------------------------------------------------

/// The materialization pass's OWN hook replaces a staged frame in place, after its rehash and before its
/// third decode, with another VALID frame of the same length: the decode reaches a verified trailer and the
/// bytes it wrote stay on disk, but the binding of the consumed bytes refuses `StagedInputChanged`, with the
/// right last completed step for the worktree, the index, and an evidence class. Prevalidation's own hook
/// fires once per frame, in its own pass, and never in this one. An entry the plan did not admit (a renamed
/// file in the replacement) is never created, even before the binding refuses.
#[test]
fn c2b_a_frame_replaced_during_the_materialization_pass_refuses_as_changed() {
    let capsule = frames_capsule();
    type RowV1 = (
        ClassV1,
        Vec<u8>,
        Vec<u8>,
        Option<PayloadPlaneStepV1>,
        &'static [u8],
        &'static [u8],
    );
    let rows: [RowV1; 3] = [
        (
            P_WORKTREE_V1,
            p_frame(P_WORKTREE_V1, &[file("a", b"1111")]),
            p_frame(P_WORKTREE_V1, &[file("a", b"2222")]),
            Some(PayloadPlaneStepV1::Prevalidation),
            b"repository/a",
            b"2222",
        ),
        (
            P_INDEX_V1,
            p_frame(P_INDEX_V1, &[file("index", b"DIRC0001")]),
            p_frame(P_INDEX_V1, &[file("index", b"DIRC0002")]),
            Some(PayloadPlaneStepV1::Worktree),
            b"repository/.git/index",
            b"DIRC0002",
        ),
        (
            P_BRIDGE_V1,
            p_frame(P_BRIDGE_V1, &[file("e", b"1111")]),
            p_frame(P_BRIDGE_V1, &[file("e", b"2222")]),
            Some(PayloadPlaneStepV1::Index),
            b"evidence/bridge_evidence/e",
            b"2222",
        ),
    ];
    for (class, original, replacement, completed, name, bytes) in rows {
        assert_eq!(original.len(), replacement.len());
        let row = format!("{class:?}");
        let case = PCaseV1::with_frames(&capsule, &[(class, original)]);
        let handle = case.staged_handle(class);
        let prevalidated = StdRc::new(StdCell::new(0_usize));
        let counted = prevalidated.clone();
        let _prevalidation = install_payload_hook_for_test(
            PayloadHookPointV1::PrevalidationRehashed,
            move |event| {
                if event.class == Some(class) {
                    counted.set(counted.get() + 1);
                }
            },
        );
        let replaced = StdRc::new(StdCell::new(false));
        let seen = replaced.clone();
        let _hook = install_payload_hook_for_test(
            PayloadHookPointV1::MaterializationRehashed,
            move |event| {
                if event.class == Some(class) && !seen.replace(true) {
                    handle
                        .write_all_at(&replacement, 0)
                        .expect("another frame in place");
                }
            },
        );
        let written = case.write(&row);
        assert!(replaced.get(), "{row}: the hook ran");
        assert_eq!(
            prevalidated.get(),
            1,
            "{row}: prevalidation's hook, in its own pass"
        );
        let failure = written.failure(&row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::StagedInputChanged { .. }
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, completed, "{row}");
        assert!(failure.payload_materialization_began, "{row}");
        assert_eq!(
            written.added().get(name),
            Some(&DiskV1::File(0o600, bytes.to_vec())),
            "{row}: the written bytes stay"
        );
        written.assert_created_is_the_disk(&row, &[], &[]);
    }

    // Changed after prevalidation and before the materialization pass's rehash: that rehash refuses it
    // before anything is created.
    let original = p_frame(P_WORKTREE_V1, &[file("a", b"1111")]);
    let replacement = p_frame(P_WORKTREE_V1, &[file("a", b"2222")]);
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, original.clone())]);
    let handle = case.staged_handle(P_WORKTREE_V1);
    let other = replacement.clone();
    let done = StdCell::new(false);
    let _changed = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "payload-write-frame" && !done.replace(true) {
            handle
                .write_all_at(&other, 0)
                .expect("another frame in place");
        }
    });
    let written = case.write("changed before the materialization rehash");
    let failure = written.failure("changed before the materialization rehash");
    assert!(
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::StagedInputChanged { .. }
        ),
        "{:?}",
        failure.cause
    );
    assert!(!failure.payload_materialization_began);
    assert!(written.added().is_empty(), "refused before any create");
    drop(_changed);

    // A refusal of what was done with an entry (a collision planted at its slot) is held until the binding,
    // which refuses the replaced frame as changed, never as the collision.
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, original)]);
    let handle = case.staged_handle(P_WORKTREE_V1);
    let _replace =
        install_payload_hook_for_test(PayloadHookPointV1::MaterializationRehashed, move |_| {
            handle
                .write_all_at(&replacement, 0)
                .expect("another frame in place");
        });
    let target = case.dest.path.join("repository/a");
    let planted = target.clone();
    let _plant = install_payload_hook_for_test(
        PayloadHookPointV1::BeforeCreate,
        drifting_when(
            &case,
            move |event| event.path == target,
            move |_| std::fs::write(&planted, b"planted").expect("a planted file"),
        ),
    );
    let written = case.write("a held collision");
    let failure = written.failure("a held collision");
    assert!(
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::StagedInputChanged { .. }
        ),
        "the binding outranks the held collision: {:?}",
        failure.cause
    );
    assert_eq!(
        std::fs::read(written.dest.path.join("repository/a")).expect("the planted file"),
        b"planted"
    );
    drop((_replace, _plant));

    let original = p_frame(P_WORKTREE_V1, &[dir("d"), file("d/aaaa", b"1")]);
    let replacement = p_frame(P_WORKTREE_V1, &[dir("d"), file("d/bbbb", b"1")]);
    assert_eq!(original.len(), replacement.len());
    let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, original)]);
    let handle = case.staged_handle(P_WORKTREE_V1);
    let _hook =
        install_payload_hook_for_test(PayloadHookPointV1::MaterializationRehashed, move |_| {
            handle
                .write_all_at(&replacement, 0)
                .expect("another frame in place");
        });
    let written = case.write("an unplanned name");
    let failure = written.failure("an unplanned name");
    assert!(
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::StagedInputChanged { .. }
        ),
        "{:?}",
        failure.cause
    );
    assert_eq!(
        written.added().into_keys().collect::<Vec<_>>(),
        [b"repository/d".to_vec()],
        "the entry the plan did not admit is never created"
    );
}

/// The plan materialization was handed must describe its frames: a staged payload the plan does not bind
/// refuses before anything is created, and a decoded entry that is not the planned one at its position (a
/// changed length, an entry the plan lacks) is held, nothing after it is created, and refuses after the
/// trailer is verified.
#[test]
fn c2b_a_plan_that_does_not_describe_its_frames_refuses() {
    let capsule = frames_capsule();
    let frames = [(
        P_WORKTREE_V1,
        p_frame(
            P_WORKTREE_V1,
            &[file("a", b"1"), file("b", b"22"), file("c", b"3")],
        ),
    )];
    type EditV1 = fn(&mut PayloadPlanV1);
    let rows: [(&str, EditV1, &[&[u8]]); 4] = [
        ("another digest", |plan| plan.frames[0].sha256[0] ^= 1, &[]),
        (
            "another length",
            |plan| {
                if let PlannedKindV1::Regular { length, .. } = &mut plan.frames[0].entries[1].kind {
                    *length += 1;
                }
            },
            &[b"repository/a"],
        ),
        (
            "an entry the plan lacks",
            |plan| {
                plan.frames[0].entries.pop();
            },
            &[b"repository/a", b"repository/b"],
        ),
        (
            "an entry the frame lacks",
            |plan| {
                let mut extra = plan.frames[0].entries[2].clone();
                extra.path = b"d".to_vec();
                plan.frames[0].entries.push(extra);
            },
            &[b"repository/a", b"repository/b", b"repository/c"],
        ),
    ];
    for (row, edit, kept) in rows {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let _edit = edit_plan_for_test(edit);
        let written = case.write(row);
        let failure = written.failure(row);
        assert!(
            matches!(
                &failure.cause,
                PayloadPlaneErrorV1::PlanMismatch {
                    class: P_WORKTREE_V1,
                    ..
                }
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Prevalidation)
        );
        assert_eq!(
            written.added().into_keys().collect::<Vec<_>>(),
            kept.iter().map(|name| name.to_vec()).collect::<Vec<_>>(),
            "{row}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The step gates
// ---------------------------------------------------------------------------------------------

/// A created entry that is replaced, retyped, or removed after its creation and before its step's gate
/// refuses as an identity change at that gate, with the right last completed step: a worktree file as
/// another inode with the same bytes, a directory swapped for a new one, a symlink as another link with the
/// same target, a file replaced by a link, a removed file, the index file, an evidence file, `evidence/`
/// itself, and a slot directory. A file replaced during its own write is refused by name at its own create,
/// before any later entry; one replaced right after its creation is refused at its step's gate even though
/// the later entries were created.
#[test]
fn c2b_an_entry_replaced_after_creation_refuses_at_its_step_gate() {
    type ActV1 = fn(&Path);
    let rows: [(&str, PayloadPlaneStepV1, ActV1); 9] = [
        (
            "a file as another inode",
            PayloadPlaneStepV1::Worktree,
            |root| {
                replace_with_same_bytes(&root.join("repository/a"), root.parent().expect("area"));
            },
        ),
        (
            "a swapped directory",
            PayloadPlaneStepV1::Worktree,
            |root| {
                swap_out(root, &root.join("repository/d"));
            },
        ),
        (
            "a link as another link",
            PayloadPlaneStepV1::Worktree,
            |root| {
                let path = root.join("repository/l");
                let beside = root.parent().expect("area").join("replacement-link");
                std::os::unix::fs::symlink("a", &beside).expect("another link");
                std::fs::rename(&beside, path).expect("it takes the name");
            },
        ),
        (
            "a file replaced by a link",
            PayloadPlaneStepV1::Worktree,
            |root| {
                let beside = root.parent().expect("area").join("a-link");
                std::os::unix::fs::symlink("d/x", &beside).expect("a link");
                std::fs::rename(&beside, root.join("repository/a")).expect("it takes the name");
            },
        ),
        ("a removed file", PayloadPlaneStepV1::Worktree, |root| {
            std::fs::remove_file(root.join("repository/d/x")).expect("removed");
        }),
        ("the index file", PayloadPlaneStepV1::Index, |root| {
            replace_with_same_bytes(
                &root.join("repository/.git/index"),
                root.parent().expect("area"),
            );
        }),
        ("an evidence file", PayloadPlaneStepV1::Evidence, |root| {
            replace_with_same_bytes(
                &root.join("evidence/bridge_evidence/e"),
                root.parent().expect("area"),
            );
        }),
        ("evidence/ itself", PayloadPlaneStepV1::Evidence, |root| {
            swap_out(root, &root.join("evidence"));
        }),
        ("a slot directory", PayloadPlaneStepV1::Evidence, |root| {
            swap_out(root, &root.join("evidence/bridge_evidence"));
        }),
    ];
    let capsule = frames_capsule();
    let frames = three_step_frames();
    let completed_before = |step| match step {
        PayloadPlaneStepV1::Worktree => PayloadPlaneStepV1::Prevalidation,
        PayloadPlaneStepV1::Index => PayloadPlaneStepV1::Worktree,
        _ => PayloadPlaneStepV1::Index,
    };
    let identity_changed = |failure: &PayloadPlaneRestoreFailureV1| {
        matches!(
            failure.cause,
            PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
        )
    };
    for (row, step, act) in rows {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let _hook = install_payload_hook_for_test(
            PayloadHookPointV1::BeforeStepGate,
            drifting_when(&case, at_step(step), act),
        );
        let written = case.write(row);
        let failure = written.failure(row);
        assert!(identity_changed(failure), "{row}: {:?}", failure.cause);
        assert_eq!(
            failure.last_completed,
            Some(completed_before(step)),
            "{row}"
        );
    }

    // Replaced during its own write: its create refuses by name at once, and nothing after it is created.
    let case = PCaseV1::with_frames(&capsule, &frames);
    let path = case.dest.path.join("repository/a");
    let _hook = install_git_hook_for_test(GitHookPointV1::ObjectIo, {
        let act = drifting_when(
            &case,
            |_| true,
            |root| {
                replace_with_same_bytes(&root.join("repository/a"), root.parent().expect("area"))
            },
        );
        move |event| {
            if event.label == "write" && event.path == path {
                act(&PayloadHookEventV1 {
                    class: None,
                    step: None,
                    path: event.path,
                });
            }
        }
    });
    let written = case.write("replaced during its own write");
    let failure = written.failure("replaced during its own write");
    assert!(identity_changed(failure), "{:?}", failure.cause);
    assert_eq!(
        written.added().into_keys().collect::<Vec<_>>(),
        [b"repository/a".to_vec()],
        "refused at its own create: nothing after it"
    );
    drop(_hook);

    let case = PCaseV1::with_frames(&capsule, &frames);
    let _hook = install_payload_hook_for_test(
        PayloadHookPointV1::EntryCreated,
        drifting_when(
            &case,
            |event| event.path.ends_with("repository/a"),
            |root| {
                replace_with_same_bytes(&root.join("repository/a"), root.parent().expect("area"))
            },
        ),
    );
    let written = case.write("replaced right after its creation");
    let failure = written.failure("replaced right after its creation");
    assert!(identity_changed(failure), "{:?}", failure.cause);
    assert_eq!(
        failure.last_completed,
        Some(PayloadPlaneStepV1::Prevalidation)
    );
    let added = written.added();
    for later in [&b"repository/d/x"[..], b"repository/l"] {
        assert!(added.contains_key(later), "{} was created", lossy(later));
    }
}

/// Every created directory, file, and symlink must be on the destination root's device: an entry reported
/// on another device refuses as a device crossing at its own create, in every plane, with its entry kept
/// and charged and the right last completed step.
#[test]
fn c2b_a_device_crossing_refuses_and_keeps_the_charge() {
    let capsule = frames_capsule();
    let frames = three_step_frames();
    let rows: [(&str, PayloadPlaneStepV1, bool); 7] = [
        ("repository/d", PayloadPlaneStepV1::Prevalidation, false),
        ("repository/a", PayloadPlaneStepV1::Prevalidation, true),
        ("repository/l", PayloadPlaneStepV1::Prevalidation, false),
        ("repository/.git/index", PayloadPlaneStepV1::Worktree, true),
        ("evidence", PayloadPlaneStepV1::Index, false),
        ("evidence/bridge_evidence", PayloadPlaneStepV1::Index, false),
        (
            "evidence/bridge_evidence/e",
            PayloadPlaneStepV1::Index,
            true,
        ),
    ];
    for (name, completed, in_flight) in rows {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let dev = case.plane.capsule().destination.dev;
        let _dev = override_created_dev_for_test(name, dev.wrapping_add(1));
        let written = case.write(name);
        let failure = written.failure(name);
        assert!(
            matches!(&failure.cause, PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing { name: found }) if found == name),
            "{name}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, Some(completed), "{name}");
        assert!(failure.payload_materialization_began);
        assert!(
            written.added().contains_key(name.as_bytes()),
            "{name} exists and stays"
        );
        let unrecorded: &[&[u8]] = if in_flight { &[] } else { &[name.as_bytes()] };
        written.assert_created_is_the_disk(name, unrecorded, &[]);
        if in_flight {
            assert_eq!(
                failure
                    .partial
                    .created
                    .in_flight
                    .as_ref()
                    .map(|file| file.written),
                Some(0),
                "{name}: no byte before the device is proven"
            );
        }
    }
}

/// The steps' failures name the right last completed step. A collision planted in an intended slot just
/// before its create refuses at each step (a worktree file and symlink, the index, `evidence/`, a slot
/// directory), gives its reservation back, and
/// leaves the planted entry exactly as it was; a mount appearing at each step gate's census refuses there;
/// and a clean run takes exactly five censuses (two planning gates and three step gates).
#[test]
fn c2b_every_step_names_its_last_completed_step() {
    let capsule = frames_capsule();
    let frames = three_step_frames();
    type RowV1 = (&'static str, Option<PayloadPlaneStepV1>, bool);
    let rows: [RowV1; 5] = [
        (
            "repository/a",
            Some(PayloadPlaneStepV1::Prevalidation),
            false,
        ),
        (
            "repository/l",
            Some(PayloadPlaneStepV1::Prevalidation),
            true,
        ),
        (
            "repository/.git/index",
            Some(PayloadPlaneStepV1::Worktree),
            true,
        ),
        ("evidence", Some(PayloadPlaneStepV1::Index), true),
        (
            "evidence/bridge_evidence",
            Some(PayloadPlaneStepV1::Index),
            true,
        ),
    ];
    for (name, completed, began) in rows {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let target = case.dest.path.join(name);
        let planted = target.clone();
        let _hook = install_payload_hook_for_test(
            PayloadHookPointV1::BeforeCreate,
            drifting_when(
                &case,
                move |event| event.path == target,
                move |_| std::fs::write(&planted, b"planted").expect("a planted file"),
            ),
        );
        let written = case.write(name);
        let failure = written.failure(name);
        assert!(
            matches!(&failure.cause, PayloadPlaneErrorV1::DestinationCollision { name: found } if found == name),
            "{name}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, completed, "{name}");
        assert_eq!(failure.payload_materialization_began, began, "{name}");
        assert_eq!(
            std::fs::read(written.dest.path.join(name)).expect("the planted file"),
            b"planted",
            "{name}: the planted entry is untouched"
        );
        written.assert_created_is_the_disk(name, &[], &[name.as_bytes()]);
    }

    let census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec()]);
    let case = PCaseV1::with_frames(&capsule, &frames);
    let phase_g = census.calls();
    let _ = case.write("clean").success("clean");
    assert_eq!(
        census.calls() - phase_g,
        5,
        "two planning gates and three step gates"
    );
    drop(census);
    for (at, completed) in [
        (3, PayloadPlaneStepV1::Prevalidation),
        (4, PayloadPlaneStepV1::Worktree),
        (5, PayloadPlaneStepV1::Index),
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
        let row = format!("a mount at census {at}");
        let written = case.write(&row);
        let failure = written.failure(&row);
        assert!(
            matches!(
                failure.cause,
                PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::MountBoundary(_))
            ),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(failure.last_completed, Some(completed), "{row}");
    }
}

// ---------------------------------------------------------------------------------------------
// Names a destination may fold
// ---------------------------------------------------------------------------------------------

/// Whether the file system holding `area` folds `second` into `first`: `first` is created in a probe there,
/// and `second` looked up.
fn folds(area: &Path, first: &[u8], second: &[u8]) -> bool {
    let probe = tempfile::TempDir::new_in(area).expect("a probe");
    std::fs::write(probe.path().join(OsStr::from_bytes(first)), b"")
        .expect("the probe's first name");
    std::fs::symlink_metadata(probe.path().join(OsStr::from_bytes(second))).is_ok()
}

/// `A` and `a`, and an NFD and an NFC `é`, are distinct frame names. On a destination that keeps them apart
/// both restore byte-exact; on one that folds them (detected at run time; APFS folds case by default and
/// normalization always), the second create meets the first and refuses `DestinationCollision` after writes
/// began, with the first file kept exactly as written.
#[test]
fn c2b_case_and_normalization_variants_restore_or_collide_as_the_destination_decides() {
    let capsule = frames_capsule();
    let nfd = "e\u{301}".as_bytes();
    let nfc = "\u{e9}".as_bytes();
    for (row, first, second) in [("case", &b"A"[..], &b"a"[..]), ("normalization", nfd, nfc)] {
        let frame = p_frame(
            P_WORKTREE_V1,
            &[
                raw(first, EntryV1::File(b"first")),
                raw(second, EntryV1::File(b"second")),
            ],
        );
        let case = PCaseV1::with_frames(&capsule, &[(P_WORKTREE_V1, frame)]);
        let folding = folds(
            case.dest.path.parent().expect("the destination's area"),
            first,
            second,
        );
        println!("PAYLOAD-FOLDING {row} folds={folding}");
        let written = case.write(row);
        let added = written.added();
        let first_name = joined(b"repository", first);
        let second_name = joined(b"repository", second);
        assert_eq!(
            added.get(&first_name),
            Some(&DiskV1::File(0o600, b"first".to_vec())),
            "{row}"
        );
        if folding {
            let failure = written.failure(row);
            assert!(
                matches!(&failure.cause, PayloadPlaneErrorV1::DestinationCollision { name } if name.as_bytes() == second_name.as_slice()),
                "{row}: {:?}",
                failure.cause
            );
            assert!(failure.payload_materialization_began);
            assert_eq!(
                failure.last_completed,
                Some(PayloadPlaneStepV1::Prevalidation)
            );
            assert_eq!(added.len(), 1, "{row}: only the first name exists");
        } else {
            let _ = written.success(row);
            assert_eq!(
                added.get(&second_name),
                Some(&DiskV1::File(0o600, b"second".to_vec())),
                "{row}"
            );
        }
        written.assert_created_is_the_disk(row, &[], &[]);
    }
}

// ---------------------------------------------------------------------------------------------
// Empty classes, and a second invocation
// ---------------------------------------------------------------------------------------------

/// An Empty class creates nothing for itself: all five Empty add nothing at all (no `evidence/`); one
/// evidence class Captured adds only `evidence/` and its own slot; an Empty worktree beside a Captured index
/// leaves `repository/` holding only `.git`; and a Captured evidence frame with no entry creates neither its
/// slot nor `evidence/`. Each records exactly what it added.
#[test]
fn c2b_an_empty_class_creates_nothing() {
    let capsule = frames_capsule();
    let empty_bridge = p_frame(P_BRIDGE_V1, &[]);
    type RowV1 = (
        &'static str,
        Vec<(ClassV1, Vec<u8>)>,
        &'static [&'static [u8]],
    );
    let rows: [RowV1; 4] = [
        ("every class Empty", vec![], &[]),
        (
            "one evidence class",
            vec![(
                P_OPERATIONS_V1,
                p_frame(P_OPERATIONS_V1, &[file("MERGE_HEAD", b"x")]),
            )],
            &[
                b"evidence",
                b"evidence/in_progress_git_operations",
                b"evidence/in_progress_git_operations/MERGE_HEAD",
            ],
        ),
        (
            "an index alone",
            vec![(P_INDEX_V1, p_frame(P_INDEX_V1, &[file("index", b"DIRC")]))],
            &[b"repository/.git/index"],
        ),
        (
            "a Captured evidence frame with no entry",
            vec![(P_BRIDGE_V1, empty_bridge)],
            &[],
        ),
    ];
    for (row, frames, want) in rows {
        let written = PCaseV1::with_frames(&capsule, &frames).write(row);
        let done = written.success(row);
        assert_eq!(
            written.added().into_keys().collect::<Vec<_>>(),
            want.iter().map(|name| name.to_vec()).collect::<Vec<_>>(),
            "{row}"
        );
        assert_eq!(
            listing(&stored(&written.dest, "repository")),
            [".git"],
            "{row}: no worktree entry"
        );
        written.assert_created_is_the_disk(row, &[], &[]);
        assert_eq!(
            done.created.evidence.root.is_some(),
            !want.is_empty() && want[0] == b"evidence"
        );
    }
}

/// A second invocation over a destination phase P already wrote refuses at its first create as a collision:
/// nothing changes on disk, its reservation is given back (the one ledger is unchanged), and
/// materialization never began. With an Empty worktree and index, the first create is `evidence/`'s, after
/// the worktree and index steps completed (empty).
#[test]
fn c2b_a_second_invocation_refuses_and_changes_nothing() {
    let capsule = frames_capsule();
    for (row, frames, completed) in [
        (
            "every step",
            three_step_frames(),
            PayloadPlaneStepV1::Prevalidation,
        ),
        (
            "evidence alone",
            vec![(P_BRIDGE_V1, p_frame(P_BRIDGE_V1, &[file("e", b"evidence")]))],
            PayloadPlaneStepV1::Index,
        ),
    ] {
        let written = PCaseV1::with_frames(&capsule, &frames).write(row);
        let WrittenV1 { dest, outcome, .. } = written;
        let done = outcome.unwrap_or_else(|failure| panic!("{row}: {:?}", failure.cause));
        let first = if row == "every step" {
            "repository/a"
        } else {
            "evidence"
        };
        let again = PCaseV1 {
            dest,
            plane: done.plane,
            drift: StdRc::new(StdRefCell::new(None)),
        };
        let before = disk_tree(&again.dest.path);
        let ledger = again.plane.ledger_used();
        let written = again.write(&format!("{row}: again"));
        let failure = written.failure(row);
        assert!(
            matches!(&failure.cause, PayloadPlaneErrorV1::DestinationCollision { name } if name == first),
            "{row}: {:?}",
            failure.cause
        );
        assert!(!failure.payload_materialization_began, "{row}");
        assert_eq!(failure.last_completed, Some(completed), "{row}");
        assert_eq!(failure.ledger_used, ledger, "{row}: nothing stays charged");
        assert_eq!(
            disk_tree(&written.dest.path),
            before,
            "{row}: nothing changed"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Structural controls
// ---------------------------------------------------------------------------------------------

/// STRUCTURAL ONLY (evidence of shape, never of behavior; the behavioral evidence is every row above). The
/// materialization section creates only through the create-new primitives beneath retained pins, charges the
/// one ledger, reads only retained descriptors, and keeps the deadline; it renames, removes, truncates,
/// chmods, links by path, syncs, and runs Git nowhere, names no Git metadata file of its own, and holds no
/// `unsafe`.
#[test]
fn c2b_the_materialization_section_is_create_new_only() {
    let source = include_str!("custody_restore_payload.rs");
    let (_planner, section) = source.split_once(P_WRITE_MARKER_V1).expect("the marker");
    let code: String = section
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "std::fs",
        "File::open(",
        "OpenOptions",
        "read_dir(",
        "rename",
        "remove_",
        "unlink",
        "set_len",
        "truncate",
        "hard_link",
        "::symlink(",
        "set_permissions",
        "set_exact",
        "chmod",
        ".sync(",
        "sync_all",
        "GitRunnerV1",
        "GitRunRequestV1",
        "Command",
        "spawn",
        "unsafe",
        "b\"HEAD\"",
        "b\"config\"",
        "b\"hooks\"",
        "b\"refs",
        "b\"objects",
        "b\"index\"",
    ] {
        assert!(
            !code.contains(forbidden),
            "the materialization section mentions `{forbidden}`"
        );
    }
    for required in [
        "create_staging_directory(",
        "create_new_regular_child(",
        "create_new_symlink_child(",
        "reserve_entries(",
        "stream_into(",
        "reverify_staged_v1(",
        "CustodyFrameDecoderV1::new(",
        "observe_deadline(",
        "before_deadline(",
        "gate(",
        "try_clone(",
    ] {
        assert!(
            code.contains(required),
            "the materialization section lacks `{required}`"
        );
    }
}
