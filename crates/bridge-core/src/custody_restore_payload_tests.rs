//! In-crate controls for ADR-0041 slice 2B3c increments C2a (phase P's admission, prevalidation, and plan),
//! C2b (materialization: the worktree, the index, and the evidence), and C3 (the terminal gate, the final
//! modes, and the complete result).
//!
//! Every row runs phase P over a REAL phase-G value: a real 2B2 export carried through the real 2B3a reader
//! (phase V) and the real 2B3b wrapper (phase G, with real Git). The export is the rich plan-backed clone
//! (with a real split index for C2b), a planned in-worktree separate Git directory, or the empty custom
//! capsule with a captured unborn HEAD beside hand-encoded phase-P frames, built by the real frame encoder,
//! that stand where a sealed capsule's staged payloads would (the 2B3b `stage_frame` fixture, applied before
//! phase G runs). Planning writes nothing, so every C2a row, a refusal or a success, also proves that the
//! destination tree and the one ledger are exactly what they were; every C2b row proves that the one ledger
//! moved by exactly what phase P reports it charged and that nothing phase V or phase G made changed.
//! Every C3 row restores V to G to P and inspects sealed entries only through descriptors held open before
//! the terminal gate, or through the recorded inventory, never by changing a mode back. Mounted beneath the
//! 2B3b controls, this reuses their fixtures without widening any of them. The mutation matrices in
//! `.git/a2a-evidence/2b3c-c2a/`, `.git/a2a-evidence/2b3c-c2b/`, and `.git/a2a-evidence/2b3c-c3/` map each
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

/// One real phase-G value with its destination, and the one ledger's charge when phase V returned.
struct PCaseV1 {
    dest: DestV1,
    plane: RestoredGitPlaneV1,
    drift: DriftV1,
    phase_v: u64,
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
            phase_v,
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
            phase_v,
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
    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_DEADLINE_V1);
    let count = StdCell::new(0_usize);
    let fired = StdRc::new(StdCell::new(false));
    let marked = fired.clone();
    let _guard = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == label {
            if count.get() == nth {
                marked.set(deadline_now_for_test() < deadline);
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
    let _clock = DeadlineClockV1::freeze();
    let failure = case.refused_within("spent", deadline_now_for_test(), P_TABLE_BOUND_V1);
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
    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_DEADLINE_V1);
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
    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_DEADLINE_V1);
    let _brief = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, |_| {
        DeadlineClockV1::advance_to(deadline_now_for_test() + Duration::from_millis(2));
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
    disk_tree_where(root, |_| true)
}

/// [`disk_tree`] of only the entries whose relative name `keep` admits: any other entry is neither
/// observed, read, nor descended into.
fn disk_tree_where(root: &Path, keep: impl Fn(&[u8]) -> bool) -> DiskTreeV1 {
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
            if !keep(&relative) {
                continue;
            }
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
    expected_tree(plane, false)
}

/// [`expected_added`] once the terminal gate sealed it: a worktree entry has its frame's exact mode (D4),
/// an index file `0600` (D4), and every evidence file `0400` and directory `0500`, `evidence/` and its slots
/// included (D2).
fn expected_final(plane: &RestoredGitPlaneV1) -> DiskTreeV1 {
    expected_tree(plane, true)
}

/// [`expected_added`] (`sealed` false) or [`expected_final`] (`sealed` true).
fn expected_tree(plane: &RestoredGitPlaneV1, sealed: bool) -> DiskTreeV1 {
    let directory_mode = |evidence: bool, frame: u32| match (sealed, evidence) {
        (false, _) => 0o700,
        (true, true) => 0o500,
        (true, false) => frame,
    };
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
                let mode = directory_mode(true, 0);
                tree.insert(b"evidence".to_vec(), DiskV1::Dir(mode));
                tree.insert(joined(b"evidence", slot), DiskV1::Dir(mode));
            }
            let entry = match kind {
                OwnKindV1::Dir(mode) => DiskV1::Dir(directory_mode(slot.is_some(), mode)),
                OwnKindV1::File(mode, _) => {
                    let mode = match (sealed, &slot, class) {
                        (false, _, _) | (true, None, P_INDEX_V1) => 0o600,
                        (true, Some(_), _) => 0o400,
                        (true, None, _) => mode,
                    };
                    DiskV1::File(mode, content)
                }
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
            fired.set(Some(deadline_now_for_test() < deadline));
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
                    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
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
    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
    let _brief = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, |_| {
        DeadlineClockV1::advance_to(deadline_now_for_test() + Duration::from_millis(2));
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
    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
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
                    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
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
        let case = PCaseV1::with_frames(&capsule, &frames);
        let phase_v = case.phase_v;
        let written = case.write(row);
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
            phase_v,
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
/// materialization section (up to the C3 terminal gate's section, which follows it) creates only through the
/// create-new primitives beneath retained pins, charges the one ledger, reads only retained descriptors, and
/// keeps the deadline; it renames, removes, truncates, chmods, links by path, syncs, and runs Git nowhere,
/// names no Git metadata file of its own, and holds no `unsafe`.
#[test]
fn c2b_the_materialization_section_is_create_new_only() {
    let source = include_str!("custody_restore_payload.rs");
    assert_eq!(source.matches(P_SEAL_MARKER_V1).count(), 1);
    let (_planner, rest) = source.split_once(P_WRITE_MARKER_V1).expect("the marker");
    let (section, _terminal) = rest.split_once(P_SEAL_MARKER_V1).expect("the C3 marker");
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

// =============================================================================================
// Increment C3: the terminal gate, the final modes, and the complete result
//
// Every row drives `restore_payload_plane_v1` over a REAL phase-G value. A sealed entry is never
// re-chmodded to be inspected: every entry phase P added is held open (a directory or a file by a
// descriptor) at the hook that runs immediately before the terminal gate, while every file is still `0600`
// and every directory `0700`, and its final mode and bytes are read afterwards through that descriptor (a
// link by its target, which no seal changes) or from the recorded inventory. The runner holds, on EVERY
// outcome, that the one ledger moved by exactly what phase P charged and that nothing phase V or phase G
// made changed (unless the row's own hook drifted it), and on success the one-ledger accounting: phase V's
// charge, plus phase G's and phase P's on-disk charge by the test's own measure. A row's destination is made
// removable again only when it is dropped, after every inspection.
// =============================================================================================

/// The terminal gate's section header in the production source.
const P_SEAL_MARKER_V1: &str =
    "// The terminal gate (increment C3): every created entry verified and sealed, and the result";

type RestoredOutcomeV1 = Result<RestoredPayloadPlaneV1, Box<PayloadPlaneRestoreFailureV1>>;
type StepActV1 = Box<dyn Fn(&PayloadHookEventV1<'_>)>;

/// One entry of a hand-built frame, with the mode the frame records for it.
#[derive(Clone, Copy)]
enum ModedV1<'a> {
    Dir(u32),
    File(u32, &'a [u8]),
    Link(&'a [u8]),
}

/// A frame's bytes from the real encoder, with each entry's own mode, in canonical order.
fn moded_frame(class: ClassV1, entries: &[(&[u8], ModedV1<'_>)]) -> Vec<u8> {
    let mut ordered: Vec<(CustodyFramePathV1, ModedV1<'_>)> = entries
        .iter()
        .map(|(path, entry)| {
            (
                CustodyFramePathV1::from_bytes(path).expect("a frame path"),
                *entry,
            )
        })
        .collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let header = CustodyFrameHeaderV1::new(class, GENERATION_V1).expect("a frame header");
    let mut bytes = Vec::new();
    let mut encoder =
        CustodyFrameEncoderV1::new(&mut bytes, &header, frame_budget()).expect("an encoder");
    for (path, entry) in ordered {
        match entry {
            ModedV1::Dir(mode) => encoder.directory(&path, mode).expect("a directory"),
            ModedV1::File(mode, content) => {
                let mut reader: &[u8] = content;
                encoder
                    .regular_file(&path, mode, content.len() as u64, &mut reader)
                    .expect("a regular file");
            }
            ModedV1::Link(target) => encoder.symlink(&path, target).expect("a symlink"),
        }
    }
    encoder.finish().expect("the trailer");
    bytes
}

/// One entry phase P added, held at the hook immediately before the terminal gate: a directory or a file by a
/// descriptor opened while it was still `0700` or `0600`, a link by its target and identity.
enum HeldV1 {
    Dir(std::fs::File),
    File(std::fs::File),
    Link(Vec<u8>, (u64, u64)),
}

/// Every entry phase P added, by its exact destination-relative name, held.
struct HeldTreeV1(BTreeMap<Vec<u8>, HeldV1>);

impl HeldTreeV1 {
    /// Every entry in phase P's planes that `before` does not name, held open.
    fn hold(root: &Path, before: &DiskTreeV1) -> Self {
        use std::os::unix::ffi::OsStringExt as _;
        use std::os::unix::fs::MetadataExt as _;
        let mut held = BTreeMap::new();
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
                if metadata.is_dir() {
                    frontier.push(path.clone());
                }
                if !in_phase_p_planes(&relative) || before.contains_key(&relative) {
                    continue;
                }
                let one = if metadata.file_type().is_symlink() {
                    HeldV1::Link(
                        std::fs::read_link(&path)
                            .expect("a link target")
                            .into_os_string()
                            .into_vec(),
                        (metadata.dev(), metadata.ino()),
                    )
                } else if metadata.is_dir() {
                    HeldV1::Dir(std::fs::File::open(&path).expect("a directory held open"))
                } else {
                    HeldV1::File(std::fs::File::open(&path).expect("a file held open"))
                };
                held.insert(relative, one);
            }
        }
        Self(held)
    }

    /// What every held entry is now: a directory's and a file's mode by its held descriptor's `fstat`, a
    /// file's bytes read through that descriptor (opened before any seal, so no mode is ever changed to read
    /// it), and a link's target.
    fn now(&self) -> DiskTreeV1 {
        use std::io::{Read as _, Seek as _};
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |file: &std::fs::File| {
            file.metadata()
                .expect("a held descriptor's fstat")
                .permissions()
                .mode()
                & 0o7777
        };
        self.0
            .iter()
            .map(|(name, one)| {
                let now = match one {
                    HeldV1::Dir(file) => DiskV1::Dir(mode(file)),
                    HeldV1::File(file) => {
                        let mut handle = file.try_clone().expect("a duplicate descriptor");
                        handle.seek(SeekFrom::Start(0)).expect("rewind");
                        let mut bytes = Vec::new();
                        handle
                            .read_to_end(&mut bytes)
                            .expect("its bytes, through the descriptor held before the seal");
                        DiskV1::File(mode(file), bytes)
                    }
                    HeldV1::Link(target, _) => DiskV1::Link(target.clone()),
                };
                (name.clone(), now)
            })
            .collect()
    }

    /// The held entry's `(dev, ino)`.
    fn identity(&self, name: &[u8]) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt as _;
        match self.0.get(name) {
            Some(HeldV1::Dir(file) | HeldV1::File(file)) => {
                let metadata = file.metadata().expect("a held descriptor's fstat");
                (metadata.dev(), metadata.ino())
            }
            Some(HeldV1::Link(_, identity)) => *identity,
            None => panic!("{} was not added", lossy(name)),
        }
    }
}

/// Makes every directory beneath a destination's area searchable and writable again when dropped: a non-root owner
/// cannot otherwise remove the entries of a sealed `0500`, `0555`, or `0000` directory (as root it changes
/// nothing that matters). Cleanup only, after every inspection: nothing is ever inspected through it.
struct UnsealV1(PathBuf);

impl Drop for UnsealV1 {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let mut frontier = vec![self.0.clone()];
        while let Some(directory) = frontier.pop() {
            let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
                    frontier.push(path);
                }
            }
        }
    }
}

/// Whether this test process runs as root: a file it creates is owned by uid 0.
fn running_as_root() -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let probe = tempfile::NamedTempFile::new().expect("a probe file");
    probe.as_file().metadata().expect("its metadata").uid() == 0
}

/// One restore's outcome: the destination (made removable again when dropped, before its area is removed),
/// every entry phase P added held open immediately before the terminal gate (none when the run refused
/// earlier), and the outcome.
struct RestoredRunV1 {
    /// Declared first, so dropped first.
    _unseal: UnsealV1,
    dest: DestV1,
    held: Option<HeldTreeV1>,
    outcome: RestoredOutcomeV1,
}

impl PCaseV1 {
    /// Restores under `deadline`, with `act` run at every step-gate hook (after the entries are held, at the
    /// terminal gate's), and requires what EVERY outcome keeps (this section's head).
    fn restore_with(self, row: &str, deadline: Instant, act: Option<StepActV1>) -> RestoredRunV1 {
        let PCaseV1 {
            dest,
            plane,
            drift,
            phase_v,
        } = self;
        let ledger = plane.ledger_used();
        let before = disk_tree(&dest.path);
        let phase_g = charge_on_disk(&dest, &PLANE_SUBTREES_V1);
        assert_eq!(
            ledger,
            phase_v + phase_g,
            "{row}: phase G's one-ledger accounting holds as phase P starts"
        );
        let slot = StdRc::new(StdRefCell::new(None));
        let hook = {
            let (slot, root, before) = (slot.clone(), dest.path.clone(), before.clone());
            install_payload_hook_for_test(PayloadHookPointV1::BeforeStepGate, move |event| {
                if event.step == Some(PayloadPlaneStepV1::TerminalGate) {
                    *slot.borrow_mut() = Some(HeldTreeV1::hold(&root, &before));
                }
                if let Some(act) = &act {
                    act(event);
                }
            })
        };
        let outcome = restore_payload_plane_v1(plane, frame_budget(), deadline);
        drop(hook);
        let held = slot.borrow_mut().take();
        match &outcome {
            Ok(done) => {
                assert_eq!(done.step, PayloadPlaneStepV1::TerminalGate, "{row}");
                assert!(
                    done.plane
                        .capsule()
                        .staged
                        .iter()
                        .all(|one| position(one) == 0),
                    "{row}: every staged descriptor is back at offset 0"
                );
                let held = held.as_ref().expect("held before the terminal gate");
                assert_eq!(
                    done.plane.ledger_used(),
                    phase_v + phase_g + disk_charge(&held.now()),
                    "{row}: the one ledger is phase V's charge plus phase G's and phase P's on disk"
                );
            }
            Err(failure) => {
                assert_eq!(
                    failure.ledger_used,
                    failure.partial.plane.ledger_used(),
                    "{row}: the failure's figure is the one ledger's"
                );
                assert_eq!(
                    failure.partial.plane.ledger_used(),
                    ledger + charge_of(failure.partial.created.charged),
                    "{row}: the one ledger moved by exactly what phase P charged"
                );
            }
        }
        if drift.borrow_mut().take().is_none() {
            let outside: DiskTreeV1 = before
                .iter()
                .filter(|(name, _)| !in_phase_p_planes(name))
                .map(|(name, entry)| (name.clone(), entry.clone()))
                .collect();
            assert_eq!(
                disk_tree_where(&dest.path, |name| !in_phase_p_planes(name)),
                outside,
                "{row}: nothing phase V or phase G made changed, and nothing lies outside phase P's planes"
            );
        }
        RestoredRunV1 {
            // The whole area: a row may have moved a sealed tree out beside the destination.
            _unseal: UnsealV1(
                dest.path
                    .parent()
                    .expect("the destination's area")
                    .to_path_buf(),
            ),
            dest,
            held,
            outcome,
        }
    }

    fn restore(self, row: &str) -> RestoredRunV1 {
        self.restore_with(row, far_deadline(), None)
    }
}

impl RestoredRunV1 {
    fn success(&self, row: &str) -> &RestoredPayloadPlaneV1 {
        match &self.outcome {
            Ok(done) => done,
            Err(failure) => panic!(
                "{row}: must restore: {:?} (completed {:?})",
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

    fn held(&self, row: &str) -> &HeldTreeV1 {
        self.held
            .as_ref()
            .unwrap_or_else(|| panic!("{row}: the entries were held before the terminal gate"))
    }

    /// The success value's inventories are exactly what phase P added, as the held descriptors show it now:
    /// each row's identity is the entry's, each mode written is its mode on disk, each file's length and
    /// SHA-256 are its bytes', each link's target its target; nothing phase P added is missing from them; and
    /// every retained directory pin is the directory its row names.
    fn assert_inventory_is_the_disk(&self, row: &str) {
        let done = self.success(row);
        let held = self.held(row);
        let now = held.now();
        let mut recorded = DiskTreeV1::new();
        let mut rows = |prefix: &[u8], entries: &[RestoredEntryV1]| {
            for entry in entries {
                let name = joined(prefix, &entry.path);
                assert_eq!(
                    held.identity(&name),
                    (entry.dev, entry.ino),
                    "{row}: {} is the recorded entry",
                    lossy(&name)
                );
                let disk = match &entry.kind {
                    CreatedKindV1::Directory => {
                        DiskV1::Dir(entry.mode.expect("a directory's mode written"))
                    }
                    CreatedKindV1::Regular { length, sha256 } => {
                        let Some(DiskV1::File(_, bytes)) = now.get(&name) else {
                            panic!("{row}: {} is not a file", lossy(&name));
                        };
                        assert_eq!(bytes.len() as u64, *length, "{row}: {}", lossy(&name));
                        assert_eq!(sha256_bytes(bytes), *sha256, "{row}: {}", lossy(&name));
                        DiskV1::File(entry.mode.expect("a file's mode written"), bytes.clone())
                    }
                    CreatedKindV1::Symlink { target } => {
                        assert_eq!(
                            (entry.frame_mode, entry.mode),
                            (None, None),
                            "{row}: a link has no mode"
                        );
                        DiskV1::Link(target.clone())
                    }
                };
                recorded.insert(name, disk);
            }
        };
        rows(b"repository", &done.worktree.entries);
        rows(b"repository/.git", &done.index.files);
        let mut slots = Vec::new();
        for slot in &done.evidence.slots {
            let name = TreeAtV1::Evidence(slot.slot).prefix();
            assert_eq!(held.identity(&name), (slot.dev, slot.ino), "{row}");
            slots.push((name.clone(), DiskV1::Dir(slot.mode)));
            rows(&name, &slot.entries);
        }
        if let Some((mode, dev, ino)) = done.evidence.root {
            assert_eq!(held.identity(b"evidence"), (dev, ino), "{row}: evidence/");
            recorded.insert(b"evidence".to_vec(), DiskV1::Dir(mode));
        }
        recorded.extend(slots);
        assert_eq!(
            now, recorded,
            "{row}: the inventory is exactly what phase P added, with the modes on disk"
        );

        let pinned = |name: &[u8], pin: &PinnedDirectoryV1| {
            assert_eq!(
                (Some(held.identity(name).0), Some(held.identity(name).1)),
                (pin.identity().dev, pin.identity().ino),
                "{row}: {}'s pin",
                lossy(name)
            );
        };
        let directories = |entries: &[RestoredEntryV1]| {
            entries
                .iter()
                .filter(|entry| entry.kind == CreatedKindV1::Directory)
                .count()
        };
        assert_eq!(
            done.pins.worktree.len(),
            directories(&done.worktree.entries),
            "{row}"
        );
        for (path, pin) in &done.pins.worktree {
            pinned(&joined(b"repository", path), pin);
        }
        assert_eq!(done.pins.evidence.is_some(), done.evidence.root.is_some());
        if let Some(pin) = &done.pins.evidence {
            pinned(b"evidence", pin);
        }
        assert_eq!(
            done.pins
                .evidence_slots
                .iter()
                .map(|one| one.slot)
                .collect::<Vec<_>>(),
            done.evidence
                .slots
                .iter()
                .map(|one| one.slot)
                .collect::<Vec<_>>(),
            "{row}"
        );
        for (pins, slot) in done.pins.evidence_slots.iter().zip(&done.evidence.slots) {
            let name = TreeAtV1::Evidence(pins.slot).prefix();
            pinned(&name, &pins.pin);
            assert_eq!(pins.directories.len(), directories(&slot.entries), "{row}");
            for (path, pin) in &pins.directories {
                pinned(&joined(&name, path), pin);
            }
        }
    }

    /// A refused terminal gate kept everything: every entry the failure names as sealed has exactly that
    /// mode, and every other entry phase P added still has its created mode (`0700`, `0600`), as the held
    /// descriptors show; `changed` are the row's own mode changes, which are not compared.
    fn assert_seals_are_the_disk(&self, row: &str, changed: &[&[u8]]) {
        let failure = self.failure(row);
        let sealed: BTreeMap<Vec<u8>, u32> = failure
            .partial
            .sealed
            .iter()
            .map(|one| (one.name.clone(), one.mode))
            .collect();
        assert_eq!(
            sealed.len(),
            failure.partial.sealed.len(),
            "{row}: each entry is sealed at most once"
        );
        let now = self.held(row).now();
        for name in sealed.keys() {
            assert!(now.contains_key(name), "{row}: {} was added", lossy(name));
        }
        for (name, entry) in &now {
            if changed.contains(&name.as_slice()) {
                continue;
            }
            let (mode, created) = match entry {
                DiskV1::Dir(mode) => (Some(*mode), Some(0o700)),
                DiskV1::File(mode, _) => (Some(*mode), Some(0o600)),
                DiskV1::Link(_) => (None, None),
            };
            assert_eq!(
                mode,
                sealed.get(name).copied().or(created),
                "{row}: {}",
                lossy(name)
            );
        }
    }

    /// A refused terminal gate kept its partial whole: the plan, and a row or pin for every entry phase P added,
    /// each still that entry (its device and inode, as the held descriptors show), and nothing else.
    fn assert_partial_is_the_disk(&self, row: &str) {
        let partial = &self.failure(row).partial;
        assert!(partial.plan.is_some(), "{row}: the plan is kept");
        let created = &partial.created;
        let mut recorded = BTreeMap::new();
        let rows =
            |recorded: &mut BTreeMap<Vec<u8>, (u64, u64)>, at: TreeAtV1, tree: &CreatedTreeV1| {
                for entry in &tree.entries {
                    recorded.insert(at.name(&entry.path), (entry.dev, entry.ino));
                }
            };
        let pinned = |pin: &PinnedDirectoryV1| {
            (
                pin.identity().dev.expect("a device"),
                pin.identity().ino.expect("an inode"),
            )
        };
        rows(&mut recorded, TreeAtV1::Worktree, &created.worktree);
        rows(&mut recorded, TreeAtV1::Index, &created.index);
        if let Some(pin) = &created.evidence.root {
            recorded.insert(b"evidence".to_vec(), pinned(pin));
        }
        for slot in &created.evidence.slots {
            let at = TreeAtV1::Evidence(slot.slot);
            recorded.insert(at.prefix(), pinned(&slot.pin));
            rows(&mut recorded, at, &slot.tree);
        }
        let held = self.held(row);
        let added: BTreeMap<Vec<u8>, (u64, u64)> = held
            .now()
            .into_keys()
            .map(|name| {
                let identity = held.identity(&name);
                (name, identity)
            })
            .collect();
        assert_eq!(
            recorded, added,
            "{row}: the partial records exactly what phase P added"
        );
    }
}

/// The worktree and evidence frames of the mode controls: files `0000`, `0200`, and `0755`; a `0000`
/// directory holding a `0000` directory that holds a file; a `0750` directory holding a `0000` directory
/// that holds a file; a `0555` directory with a file, a link, and a `0700` directory with a file; a childless
/// `0711` directory and a link escaping `repository/`; and an evidence class whose own modes (an executable
/// hook, a `0000` directory) are kept only in the inventory.
fn mode_frames() -> Vec<(ClassV1, Vec<u8>)> {
    let worktree = [
        (&b"f0000"[..], ModedV1::File(0o000, b"zero\n")),
        (b"f0200", ModedV1::File(0o200, b"write-only\n")),
        (b"f0755", ModedV1::File(0o755, b"#!/bin/sh\nexit 0\n")),
        (b"zero", ModedV1::Dir(0o000)),
        (b"zero/deeper", ModedV1::Dir(0o000)),
        (
            b"zero/deeper/file",
            ModedV1::File(0o640, b"beneath two sealed directories\n"),
        ),
        (b"nest", ModedV1::Dir(0o750)),
        (b"nest/locked", ModedV1::Dir(0o000)),
        (
            b"nest/locked/file",
            ModedV1::File(0o604, b"inside a 0000 directory\n"),
        ),
        (b"ro", ModedV1::Dir(0o555)),
        (b"ro/a", ModedV1::File(0o444, b"a\n")),
        (b"ro/link", ModedV1::Link(b"a")),
        (b"ro/sub", ModedV1::Dir(0o700)),
        (b"ro/sub/b", ModedV1::File(0o600, b"b\n")),
        (b"empty", ModedV1::Dir(0o711)),
        (b"outside", ModedV1::Link(b"../outside")),
    ];
    let config = [
        (&b"config"[..], ModedV1::File(0o644, b"[core]\n")),
        (b"hooks", ModedV1::Dir(0o000)),
        (
            b"hooks/pre-commit",
            ModedV1::File(0o777, b"#!/bin/sh\nexit 0\n"),
        ),
        (b"hooks/up", ModedV1::Link(b"../config")),
    ];
    vec![
        (P_WORKTREE_V1, moded_frame(P_WORKTREE_V1, &worktree)),
        (
            P_INDEX_V1,
            moded_frame(P_INDEX_V1, &[(b"index", ModedV1::File(0o644, b"DIRC0001"))]),
        ),
        (P_CONFIG_V1, moded_frame(P_CONFIG_V1, &config)),
    ]
}

/// The inventory row at `path` of `entries`.
fn row_at<'e>(entries: &'e [RestoredEntryV1], path: &[u8]) -> &'e RestoredEntryV1 {
    entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("{} is inventoried", lossy(path)))
}

// ---------------------------------------------------------------------------------------------
// Final modes
// ---------------------------------------------------------------------------------------------

/// Files `0000`, `0200`, and `0755`, a `0000` directory nested in a `0000` and in a `0750` directory and
/// holding a file, and a `0555` directory with a file, a link, and a directory all restore, verify, and end
/// with exactly their frame modes, as a non-root owner too: every directory is searched and every file read
/// before its own seal, never after. The held descriptors show every final mode and every byte, the inventory
/// shows the same modes beside the frame modes, and the index ends `0600` with its frame mode `0644` kept.
/// As a non-root owner, the sealed `0000` directories really are closed to a lookup now.
#[test]
fn c3_every_frame_mode_restores_and_is_sealed_exactly() {
    let capsule = frames_capsule();
    let run = PCaseV1::with_frames(&capsule, &mode_frames()).restore("modes");
    let done = run.success("modes");
    assert_eq!(
        run.held("modes").now(),
        expected_final(&done.plane),
        "every byte, with its final mode"
    );
    run.assert_inventory_is_the_disk("modes");
    let now = run.held("modes").now();
    for (name, want) in [
        (
            &b"repository/f0000"[..],
            DiskV1::File(0o000, b"zero\n".to_vec()),
        ),
        (
            b"repository/f0200",
            DiskV1::File(0o200, b"write-only\n".to_vec()),
        ),
        (
            b"repository/f0755",
            DiskV1::File(0o755, b"#!/bin/sh\nexit 0\n".to_vec()),
        ),
        (b"repository/zero", DiskV1::Dir(0o000)),
        (b"repository/zero/deeper", DiskV1::Dir(0o000)),
        (b"repository/nest", DiskV1::Dir(0o750)),
        (b"repository/nest/locked", DiskV1::Dir(0o000)),
        (
            b"repository/nest/locked/file",
            DiskV1::File(0o604, b"inside a 0000 directory\n".to_vec()),
        ),
        (b"repository/ro", DiskV1::Dir(0o555)),
        (b"repository/ro/a", DiskV1::File(0o444, b"a\n".to_vec())),
        (b"repository/ro/link", DiskV1::Link(b"a".to_vec())),
        (b"repository/ro/sub", DiskV1::Dir(0o700)),
        (
            b"repository/.git/index",
            DiskV1::File(0o600, b"DIRC0001".to_vec()),
        ),
    ] {
        assert_eq!(now.get(name), Some(&want), "{}", lossy(name));
    }
    for (path, frame, written) in [
        (&b"f0000"[..], 0o000, 0o000),
        (b"f0200", 0o200, 0o200),
        (b"f0755", 0o755, 0o755),
        (b"zero/deeper", 0o000, 0o000),
        (b"nest/locked", 0o000, 0o000),
        (b"ro", 0o555, 0o555),
    ] {
        let entry = row_at(&done.worktree.entries, path);
        assert_eq!(
            (entry.frame_mode, entry.mode),
            (Some(frame), Some(written)),
            "{}",
            lossy(path)
        );
    }
    let index = row_at(&done.index.files, b"index");
    assert_eq!((index.frame_mode, index.mode), (Some(0o644), Some(0o600)));
    assert_eq!(
        done.worktree.state,
        PayloadClassStateV1::Captured {
            staged: staged_index(done.plane.capsule(), P_WORKTREE_V1)
        }
    );
    assert_eq!(done.worktree.root_git, RootGitDispositionV1::Absent);
    assert_eq!(done.worktree.childless_directories, [b"empty".to_vec()]);
    assert_eq!(now.get(&b"repository/empty"[..]), Some(&DiskV1::Dir(0o711)));
    assert_eq!(
        done.declared_external_references,
        [ExternalSymlinkV1 {
            plane: PayloadPlaneV1::Worktree,
            path: b"outside".to_vec(),
            target: b"../outside".to_vec(),
        }]
    );
    if !running_as_root() {
        for sealed in ["repository/zero/deeper", "repository/nest/locked/file"] {
            let error = std::fs::symlink_metadata(run.dest.path.join(sealed))
                .expect_err("a lookup beneath a sealed 0000 directory");
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{sealed}"
            );
        }
    }
    println!("PAYLOAD-SEAL-MODES as_root={}", running_as_root());
}

/// D3's opposite branch, restored whole on a REAL planned capsule (a worktree whose root `.git` is a gitfile
/// naming the in-worktree separate Git directory `.gd`, plus an ordinary empty directory): the result reports
/// the root gitfile's disposition and both childless directories, with no claim about which was a Git
/// directory; the gitfile ends inert at `evidence/worktree_root_gitfile/gitfile` (`0400`, with its frame mode
/// inventoried, beneath a `0500` slot and `evidence/`) holding the source's bytes; both childless directories
/// end with their frame modes; and nothing is at `repository/.git` but phase G's directory.
#[test]
fn c3_the_root_gitfile_ends_inert_and_every_childless_directory_is_reported() {
    let source = SeparateGitDirV1::export();
    let repo = source
        .capsule
        .parent()
        .and_then(Path::parent)
        .expect("the fixture's area")
        .join("repo");
    let gitfile = std::fs::read(repo.join(".git")).expect("the source gitfile");
    let run = PCaseV1::sealed(&source.capsule).restore("separate git dir");
    let done = run.success("separate git dir");
    let now = run.held("separate git dir").now();
    assert_eq!(now, expected_final(&done.plane));
    run.assert_inventory_is_the_disk("separate git dir");
    assert_eq!(
        done.worktree.root_git,
        RootGitDispositionV1::RootGitfileToEvidence
    );
    assert_eq!(
        done.worktree.childless_directories,
        [b".gd".to_vec(), b"empty".to_vec()]
    );
    assert_eq!(
        now.get(&b"evidence/worktree_root_gitfile/gitfile"[..]),
        Some(&DiskV1::File(0o400, gitfile))
    );
    let slot = &done.evidence.slots[0];
    assert_eq!(
        (slot.slot, slot.mode),
        (EvidenceSlotV1::WorktreeRootGitfile, 0o500)
    );
    let frame: BTreeMap<Vec<u8>, OwnKindV1> = own_contents(&done.plane, P_WORKTREE_V1)
        .into_iter()
        .map(|(path, kind, _)| (path, kind))
        .collect();
    let Some(OwnKindV1::File(gitfile_mode, _)) = frame.get(&b".git"[..]) else {
        panic!("the frame's root gitfile");
    };
    let row = row_at(&slot.entries, b"gitfile");
    assert_eq!(
        (row.frame_mode, row.mode),
        (Some(*gitfile_mode), Some(0o400))
    );
    for name in [&b".gd"[..], b"empty"] {
        let Some(OwnKindV1::Dir(mode)) = frame.get(name) else {
            panic!("{} is a frame directory", lossy(name));
        };
        assert_eq!(
            now.get(&joined(b"repository", name)),
            Some(&DiskV1::Dir(*mode)),
            "{}",
            lossy(name)
        );
    }
    use std::os::unix::fs::MetadataExt as _;
    let git =
        std::fs::symlink_metadata(run.dest.path.join("repository/.git")).expect("repository/.git");
    assert_eq!(
        (git.is_dir(), Some(git.dev()), Some(git.ino())),
        (
            true,
            done.plane.objects.pins.git.identity().dev,
            done.plane.objects.pins.git.identity().ino
        ),
        "repository/.git is still phase G's directory"
    );
}

/// What the instrumented seam saw of the terminal gate, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SeenV1 {
    Entry(PathBuf),
    Lookup(PathBuf),
    Sealed(PathBuf),
}

/// The instrumented seam: every by-name lookup of the terminal gate, every entry's one terminal observation,
/// and every seal, in order. No name is looked up at or beneath an entry once it is sealed (a sealed `0000`
/// directory would refuse a non-root lookup); every entry phase P added is observed exactly once and every
/// file and directory sealed once, a directory after everything beneath it; a directory's one observation
/// (its own name, from its parent) follows every event beneath it and leads straight to its seal; and no
/// retained phase-V or phase-G pin, which the chains after the walk look names up beneath, lies at or
/// beneath a sealed entry.
#[test]
fn c3_no_name_is_looked_up_beneath_a_sealed_directory() {
    let capsule = frames_capsule();
    let mut frames = mode_frames();
    frames.push((P_BRIDGE_V1, p_frame(P_BRIDGE_V1, &[file("e", b"evidence")])));
    let case = PCaseV1::with_frames(&capsule, &frames);
    let root = case.dest.path.clone();
    let seen = StdRc::new(StdRefCell::new(Vec::new()));
    let record = |kind: fn(PathBuf) -> SeenV1| {
        let seen = seen.clone();
        move |event: &PayloadHookEventV1<'_>| seen.borrow_mut().push(kind(event.path.to_path_buf()))
    };
    let _entry =
        install_payload_hook_for_test(PayloadHookPointV1::TerminalEntry, record(SeenV1::Entry));
    let _lookup =
        install_payload_hook_for_test(PayloadHookPointV1::TerminalLookup, record(SeenV1::Lookup));
    let _sealed =
        install_payload_hook_for_test(PayloadHookPointV1::TerminalSealed, record(SeenV1::Sealed));
    let run = case.restore("the seam");
    let done = run.success("the seam");
    let seen = seen.borrow().clone();
    assert!(
        seen.iter().any(|one| matches!(one, SeenV1::Lookup(_))),
        "the seam observed the walk"
    );

    let mut sealed: Vec<&PathBuf> = Vec::new();
    for (at, one) in seen.iter().enumerate() {
        let path = match one {
            SeenV1::Entry(path) | SeenV1::Lookup(path) | SeenV1::Sealed(path) => path,
        };
        if let Some(closed) = sealed.iter().find(|closed| path.starts_with(closed)) {
            panic!(
                "event {at} ({one:?}) is at or beneath {}, sealed before it",
                closed.display()
            );
        }
        if let SeenV1::Sealed(path) = one {
            sealed.push(path);
        }
    }

    let held = run.held("the seam").now();
    let mut added: Vec<PathBuf> = held
        .keys()
        .map(|name| root.join(OsStr::from_bytes(name)))
        .collect();
    added.sort();
    let mut entries: Vec<PathBuf> = seen
        .iter()
        .filter_map(|one| match one {
            SeenV1::Entry(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    let observed = entries.len();
    entries.sort();
    entries.dedup();
    assert_eq!(observed, entries.len(), "each entry is observed once");
    assert_eq!(entries, added, "every entry phase P added is observed");
    let mut sealed: Vec<PathBuf> = sealed.into_iter().cloned().collect();
    let seals = sealed.len();
    sealed.sort();
    sealed.dedup();
    assert_eq!(seals, sealed.len(), "each entry is sealed once");
    let unsealed: Vec<PathBuf> = held
        .iter()
        .filter(|(_, entry)| matches!(entry, DiskV1::Link(_)))
        .map(|(name, _)| root.join(OsStr::from_bytes(name)))
        .collect();
    assert_eq!(
        sealed,
        added
            .iter()
            .filter(|path| !unsealed.contains(path))
            .cloned()
            .collect::<Vec<_>>(),
        "every file and directory is sealed, and no link"
    );

    // Post-order observation: a directory's own name is checked only once nothing beneath it is visited
    // again, and nothing but that check comes between it and the directory's seal.
    let path_of = |one: &SeenV1| match one {
        SeenV1::Entry(path) | SeenV1::Lookup(path) | SeenV1::Sealed(path) => path.clone(),
    };
    let directories: Vec<PathBuf> = held
        .iter()
        .filter(|(_, entry)| matches!(entry, DiskV1::Dir(_)))
        .map(|(name, _)| root.join(OsStr::from_bytes(name)))
        .collect();
    assert!(
        directories.len() > 5,
        "the frames hold worktree and evidence directories"
    );
    for directory in &directories {
        let observed = seen
            .iter()
            .position(|one| *one == SeenV1::Entry(directory.clone()))
            .unwrap_or_else(|| panic!("{} is observed", directory.display()));
        let sealed_at = seen
            .iter()
            .position(|one| *one == SeenV1::Sealed(directory.clone()))
            .unwrap_or_else(|| panic!("{} is sealed", directory.display()));
        if let Some(last) = seen.iter().rposition(|one| {
            let path = path_of(one);
            path != *directory && path.starts_with(directory)
        }) {
            assert!(
                last < observed,
                "event {last} ({:?}) is beneath {}, observed before it at event {observed}",
                seen[last],
                directory.display()
            );
        }
        assert!(
            observed < sealed_at
                && seen[observed..sealed_at]
                    .iter()
                    .all(|one| path_of(one) == *directory),
            "{}: only its own observation precedes its seal",
            directory.display()
        );
    }

    // The chains after the walk look names up only beneath retained phase-V and phase-G pins.
    let plane = &done.plane;
    let dest = &plane.capsule().destination;
    let pins = &plane.objects.pins;
    let mut retained: Vec<&Path> = vec![
        dest.root.canonical_path(),
        dest.work.canonical_path(),
        dest.plain.canonical_path(),
        plane.bootstrap.home.canonical_path(),
        plane.bootstrap.xdg.canonical_path(),
    ];
    for pin in [
        &pins.repository,
        &pins.git,
        &pins.objects,
        &pins.pack,
        &pins.info,
    ] {
        retained.push(pin.canonical_path());
    }
    retained.extend(
        plane
            .metadata
            .directories
            .values()
            .map(|pin| pin.canonical_path()),
    );
    retained.extend(
        plane
            .bootstrap
            .directories
            .values()
            .map(|pin| pin.canonical_path()),
    );
    let staging: Vec<PathBuf> = dest
        .staging
        .borrow()
        .values()
        .map(|pin| pin.canonical_path().to_path_buf())
        .collect();
    retained.extend(staging.iter().map(PathBuf::as_path));
    for pin in retained {
        assert!(
            !sealed.iter().any(|closed| pin.starts_with(closed)),
            "{} lies beneath a sealed entry",
            pin.display()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Each entry's one terminal observation
// ---------------------------------------------------------------------------------------------

/// The names the three-step frames' terminal gate seals, in its order: the worktree's `a`, `d/x`, then `d`
/// (`l` is a link), the index, and the evidence file, its slot, then `evidence/`.
const SEAL_ORDER_V1: [&str; 7] = [
    "repository/a",
    "repository/d/x",
    "repository/d",
    "repository/.git/index",
    "evidence/bridge_evidence/e",
    "evidence/bridge_evidence",
    "evidence",
];

/// Runs `act` once, at the `nth` event (from 0) whose path is `target`.
fn nth_at(
    target: PathBuf,
    nth: usize,
    act: impl Fn(&Path) + 'static,
) -> impl Fn(&PayloadHookEventV1<'_>) + 'static {
    let seen = StdCell::new(0_usize);
    move |event| {
        if event.path == target {
            if seen.get() == nth {
                act(event.path);
            }
            seen.set(seen.get() + 1);
        }
    }
}

/// `path` as a symlink to `target`, atomically: the link is made beside the destination and renamed over.
fn replace_with_target(path: &Path, outside: &Path, target: &str) {
    let staged = outside.join("replacement-target-link");
    std::os::unix::fs::symlink(target, &staged).expect("a link");
    std::fs::rename(&staged, path).expect("the link takes the name");
}

/// The typed refusal of a terminal-gate row: the cause `is`, `Evidence` the last step completed, writes
/// began, and exactly `sealed` sealed, in order.
fn refused_at_the_terminal_gate(
    run: &RestoredRunV1,
    row: &str,
    is: impl Fn(&PayloadPlaneErrorV1) -> bool,
    sealed: &[&str],
) {
    let failure = run.failure(row);
    assert!(is(&failure.cause), "{row}: {:?}", failure.cause);
    assert_eq!(
        failure.last_completed,
        Some(PayloadPlaneStepV1::Evidence),
        "{row}"
    );
    assert!(failure.payload_materialization_began, "{row}");
    assert_eq!(
        failure
            .partial
            .sealed
            .iter()
            .map(|one| lossy(&one.name))
            .collect::<Vec<_>>(),
        sealed,
        "{row}: exactly what was sealed before it"
    );
}

fn identity_changed(cause: &PayloadPlaneErrorV1) -> bool {
    matches!(
        cause,
        PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
    )
}

/// A replacement immediately before an entry's one terminal observation refuses as an identity change at
/// that entry, with `Evidence` the last step completed: a worktree file as another inode with the same bytes,
/// a directory swapped for a new one (its observation follows its subtree, which is sealed), a file beneath
/// it, a link as another link with the same target, the index file, `evidence/` itself, a slot directory,
/// and an evidence file. Everything sealed before it keeps its final mode and nothing after it is sealed or
/// removed. A directory's own name is checked after its subtree, never before: `repository/d`, a slot, or
/// `evidence/` renamed away (still walked through its retained pin) and a new directory put at its name while
/// a file beneath it is observed refuses at that directory's observation, with the file sealed and the
/// directory not. Inside one observation: a file replaced between its by-name stat and its reopen (by
/// another inode, or by a link the reopen will not follow), and a link retargeted before its `readlinkat` or
/// replaced before its closing stat, refuse the same way. A file made `0000` in place (which a non-root
/// reopen could not even open) and a link replaced by a regular file of its target's length (which
/// `readlinkat` cannot read) are refused typed by the by-name stat itself.
#[test]
fn c3_a_replacement_before_an_entrys_terminal_observation_refuses_typed() {
    type ActV1 = fn(&Path, &Path);
    let capsule = frames_capsule();
    let frames = three_step_frames();
    let rows: [(&str, &str, usize, ActV1, usize); 17] = [
        (
            "a file as another inode",
            "repository/a",
            0,
            |path, area| replace_with_same_bytes(path, area),
            0,
        ),
        (
            "a directory swapped",
            "repository/d",
            0,
            |path, area| swap_directory(path, area),
            2,
        ),
        (
            "a directory's own name swapped while its file is observed",
            "repository/d/x",
            0,
            |path, area| swap_directory(path.parent().expect("d"), area),
            2,
        ),
        (
            "a slot's own name swapped while its file is observed",
            "evidence/bridge_evidence/e",
            0,
            |path, area| swap_directory(path.parent().expect("the slot"), area),
            5,
        ),
        (
            "evidence/'s own name swapped while a slot's file is observed",
            "evidence/bridge_evidence/e",
            0,
            |path, area| {
                let evidence = path.parent().and_then(Path::parent).expect("evidence/");
                swap_directory(evidence, area);
            },
            6,
        ),
        (
            "a file beneath a directory",
            "repository/d/x",
            0,
            |path, area| replace_with_same_bytes(path, area),
            1,
        ),
        (
            "a link as another link",
            "repository/l",
            0,
            |path, area| replace_with_target(path, area, "a"),
            3,
        ),
        (
            "the index file",
            "repository/.git/index",
            0,
            |path, area| replace_with_same_bytes(path, area),
            3,
        ),
        (
            "evidence/ itself",
            "evidence",
            0,
            |path, area| swap_directory(path, area),
            6,
        ),
        (
            "a slot directory",
            "evidence/bridge_evidence",
            0,
            |path, area| swap_directory(path, area),
            5,
        ),
        (
            "an evidence file",
            "evidence/bridge_evidence/e",
            0,
            |path, area| replace_with_same_bytes(path, area),
            4,
        ),
        (
            "a file replaced before its reopen",
            "repository/a",
            1,
            |path, area| replace_with_same_bytes(path, area),
            0,
        ),
        (
            "a file replaced by a link before its reopen",
            "repository/a",
            1,
            |path, area| replace_with_target(path, area, "d/x"),
            0,
        ),
        (
            "a link retargeted before its readlinkat",
            "repository/l",
            1,
            |path, area| replace_with_target(path, area, "b"),
            3,
        ),
        (
            "a link replaced before its closing stat",
            "repository/l",
            2,
            |path, area| replace_with_target(path, area, "a"),
            3,
        ),
        (
            "a file made 0000 before its observation",
            "repository/a",
            0,
            |path, _| {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
                    .expect("a mode change");
            },
            0,
        ),
        (
            "a link replaced by a file before its observation",
            "repository/l",
            0,
            |path, area| {
                let staged = area.join("replacement-file");
                std::fs::write(&staged, b"a").expect("a file of the link's length");
                std::fs::rename(&staged, path).expect("it takes the name");
            },
            3,
        ),
    ];
    for (row, name, lookup, act, sealed) in rows {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let target = case.dest.path.join(name);
        let area = case.dest.path.parent().expect("the area").to_path_buf();
        let hook = move |path: &Path| act(path, &area);
        let _guard = if lookup == 0 {
            install_payload_hook_for_test(
                PayloadHookPointV1::TerminalEntry,
                nth_at(target, 0, hook),
            )
        } else {
            install_payload_hook_for_test(
                PayloadHookPointV1::TerminalLookup,
                nth_at(target, lookup, hook),
            )
        };
        let run = case.restore(row);
        refused_at_the_terminal_gate(&run, row, identity_changed, &SEAL_ORDER_V1[..sealed]);
        let changed: &[&[u8]] = if row.contains("0000") {
            &[name.as_bytes()]
        } else {
            &[]
        };
        run.assert_seals_are_the_disk(row, changed);
    }
}

// ---------------------------------------------------------------------------------------------
// Byte integrity
// ---------------------------------------------------------------------------------------------

/// `path`'s first byte flipped in place: the same inode and the same length, other bytes.
fn rewrite_in_place(path: &Path) {
    use std::os::unix::fs::FileExt as _;
    let bytes = std::fs::read(path).expect("the file");
    let handle = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("the file, for writing");
    handle
        .write_all_at(&[!bytes[0]], 0)
        .expect("one byte rewritten in place");
}

/// A file rewritten in place (the same inode and length, other bytes) before the terminal gate refuses with
/// the typed byte-integrity error naming it, with `Evidence` the last step completed: a worktree file, the
/// index, and an evidence file, rewritten just before the terminal gate, and a worktree file rewritten before
/// the worktree's own step gate, which compares identity and mode and so passes it. The rewritten bytes stay
/// (nothing is removed) and the file stays `0600`. A file rewritten with its own bytes after it was read and
/// before its descriptor's closing `fstat` refuses as an identity change: it changed while it was read.
#[test]
fn c3_a_file_rewritten_in_place_refuses_as_content_changed() {
    let capsule = frames_capsule();
    let frames = three_step_frames();
    let rows: [(&str, PayloadPlaneStepV1, &str, usize); 4] = [
        (
            "a worktree file before the terminal gate",
            PayloadPlaneStepV1::TerminalGate,
            "repository/a",
            0,
        ),
        (
            "the index before the terminal gate",
            PayloadPlaneStepV1::TerminalGate,
            "repository/.git/index",
            3,
        ),
        (
            "an evidence file before the terminal gate",
            PayloadPlaneStepV1::TerminalGate,
            "evidence/bridge_evidence/e",
            4,
        ),
        (
            "a worktree file before the worktree's step gate",
            PayloadPlaneStepV1::Worktree,
            "repository/d/x",
            1,
        ),
    ];
    for (row, step, name, sealed) in rows {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let target = case.dest.path.join(name);
        let act: StepActV1 = Box::new(move |event| {
            if event.step == Some(step) {
                rewrite_in_place(&target);
            }
        });
        let run = case.restore_with(row, far_deadline(), Some(act));
        refused_at_the_terminal_gate(
            &run,
            row,
            |cause| matches!(cause, PayloadPlaneErrorV1::ContentChanged { name: found } if found == name),
            &SEAL_ORDER_V1[..sealed],
        );
        run.assert_seals_are_the_disk(row, &[]);
        let now = run.held(row).now();
        let Some(DiskV1::File(mode, bytes)) = now.get(name.as_bytes()) else {
            panic!("{row}: {name} is held");
        };
        let original = expected_added(&run.failure(row).partial.plane);
        let Some(DiskV1::File(_, written)) = original.get(name.as_bytes()) else {
            panic!("{row}: {name} is planned");
        };
        assert_eq!(*mode, 0o600, "{row}: not sealed");
        assert_eq!(bytes.len(), written.len(), "{row}: the same length");
        assert_eq!(bytes[0], !written[0], "{row}: the rewritten bytes stay");
    }

    let case = PCaseV1::with_frames(&capsule, &frames);
    let target = case.dest.path.join("repository/a");
    let _hook = install_payload_hook_for_test(
        PayloadHookPointV1::TerminalDigested,
        nth_at(target, 0, |path| {
            // A write is seen by the closing `fstat` through its timestamps, which a coarse clock could
            // leave unchanged inside one tick.
            std::thread::sleep(Duration::from_millis(50));
            let bytes = std::fs::read(path).expect("the file");
            use std::os::unix::fs::FileExt as _;
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .expect("the file, for writing")
                .write_all_at(&bytes, 0)
                .expect("its own bytes, again");
        }),
    );
    let row = "rewritten after it was read";
    let run = case.restore(row);
    refused_at_the_terminal_gate(&run, row, identity_changed, &[]);
    run.assert_seals_are_the_disk(row, &[]);
}

// ---------------------------------------------------------------------------------------------
// The seal's confirmation
// ---------------------------------------------------------------------------------------------

/// A mode changed between an entry's `fchmod` (the primitive's, confirmed by the primitive) and phase P's own
/// confirming `fstat` of the same descriptor refuses as an identity change, with `Evidence` the last step
/// completed and the entry not recorded as sealed: a worktree file, a worktree directory, the index, an
/// evidence file, and `evidence/` itself. The primitive's own window (after its `fchmod`, before its `fstat`)
/// is C1's seam, private to `fs_custody`; C1's `mode_03` covers it.
#[test]
fn c3_a_mode_changed_after_its_fchmod_refuses() {
    use std::os::unix::fs::PermissionsExt as _;
    let capsule = frames_capsule();
    let frames = three_step_frames();
    let rows: [(&str, &str, u32, usize); 5] = [
        ("a worktree file", "repository/a", 0o600, 0),
        ("a worktree directory", "repository/d", 0o700, 2),
        ("the index", "repository/.git/index", 0o640, 3),
        ("an evidence file", "evidence/bridge_evidence/e", 0o600, 4),
        ("evidence/ itself", "evidence", 0o700, 6),
    ];
    for (row, name, other, sealed) in rows {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let target = case.dest.path.join(name);
        let _hook = install_payload_hook_for_test(
            PayloadHookPointV1::TerminalModeSet,
            nth_at(target, 0, move |path| {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(other))
                    .expect("another mode");
            }),
        );
        let run = case.restore(row);
        refused_at_the_terminal_gate(&run, row, identity_changed, &SEAL_ORDER_V1[..sealed]);
        run.assert_seals_are_the_disk(row, &[name.as_bytes()]);
        let now = run.held(row).now();
        let mode = match now.get(name.as_bytes()) {
            Some(DiskV1::Dir(mode) | DiskV1::File(mode, _)) => *mode,
            other => panic!("{row}: {other:?}"),
        };
        assert_eq!(mode, other, "{row}: the changed mode is what refused");
    }
}

// ---------------------------------------------------------------------------------------------
// The gate's order, and a refused terminal gate
// ---------------------------------------------------------------------------------------------

/// A refused terminal gate names `Evidence` as the last step completed and keeps everything: a mount
/// appearing at its census (the sixth of a clean restore: two planning gates, three step gates, and the
/// terminal gate) refuses before anything is sealed; and every retained phase-V and phase-G chain changed
/// just before the terminal gate (the metadata `refs` directory, the object `info` directory, `HEAD` and a
/// copied object file as other inodes, an admitted bootstrap file and a bootstrap directory on a Captured
/// database, the plain staging, HOME, and XDG directories) refuses as an identity change after every entry
/// is sealed, so those chains are rechecked after the walk; the destination root swapped refuses at the
/// root's identity, LAST, after every entry is sealed and every retained chain passed through its pins.
#[test]
fn c3_a_refused_terminal_gate_names_evidence_and_keeps_everything() {
    let capsule = frames_capsule();
    let frames = three_step_frames();

    let census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec()]);
    let case = PCaseV1::with_frames(&capsule, &frames);
    let phase_g = census.calls();
    let _ = case.restore("clean").success("clean");
    assert_eq!(
        census.calls() - phase_g,
        6,
        "two planning gates, three step gates, and the terminal gate"
    );
    drop(census);

    let case = PCaseV1::with_frames(&capsule, &frames);
    let inside = mount_inside(&case.dest.path);
    let _census = crate::custody_mounts::seam::install(Box::new(move |call| {
        if call == 6 {
            Ok(vec![b"/".to_vec(), inside.clone()])
        } else {
            Ok(vec![b"/".to_vec()])
        }
    }));
    let row = "a mount at the terminal census";
    let run = case.restore(row);
    refused_at_the_terminal_gate(
        &run,
        row,
        |cause| {
            matches!(
                cause,
                PayloadPlaneErrorV1::Restore(CustodyRestoreErrorV1::MountBoundary(_))
            )
        },
        &[],
    );
    run.assert_seals_are_the_disk(row, &[]);
    drop(_census);

    // Each retained chain, changed just before the terminal gate: on the three-step frames, every entry is
    // sealed first; on a Captured database (which restores no phase-P class), nothing was created.
    type ActV1 = fn(&Path);
    let rows: [(&str, bool, ActV1); 10] = [
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
        ("the XDG directory", false, |root| {
            swap_out(root, &root.join(".restore-work/xdg"));
        }),
        ("the destination root", false, |root| {
            swap_out(root, root);
        }),
    ];
    let captured_capsule = captured(CustodyGitObjectFormatV1::Sha1);
    for (row, captured_database, act) in rows {
        let case = if captured_database {
            PCaseV1::of(PlaneInV1::rich(
                &captured_capsule,
                CustodyGitObjectFormatV1::Sha1,
            ))
        } else {
            PCaseV1::with_frames(&capsule, &frames)
        };
        let act = drifting_when(&case, at_step(PayloadPlaneStepV1::TerminalGate), act);
        let run = case.restore_with(row, far_deadline(), Some(Box::new(act)));
        let failure = run.failure(row);
        assert!(
            identity_changed(&failure.cause),
            "{row}: {:?}",
            failure.cause
        );
        assert_eq!(
            failure.last_completed,
            Some(PayloadPlaneStepV1::Evidence),
            "{row}"
        );
        assert_eq!(
            failure.payload_materialization_began, !captured_database,
            "{row}"
        );
        let sealed: &[&str] = if captured_database {
            &[]
        } else {
            &SEAL_ORDER_V1
        };
        assert_eq!(
            failure
                .partial
                .sealed
                .iter()
                .map(|one| lossy(&one.name))
                .collect::<Vec<_>>(),
            sealed,
            "{row}: the walk ran before every retained chain"
        );
        run.assert_seals_are_the_disk(row, &[]);
    }
}

// ---------------------------------------------------------------------------------------------
// The one deadline
// ---------------------------------------------------------------------------------------------

/// The terminal gate's own guards for the three-step frames, in order: its own, then each entry's before its
/// observation, each file's and directory's before its seal, and one after every call the walk makes before
/// it makes another or records a seal (post-order: a directory's observation and seal after everything
/// beneath it); then the retained chains' (one per object-store directory and copied file, metadata directory
/// and file, each of an admitted bootstrap file's two calls, each edge of every staging chain, HOME and XDG,
/// and bootstrap directory), and the root's own guards last, before and after its identity's lookup.
fn terminal_guards(plane: &RestoredGitPlaneV1) -> Vec<&'static str> {
    let entry = "payload-terminal-entry";
    // A file: its stat, its reopen, its digest (whose reads the digest guards itself), its closing `fstat`
    // (the seal's guard), its mode primitive, and its confirming `fstat`. A directory: its stat (the seal's
    // guard), its mode primitive, and its confirming `fstat`. A link: its stat and its `readlinkat`; its
    // closing stat is followed by the next entry's guard.
    let file = [
        entry,
        "payload-terminal-statted",
        "payload-terminal-reopened",
        "payload-terminal-digested",
        "payload-terminal-seal",
        "payload-terminal-mode-set",
        "payload-terminal-confirmed",
    ];
    let directory = [
        entry,
        "payload-terminal-seal",
        "payload-terminal-mode-set",
        "payload-terminal-confirmed",
    ];
    let link = [
        entry,
        "payload-terminal-statted",
        "payload-terminal-link-read",
    ];
    let mut guards = vec!["payload-terminal-gate"];
    // The worktree: `a`, `d/x`, then `d`, each observed and sealed, and `l` observed; the index; `e`, then
    // its slot, then `evidence/`, each observed and sealed.
    guards.extend(file.iter().chain(&file).chain(&directory).chain(&link));
    guards.extend(file);
    guards.extend(file.iter().chain(&directory).chain(&directory));
    let admitted = match &plane.bootstrap.objects {
        BootstrapObjectsV1::Captured { files, .. } => files.len(),
        BootstrapObjectsV1::Empty => 0,
    };
    guards.extend(std::iter::repeat_n(
        "objects-chain",
        5 + plane.objects.files.len(),
    ));
    guards.extend(std::iter::repeat_n(
        "payload-metadata-chain",
        plane.metadata.directories.len() + plane.metadata.files.len(),
    ));
    for _ in 0..admitted {
        guards.extend(["objects-source", "payload-source-statted"]);
    }
    // Each staging chain: the work directory's edge, the plain directory's, and one per component of its key.
    let staging = plane.capsule().destination.staging.borrow();
    for key in std::iter::once(&b""[..]).chain(staging.keys().map(Vec::as_slice)) {
        guards.extend(["payload-chain", "payload-chain-step"]);
        let components = if key.is_empty() {
            0
        } else {
            key.split(|byte| *byte == b'/').count()
        };
        guards.extend(std::iter::repeat_n("payload-chain-step", components));
    }
    guards.extend(std::iter::repeat_n(
        "payload-chain",
        2 + plane.bootstrap.directories.len(),
    ));
    guards.extend(["payload-root", "payload-root-statted"]);
    guards
}

/// The deadline in the terminal gate. A clean restore's guards are the materialization's, then exactly the
/// terminal gate's own. Each terminal guard halts the run alone with the typed timeout and `Evidence` the
/// last step completed, having sealed exactly the entries before it: its first guard, the first and last
/// entry's, the first and last seal's, the first and last retained chain's, the first later edge of a staging
/// chain's, and the root's two (before and after its lookup). A call of the walk that returns after the
/// deadline is never followed by another call or by the record of a seal: for each call (a file's stat,
/// reopen, opening `fstat`, digest, closing `fstat`, mode primitive, and confirming `fstat`; a link's stat,
/// `readlinkat`, and closing stat; a directory's stat, mode primitive, and confirming `fstat`), the deadline
/// is spent at the seam immediately before it (or, for the digest and a mode primitive, immediately after it
/// returns), and the run halts with the typed timeout with no later event at all: the next call's own seam
/// never runs, nothing more is sealed, and an entry whose mode primitive or confirmation returned late is not
/// recorded as sealed, though it has its final mode. The root's identity, the gate's last call, is no
/// exception (review fix 3): its lookup returning after the deadline (spent at the seam immediately after it
/// returns) halts the run with the typed timeout, `Evidence`, every entry sealed and reported, and the partial
/// kept, never success; an unchanged root looked up on time restores; and a replaced root refuses as an
/// identity change, on time or with its lookup returning late (a call's own refusal precedes the observation
/// after it, as for every call of the gate). On a Captured object database, the guard between an admitted
/// bootstrap file's two calls halts the run alone.
#[test]
fn c3_the_one_deadline_is_kept_before_every_entry_and_every_seal() {
    let capsule = frames_capsule();
    let frames = three_step_frames();
    // First, each row makes one call of the walk return late: the deadline is spent at `spend` (the `nth` event
    // of that seam for that name), and `next` (the `nth` event of that seam for that name) is the seam of the
    // call that would follow, which must never run. `sealed` entries were sealed before it; `set` is an entry
    // whose mode primitive ran but whose seal is not recorded.
    use PayloadHookPointV1::{
        TerminalDescriptor, TerminalDigested, TerminalEntry, TerminalLookup, TerminalModeSet,
        TerminalSealed,
    };
    type AtV1 = (PayloadHookPointV1, &'static str, usize);
    type SlowRowV1 = (&'static str, AtV1, AtV1, usize, Option<&'static str>);
    let (a, d, x, l) = (
        "repository/a",
        "repository/d",
        "repository/d/x",
        "repository/l",
    );
    let slow: [SlowRowV1; 13] = [
        (
            "a slow file lookup",
            (TerminalLookup, a, 0),
            (TerminalLookup, a, 1),
            0,
            None,
        ),
        (
            "a slow lookup",
            (TerminalLookup, x, 1),
            (TerminalDescriptor, x, 0),
            1,
            None,
        ),
        (
            "a slow opening fstat",
            (TerminalDescriptor, a, 0),
            (TerminalDigested, a, 0),
            0,
            None,
        ),
        (
            "a slow digest",
            (TerminalDigested, a, 0),
            (TerminalDescriptor, a, 1),
            0,
            None,
        ),
        (
            "a slow closing fstat",
            (TerminalDescriptor, a, 1),
            (TerminalDescriptor, a, 2),
            0,
            None,
        ),
        (
            "a slow file seal",
            (TerminalModeSet, a, 0),
            (TerminalDescriptor, a, 3),
            0,
            Some(a),
        ),
        (
            "a slow file confirmation",
            (TerminalDescriptor, a, 3),
            (TerminalSealed, a, 0),
            0,
            Some(a),
        ),
        (
            "a slow link lookup",
            (TerminalLookup, l, 0),
            (TerminalLookup, l, 1),
            3,
            None,
        ),
        (
            "a slow readlinkat",
            (TerminalLookup, l, 1),
            (TerminalLookup, l, 2),
            3,
            None,
        ),
        (
            "a slow closing link stat",
            (TerminalLookup, l, 2),
            (TerminalEntry, "repository/.git/index", 0),
            3,
            None,
        ),
        (
            "a slow directory lookup",
            (TerminalLookup, d, 0),
            (TerminalDescriptor, d, 0),
            2,
            None,
        ),
        (
            "a slow directory seal",
            (TerminalModeSet, d, 0),
            (TerminalDescriptor, d, 1),
            2,
            Some(d),
        ),
        (
            "a slow directory confirmation",
            (TerminalDescriptor, d, 1),
            (TerminalSealed, d, 0),
            2,
            Some(d),
        ),
    ];
    let seams = [
        TerminalEntry,
        TerminalLookup,
        TerminalDescriptor,
        TerminalDigested,
        TerminalModeSet,
        TerminalSealed,
    ];
    std::thread::scope(|scope| {
        let handles: Vec<_> = slow
            .into_iter()
            .map(|(row, spend_at, next, sealed, set)| {
                let (capsule, frames) = (&capsule, &frames);
                scope.spawn(move || {
                    let case = PCaseV1::with_frames(capsule, frames);
                    let root = case.dest.path.clone();
                    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
                    let fired: FiredV1 = StdRc::new(StdCell::new(None));
                    let act = StdRc::new(spend(deadline, &fired));
                    // Every terminal event in order, and how many had run when the deadline was spent.
                    let events: StdRc<StdRefCell<Vec<(PayloadHookPointV1, PathBuf)>>> =
                        StdRc::default();
                    let spent: StdRc<StdCell<Option<usize>>> = StdRc::default();
                    let _seams: Vec<_> = seams
                        .iter()
                        .map(|&point| {
                            let (events, spent, act) = (events.clone(), spent.clone(), act.clone());
                            let (target, seen) = (root.join(spend_at.1), StdCell::new(0_usize));
                            install_payload_hook_for_test(point, move |event| {
                                events.borrow_mut().push((point, event.path.to_path_buf()));
                                if point == spend_at.0 && event.path == target {
                                    if seen.get() == spend_at.2 {
                                        spent.set(Some(events.borrow().len()));
                                        (*act)();
                                    }
                                    seen.set(seen.get() + 1);
                                }
                            })
                        })
                        .collect();
                    let run = case.restore_with(row, deadline, None);
                    assert_eq!(
                        fired.get(),
                        Some(true),
                        "{row}: INVALID (not behavioral evidence): the hook was not reached before the deadline"
                    );
                    let events = events.borrow();
                    let (point, name, nth) = next;
                    let next_path = root.join(name);
                    assert_eq!(
                        events
                            .iter()
                            .filter(|(one, path)| *one == point && *path == next_path)
                            .count(),
                        nth,
                        "{row}: the next call's seam ({point:?} #{nth} of {name}) never runs"
                    );
                    let after = &events[spent.get().expect("the deadline was spent")..];
                    assert!(
                        after.is_empty(),
                        "{row}: no event follows the late call: {after:?}"
                    );
                    refused_at_the_terminal_gate(
                        &run,
                        row,
                        |cause| matches!(cause, PayloadPlaneErrorV1::Timeout),
                        &SEAL_ORDER_V1[..sealed],
                    );
                    let Some(set) = set else {
                        run.assert_seals_are_the_disk(row, &[]);
                        return;
                    };
                    run.assert_seals_are_the_disk(row, &[set.as_bytes()]);
                    let mode = |tree: &DiskTreeV1| match tree.get(set.as_bytes()) {
                        Some(DiskV1::Dir(mode) | DiskV1::File(mode, _)) => *mode,
                        other => panic!("{row}: {set} is {other:?}"),
                    };
                    assert_eq!(
                        mode(&run.held(row).now()),
                        mode(&expected_final(&run.failure(row).partial.plane)),
                        "{row}: {set} has its final mode, which is not recorded as sealed"
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("a slow row's thread");
        }
    });

    // The root's identity, the gate's last call: `late` spends the deadline at the seam immediately after its
    // lookup returns; `replaced` swaps the destination root for a new directory just before the terminal gate,
    // which only that lookup can see (everything else goes through retained pins).
    let root_rows: [(&str, bool, bool); 4] = [
        ("an unchanged root, on time", false, false),
        ("a slow root lookup", false, true),
        ("a replaced root, on time", true, false),
        ("a replaced root, its lookup late", true, true),
    ];
    std::thread::scope(|scope| {
        let handles: Vec<_> = root_rows
            .into_iter()
            .map(|(row, replaced, late)| {
                let (capsule, frames) = (&capsule, &frames);
                scope.spawn(move || {
                    let case = PCaseV1::with_frames(capsule, frames);
                    let root = case.dest.path.clone();
                    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
                    let fired: FiredV1 = StdRc::new(StdCell::new(None));
                    let act = spend(deadline, &fired);
                    let looked: StdRc<StdRefCell<Vec<PathBuf>>> = StdRc::default();
                    let _seam = {
                        let looked = looked.clone();
                        install_payload_hook_for_test(
                            PayloadHookPointV1::TerminalRootStatted,
                            move |event| {
                                looked.borrow_mut().push(event.path.to_path_buf());
                                if late {
                                    act();
                                }
                            },
                        )
                    };
                    let swap = replaced.then(|| {
                        Box::new(drifting_when(
                            &case,
                            at_step(PayloadPlaneStepV1::TerminalGate),
                            |root| swap_out(root, root),
                        )) as StepActV1
                    });
                    let run = case.restore_with(row, deadline, swap);
                    assert_eq!(
                        *looked.borrow(),
                        [root],
                        "{row}: the root's identity is looked up once, last"
                    );
                    if late {
                        assert_eq!(
                            fired.get(),
                            Some(true),
                            "{row}: INVALID (not behavioral evidence): the seam was not reached before the deadline"
                        );
                    }
                    if !replaced && !late {
                        run.assert_inventory_is_the_disk(row);
                        return;
                    }
                    let is: fn(&PayloadPlaneErrorV1) -> bool = if replaced {
                        identity_changed
                    } else {
                        |cause| matches!(cause, PayloadPlaneErrorV1::Timeout)
                    };
                    refused_at_the_terminal_gate(&run, row, is, &SEAL_ORDER_V1);
                    run.assert_seals_are_the_disk(row, &[]);
                    run.assert_partial_is_the_disk(row);
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("a root row's thread");
        }
    });

    let materialized = StdRc::new(StdRefCell::new(Vec::new()));
    {
        let case = PCaseV1::with_frames(&capsule, &frames);
        let _labels = install_git_hook_for_test(
            GitHookPointV1::DeadlineObserved,
            guard_labels(&materialized),
        );
        let _ = case.write("materialized").success("materialized");
    }
    let labels = StdRc::new(StdRefCell::new(Vec::new()));
    let case = PCaseV1::with_frames(&capsule, &frames);
    let expected = terminal_guards(&case.plane);
    {
        let _labels =
            install_git_hook_for_test(GitHookPointV1::DeadlineObserved, guard_labels(&labels));
        let _ = case.restore("clean guards").success("clean guards");
    }
    let prefix = materialized.borrow().clone();
    let labels = labels.borrow().clone();
    assert_eq!(
        labels[..prefix.len()],
        prefix[..],
        "the materialization's guards"
    );
    assert_eq!(
        labels[prefix.len()..],
        expected[..],
        "the terminal gate's guards"
    );

    // The occurrence of `label` that is the terminal gate's `k`th (counting from its end when `from_end`).
    let terminal = |label: &str, k: usize, from_end: bool| -> usize {
        let before = prefix.iter().filter(|one| **one == label).count();
        let within = expected.iter().filter(|one| **one == label).count();
        before + if from_end { within - 1 - k } else { k }
    };
    let rows: Vec<(&'static str, usize, usize)> = vec![
        (
            "payload-terminal-gate",
            terminal("payload-terminal-gate", 0, false),
            0,
        ),
        (
            "payload-terminal-entry",
            terminal("payload-terminal-entry", 0, false),
            0,
        ),
        (
            "payload-terminal-entry",
            terminal("payload-terminal-entry", 0, true),
            6,
        ),
        (
            "payload-terminal-seal",
            terminal("payload-terminal-seal", 0, false),
            0,
        ),
        (
            "payload-terminal-seal",
            terminal("payload-terminal-seal", 0, true),
            6,
        ),
        ("objects-chain", terminal("objects-chain", 0, false), 7),
        (
            "payload-chain-step",
            terminal("payload-chain-step", 0, false),
            7,
        ),
        ("payload-chain", terminal("payload-chain", 0, true), 7),
        ("payload-root", terminal("payload-root", 0, true), 7),
        (
            "payload-root-statted",
            terminal("payload-root-statted", 0, true),
            7,
        ),
    ];
    std::thread::scope(|scope| {
        let handles: Vec<_> = rows
            .into_iter()
            .map(|(label, nth, sealed)| {
                let (capsule, frames) = (&capsule, &frames);
                scope.spawn(move || {
                    let row = format!("{label} #{nth}");
                    let case = PCaseV1::with_frames(capsule, frames);
                    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
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
                    let run = case.restore_with(&row, deadline, None);
                    assert_eq!(
                        fired.get(),
                        Some(true),
                        "{row}: INVALID (not behavioral evidence): the guard was not reached before the deadline"
                    );
                    refused_at_the_terminal_gate(
                        &run,
                        &row,
                        |cause| matches!(cause, PayloadPlaneErrorV1::Timeout),
                        &SEAL_ORDER_V1[..sealed],
                    );
                    run.assert_seals_are_the_disk(&row, &[]);
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("a halting row's thread");
        }
    });

    // A Captured object database (which restores no phase-P class): the guard between an admitted bootstrap
    // file's by-name stat and its retained descriptor's `fstat` halts the run alone, with nothing created or
    // sealed.
    let row = "payload-source-statted #0";
    let captured_capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let case = PCaseV1::of(PlaneInV1::rich(
        &captured_capsule,
        CustodyGitObjectFormatV1::Sha1,
    ));
    let (_clock, deadline) = DeadlineClockV1::with_headroom(P_WRITE_DEADLINE_V1);
    let fired: FiredV1 = StdRc::new(StdCell::new(None));
    let act = spend(deadline, &fired);
    let guard = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "payload-source-statted" {
            act();
        }
    });
    let run = case.restore_with(row, deadline, None);
    drop(guard);
    assert_eq!(
        fired.get(),
        Some(true),
        "{row}: INVALID (not behavioral evidence): the guard was not reached before the deadline"
    );
    let failure = run.failure(row);
    assert!(
        matches!(failure.cause, PayloadPlaneErrorV1::Timeout),
        "{row}: {:?}",
        failure.cause
    );
    assert_eq!(
        failure.last_completed,
        Some(PayloadPlaneStepV1::Evidence),
        "{row}"
    );
    assert!(!failure.payload_materialization_began, "{row}");
    assert!(failure.partial.sealed.is_empty(), "{row}");
}

// ---------------------------------------------------------------------------------------------
// The rich clone, V to G to P
// ---------------------------------------------------------------------------------------------

/// The rich plan-backed clone given a REAL split index and a worktree symlink (as C2b's byte-exact control
/// gives it), with `plant` applied to its source before it is planned and exported, carried through phase V
/// and phase G: ready for phase P. The fixture is returned so its area outlives the case.
fn rich_case(plant: impl FnOnce(&Path)) -> (PlanFixtureV1, PathBuf, PCaseV1) {
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
    std::os::unix::fs::symlink("README", clone.join("readme-link")).expect("a worktree link");
    plant(&clone);
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    let case = PCaseV1::sealed(&fixture.capsule_dir());
    (fixture, clone, case)
}

/// How many descriptors this process has open (process-wide: exact only when nothing else runs).
fn open_descriptors() -> usize {
    std::fs::read_dir("/dev/fd").map_or(0, Iterator::count)
}

/// End to end, V to G to P, on the rich clone with a real split index and a worktree symlink: the restore
/// succeeds with every entry byte-exact at its final mode (the worktree's frame modes, `0600` for `index` and
/// the shared index, `0400`/`0500` for evidence), the inventories are exactly the disk (identities, modes,
/// lengths, SHA-256, targets, pins), and they report the skipped connector, the childless directories, no
/// external reference, every class Captured, and each frame mode as the frame says. The source's own bytes
/// agree. The one ledger is phase V's charge plus phase G's and phase P's on-disk charge (the runner's own
/// accounting, restated here with its figures). Prints the descriptors phase P's value retains, the
/// process-wide measure (exact only when this control runs alone), and the soft `RLIMIT_NOFILE`
/// (`PAYLOAD-DESCRIPTORS`).
#[test]
fn c3_the_rich_clone_restores_end_to_end_on_the_one_ledger() {
    let baseline = open_descriptors();
    let (_fixture, clone, case) = rich_case(|_| {});
    let shared = sharedindex_names(&clone.join(".git"));
    assert_eq!(shared.len(), 1, "real Git wrote one shared index");
    let (phase_v, ledger_before) = (case.phase_v, case.plane.ledger_used());
    let with_case = open_descriptors();
    let run = case.restore("rich");
    let done = run.success("rich");
    let now = run.held("rich").now();
    assert_eq!(
        now,
        expected_final(&done.plane),
        "byte-exact, every entry at its final mode"
    );
    run.assert_inventory_is_the_disk("rich");

    let captured = |class: ClassV1| PayloadClassStateV1::Captured {
        staged: staged_index(done.plane.capsule(), class),
    };
    assert_eq!(done.worktree.state, captured(P_WORKTREE_V1));
    assert_eq!(done.index.state, captured(P_INDEX_V1));
    assert_eq!(
        done.evidence.classes,
        [
            (P_OPERATIONS_V1, captured(P_OPERATIONS_V1)),
            (P_CONFIG_V1, captured(P_CONFIG_V1)),
            (P_BRIDGE_V1, captured(P_BRIDGE_V1)),
        ]
    );
    assert_eq!(
        done.worktree.root_git,
        RootGitDispositionV1::ConnectorDirectorySkipped
    );
    assert!(done.declared_external_references.is_empty());
    let worktree = own_contents(&done.plane, P_WORKTREE_V1);
    let own_childless: Vec<Vec<u8>> = worktree
        .iter()
        .filter(|(path, kind, _)| {
            matches!(kind, OwnKindV1::Dir(_))
                && path != b".git"
                && !worktree.iter().any(|(other, _, _)| {
                    other.len() > path.len() && other.starts_with(path) && other[path.len()] == b'/'
                })
        })
        .map(|(path, _, _)| path.clone())
        .collect();
    assert_eq!(done.worktree.childless_directories, own_childless);
    let frame_modes = |class: ClassV1| -> BTreeMap<Vec<u8>, Option<u32>> {
        own_contents(&done.plane, class)
            .into_iter()
            .map(|(path, kind, _)| match kind {
                OwnKindV1::Dir(mode) | OwnKindV1::File(mode, _) => (path, Some(mode)),
                OwnKindV1::Link(_) => (path, None),
            })
            .collect()
    };
    let check = |entries: &[RestoredEntryV1], class: ClassV1| {
        let modes = frame_modes(class);
        for entry in entries {
            assert_eq!(
                Some(&entry.frame_mode),
                modes.get(&entry.path),
                "{}",
                lossy(&entry.path)
            );
        }
    };
    check(&done.worktree.entries, P_WORKTREE_V1);
    check(&done.index.files, P_INDEX_V1);
    assert_eq!(
        done.index
            .files
            .iter()
            .map(|entry| (entry.path.clone(), entry.mode))
            .collect::<Vec<_>>(),
        [
            (b"index".to_vec(), Some(0o600)),
            (shared[0].clone(), Some(0o600))
        ]
    );
    for (slot, class) in done
        .evidence
        .slots
        .iter()
        .zip([P_OPERATIONS_V1, P_CONFIG_V1, P_BRIDGE_V1])
    {
        assert_eq!(slot.slot, EvidenceSlotV1::of_class(class).expect("a slot"));
        assert_eq!(slot.mode, 0o500);
        check(&slot.entries, class);
    }
    for (name, source) in [
        (
            joined(b"repository/.git", &shared[0]),
            clone.join(".git").join(OsStr::from_bytes(&shared[0])),
        ),
        (b"repository/README".to_vec(), clone.join("README")),
        (
            b"evidence/bridge_evidence/a2a-bridge/evidence.json".to_vec(),
            clone.join(".git/a2a-bridge/evidence.json"),
        ),
    ] {
        let Some(DiskV1::File(_, bytes)) = now.get(&name) else {
            panic!("{} is restored", lossy(&name));
        };
        assert_eq!(
            *bytes,
            std::fs::read(&source).expect("the source file"),
            "{} is the source's bytes",
            lossy(&name)
        );
    }
    assert_eq!(
        now.get(&b"repository/readme-link"[..]),
        Some(&DiskV1::Link(b"README".to_vec()))
    );

    let phase_p = disk_charge(&now);
    let phase_g = ledger_before - phase_v;
    assert_eq!(
        done.plane.ledger_used(),
        phase_v + phase_g + phase_p,
        "the one ledger: phase V, phase G, phase P"
    );
    println!(
        "PAYLOAD-ONE-LEDGER phase_v={phase_v} phase_g_on_disk={phase_g} phase_p_on_disk={phase_p} \
         ledger={}",
        done.plane.ledger_used()
    );

    let pins = &done.pins;
    let slot_directories: usize = pins
        .evidence_slots
        .iter()
        .map(|slot| slot.directories.len())
        .sum();
    let phase_p_pins = pins.worktree.len()
        + usize::from(pins.evidence.is_some())
        + pins.evidence_slots.len()
        + slot_directories;
    let worktree_pins = pins.worktree.len();
    let RestoredRunV1 {
        _unseal,
        dest,
        held,
        outcome,
    } = run;
    drop(held);
    let with_value = open_descriptors();
    drop(outcome);
    let without = open_descriptors();
    let soft = Command::new("/bin/sh")
        .args(["-c", "ulimit -n"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    println!(
        "PAYLOAD-DESCRIPTORS phase_p_pins={phase_p_pins} worktree_dirs={} evidence_dirs={} \
         process_baseline={baseline} process_with_plane={with_case} \
         process_with_restored_value={with_value} process_after_drop={without} soft_nofile={soft}",
        worktree_pins,
        phase_p_pins - worktree_pins
    );
    drop((_unseal, dest));
}

/// Evidence is inert: every file beneath `evidence/` ends `0400` and every directory `0500` (`evidence/` and
/// each slot included), whatever the frame says, and the inventory keeps each original frame mode beside the
/// mode written. The rich clone's executable hook (frame mode `0755`) is never executable: `0600` when it is
/// created and at every gate, `0400` once sealed and after.
#[test]
fn c3_evidence_ends_inert_and_the_rich_clones_hook_is_never_executable() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_fixture, _clone, case) = rich_case(|_| {});
    let hook = case
        .dest
        .path
        .join("evidence/git_configuration_and_hooks/hooks/pre-commit");
    let modes = StdRc::new(StdRefCell::new(Vec::<(String, u32)>::new()));
    let sample = {
        let (modes, hook) = (modes.clone(), hook.clone());
        move |when: String| {
            if let Ok(metadata) = std::fs::symlink_metadata(&hook) {
                modes
                    .borrow_mut()
                    .push((when, metadata.permissions().mode() & 0o7777));
            }
        }
    };
    let at = |point: &'static str| {
        let (sample, hook) = (sample.clone(), hook.clone());
        move |event: &PayloadHookEventV1<'_>| {
            if event.path == hook {
                sample(point.to_owned());
            }
        }
    };
    let _created = install_payload_hook_for_test(PayloadHookPointV1::EntryCreated, at("created"));
    let _sealed = install_payload_hook_for_test(PayloadHookPointV1::TerminalSealed, at("sealed"));
    let gates = sample.clone();
    let act: StepActV1 = Box::new(move |event| gates(format!("{:?} gate", event.step)));
    let run = case.restore_with("evidence", far_deadline(), Some(act));
    let done = run.success("evidence");
    let now = run.held("evidence").now();
    let evidence: Vec<(&Vec<u8>, &DiskV1)> = now
        .iter()
        .filter(|(name, _)| name.starts_with(b"evidence"))
        .collect();
    assert!(evidence.len() > 3, "the rich clone has evidence");
    for (name, entry) in evidence {
        match entry {
            DiskV1::Dir(mode) => assert_eq!(*mode, 0o500, "{}", lossy(name)),
            DiskV1::File(mode, _) => assert_eq!(*mode, 0o400, "{}", lossy(name)),
            DiskV1::Link(_) => {}
        }
    }
    let config = done
        .evidence
        .slots
        .iter()
        .find(|slot| slot.slot == EvidenceSlotV1::GitConfigurationAndHooks)
        .expect("the configuration slot");
    let frame: BTreeMap<Vec<u8>, OwnKindV1> = own_contents(&done.plane, P_CONFIG_V1)
        .into_iter()
        .map(|(path, kind, _)| (path, kind))
        .collect();
    for entry in &config.entries {
        let (frame_mode, written) = match frame.get(&entry.path) {
            Some(OwnKindV1::Dir(mode)) => (Some(*mode), Some(0o500)),
            Some(OwnKindV1::File(mode, _)) => (Some(*mode), Some(0o400)),
            Some(OwnKindV1::Link(_)) => (None, None),
            None => panic!("{} is in the frame", lossy(&entry.path)),
        };
        assert_eq!(
            (entry.frame_mode, entry.mode),
            (frame_mode, written),
            "{}",
            lossy(&entry.path)
        );
    }
    let row = row_at(&config.entries, b"hooks/pre-commit");
    assert_eq!((row.frame_mode, row.mode), (Some(0o755), Some(0o400)));
    let Some(DiskV1::File(after, _)) =
        now.get(&b"evidence/git_configuration_and_hooks/hooks/pre-commit"[..])
    else {
        panic!("the hook is restored");
    };
    let mut seen = modes.borrow().clone();
    seen.push(("after".to_owned(), *after));
    assert_eq!(
        seen,
        [
            ("created".to_owned(), 0o600),
            ("Some(Evidence) gate".to_owned(), 0o600),
            ("Some(TerminalGate) gate".to_owned(), 0o600),
            ("sealed".to_owned(), 0o400),
            ("after".to_owned(), 0o400),
        ],
        "the hook's mode at every observation"
    );
    assert!(
        seen.iter().all(|(_, mode)| mode & 0o111 == 0),
        "the hook is never executable"
    );
}

/// The token every planted marker carries, so its bytes are recognized wherever they land.
const PLANTED_TOKEN_V1: &[u8] = b"A2A-C3-PLANTED-MARKER";

/// The rich clone's source with planted active configuration: `core.fsmonitor`, `core.pager`, an alias, a
/// required `filter.marker` (clean, smudge, process) with `info/attributes` routing every path through it, an
/// `[include]` of a file that sets `core.editor`, and executable `post-checkout`, `post-merge`, and
/// `reference-transaction` hooks: each runs a marker script that records its own name, and the script is
/// proven to record before the export. Whatever the 2B2 planning and export children did with the source is
/// not phase P's, so the record starts empty after it. V to G to P then restores: no marker ran, the planted
/// bytes are byte-exact inert evidence at `0400` beneath `evidence/git_configuration_and_hooks/`, and nothing
/// beneath `repository/` holds a planted name or byte (its `.git/config` is phase G's own, with no hooks
/// directory and no `info/`).
#[test]
fn c3_planted_config_hook_filter_and_fsmonitor_markers_never_run() {
    use std::os::unix::fs::PermissionsExt as _;
    let markers = tempfile::TempDir::new().expect("a marker area");
    let area = markers.path().canonicalize().expect("a canonical area");
    let tokened = area.join(OsStr::from_bytes(PLANTED_TOKEN_V1));
    std::fs::create_dir(&tokened).expect("the token directory");
    let ran = area.join("ran");
    std::fs::create_dir(&ran).expect("the record");
    let script = tokened.join("mark.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'/\"$1\"\nexit 0\n",
            ran.display()
        ),
    )
    .expect("the marker script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("the marker script is executable");
    let recorded = || -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&ran)
            .expect("the record")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    };
    assert!(
        retry_on_text_file_busy(|| Command::new(&script).arg("probe").status())
            .expect("the marker runs")
            .success()
    );
    assert_eq!(recorded(), ["probe"], "the marker records when it runs");
    std::fs::remove_file(ran.join("probe")).expect("the probe's record");

    let run = script.display().to_string();
    let hooks = ["post-checkout", "post-merge", "reference-transaction"];
    let (_fixture, clone, case) = rich_case(|clone| {
        let git = clone.join(".git");
        let mut config = std::fs::read(git.join("config")).expect("the source config");
        config.extend(
            format!(
                "[core]\n\tfsmonitor = {run} fsmonitor\n\tpager = {run} pager\n\
                 [filter \"marker\"]\n\tclean = {run} filter-clean\n\tsmudge = {run} filter-smudge\n\
                 \tprocess = {run} filter-process\n\trequired = true\n\
                 [alias]\n\tmarker = !{run} alias\n\
                 [include]\n\tpath = {}\n",
                tokened.join("included.config").display()
            )
            .into_bytes(),
        );
        std::fs::write(git.join("config"), config).expect("the planted config");
        std::fs::write(
            tokened.join("included.config"),
            format!("[core]\n\teditor = {run} editor\n"),
        )
        .expect("the included config");
        for hook in hooks {
            let path = git.join("hooks").join(hook);
            std::fs::write(&path, format!("#!/bin/sh\n{run} hook-{hook}\n")).expect("a hook");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("the hook is executable");
        }
        std::fs::create_dir_all(git.join("info")).expect("info/");
        let mut attributes = PLANTED_TOKEN_V1.to_vec();
        attributes.splice(0..0, b"# ".iter().copied());
        attributes.extend_from_slice(b"\n* filter=marker\n");
        std::fs::write(git.join("info/attributes"), attributes).expect("the attributes");
    });
    let during_export = recorded();
    println!("PAYLOAD-MARKERS during_planning_and_export={during_export:?}");
    for name in &during_export {
        std::fs::remove_file(ran.join(name)).expect("a record from before the restore");
    }

    let run = case.restore("planted");
    let _ = run.success("planted");
    assert_eq!(recorded(), Vec::<String>::new(), "no planted marker ran");
    let now = run.held("planted").now();
    let holds_token = |bytes: &[u8]| {
        bytes
            .windows(PLANTED_TOKEN_V1.len())
            .any(|window| window == PLANTED_TOKEN_V1)
    };
    let mut planted = vec!["config".to_owned(), "info/attributes".to_owned()];
    planted.extend(hooks.iter().map(|hook| format!("hooks/{hook}")));
    for leaf in &planted {
        let name = joined(b"evidence/git_configuration_and_hooks", leaf.as_bytes());
        let Some(DiskV1::File(mode, bytes)) = now.get(&name) else {
            panic!("{} is restored as evidence", lossy(&name));
        };
        assert_eq!(*mode, 0o400, "{leaf}: inert");
        assert!(holds_token(bytes), "{leaf}: the planted bytes");
        assert_eq!(
            *bytes,
            std::fs::read(clone.join(".git").join(leaf)).expect("the planted source"),
            "{leaf}: byte-exact"
        );
    }
    let outside = disk_tree_where(&run.dest.path, |name| !in_phase_p_planes(name));
    for (name, entry) in now.iter().chain(outside.iter()) {
        if !name.starts_with(b"repository") {
            continue;
        }
        let bytes: &[u8] = match entry {
            DiskV1::File(_, bytes) | DiskV1::Link(bytes) => bytes,
            DiskV1::Dir(_) => &[],
        };
        assert!(
            !holds_token(bytes) && !holds_token(name),
            "{} holds a planted marker",
            lossy(name)
        );
        assert!(
            !hooks.iter().any(|hook| name.ends_with(hook.as_bytes())),
            "{} is a planted hook",
            lossy(name)
        );
    }
    for absent in ["repository/.git/hooks", "repository/.git/info"] {
        assert!(
            std::fs::symlink_metadata(run.dest.path.join(absent)).is_err(),
            "{absent} does not exist"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Structural controls
// ---------------------------------------------------------------------------------------------

/// STRUCTURAL ONLY (evidence of shape and order, never of behavior; the behavioral evidence is every row
/// above). The terminal gate's section creates, writes, renames, removes, charges, and runs Git nowhere, and
/// changes a mode only through a descriptor (`set_exact_file_mode`, `set_exact_mode`), never by path. The gate
/// is the census, the walk of the worktree, the index, and the evidence, then the retained chains (each call
/// behind its own guard), and the root's identity LAST, its guard immediately before it and the deadline
/// observed immediately after it (review fix 3), with nothing else after it; a file is observed by name,
/// reopened, digested, guarded, and only then sealed, and nothing is looked up after its seal; a directory (a
/// worktree or evidence directory, a slot, `evidence/`) is observed by name only after its subtree and
/// immediately before its seal; the deadline is observed after each call of a file's, a link's, and a
/// directory's before the next, and before a seal is recorded; and nothing fallible follows the gate but
/// building the result.
#[test]
fn c3_the_terminal_gate_section_seals_through_descriptors_in_its_order() {
    let source = include_str!("custody_restore_payload.rs");
    let (_before, section) = source.split_once(P_SEAL_MARKER_V1).expect("the C3 marker");
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
        "create_new",
        "create_staging_directory",
        "symlink_child(",
        "set_permissions",
        "rename",
        "remove_",
        "unlink",
        "set_len",
        "truncate",
        "write_all",
        ".write(",
        "reserve",
        ".release(",
        "sync_all",
        "GitRunnerV1",
        "GitRunRequestV1",
        "Command",
        "spawn",
        "created_chains(",
        " gate(",
        "unsafe",
    ] {
        assert!(
            !code.contains(forbidden),
            "the terminal gate's section mentions `{forbidden}`"
        );
    }
    let at = |body: &str, needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("`{needle}` is in `{body}`"))
    };
    let gate = body_of(source, "fn terminal_gate(");
    let order = [
        "pace(\"payload-terminal-gate\")",
        "refuse_mounts_within(",
        "seal_tree(&pins.repository",
        "seal_tree(&pins.git",
        "seal_evidence(",
        "output_chains(",
        "metadata_chains(",
        "terminal_source_files_unchanged(",
        "terminal_retained_chains(",
        "pace(\"payload-root\")",
        "terminal_root_unchanged(",
    ];
    // The chains make one call per guard: the shared sweeps that make several behind one are the step
    // gates', not the terminal gate's.
    for (paced, shared) in [
        (
            "terminal_source_files_unchanged(",
            "source_files_unchanged(",
        ),
        ("terminal_retained_chains(", "retained_chains("),
    ] {
        assert_eq!(
            gate.matches(shared).count(),
            gate.matches(paced).count(),
            "the terminal gate calls `{paced}`, never `{shared}` itself"
        );
    }
    let staging = body_of(source, "fn terminal_retained_chains(");
    let calls: Vec<&str> = staging.split("recheck_entry(").collect();
    assert!(
        calls.len() > 1
            && calls[..calls.len() - 1]
                .iter()
                .all(|before| before.contains("pace(")),
        "every call of the terminal gate's staging, HOME, XDG, and bootstrap chains has its own guard"
    );
    let source_files = body_of(source, "fn terminal_source_files_unchanged(");
    let order_of_source = [
        "pace(\"objects-source\")",
        "child_metadata_no_follow(",
        "pace(\"payload-source-statted\")",
        "admitted.file.metadata()",
    ];
    for pair in order_of_source.windows(2) {
        assert!(
            at(source_files, pair[0]) < at(source_files, pair[1]),
            "`{}` precedes `{}` for an admitted file",
            pair[0],
            pair[1]
        );
    }
    for pair in order.windows(2) {
        assert!(
            at(gate, pair[0]) < at(gate, pair[1]),
            "`{}` precedes `{}`",
            pair[0],
            pair[1]
        );
    }
    let tail = &gate[at(gate, "terminal_root_unchanged(")..];
    assert!(
        !tail.contains('?') && !tail.contains(';') && !gate.contains("pinned_root_unchanged("),
        "the root's identity, through the helper that observes the deadline after it, is the gate's tail \
         expression: {tail}"
    );
    // Review fix 3: the root's lookup is the gate's last filesystem call, and the deadline observed after it
    // returns is the helper's tail, so a lookup that returns late is a timeout, never success.
    let root = body_of(source, "fn terminal_root_unchanged(");
    let order_of_root = [
        "pinned_root_unchanged(",
        "PayloadHookPointV1::TerminalRootStatted",
        "IdentityChanged",
        "pace(\"payload-root-statted\")",
    ];
    for pair in order_of_root.windows(2) {
        assert!(
            at(root, pair[0]) < at(root, pair[1]),
            "`{}` precedes `{}` for the root's identity",
            pair[0],
            pair[1]
        );
    }
    let observed = &root[at(root, "pace(\"payload-root-statted\")")..];
    assert!(
        root.matches("pinned_root_unchanged(").count() == 1
            && !observed.contains('?')
            && !observed.contains(';'),
        "the deadline observed after the root's one lookup is the helper's tail expression: {observed}"
    );
    let file = body_of(source, "fn seal_file(");
    let order = [
        "self.enter(",
        "recheck_created(",
        "pace(\"payload-terminal-statted\")",
        "open_regular_file(",
        "pace(\"payload-terminal-reopened\")",
        "stream_digest(",
        "ContentChanged",
        "pace(\"payload-terminal-digested\")",
        "pace(\"payload-terminal-seal\")",
        "set_exact_file_mode(",
        "pace(\"payload-terminal-mode-set\")",
        "confirm_sealed(",
    ];
    for pair in order.windows(2) {
        assert!(
            at(file, pair[0]) < at(file, pair[1]),
            "`{}` precedes `{}` in a file's seal",
            pair[0],
            pair[1]
        );
    }
    let sealed = &file[at(file, "set_exact_file_mode(")..];
    for lookup in ["recheck_created(", "open_regular_file(", "lookup("] {
        assert!(
            !sealed.contains(lookup),
            "nothing is looked up after a file's seal: `{lookup}`"
        );
    }
    let directory = body_of(source, "fn seal_directory(");
    assert!(
        at(directory, "pace(\"payload-terminal-seal\")") < at(directory, ".set_exact_mode(")
            && at(directory, ".set_exact_mode(") < at(directory, "pace(\"payload-terminal-mode-set\")")
            && at(directory, "pace(\"payload-terminal-mode-set\")")
                < at(directory, ".current_metadata(")
            && !directory.contains("recheck_")
            && !directory.contains("lookup("),
        "a directory is sealed through its pin after its guard, with no lookup, and the deadline is observed \
         between its mode primitive and its confirming `fstat`"
    );
    let confirm = body_of(source, "fn confirm_sealed(");
    assert!(
        at(confirm, "pace(\"payload-terminal-confirmed\")") < at(confirm, "self.sealed.push("),
        "a seal is recorded only after the deadline is observed after its confirming `fstat`"
    );
    let link = body_of(source, "fn verify_symlink(");
    let order_of_link = [
        "self.enter(",
        "recheck_created(",
        "pace(\"payload-terminal-statted\")",
        "read_child_symlink(",
        "pace(\"payload-terminal-link-read\")",
    ];
    for pair in order_of_link.windows(2) {
        assert!(
            at(link, pair[0]) < at(link, pair[1]),
            "`{}` precedes `{}` for a link",
            pair[0],
            pair[1]
        );
    }
    assert!(
        at(link, "pace(\"payload-terminal-link-read\")")
            < link.rfind("recheck_created(").expect("the closing stat"),
        "a link's closing stat follows the guard after its `readlinkat`"
    );
    let tree = body_of(source, "fn seal_tree(");
    assert!(
        tree.matches("observe_directory(").count() == 1
            && at(tree, "open.pop()") < at(tree, "self.observe_directory(")
            && at(tree, "self.observe_directory(") < at(tree, "self.seal_directory("),
        "a directory's one observation is made when it is popped, after its children, before its seal"
    );
    let evidence = body_of(source, "fn seal_evidence(");
    let order = [
        "self.seal_tree(&slot.pin",
        "self.observe_directory(directory,",
        "self.seal_directory(&slot.pin,",
        "self.observe_directory(root,",
        "self.seal_directory(directory,",
    ];
    for pair in order.windows(2) {
        assert!(
            at(evidence, pair[0]) < at(evidence, pair[1]),
            "`{}` precedes `{}` in the evidence walk",
            pair[0],
            pair[1]
        );
    }
    let entry = body_of(source, "pub(crate) fn restore_payload_plane_bounded_v1(");
    let arm = &entry[at(entry, "Ok(()) =>")..];
    assert!(
        arm.starts_with("Ok(()) => Ok(restored(plane, &plan, created)),"),
        "nothing fallible follows the terminal gate but building the result"
    );
    let build = body_of(source, "fn restored(");
    assert!(
        !build.contains('?') && !build.contains("Result"),
        "building the result is infallible"
    );
}
