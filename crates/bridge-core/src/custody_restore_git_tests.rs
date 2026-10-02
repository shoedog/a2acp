//! In-crate controls for ADR-0041 slice 2B3b pass A: the isolated Git object bootstrap.
//!
//! Every value here is a real 2B2 export carried through the real 2B3a reader (phase V) and then
//! through the real Git runner. A fixture Git route (a sealed shell script that records each child
//! and delegates to the lane Git) is used only to tamper with ONE child's output or effects, and
//! its log proves the intended child started. The mutation matrix in
//! `.git/a2a-evidence/2b3b-pass-a/` maps each guard to the controls that turn red without it.

use super::git::*;
use super::*;
use crate::custody_envelope_fixture::{
    fixture_envelope_format_v1, FixtureOpenerV1, FixtureSealerV1,
};
use crate::custody_export::tests::{expect_sealed, PlanFixtureV1};
use crate::custody_export::{
    export_capsule_v1, index_pack_logical_bound, CustodyCaptureCapabilityV1,
    CustodyExportBudgetsV1, CustodyExportRequestV1, CustodyQuiescenceDecisionV1,
    GitDirectoryBudgetV1, ENTRY_ALLOWANCE_BYTES_V1, INDEX_PACK_ENTRIES_V1, INIT_BARE_ENTRIES_V1,
    INIT_BARE_LOGICAL_BYTES_V1,
};
use crate::custody_frame::{CustodyFrameEncoderV1, CustodyFramePathV1};
use crate::custody_git::{
    CustodyGitError, ExpectedGitDigestV1, GitCommandV1, GitObjectFormatV1, GitRootNamesV1,
    GitRouteRequestV1, GitRunRequestV1, GitRunnerV1,
};
use crate::custody_git_tests::retry_on_text_file_busy;
use crate::custody_inventory::CustodyStateClassV1;
use crate::custody_seal::{
    CustodyCoverageEntryV1, CustodyGitObjectFormatV1, CustodyGitObjectKindV1,
    CustodyOriginalObjectV1,
};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// The lane Git
// ---------------------------------------------------------------------------------------------

fn lane_git_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("A2A_LANE_GIT") {
        return PathBuf::from(explicit);
    }
    let path = std::env::var_os("PATH").expect("the test lane has a PATH");
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join("git");
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.file_type().is_file() {
            return candidate;
        }
    }
    panic!("no non-symlink `git` regular file on PATH; set A2A_LANE_GIT");
}

fn sha256_of_file(path: &Path) -> [u8; 32] {
    let bytes = std::fs::read(path).expect("a readable fixture file");
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, &bytes).as_ref());
    out
}

fn lane_git_route() -> GitRouteRequestV1 {
    let path = lane_git_path();
    let digest = ExpectedGitDigestV1::from_bytes(sha256_of_file(&path));
    GitRouteRequestV1::for_test_system(path, digest).expect("the lane Git route is absolute")
}

fn private_dir(path: &Path) {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .expect("an owner-private fixture directory");
}

/// Plain plumbing against `git_dir`, with a closed environment. This builds fixtures; it is not
/// the restore's effect seam.
fn plumb(git_dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> String {
    use std::io::Write as _;
    let mut child = Command::new(lane_git_path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_DIR", git_dir)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the fixture Git child starts");
    {
        let mut pipe = child.stdin.take().expect("piped stdin");
        if let Some(stdin) = stdin {
            pipe.write_all(stdin).expect("fixture stdin");
        }
    }
    let output = child
        .wait_with_output()
        .expect("the fixture Git child ends");
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("fixture stdout")
        .trim_end()
        .to_owned()
}

// ---------------------------------------------------------------------------------------------
// Custom exports: any object format, with or without objects
// ---------------------------------------------------------------------------------------------

const GENERATION_V1: &str = "generation-2b3b-a";

fn kind_of(name: &str) -> CustodyGitObjectKindV1 {
    match name {
        "commit" => CustodyGitObjectKindV1::Commit,
        "tree" => CustodyGitObjectKindV1::Tree,
        "blob" => CustodyGitObjectKindV1::Blob,
        "tag" => CustodyGitObjectKindV1::Tag,
        other => panic!("an unexpected object type {other}"),
    }
}

/// A real source repository in `format` holding a blob, a tree, a commit, and an annotated tag
/// (all four kinds), or nothing.
struct SourceV1 {
    git_dir: PathBuf,
    objects: Vec<CustodyOriginalObjectV1>,
}

impl SourceV1 {
    fn build(area: &Path, format: CustodyGitObjectFormatV1, populated: bool) -> Self {
        let git_dir = area.join("source.git");
        let object_format = match format {
            CustodyGitObjectFormatV1::Sha1 => "--object-format=sha1",
            CustodyGitObjectFormatV1::Sha256 => "--object-format=sha256",
        };
        std::fs::create_dir(&git_dir).expect("the source git directory");
        let _ = Command::new(lane_git_path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["init", "--bare", "-q", "--template=", object_format])
            .arg(&git_dir)
            .output()
            .expect("the source init runs");
        let mut objects = Vec::new();
        if populated {
            let blob = plumb(
                &git_dir,
                &["hash-object", "-w", "--stdin"],
                Some(b"pass-a blob\n"),
            );
            let tree = plumb(
                &git_dir,
                &["mktree"],
                Some(format!("100644 blob {blob}\tREADME\n").as_bytes()),
            );
            let commit = plumb(&git_dir, &["commit-tree", &tree, "-m", "pass-a"], None);
            let tag_body = format!(
                "object {commit}\ntype commit\ntag v1\ntagger A <a@example.invalid> 1700000000 +0000\n\ntag message\n"
            );
            plumb(
                &git_dir,
                &["hash-object", "-t", "tag", "-w", "--stdin"],
                Some(tag_body.as_bytes()),
            );
            for line in plumb(
                &git_dir,
                &[
                    "cat-file",
                    "--batch-all-objects",
                    "--batch-check=%(objectname) %(objecttype)",
                ],
                None,
            )
            .lines()
            {
                let (object_id, kind) = line.split_once(' ').expect("an `id type` row");
                objects.push(
                    CustodyOriginalObjectV1::new(format, object_id, kind_of(kind))
                        .expect("a listed object id"),
                );
            }
            assert_eq!(objects.len(), 4, "blob, tree, commit and tag");
        }
        Self { git_dir, objects }
    }
}

fn coverage_rows(object_database: CustodyStateClassV1) -> Vec<CustodyCoverageEntryV1> {
    CustodyCoverageClassV1::ALL
        .into_iter()
        .map(|class| {
            let state = if class == CustodyCoverageClassV1::ObjectDatabase {
                object_database
            } else {
                CustodyStateClassV1::Empty
            };
            CustodyCoverageEntryV1::new(class, state, vec![], None).expect("a coverage row")
        })
        .collect()
}

fn manifest_of(
    object_database: CustodyStateClassV1,
    objects: Vec<CustodyOriginalObjectV1>,
) -> CustodyManifestV1 {
    CustodyManifestV1::new(
        "unit-2b3b-a",
        "run-2b3b-a",
        "materialization-2b3b-a",
        GENERATION_V1,
        coverage_rows(object_database),
        vec![],
        objects,
        vec![],
        vec![],
    )
    .expect("a fixture manifest")
}

/// One sealed capsule built here, so its source can be removed.
struct CustomCapsuleV1 {
    _temp: tempfile::TempDir,
    area: PathBuf,
    capsule: PathBuf,
    source: SourceV1,
    format: CustodyGitObjectFormatV1,
}

impl CustomCapsuleV1 {
    fn export(format: CustodyGitObjectFormatV1, populated: bool) -> Self {
        let temp = tempfile::TempDir::new().expect("a fixture area");
        let area = temp.path().canonicalize().expect("a canonical area");
        let source = SourceV1::build(&area, format, populated);
        let scratch = area.join("scratch");
        private_dir(&scratch);
        let state = if populated {
            CustodyStateClassV1::Captured
        } else {
            CustodyStateClassV1::Empty
        };
        let manifest = manifest_of(state, source.objects.clone());
        let capability = CustodyCaptureCapabilityV1::from_fixture_quiescence(
            CustodyQuiescenceDecisionV1::CoherentSnapshot,
            &manifest,
            &source.git_dir,
            &source.git_dir,
            &source.git_dir.join("objects"),
            Vec::new(),
        )
        .expect("the fixture capability mints");
        let outcome = export_capsule_v1(CustodyExportRequestV1 {
            manifest: &manifest,
            envelope_format: &fixture_envelope_format_v1(),
            recipients: &["fixture-recipient".to_owned()],
            budgets: CustodyExportBudgetsV1::v1_ceilings(),
            sealer: &FixtureSealerV1::honest(),
            capability,
            git_route: lane_git_route(),
            scratch_root: &scratch,
            deadline: Instant::now() + Duration::from_secs(120),
        })
        .expect("the custom export seals");
        let _ = expect_sealed(outcome);
        Self {
            _temp: temp,
            area: area.clone(),
            capsule: scratch.join("capsule"),
            source,
            format,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Phase V, the destination, and the runner
// ---------------------------------------------------------------------------------------------

fn frame_budget() -> CustodyFrameBudgetV1 {
    CustodyFrameBudgetV1::new(1 << 20, 1 << 30).expect("the fixture frame budget")
}

/// A new, empty, owner-private destination.
struct DestV1 {
    _area: tempfile::TempDir,
    path: PathBuf,
}

impl DestV1 {
    fn new() -> Self {
        let area = tempfile::TempDir::new().expect("a destination area");
        let canonical = area.path().canonicalize().expect("a canonical area");
        let path = canonical.join("dest");
        private_dir(&path);
        Self { _area: area, path }
    }

    fn work(&self) -> PathBuf {
        self.path.join(".restore-work")
    }

    fn bootstrap(&self) -> PathBuf {
        self.work().join("git-bootstrap")
    }

    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.path)
            .expect("the destination lists")
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
    }

    fn work_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.work())
            .expect("the work directory lists")
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
    }
}

fn verify_stage(
    capsule: &Path,
    dest: &DestV1,
    budget: CustodyRestoreBudgetV1,
) -> VerifiedCapsuleV1 {
    verify_and_stage_v1(
        capsule,
        &dest.path,
        budget,
        frame_budget(),
        &FixtureOpenerV1::honest(),
    )
    .expect("phase V verifies and stages the capsule")
}

fn phase_v(capsule: &Path) -> (DestV1, VerifiedCapsuleV1) {
    let dest = DestV1::new();
    let verified = verify_stage(capsule, &dest, CustodyRestoreBudgetV1::ceiling());
    (dest, verified)
}

fn far_deadline() -> Instant {
    Instant::now() + Duration::from_secs(120)
}

/// A runner admitted against a scratch pin beside the destination. The bootstrap is rooted at the
/// work pin, so the caller's runner only supplies the admitted Git route.
fn runner_for(route: GitRouteRequestV1) -> (tempfile::TempDir, GitRunnerV1) {
    let area = tempfile::TempDir::new().expect("a runner area");
    let root = PinnedDirectoryV1::open(area.path(), "pass A runner root").expect("a runner root");
    let names = GitRootNamesV1::new("home", "xdg", "git-bootstrap").expect("names");
    let runner =
        GitRunnerV1::admit(route, &root, &names, far_deadline()).expect("the runner admits");
    (area, runner)
}

fn lane_runner() -> (tempfile::TempDir, GitRunnerV1) {
    runner_for(lane_git_route())
}

fn bootstrap(
    verified: VerifiedCapsuleV1,
    runner: &GitRunnerV1,
    format: GitObjectFormatV1,
) -> Result<VerifiedGitBootstrapV1, GitBootstrapFailureV1> {
    bootstrap_git_objects_v1(verified, runner, format, far_deadline())
}

fn must_bootstrap(
    verified: VerifiedCapsuleV1,
    format: GitObjectFormatV1,
) -> VerifiedGitBootstrapV1 {
    let (_area, runner) = lane_runner();
    bootstrap(verified, &runner, format)
        .unwrap_or_else(|failure| panic!("the bootstrap must succeed: {failure:?}"))
}

fn refusal(
    verified: VerifiedCapsuleV1,
    runner: &GitRunnerV1,
    format: GitObjectFormatV1,
) -> GitBootstrapFailureV1 {
    match bootstrap(verified, runner, format) {
        Ok(_) => panic!("the bootstrap must refuse"),
        Err(failure) => failure,
    }
}

// ---------------------------------------------------------------------------------------------
// The fixture Git route: records each child, tampers with one
// ---------------------------------------------------------------------------------------------

struct SealedAnchorV1 {
    anchor: PathBuf,
}

impl Drop for SealedAnchorV1 {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.anchor, std::fs::Permissions::from_mode(0o700));
    }
}

/// A route whose script records every child's arguments in `log` and, for each `(needle, shell)`
/// rule whose needle is a space-delimited part of the arguments, runs `shell` instead of
/// delegating at once. A rule may `exec` something else, or run the lane Git itself as `$REAL`.
struct RouteV1 {
    _area: tempfile::TempDir,
    _anchor: SealedAnchorV1,
    log: PathBuf,
    route: GitRouteRequestV1,
}

impl RouteV1 {
    fn new(rules: &[(&str, &str)]) -> Self {
        let area = tempfile::TempDir::new().expect("a route area");
        let root = area.path().canonicalize().expect("a canonical route area");
        let anchor = root.join("anchor");
        private_dir(&anchor);
        let log = root.join("children.log");
        let script = anchor.join("git");
        let mut body = format!(
            "#!/bin/sh\nREAL='{}'\nLOGFILE='{}'\necho \"$*\" >> \"$LOGFILE\"\nS=\" $* \"\ncase \"$S\" in\n",
            lane_git_path().display(),
            log.display()
        );
        for (needle, shell) in rules {
            // An arm that neither `exec`s nor `exit`s leaves with its own last status, so it can
            // never fall through to the delegating `exec` below and run Git a second time.
            body.push_str(&format!("  *\" {needle} \"*) {shell} ; exit $? ;;\n"));
        }
        body.push_str("esac\nexec \"$REAL\" \"$@\"\n");
        std::fs::write(&script, body).expect("the route script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o500))
            .expect("seal the route script");
        std::fs::set_permissions(&anchor, std::fs::Permissions::from_mode(0o500))
            .expect("seal the route anchor");
        let sealed = SealedAnchorV1 {
            anchor: anchor.clone(),
        };
        retry_on_text_file_busy(|| Command::new(&script).arg("--version").output())
            .expect("the route warm-up exec");
        // The warm-up and the admission's `version` child are not restore children.
        let digest = ExpectedGitDigestV1::from_bytes(sha256_of_file(&script));
        let route = GitRouteRequestV1::for_test_fixture(script, digest, anchor)
            .expect("the fixture route is absolute");
        Self {
            _area: area,
            _anchor: sealed,
            log,
            route,
        }
    }

    fn recorder() -> Self {
        Self::new(&[])
    }

    /// The restore's children, in order, each as its subcommand token (`init`, `index-pack`, ...).
    /// The warm-up exec and the runner's admission `version` are filtered out.
    fn children(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(&self.log) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| {
                line.split_whitespace()
                    .find(|token| {
                        [
                            "init",
                            "index-pack",
                            "verify-pack",
                            "cat-file",
                            "rev-list",
                            "fsck",
                            "pack-objects",
                        ]
                        .contains(token)
                    })
                    .map(str::to_owned)
            })
            .collect()
    }

    /// The restore's children by their full subcommand argument tail, for a precise assertion.
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .filter(|line| !line.contains("version") && !line.trim().is_empty())
            .collect()
    }

    fn runner(&self) -> (tempfile::TempDir, GitRunnerV1) {
        runner_for(self.route.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// Fixture helpers shared by the controls
// ---------------------------------------------------------------------------------------------

const ENTRY_V1: u64 = ENTRY_ALLOWANCE_BYTES_V1;

fn core_format(format: CustodyGitObjectFormatV1) -> GitObjectFormatV1 {
    match format {
        CustodyGitObjectFormatV1::Sha1 => GitObjectFormatV1::Sha1,
        CustodyGitObjectFormatV1::Sha256 => GitObjectFormatV1::Sha256,
    }
}

fn captured(format: CustodyGitObjectFormatV1) -> CustomCapsuleV1 {
    CustomCapsuleV1::export(format, true)
}

fn empty(format: CustodyGitObjectFormatV1) -> CustomCapsuleV1 {
    CustomCapsuleV1::export(format, false)
}

fn ledger_used(verified: &VerifiedCapsuleV1) -> u64 {
    verified.destination.ledger.borrow().used()
}

/// Every regular file's bytes beneath `root`, by relative name; directories as `None`.
fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    let mut frontier = vec![root.to_path_buf()];
    while let Some(directory) = frontier.pop() {
        for entry in std::fs::read_dir(&directory).expect("a fixture directory") {
            let path = entry.expect("a fixture entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("beneath the root")
                .to_string_lossy()
                .into_owned();
            let metadata = std::fs::symlink_metadata(&path).expect("fixture metadata");
            if metadata.is_dir() {
                entries.insert(relative, None);
                frontier.push(path);
            } else if metadata.file_type().is_symlink() {
                let target = std::fs::read_link(&path).expect("a link target");
                entries.insert(relative, Some(target.as_os_str().as_bytes().to_vec()));
            } else {
                entries.insert(
                    relative,
                    Some(std::fs::read(&path).expect("a fixture file")),
                );
            }
        }
    }
    entries
}

/// The sum of every regular file's length beneath `root`, measured by the test, not by the seam.
fn tree_bytes(root: &Path) -> u64 {
    snapshot(root)
        .values()
        .map(|bytes| bytes.as_ref().map_or(0, |bytes| bytes.len() as u64))
        .sum()
}

/// A path under the destination's staging tree.
fn staged_path(dest: &DestV1, relative: &str) -> PathBuf {
    dest.work().join("plain").join(relative)
}

fn failure_cause(failure: &GitBootstrapFailureV1) -> String {
    format!("{:?}", failure.cause)
}

/// No `repository/` entry and no phase-G directory exists: the destination holds only
/// `.restore-work/`, whose children are exactly `plain`.
fn assert_no_phase_g_effect(dest: &DestV1) {
    assert_eq!(dest.entries(), [".restore-work"]);
    assert_eq!(dest.work_entries(), ["plain"]);
}

/// A new, owner-private directory outside the destination holding `names` as planted files.
fn external_dir_with(names: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let area = tempfile::TempDir::new().expect("an external area");
    let path = area
        .path()
        .canonicalize()
        .expect("a canonical external area");
    for name in names {
        std::fs::write(path.join(name), b"external bytes").expect("an external file");
    }
    (area, path)
}

// ---------------------------------------------------------------------------------------------
// Fixture self-checks: these hold independently of the seam's behavior
// ---------------------------------------------------------------------------------------------

#[test]
fn fixture_custom_sha1_sha256_and_empty_capsules_pass_phase_v() {
    for format in [
        CustodyGitObjectFormatV1::Sha1,
        CustodyGitObjectFormatV1::Sha256,
    ] {
        let capsule = captured(format);
        assert_eq!(capsule.source.objects.len(), 4);
        let (_dest, verified) = phase_v(&capsule.capsule);
        assert_eq!(
            verified
                .staged
                .iter()
                .filter(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
                .count(),
            1
        );
    }
    let capsule = empty(CustodyGitObjectFormatV1::Sha1);
    assert!(capsule.source.objects.is_empty());
    let (_dest, verified) = phase_v(&capsule.capsule);
    assert!(verified
        .staged
        .iter()
        .all(|one| one.role != CustodyCapsuleArtifactRoleV1::GitObjectPack));
}

#[test]
fn fixture_the_recording_route_records_the_children_it_runs() {
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let root = tempfile::TempDir::new().expect("a root");
    let pin = PinnedDirectoryV1::open(root.path(), "root").expect("a pin");
    let names = GitRootNamesV1::new("home", "xdg", "git-bootstrap").expect("names");
    let request = GitRunRequestV1::new(
        GitCommandV1::Version,
        Vec::new(),
        1024,
        1024,
        far_deadline(),
    );
    let result = runner
        .run(&pin, &names, request, || Ok(()), || Ok(()))
        .expect("the version child runs");
    assert!(result.status.success());
    assert_eq!(
        route.children(),
        Vec::<String>::new(),
        "no restore child ran"
    );
}

// ---------------------------------------------------------------------------------------------
// Task 1: admission
// ---------------------------------------------------------------------------------------------

/// A real phase-V value enters phase G and returns a retained, strict, isolated bootstrap, with
/// the format provenance, the pins, the admitted files, and the closure evidence pass B needs; one
/// ledger accounts for everything; and NOTHING exists under `repository/`.
#[test]
fn a_real_phase_v_value_enters_g_and_yields_a_verified_bootstrap() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let used_before = ledger_used(&verified);
    let staged_before: Vec<_> = verified
        .staged
        .iter()
        .map(|one| (one.name.clone(), one.length, one.sha256))
        .collect();
    let manifest_digest = verified
        .control
        .manifest
        .content_digest()
        .expect("a digest");
    let pack_length = verified
        .staged
        .iter()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .map(|one| (one.length, one.sha256))
        .expect("the staged pack");

    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let bootstrap = bootstrap(verified, &runner, GitObjectFormatV1::Sha1)
        .unwrap_or_else(|failure| panic!("the bootstrap must succeed: {failure:?}"));

    // Format provenance, closure evidence, and the retained original value.
    assert_eq!(
        bootstrap.provenance,
        GitFormatProvenanceV1::CapsuleObjectsAndPack
    );
    assert_eq!(bootstrap.object_format, GitObjectFormatV1::Sha1);
    assert_eq!(
        bootstrap.closure,
        GitClosureEvidenceV1 {
            inventory_objects: 4,
            closure: GitClosureProofV1::Reachable { objects: 4 },
            fsck_strict: true,
        }
    );
    assert_eq!(
        bootstrap
            .verified
            .control
            .manifest
            .content_digest()
            .expect("a digest"),
        manifest_digest
    );
    let staged_after: Vec<_> = bootstrap
        .verified
        .staged
        .iter()
        .map(|one| (one.name.clone(), one.length, one.sha256))
        .collect();
    assert_eq!(staged_after, staged_before);
    assert!(
        bootstrap.active_frames.is_empty(),
        "this capsule has no active class"
    );

    // The exact admitted files: the pack is the staged bytes, and each is what the disk holds.
    let BootstrapObjectsV1::Captured { pack_hash, files } = &bootstrap.objects else {
        panic!("a Captured capsule admits a pack");
    };
    assert_eq!(pack_hash.len(), 40);
    let relatives: Vec<&str> = files.iter().map(|file| file.relative.as_str()).collect();
    assert_eq!(
        relatives,
        [
            format!("objects/pack/pack-{pack_hash}.pack"),
            format!("objects/pack/pack-{pack_hash}.idx"),
            format!("objects/pack/pack-{pack_hash}.rev"),
        ]
    );
    assert_eq!((files[0].length, files[0].sha256), pack_length);
    for file in files {
        let on_disk =
            std::fs::read(dest.bootstrap().join(&file.relative)).expect("an admitted file");
        assert_eq!(on_disk.len() as u64, file.length, "{}", file.relative);
        assert!(file.length > 0);
    }

    // The pins: HOME and XDG empty, every bootstrap directory retained.
    assert_eq!(
        bootstrap
            .directories
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "",
            "objects",
            "objects/info",
            "objects/pack",
            "refs",
            "refs/heads",
            "refs/tags"
        ]
    );
    assert!(bootstrap.bootstrap().is_some() && bootstrap.pack_directory().is_some());
    assert_eq!(snapshot(&dest.work().join("home")), BTreeMap::new());
    assert_eq!(snapshot(&dest.work().join("xdg")), BTreeMap::new());

    // Only the work tree grew; nothing is under `repository/`, and there is no `.git`.
    assert_eq!(dest.entries(), [".restore-work"]);
    assert_eq!(
        dest.work_entries(),
        ["git-bootstrap", "home", "plain", "xdg"]
    );

    // One ledger: HOME and XDG, the nine-entry init reservation, the three index-pack entries,
    // and exactly the logical bytes the bootstrap holds.
    let charged = ledger_used(&bootstrap.verified) - used_before;
    assert_eq!(
        charged,
        (2 + INIT_BARE_ENTRIES_V1 + INDEX_PACK_ENTRIES_V1) * ENTRY_V1
            + tree_bytes(&dest.bootstrap())
    );

    // The six children, in order, each rooted by the closed environment and no object-store route.
    let labels: Vec<&str> = bootstrap.runs.iter().map(|run| run.label).collect();
    assert_eq!(
        labels,
        [
            "init --bare",
            "index-pack --strict --stdin",
            "verify-pack",
            "cat-file --batch-all-objects",
            "rev-list --missing=print",
            "fsck --strict"
        ]
    );
    assert_eq!(
        route.children(),
        [
            "init",
            "index-pack",
            "verify-pack",
            "cat-file",
            "rev-list",
            "fsck"
        ]
    );
    for run in &bootstrap.runs {
        assert_eq!(run.exit_status, Some(0), "{}", run.label);
        assert_eq!(
            run.environment.get("HOME").map(String::as_str),
            Some("home")
        );
        assert_eq!(
            run.environment.get("XDG_CONFIG_HOME").map(String::as_str),
            Some("xdg")
        );
        assert_eq!(
            run.environment.get("GIT_CONFIG_GLOBAL").map(String::as_str),
            Some("/dev/null")
        );
        assert_eq!(
            run.environment
                .get("GIT_CONFIG_NOSYSTEM")
                .map(String::as_str),
            Some("1")
        );
        assert!(
            !run.environment.contains_key("GIT_OBJECT_DIRECTORY"),
            "{}",
            run.label
        );
        assert!(
            !run.environment
                .contains_key("GIT_ALTERNATE_OBJECT_DIRECTORIES"),
            "{}",
            run.label
        );
        if run.label == "init --bare" {
            assert!(!run.environment.contains_key("GIT_DIR"));
        } else {
            assert_eq!(
                run.environment.get("GIT_DIR").map(String::as_str),
                Some("git-bootstrap")
            );
        }
    }
    let init: Vec<String> = bootstrap.runs[0]
        .argv
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert!(init.windows(5).any(|window| window
        == [
            "init",
            "--bare",
            "--template=",
            "--object-format=sha1",
            "git-bootstrap"
        ]));
    // `index-pack` was fed exactly the staged bytes.
    assert_eq!(bootstrap.runs[1].stdin.length as u64, pack_length.0);
    assert_eq!(bootstrap.runs[1].stdin.sha256, pack_length.1);

    // Every staged descriptor is back at offset 0 (the runner advanced a shared duplicate to EOF),
    // and so is every admitted object file.
    for one in &bootstrap.verified.staged {
        assert_eq!(
            (&one.file).stream_position().expect("a position"),
            0,
            "{}",
            lossy(one.name.as_bytes())
        );
    }
    for file in files {
        assert_eq!((&file.file).stream_position().expect("a position"), 0);
    }
}

/// SHA-256 indexes and proves the same way, and the source is not needed.
#[test]
fn sha256_objects_index_and_prove_without_the_source() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha256);
    let (dest, verified) = phase_v(&capsule.capsule);
    // The source repository is made unavailable: the bootstrap may use only its own database.
    std::fs::remove_dir_all(&capsule.source.git_dir).expect("the source is removed");
    let bootstrap = must_bootstrap(verified, GitObjectFormatV1::Sha256);

    assert_eq!(
        bootstrap.provenance,
        GitFormatProvenanceV1::CapsuleObjectsAndPack
    );
    assert_eq!(
        bootstrap.closure.closure,
        GitClosureProofV1::Reachable { objects: 4 }
    );
    let BootstrapObjectsV1::Captured { pack_hash, .. } = &bootstrap.objects else {
        panic!("a Captured capsule admits a pack");
    };
    assert_eq!(pack_hash.len(), 64, "a SHA-256 pack name");
    let config = std::fs::read_to_string(dest.bootstrap().join("config")).expect("the config");
    assert!(config.contains("objectformat = sha256"), "{config}");
    assert_eq!(dest.entries(), [".restore-work"]);
}

/// A rich plan-backed capsule identifies its `RefsAndHead` and `StashAndReflogs` frames by role.
#[test]
fn a_rich_capsule_identifies_its_active_frames_by_bound_role() {
    let fixture = PlanFixtureV1::rich_clone();
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    let (dest, verified) = phase_v(&fixture.capsule_dir());
    let bootstrap = must_bootstrap(verified, GitObjectFormatV1::Sha1);

    let classes: Vec<_> = bootstrap
        .active_frames
        .iter()
        .map(|frame| frame.class)
        .collect();
    assert_eq!(
        classes,
        [
            CustodyCoverageClassV1::RefsAndHead,
            CustodyCoverageClassV1::StashAndReflogs
        ]
    );
    for frame in &bootstrap.active_frames {
        assert_eq!(
            bootstrap.verified.staged[frame.staged].role,
            CustodyCapsuleArtifactRoleV1::CoveragePayload(frame.class)
        );
    }
    assert!(bootstrap.closure.inventory_objects > 4);
    assert_eq!(dest.entries(), [".restore-work"]);
}

/// A wrong nonempty object format refuses before any write or child, in both directions.
#[test]
fn a_wrong_nonempty_object_format_refuses_before_any_write() {
    for (capsule_format, caller) in [
        (CustodyGitObjectFormatV1::Sha1, GitObjectFormatV1::Sha256),
        (CustodyGitObjectFormatV1::Sha256, GitObjectFormatV1::Sha1),
    ] {
        let capsule = captured(capsule_format);
        let (dest, verified) = phase_v(&capsule.capsule);
        let route = RouteV1::recorder();
        let (_area, runner) = route.runner();
        let failure = refusal(verified, &runner, caller);
        assert!(
            matches!(failure.cause, GitBootstrapErrorV1::FormatMismatch(_)),
            "{}",
            failure_cause(&failure)
        );
        assert_eq!(failure.last_completed, None);
        assert!(!failure.active_materialization_began);
        assert_no_phase_g_effect(&dest);
        assert_eq!(route.children(), Vec::<String>::new());
    }
}

/// Absent or multiple Captured packs refuse; so does a Captured row that lists no object.
#[test]
fn absent_or_multiple_captured_packs_refuse() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();

    let (dest, mut verified) = phase_v(&capsule.capsule);
    verified
        .staged
        .retain(|one| one.role != CustodyCapsuleArtifactRoleV1::GitObjectPack);
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause, GitBootstrapErrorV1::PackShape(detail) if detail.contains("found 0")),
        "{}",
        failure_cause(&failure)
    );
    assert_no_phase_g_effect(&dest);

    let (dest, mut verified) = phase_v(&capsule.capsule);
    let pack = verified
        .staged
        .iter()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .expect("the staged pack");
    let second = StagedPlaintextV1 {
        name: pack.name.clone(),
        role: pack.role.clone(),
        file: pack.file.try_clone().expect("a duplicate descriptor"),
        length: pack.length,
        sha256: pack.sha256,
    };
    verified.staged.push(second);
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause, GitBootstrapErrorV1::PackShape(detail) if detail.contains("found 2")),
        "{}",
        failure_cause(&failure)
    );
    assert_no_phase_g_effect(&dest);

    let (dest, mut verified) = phase_v(&capsule.capsule);
    verified.control.manifest = manifest_of(CustodyStateClassV1::Captured, Vec::new());
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause, GitBootstrapErrorV1::PackShape(detail) if detail.contains("no objects")),
        "{}",
        failure_cause(&failure)
    );
    assert_no_phase_g_effect(&dest);
    assert_eq!(route.children(), Vec::<String>::new());
}

/// A valid Empty database takes either explicit format, reports `CallerSelectedEmpty`, and runs no
/// indexing, `verify-pack`, or `rev-list` child; its closure is explicitly vacuous.
#[test]
fn empty_objects_work_for_both_explicit_formats_and_report_caller_selected_provenance() {
    for format in [GitObjectFormatV1::Sha1, GitObjectFormatV1::Sha256] {
        let capsule = empty(CustodyGitObjectFormatV1::Sha1);
        let (dest, verified) = phase_v(&capsule.capsule);
        let used_before = ledger_used(&verified);
        let route = RouteV1::recorder();
        let (_area, runner) = route.runner();
        let bootstrap = bootstrap(verified, &runner, format)
            .unwrap_or_else(|failure| panic!("the Empty bootstrap must succeed: {failure:?}"));

        assert_eq!(
            bootstrap.provenance,
            GitFormatProvenanceV1::CallerSelectedEmpty
        );
        assert_eq!(bootstrap.object_format, format);
        assert!(matches!(bootstrap.objects, BootstrapObjectsV1::Empty));
        assert_eq!(
            bootstrap.closure,
            GitClosureEvidenceV1 {
                inventory_objects: 0,
                closure: GitClosureProofV1::Vacuous,
                fsck_strict: true,
            }
        );
        // Exactly init, an empty inventory, and fsck: no index-pack, verify-pack, or rev-list.
        assert_eq!(
            bootstrap
                .runs
                .iter()
                .map(|run| run.label)
                .collect::<Vec<_>>(),
            [
                "init --bare",
                "cat-file --batch-all-objects",
                "fsck --strict"
            ]
        );
        assert_eq!(route.children(), ["init", "cat-file", "fsck"]);
        assert_eq!(bootstrap.runs[1].stdout.length, 0, "an empty inventory");
        // The explicit caller format was used.
        let config = std::fs::read_to_string(dest.bootstrap().join("config")).expect("the config");
        assert_eq!(
            config.contains("objectformat = sha256"),
            format == GitObjectFormatV1::Sha256,
            "{config}"
        );
        // No pack files; the ledger holds HOME, XDG, the nine init entries, and the init bytes.
        assert!(snapshot(&dest.bootstrap().join("objects/pack")).is_empty());
        assert_eq!(
            ledger_used(&bootstrap.verified) - used_before,
            (2 + INIT_BARE_ENTRIES_V1) * ENTRY_V1 + tree_bytes(&dest.bootstrap())
        );
        assert_eq!(dest.entries(), [".restore-work"]);
    }
}

/// A pack for Empty, an inventory for Empty, and every unsupported coverage state refuse.
#[test]
fn contradicted_or_unsupported_object_coverage_refuses_before_any_write() {
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();

    // A pack present for an Empty database.
    let donor = captured(CustodyGitObjectFormatV1::Sha1);
    let (_donor_dest, donor_verified) = phase_v(&donor.capsule);
    let pack = donor_verified
        .staged
        .iter()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .expect("the donor pack");
    let capsule = empty(CustodyGitObjectFormatV1::Sha1);
    let (dest, mut verified) = phase_v(&capsule.capsule);
    verified.staged.push(StagedPlaintextV1 {
        name: pack.name.clone(),
        role: pack.role.clone(),
        file: pack.file.try_clone().expect("a duplicate descriptor"),
        length: pack.length,
        sha256: pack.sha256,
    });
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause, GitBootstrapErrorV1::EmptyShape(detail) if detail.contains("pack")),
        "{}",
        failure_cause(&failure)
    );
    assert_no_phase_g_effect(&dest);

    // A nonempty inventory for an Empty database.
    let (dest, mut verified) = phase_v(&capsule.capsule);
    verified.control.manifest =
        manifest_of(CustodyStateClassV1::Empty, donor.source.objects.clone());
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause, GitBootstrapErrorV1::EmptyShape(detail) if detail.contains("objects")),
        "{}",
        failure_cause(&failure)
    );
    assert_no_phase_g_effect(&dest);

    // The object database is neither Captured nor Empty: never downgraded to Empty.
    for state in [
        CustodyStateClassV1::Unresolved,
        CustodyStateClassV1::ExcludedReproducible,
    ] {
        let (dest, mut verified) = phase_v(&capsule.capsule);
        let rows = coverage_rows(CustodyStateClassV1::Empty)
            .into_iter()
            .map(|row| {
                if row.class() == CustodyCoverageClassV1::ObjectDatabase {
                    let exclusion = (state == CustodyStateClassV1::ExcludedReproducible)
                        .then(|| "exclusion-v1".to_owned());
                    let reasons = if state == CustodyStateClassV1::Unresolved {
                        vec![crate::custody_inventory::CustodyReasonCodeV1::ContentUnresolved]
                    } else {
                        vec![]
                    };
                    CustodyCoverageEntryV1::new(row.class(), state, reasons, exclusion)
                        .expect("a coverage row")
                } else {
                    row
                }
            })
            .collect();
        verified.control.manifest = manifest_with_rows_unchecked(rows);
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        assert!(
            matches!(
                failure.cause,
                GitBootstrapErrorV1::UnsupportedCoverage {
                    class: CustodyCoverageClassV1::ObjectDatabase,
                    ..
                }
            ),
            "{state:?}: {}",
            failure_cause(&failure)
        );
        assert_no_phase_g_effect(&dest);
    }
    assert_eq!(route.children(), Vec::<String>::new());
}

/// Captured active classes need their payload; Empty ones need none; nothing else is restorable.
#[test]
fn active_payload_shapes_refuse_when_impossible_or_unsupported() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    for class in [
        CustodyCoverageClassV1::RefsAndHead,
        CustodyCoverageClassV1::StashAndReflogs,
    ] {
        // Captured with no staged payload.
        let (dest, mut verified) = phase_v(&capsule.capsule);
        verified.control.manifest =
            manifest_with_row(class, CustodyStateClassV1::Captured, &verified);
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        assert!(
            matches!(&failure.cause, GitBootstrapErrorV1::UnsupportedCoverage { class: observed, detail }
                if *observed == class && detail.contains("exactly one")),
            "{class:?}: {}",
            failure_cause(&failure)
        );
        assert_no_phase_g_effect(&dest);

        // Empty with a staged payload.
        let (dest, mut verified) = phase_v(&capsule.capsule);
        let donor = verified
            .staged
            .iter()
            .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
            .expect("the pack");
        let payload = StagedPlaintextV1 {
            name: donor.name.clone(),
            role: CustodyCapsuleArtifactRoleV1::CoveragePayload(class),
            file: donor.file.try_clone().expect("a duplicate descriptor"),
            length: donor.length,
            sha256: donor.sha256,
        };
        verified.staged.push(payload);
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        assert!(
            matches!(&failure.cause, GitBootstrapErrorV1::UnsupportedCoverage { class: observed, detail }
                if *observed == class && detail.contains("Empty active class")),
            "{class:?}: {}",
            failure_cause(&failure)
        );
        assert_no_phase_g_effect(&dest);

        // Neither Captured nor Empty.
        let (dest, mut verified) = phase_v(&capsule.capsule);
        verified.control.manifest =
            manifest_with_row(class, CustodyStateClassV1::Unresolved, &verified);
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        assert!(
            matches!(&failure.cause, GitBootstrapErrorV1::UnsupportedCoverage { class: observed, .. }
                if *observed == class),
            "{class:?}: {}",
            failure_cause(&failure)
        );
        assert_no_phase_g_effect(&dest);
    }
    assert_eq!(route.children(), Vec::<String>::new());
}

/// A manifest like `base`'s with one coverage row set to `state`.
fn manifest_with_row(
    class: CustodyCoverageClassV1,
    state: CustodyStateClassV1,
    base: &VerifiedCapsuleV1,
) -> CustodyManifestV1 {
    let rows = base
        .control
        .manifest
        .coverage()
        .iter()
        .map(|row| {
            if row.class() == class {
                let reasons = if state == CustodyStateClassV1::Unresolved {
                    vec![crate::custody_inventory::CustodyReasonCodeV1::ContentUnresolved]
                } else {
                    vec![]
                };
                CustodyCoverageEntryV1::new(class, state, reasons, None).expect("a coverage row")
            } else {
                row.clone()
            }
        })
        .collect();
    CustodyManifestV1::new(
        base.control.manifest.unit_id(),
        base.control.manifest.run_id(),
        base.control.manifest.materialization_id(),
        base.control.manifest.generation_id(),
        rows,
        vec![],
        base.control.manifest.original_objects().to_vec(),
        vec![],
        vec![],
    )
    .expect("a manifest with one row changed")
}

/// A manifest over `rows` and no objects. An Excluded row needs its exclusion, so this builds one.
fn manifest_with_rows_unchecked(rows: Vec<CustodyCoverageEntryV1>) -> CustodyManifestV1 {
    let excluded = rows
        .iter()
        .any(|row| row.state() == CustodyStateClassV1::ExcludedReproducible);
    let (dependencies, exclusions) = if excluded {
        (
            vec![crate::custody_seal::CustodyDependencyV1::new(
                "dependency-v1",
                "cargo-lock",
                Sha256HexV1::digest(b"dependency"),
                CustodyStateClassV1::Captured,
                vec![],
            )
            .expect("a dependency")],
            vec![crate::custody_seal::CustodyExclusionV1::new(
                "exclusion-v1",
                "reproducible_outputs",
                "exclusion-v1",
                vec!["dependency-v1".to_owned()],
            )
            .expect("an exclusion")],
        )
    } else {
        (vec![], vec![])
    };
    CustodyManifestV1::new(
        "unit-2b3b-a",
        "run-2b3b-a",
        "materialization-2b3b-a",
        GENERATION_V1,
        rows,
        vec![],
        vec![],
        dependencies,
        exclusions,
    )
    .expect("a manifest over the rows")
}

// ---------------------------------------------------------------------------------------------
// Task 1: staged inputs are re-measured from their retained descriptors
// ---------------------------------------------------------------------------------------------

const PACK_STAGED_V1: &str = "git/objects.pack.enc";

/// A row name, a staged artifact name, and the racer's edit of that staged file.
type StagedEditV1 = (&'static str, &'static str, fn(&mut std::fs::File));

fn write_into(path: &Path, edit: impl FnOnce(&mut std::fs::File)) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("a staged file opens for the racer");
    edit(&mut file);
}

/// Changed bytes at the same length, appended bytes, and a truncation of ANY staged input refuse
/// before the first new write, and no child starts.
#[test]
fn changed_staged_descriptor_bytes_or_length_refuse_before_any_write() {
    use std::io::{Seek as _, SeekFrom, Write as _};
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let edits: [StagedEditV1; 5] = [
        ("the pack, one byte changed", PACK_STAGED_V1, |file| {
            // Offset 10 is the high half of the pack header's object count: zero, so 0xff differs.
            file.seek(SeekFrom::Start(10)).expect("seek");
            file.write_all(&[0xff]).expect("change one byte");
        }),
        ("the pack, appended", PACK_STAGED_V1, |file| {
            file.seek(SeekFrom::End(0)).expect("seek");
            file.write_all(b"x").expect("append");
        }),
        ("the pack, truncated", PACK_STAGED_V1, |file| {
            let length = file.metadata().expect("metadata").len();
            file.set_len(length - 1).expect("truncate");
        }),
        (
            "the manifest, one byte changed",
            "control/manifest.json.enc",
            |file| {
                file.seek(SeekFrom::Start(3)).expect("seek");
                file.write_all(b"Z").expect("change one byte");
            },
        ),
        (
            "the restore policy, appended",
            "control/restore-policy.json.enc",
            |file| {
                file.seek(SeekFrom::End(0)).expect("seek");
                file.write_all(b" ").expect("append");
            },
        ),
    ];
    for (row, name, edit) in edits {
        let (dest, verified) = phase_v(&capsule.capsule);
        write_into(&staged_path(&dest, name), edit);
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        assert!(
            matches!(&failure.cause, GitBootstrapErrorV1::StagedInputChanged { name: observed }
                if observed == name),
            "{row}: {}",
            failure_cause(&failure)
        );
        assert_eq!(failure.last_completed, None, "{row}");
        assert_no_phase_g_effect(&dest);
    }
    assert_eq!(route.children(), Vec::<String>::new());
}

/// Every staged leaf renamed away still reads from its retained descriptor.
#[test]
fn renamed_staged_files_are_still_read_from_their_retained_descriptors() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let plain = dest.work().join("plain");
    let mut renamed = 0_usize;
    for (relative, bytes) in snapshot(&plain) {
        if bytes.is_some() {
            let from = plain.join(&relative);
            let to = from.with_file_name(format!("renamed-{renamed}"));
            std::fs::rename(&from, &to).expect("a staged leaf is renamed");
            renamed += 1;
        }
    }
    assert_eq!(renamed, verified.staged.len());
    let bootstrap = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    assert_eq!(bootstrap.closure.inventory_objects, 4);
}

/// Drift at entry to phase G: a changed staging chain, a swapped destination root, or a mount that
/// appears refuses at the first gate, before any `home`, `xdg`, or `git-bootstrap`.
#[test]
fn drift_at_entry_to_g_refuses_before_any_write() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();

    // `plain/` is renamed away and an empty directory takes its name.
    let (dest, verified) = phase_v(&capsule.capsule);
    let work = dest.work();
    let _hook = install_git_hook_for_test(GitHookPointV1::AdmissionEntry, move |_| {
        std::fs::rename(work.join("plain"), work.join("plain-away")).expect("rename plain");
        std::fs::create_dir(work.join("plain")).expect("a replacement plain");
    });
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(
        dest.work_entries(),
        ["plain", "plain-away"],
        "no phase-G directory was created"
    );
    drop(_hook);

    // The destination root is renamed to a sibling and an empty directory takes its name.
    let (dest, verified) = phase_v(&capsule.capsule);
    let root = dest.path.clone();
    let away = root.with_file_name("dest-away");
    let _hook = install_git_hook_for_test(GitHookPointV1::AdmissionEntry, move |_| {
        std::fs::rename(&root, &away).expect("rename the root");
        std::fs::create_dir(&root).expect("a replacement root");
    });
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(
        dest.entries(),
        Vec::<String>::new(),
        "the replacement stays empty"
    );
    assert_eq!(
        std::fs::read_dir(dest.path.with_file_name("dest-away").join(".restore-work"))
            .expect("the moved work tree")
            .count(),
        1,
        "only `plain` exists in the moved tree"
    );
    drop(_hook);

    // A mount point appears inside the destination.
    let (dest, verified) = phase_v(&capsule.capsule);
    let inside = dest
        .path
        .join(".restore-work")
        .as_os_str()
        .as_bytes()
        .to_vec();
    let _census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec(), inside]);
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::MountBoundary(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_no_phase_g_effect(&dest);
    assert_eq!(route.children(), Vec::<String>::new());
}

// ---------------------------------------------------------------------------------------------
// Task 2: the bootstrap slots, the ledger, and the first child
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum PlantV1 {
    EmptyDirectory,
    RegularFile,
    Symlink,
}

fn plant(path: &Path, kind: PlantV1, outside: &Path) {
    match kind {
        PlantV1::EmptyDirectory => std::fs::create_dir(path).expect("a planted directory"),
        PlantV1::RegularFile => std::fs::write(path, b"planted").expect("a planted file"),
        PlantV1::Symlink => std::os::unix::fs::symlink(outside, path).expect("a planted link"),
    }
}

/// A preplanted empty directory, regular file, or symlink at `git-bootstrap`, `home`, or `xdg`
/// refuses before ANY Git child, and each is left untouched. The unplanted control proceeds.
#[test]
fn a_planted_bootstrap_home_or_xdg_slot_refuses_before_git_starts() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    for slot in ["git-bootstrap", "home", "xdg"] {
        for kind in [
            PlantV1::EmptyDirectory,
            PlantV1::RegularFile,
            PlantV1::Symlink,
        ] {
            let (dest, verified) = phase_v(&capsule.capsule);
            let (_outside_area, outside) = external_dir_with(&["decoy"]);
            let outside_before = snapshot(&outside);
            plant(&dest.work().join(slot), kind, &outside);
            let planted_before = snapshot(dest.work().join(slot).parent().expect("work"));

            let route = RouteV1::recorder();
            let (_area, runner) = route.runner();
            let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
            let expected = format!(".restore-work/{slot}");
            assert!(
                matches!(&failure.cause,
                    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::StagingCollision { name })
                        if *name == expected),
                "{slot} {kind:?}: {}",
                failure_cause(&failure)
            );
            assert_eq!(
                route.children(),
                Vec::<String>::new(),
                "{slot} {kind:?}: a child started"
            );
            assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Admission));
            // The planted entry and the outside directory are exactly as they were. (`home` and
            // `xdg` created before the refused slot are this invocation's own.)
            let planted_after = snapshot(&dest.work());
            for (name, bytes) in &planted_before {
                if name.starts_with(slot) {
                    assert_eq!(
                        planted_after.get(name),
                        Some(bytes),
                        "{slot} {kind:?}: {name}"
                    );
                }
            }
            assert_eq!(
                snapshot(&outside),
                outside_before,
                "{slot} {kind:?}: the outside changed"
            );
            assert!(!dest.path.join("repository").exists());
            if slot == "git-bootstrap" {
                assert!(
                    std::fs::symlink_metadata(dest.bootstrap()).is_ok(),
                    "the planted entry remains"
                );
            }
        }
    }

    // The unplanted control, over the same capsule, proceeds.
    let (_dest, verified) = phase_v(&capsule.capsule);
    let bootstrap = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    assert_eq!(bootstrap.runs.len(), 6);
}

/// An exhausted ledger refuses before each writing child starts, and the bound is exact: one byte
/// short of the proven peak refuses before `index-pack`, and the peak itself succeeds.
#[test]
fn an_exhausted_ledger_refuses_before_each_writing_child() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);

    // Learn the numbers from one full run, independently of the seam.
    let learn = DestV1::new();
    let verified = verify_stage(&capsule.capsule, &learn, CustodyRestoreBudgetV1::ceiling());
    let phase_v_used = ledger_used(&verified);
    let pack_length = verified
        .staged
        .iter()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .map(|one| one.length)
        .expect("the staged pack");
    let _ = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    let init_bytes = ["HEAD", "config"]
        .iter()
        .map(|name| {
            std::fs::metadata(learn.bootstrap().join(name))
                .expect("an init file")
                .len()
        })
        .sum::<u64>();
    let bound = index_pack_logical_bound(pack_length, 4, 20).expect("a bound");
    let base = phase_v_used + (2 + INIT_BARE_ENTRIES_V1) * ENTRY_V1;
    // The peak: init's measured bytes stay charged while index-pack's entries and bound reserve.
    let peak = base + init_bytes + INDEX_PACK_ENTRIES_V1 * ENTRY_V1 + bound;

    // HOME, XDG, and the bootstrap's entries fit, but not its 4 KiB of logical bytes.
    let short_of_init = base + INIT_BARE_LOGICAL_BYTES_V1 - 1;
    let dest = DestV1::new();
    let verified = verify_stage(
        &capsule.capsule,
        &dest,
        CustodyRestoreBudgetV1::new(short_of_init).expect("a budget"),
    );
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Admission));
    assert_eq!(
        dest.work_entries(),
        ["home", "plain", "xdg"],
        "no git-bootstrap was created"
    );
    assert_eq!(
        route.children(),
        Vec::<String>::new(),
        "the init child must not start"
    );

    // One byte short of the peak: init ran, and index-pack never started.
    let dest = DestV1::new();
    let verified = verify_stage(
        &capsule.capsule,
        &dest,
        CustodyRestoreBudgetV1::new(peak - 1).expect("a budget"),
    );
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::BootstrapInit));
    assert_eq!(
        route.children(),
        ["init"],
        "the index-pack child must not start"
    );
    assert!(failure.ledger_used < peak);

    // Exactly the peak succeeds.
    let dest = DestV1::new();
    let verified = verify_stage(
        &capsule.capsule,
        &dest,
        CustodyRestoreBudgetV1::new(peak).expect("a budget"),
    );
    let bootstrap = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    assert_eq!(bootstrap.runs.len(), 6);
}

/// The destination's interesting paths, owned so a `'static` hook can carry them.
#[derive(Clone)]
struct PathsV1 {
    root: PathBuf,
    work: PathBuf,
    bootstrap: PathBuf,
}

impl PathsV1 {
    fn of(dest: &DestV1) -> Self {
        Self {
            root: dest.path.clone(),
            work: dest.work(),
            bootstrap: dest.bootstrap(),
        }
    }
}

fn device_of(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(path).expect("metadata").dev()
}

/// Runs the Captured SHA-1 bootstrap with `plant` run in the post-exit callback of the child
/// labelled `label`, and returns the refusal with the destination and the child log.
fn refused_after_child(
    label: &'static str,
    plant: impl Fn(&PathsV1) + 'static,
) -> (DestV1, RouteV1, GitBootstrapFailureV1) {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let paths = PathsV1::of(&dest);
    let _hook = install_git_hook_for_test(GitHookPointV1::AfterChild, move |event| {
        if event.label == label {
            plant(&paths);
        }
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    (dest, route, failure)
}

/// A FIFO at `path`, made by the system `mkfifo` process. The fixture proves it ran (the process
/// started), that it succeeded, and that it left a FIFO, so a fixture that made nothing, or
/// something else, can never pass for the special-file refusal it feeds.
fn mkfifo(path: &Path) {
    use std::os::unix::fs::FileTypeExt as _;
    let status = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("the mkfifo process starts");
    assert!(status.success(), "mkfifo exited with {status}");
    let made = std::fs::symlink_metadata(path).expect("the fifo exists");
    assert!(
        made.file_type().is_fifo(),
        "mkfifo made a non-FIFO: {made:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Task 2: the bounded, descriptor-relative census
// ---------------------------------------------------------------------------------------------

/// A replaced `HEAD` link is refused, never followed, and no next writing child starts. The link
/// and the external file it names are untouched.
#[test]
fn a_replaced_head_link_refuses_without_following_it_and_no_next_child_starts() {
    let (_outside_area, outside) = external_dir_with(&["secret"]);
    let secret = outside.join("secret");
    let outside_before = snapshot(&outside);
    let link_target = secret.clone();
    let (dest, route, failure) = refused_after_child("init --bare", move |paths| {
        let head = paths.bootstrap.join("HEAD");
        std::fs::remove_file(&head).expect("HEAD goes");
        std::os::unix::fs::symlink(&link_target, &head).expect("a HEAD link");
    });
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { name }) if name == "HEAD"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Admission));
    assert_eq!(route.children(), ["init"], "no next child may start");
    assert_eq!(
        std::fs::read_link(dest.bootstrap().join("HEAD")).expect("still a link"),
        secret
    );
    assert_eq!(
        snapshot(&outside),
        outside_before,
        "the external file was not touched"
    );
}

/// Unexpected names and types are refused after the first child, each left in place.
#[test]
fn unexpected_bootstrap_entries_refuse_and_are_left_untouched() {
    type Plant = fn(&PathsV1);
    type CensusRowV1 = (&'static str, Plant, fn(&GitBootstrapErrorV1) -> bool);
    let rows: [CensusRowV1; 13] = [
        (
            "HEAD replaced by a fifo",
            |p| {
                std::fs::remove_file(p.bootstrap.join("HEAD")).expect("HEAD goes");
                mkfifo(&p.bootstrap.join("HEAD"));
            },
            |e| matches!(e, GitBootstrapErrorV1::SpecialEntry { name } if name == "HEAD"),
        ),
        (
            "HEAD replaced by a directory",
            |p| {
                std::fs::remove_file(p.bootstrap.join("HEAD")).expect("HEAD goes");
                std::fs::create_dir(p.bootstrap.join("HEAD")).expect("a HEAD directory");
            },
            |e| matches!(e, GitBootstrapErrorV1::UnexpectedEntry { name } if name == "HEAD"),
        ),
        (
            "HEAD removed",
            |p| {
                std::fs::remove_file(p.bootstrap.join("HEAD")).expect("HEAD goes");
            },
            |e| matches!(e, GitBootstrapErrorV1::MissingEntry { name } if name == "HEAD"),
        ),
        (
            "an extra file under objects/pack",
            |p| {
                std::fs::write(p.bootstrap.join("objects/pack/extra"), b"x")
                    .expect("an extra file");
            },
            |e| matches!(e, GitBootstrapErrorV1::UnexpectedEntry { name } if name == "objects/pack/extra"),
        ),
        (
            "an extra directory under objects",
            |p| {
                std::fs::create_dir(p.bootstrap.join("objects/zz")).expect("an extra directory");
            },
            |e| matches!(e, GitBootstrapErrorV1::UnexpectedEntry { name } if name == "objects/zz"),
        ),
        (
            "an extra link under refs/heads",
            |p| {
                std::os::unix::fs::symlink("/nonexistent", p.bootstrap.join("refs/heads/x"))
                    .expect("a link");
            },
            |e| matches!(e, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { name }) if name == "refs/heads/x"),
        ),
        (
            "a file written into HOME",
            |p| {
                std::fs::write(p.work.join("home/.gitconfig"), b"[core]\n").expect("a HOME file");
            },
            |e| matches!(e, GitBootstrapErrorV1::UnexpectedEntry { name } if name.starts_with("home/")),
        ),
        (
            "a file written into XDG",
            |p| {
                std::fs::write(p.work.join("xdg/config"), b"x").expect("an XDG file");
            },
            |e| matches!(e, GitBootstrapErrorV1::UnexpectedEntry { name } if name.starts_with("xdg/")),
        ),
        (
            "refs/tags removed",
            |p| {
                std::fs::remove_dir(p.bootstrap.join("refs/tags")).expect("refs/tags goes");
            },
            |e| matches!(e, GitBootstrapErrorV1::MissingEntry { name } if name == "refs/tags"),
        ),
        (
            "a link inside an unexpected directory",
            |p| {
                std::fs::create_dir(p.bootstrap.join("objects/zz")).expect("an extra directory");
                std::os::unix::fs::symlink("/nonexistent", p.bootstrap.join("objects/zz/l"))
                    .expect("a link");
            },
            |e| {
                matches!(e, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { name })
                    if name == "objects/zz/l")
            },
        ),
        (
            "a link inside HOME",
            |p| {
                std::os::unix::fs::symlink("/nonexistent", p.work.join("home/l")).expect("a link");
            },
            |e| {
                matches!(e, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { name })
                    if name == "home/l")
            },
        ),
        (
            "an unexpected directory nested deeper than the bound",
            |p| {
                let mut path = p.bootstrap.join("objects/zz");
                for level in 0..9 {
                    path = path.join(format!("d{level}"));
                }
                std::fs::create_dir_all(&path).expect("a deep tree");
            },
            |e| matches!(e, GitBootstrapErrorV1::CensusBound(detail) if detail.contains("nests deeper")),
        ),
        (
            "more entries than the bound",
            |p| {
                for index in 0..300 {
                    std::fs::write(p.bootstrap.join(format!("objects/pack/n{index}")), b"")
                        .expect("a file");
                }
            },
            |e| matches!(e, GitBootstrapErrorV1::CensusBound(_)),
        ),
    ];
    for (row, plant, matches_error) in rows {
        let (dest, route, failure) = refused_after_child("init --bare", plant);
        assert!(
            matches_error(&failure.cause),
            "{row}: {}",
            failure_cause(&failure)
        );
        assert_eq!(route.children(), ["init"], "{row}: a next child started");
        assert_eq!(
            failure.last_completed,
            Some(GitPlaneStepV1::Admission),
            "{row}"
        );
        assert!(!dest.path.join("repository").exists(), "{row}");
    }
}

/// A parent renamed away and replaced by a symlink during enumeration is never followed: the census
/// walks the retained descriptor, so the decoy beside the link is never seen or opened, and the
/// retained chain then refuses before the next child.
#[test]
fn a_symlinked_parent_during_enumeration_is_never_followed() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_outside_area, outside) = external_dir_with(&["zz-decoy"]);
    let outside_before = snapshot(&outside);
    let bootstrap = dest.bootstrap();
    let link_target = outside.clone();
    let _hook = install_git_hook_for_test(GitHookPointV1::CensusListed, move |event| {
        if event.path.ends_with("git-bootstrap/objects")
            && bootstrap.join("objects").is_dir()
            && std::fs::symlink_metadata(bootstrap.join("objects")).is_ok_and(|m| m.is_dir())
        {
            std::fs::rename(bootstrap.join("objects"), bootstrap.join("objects-away"))
                .expect("objects is renamed away");
            std::os::unix::fs::symlink(&link_target, bootstrap.join("objects"))
                .expect("a link takes its name");
        }
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    // The decoy was never listed (it would be an UnexpectedEntry), and nothing was followed.
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { name }) if name == "objects")
            || matches!(
                &failure.cause,
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
            ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(
        route.children(),
        ["init"],
        "no next writing child may start"
    );
    assert_eq!(
        snapshot(&outside),
        outside_before,
        "the external directory was not touched"
    );
    assert!(
        dest.bootstrap().join("objects-away/pack").is_dir(),
        "the moved tree is preserved"
    );
}

/// A retained subdirectory replaced by another empty directory is an identity change.
#[test]
fn a_replaced_retained_subdirectory_refuses_as_an_identity_change() {
    let (outside_area, outside) = external_dir_with(&[]);
    let away = outside.join("info-away");
    let (dest, route, failure) = refused_after_child("index-pack --strict --stdin", move |paths| {
        std::fs::rename(paths.bootstrap.join("objects/info"), &away).expect("info moves away");
        std::fs::create_dir(paths.bootstrap.join("objects/info")).expect("a replacement info");
    });
    // The gate refuses the swap, and the reconciliation of the writing child that ran meets the
    // same drift: both are kept, and the gate's is the primary cause.
    let GitBootstrapErrorV1::ChildAndReconcile { primary, reconcile } = &failure.cause else {
        panic!("{}", failure_cause(&failure));
    };
    assert!(
        matches!(&**primary,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("objects/info")),
        "{primary:?}"
    );
    assert!(
        matches!(&**reconcile,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("objects/info")),
        "{reconcile:?}"
    );
    assert_eq!(
        route.children(),
        ["init", "index-pack"],
        "verify-pack must not start"
    );
    assert!(
        outside.join("info-away").is_dir(),
        "the moved directory is preserved"
    );
    assert!(dest.bootstrap().join("objects/info").is_dir());
    drop(outside_area);
}

/// A bootstrap directory on another device refuses.
#[test]
fn a_bootstrap_directory_on_another_device_refuses() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);

    let (dest, verified) = phase_v(&capsule.capsule);
    let dev = device_of(&dest.path);
    let _device = override_census_dev_for_test("objects/pack", dev + 1);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing { name })
                if name == "git-bootstrap/objects/pack"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"]);
    drop(_device);

    // The bootstrap itself, as created: no child starts.
    let (dest, verified) = phase_v(&capsule.capsule);
    let dev = device_of(&dest.path);
    let _device = override_created_dev_for_test(".restore-work/git-bootstrap", dev + 1);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing { name })
                if name == ".restore-work/git-bootstrap"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), Vec::<String>::new());
    assert!(
        dest.bootstrap().is_dir(),
        "the created directory is preserved"
    );
}

// ---------------------------------------------------------------------------------------------
// Task 2: failed and over-budget writing children reconcile the ledger
// ---------------------------------------------------------------------------------------------

/// The ledger charge of everything up to and including the nine `for_init` entries, for a fresh
/// phase-V value over `capsule`: `(phase_v_used, base_after_pins)`.
fn pin_charge(verified: &VerifiedCapsuleV1) -> (u64, u64) {
    let used = ledger_used(verified);
    (used, used + (2 + INIT_BARE_ENTRIES_V1) * ENTRY_V1)
}

/// A writing child that exceeds its logical-byte reservation refuses, and the ledger keeps the
/// whole reservation: it is released only down to bytes the census measured and understood.
#[test]
fn a_child_that_writes_past_its_reservation_refuses_and_keeps_it_charged() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[(
        "init --bare",
        "\"$REAL\" \"$@\"; rc=$?; head -c 8192 /dev/zero >> git-bootstrap/config; exit $rc",
    )]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(detail))
                if detail.contains("above its")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.ledger_used, base + INIT_BARE_LOGICAL_BYTES_V1);
    assert_eq!(route.children(), ["init"]);
    assert!(
        std::fs::metadata(dest.bootstrap().join("config"))
            .expect("config")
            .len()
            > 8192
    );
}

/// A child that fails (nonzero exit) after writing is reconciled down to what it left, when the
/// census completes within the reservation; an entry nothing reserved is an overrun that keeps the
/// full reservation and is refused beside the child's failure.
#[test]
fn a_failed_writing_child_reconciles_the_ledger_only_for_an_understood_tree() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);

    // init runs, writes its files, and exits 3: the tree is exactly `HEAD` and `config`.
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[("init --bare", "\"$REAL\" \"$@\"; exit 3")]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            &failure.cause,
            GitBootstrapErrorV1::GitChild {
                command: "init --bare",
                status: Some(3),
                ..
            }
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Admission));
    assert_eq!(failure.ledger_used, base + tree_bytes(&dest.bootstrap()));
    assert_eq!(route.children(), ["init"]);

    // The same, but the child also leaves a tenth entry the nine-entry reservation never covered:
    // an overrun. It keeps the whole reservation and is refused BESIDE the child's failure.
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[(
        "init --bare",
        "\"$REAL\" \"$@\"; echo extra > git-bootstrap/extra; exit 3",
    )]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        combined_with(
            &failure.cause,
            |primary| matches!(
                primary,
                GitBootstrapErrorV1::GitChild {
                    status: Some(3),
                    ..
                }
            ),
            |reconcile| is_budget(reconcile, "entries above its"),
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.ledger_used, base + INIT_BARE_LOGICAL_BYTES_V1);
    assert!(
        dest.bootstrap().join("extra").is_file(),
        "the extra file is preserved"
    );

    // A failed `index-pack` that created nothing releases its bound down to the bytes present AND
    // its three entry allowances, which it never used.
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[("index-pack --strict --stdin", "cat > /dev/null; exit 1")]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause, GitBootstrapErrorV1::StrictPack(_)),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::BootstrapInit));
    assert_eq!(failure.ledger_used, base + tree_bytes(&dest.bootstrap()));
    assert_eq!(route.children(), ["init", "index-pack"]);
}

// ---------------------------------------------------------------------------------------------
// Task 2: Git output is classified strictly, child by child
// ---------------------------------------------------------------------------------------------

const ZERO_OID_V1: &str = "0000000000000000000000000000000000000000";
const FFFF_OID_V1: &str = "ffffffffffffffffffffffffffffffffffffffff";

/// Runs the Captured SHA-1 bootstrap over a route whose `rules` tamper with one child, and returns
/// the refusal with the child log. The log proves the tampered child started.
fn tampered(rules: &[(&str, &str)]) -> (RouteV1, GitBootstrapFailureV1) {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (_dest, verified) = phase_v(&capsule.capsule);
    let route = RouteV1::new(rules);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    (route, failure)
}

type ErrorCheckV1 = fn(&GitBootstrapErrorV1) -> bool;

fn table(
    child: &str,
    last_child: &str,
    last_completed: GitPlaneStepV1,
    needle: &str,
    rows: &[(&str, &str, ErrorCheckV1)],
) {
    let mut failures = Vec::new();
    for (row, shell, check) in rows {
        let (route, failure) = tampered(&[(needle, shell)]);
        let started = route.children().last().map(String::as_str) == Some(last_child);
        let ok = check(&failure.cause)
            && started
            && failure.last_completed == Some(last_completed)
            && !failure.active_materialization_began;
        if !ok {
            failures.push(format!(
                "{child}: {row}: cause {}, children {:?}, completed {:?}",
                failure_cause(&failure),
                route.children(),
                failure.last_completed
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn index_pack_output_and_status_are_classified_strictly() {
    table(
        "index-pack",
        "index-pack",
        GitPlaneStepV1::BootstrapInit,
        "index-pack --strict --stdin",
        &[
            (
                "a strict object id on stderr",
                "cat > /dev/null; echo 'error: object 0123456789abcdef0123456789abcdef01234567: zeroPaddedFilemode: contains zero-padded file modes' >&2; exit 128",
                |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
            ),
            (
                "a plain failure",
                "cat > /dev/null; echo 'fatal: bad pack' >&2; exit 1",
                |e| matches!(e, GitBootstrapErrorV1::StrictPack(detail) if detail.contains("bad pack")),
            ),
            (
                "an extra line after a valid record",
                "\"$REAL\" \"$@\"; echo noise",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a second record",
                "out=$(\"$REAL\" \"$@\"); echo \"$out\"; echo \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "invalid UTF-8 after a valid record",
                "\"$REAL\" \"$@\"; printf '\\377\\n'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "no final newline",
                "out=$(\"$REAL\" \"$@\"); printf '%s' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "trailing whitespace",
                "out=$(\"$REAL\" \"$@\"); printf '%s \\n' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "leading whitespace",
                "out=$(\"$REAL\" \"$@\"); printf ' %s\\n' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a carriage return",
                "out=$(\"$REAL\" \"$@\"); printf '%s\\r\\n' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a hash that is too short",
                "cat > /dev/null; printf 'pack\\t%039d\\n' 0",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "an uppercase hash",
                "cat > /dev/null; printf 'pack\\tAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\\n'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "no pack prefix",
                "cat > /dev/null; printf '%040d\\n' 0",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "no output at all",
                "cat > /dev/null",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
        ],
    );
}

#[test]
fn verify_pack_must_report_the_generated_pack_ok() {
    table(
        "verify-pack",
        "verify-pack",
        GitPlaneStepV1::BootstrapInit,
        "verify-pack -v",
        &[
            (
                "no integrity line",
                "echo 'something else'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(detail) if detail.contains("did not report")),
            ),
            (
                "an ok line for another pack",
                "echo 'other/objects/pack/pack-0000.pack: ok'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a bad line",
                "echo 'git-bootstrap/objects/pack/pack-x.pack: bad'; exit 0",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            ("a nonzero exit", "exit 1", |e| {
                matches!(
                    e,
                    GitBootstrapErrorV1::GitChild {
                        command: "verify-pack",
                        status: Some(1),
                        ..
                    }
                )
            }),
        ],
    );
}

#[test]
fn inventory_rows_are_classified_strictly() {
    table(
        "cat-file",
        "cat-file",
        GitPlaneStepV1::Indexing,
        "cat-file --batch-all-objects",
        &[
            (
                "a missing object",
                "\"$REAL\" \"$@\" | tail -n +2",
                |e| matches!(e, GitBootstrapErrorV1::InventoryMismatch(detail) if detail.contains("missing")),
            ),
            (
                "an extra object",
                "\"$REAL\" \"$@\"; echo 'ffffffffffffffffffffffffffffffffffffffff blob'",
                |e| matches!(e, GitBootstrapErrorV1::InventoryMismatch(detail) if detail.contains("extra object")),
            ),
            (
                "a wrong kind",
                "\"$REAL\" \"$@\" | sed 's/ blob$/ tree/'",
                |e| matches!(e, GitBootstrapErrorV1::InventoryMismatch(detail) if detail.contains("not blob")),
            ),
            (
                "a duplicate row",
                "out=$(\"$REAL\" \"$@\"); echo \"$out\"; echo \"$out\" | head -n 1",
                |e| matches!(e, GitBootstrapErrorV1::InventoryMismatch(detail) if detail.contains("twice")),
            ),
            (
                "a row with a third field",
                "\"$REAL\" \"$@\" | sed '1s/$/ junk/'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "an object id that is not hex",
                "\"$REAL\" \"$@\" | sed '1s/^[0-9a-f]/G/'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a kind git does not print",
                "\"$REAL\" \"$@\" | sed '1s/ [a-z]*$/ gizmo/'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "output that is not UTF-8",
                "\"$REAL\" \"$@\"; printf '\\377\\n'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(detail) if detail.contains("UTF-8")),
            ),
            (
                "a truncated final line",
                "out=$(\"$REAL\" \"$@\"); printf '%s' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(detail) if detail.contains("partial")),
            ),
            ("a nonzero exit", "\"$REAL\" \"$@\"; exit 2", |e| {
                matches!(
                    e,
                    GitBootstrapErrorV1::GitChild {
                        command: "cat-file --batch-all-objects",
                        status: Some(2),
                        ..
                    }
                )
            }),
        ],
    );
}

#[test]
fn closure_rows_are_classified_strictly() {
    table(
        "rev-list",
        "rev-list",
        GitPlaneStepV1::Inventory,
        "rev-list --objects",
        &[
            (
                "a missing marker",
                "cat > /dev/null; echo \"?$(printf '%040d' 0)\"",
                |e| matches!(e, GitBootstrapErrorV1::ClosureIncomplete(detail) if detail.contains("absent")),
            ),
            ("malformed output", "cat > /dev/null; echo garbage", |e| {
                matches!(e, GitBootstrapErrorV1::GitOutput(_))
            }),
            (
                "an unexpected closure object",
                "\"$REAL\" \"$@\"; echo 'ffffffffffffffffffffffffffffffffffffffff'",
                |e| matches!(e, GitBootstrapErrorV1::ClosureIncomplete(detail) if detail.contains("unexpected")),
            ),
            (
                "an inventory object that is not reached",
                "\"$REAL\" \"$@\" | tail -n +2",
                |e| matches!(e, GitBootstrapErrorV1::ClosureIncomplete(detail) if detail.contains("not reached")),
            ),
            (
                "a duplicate row",
                "out=$(\"$REAL\" \"$@\"); echo \"$out\"; echo \"$out\" | head -n 1",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(detail) if detail.contains("twice")),
            ),
            (
                "output that is not UTF-8",
                "\"$REAL\" \"$@\"; printf '\\377\\n'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(detail) if detail.contains("UTF-8")),
            ),
            (
                "a truncated final line",
                "out=$(\"$REAL\" \"$@\"); printf '%s' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(detail) if detail.contains("partial")),
            ),
            ("a nonzero exit", "cat > /dev/null; exit 1", |e| {
                matches!(
                    e,
                    GitBootstrapErrorV1::GitChild {
                        command: "rev-list --missing=print",
                        status: Some(1),
                        ..
                    }
                )
            }),
        ],
    );
}

#[test]
fn fsck_output_is_classified_before_the_exit_status() {
    table(
        "fsck",
        "fsck",
        GitPlaneStepV1::Closure,
        "fsck --strict",
        &[
            (
                "stdout noise",
                "\"$REAL\" \"$@\"; echo noise",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a strict object id and a nonzero exit",
                "echo 'error: object 0123456789abcdef0123456789abcdef01234567: zeroPaddedFilemode: contains zero-padded file modes' >&2; exit 1",
                |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
            ),
            (
                "an unclassified diagnostic",
                "echo 'warning: strange' >&2",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a nonzero exit with a clean stream",
                "exit 1",
                |e| matches!(e, GitBootstrapErrorV1::GitChild { command: "fsck --strict", status: Some(1), .. }),
            ),
        ],
    );
}

/// `index-pack` is fed exactly the staged bytes: a staged pack changed after admission, at the same
/// length, is caught by the runner's attestation of what the pipe accepted.
#[test]
fn a_staged_pack_changed_after_admission_is_caught_by_the_stdin_attestation() {
    use std::io::{Seek as _, SeekFrom, Write as _};
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let staged = staged_path(&dest, PACK_STAGED_V1);
    let _hook = install_git_hook_for_test(GitHookPointV1::BeforeChild, move |event| {
        if event.label == "index-pack --strict --stdin" {
            write_into(&staged, |file| {
                file.seek(SeekFrom::Start(10)).expect("seek");
                file.write_all(&[0xff]).expect("change one byte");
            });
        }
    });
    // The child accepts whatever it is fed, so only the attestation can object.
    let route = RouteV1::new(&[(
        "index-pack --strict --stdin",
        "cat > /dev/null; printf 'pack\\t%040d\\n' 0",
    )]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::VerificationInputMismatch
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init", "index-pack"]);
}

/// A staged pack that grew past its recorded length is refused by the runner before any spawn.
#[test]
fn a_staged_pack_that_grew_refuses_before_index_pack_spawns() {
    use std::io::{Seek as _, SeekFrom, Write as _};
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let staged = staged_path(&dest, PACK_STAGED_V1);
    // After admission's re-measurement and before the request is built: the hook at init's exit.
    let _hook = install_git_hook_for_test(GitHookPointV1::AfterChild, move |event| {
        if event.label == "init --bare" {
            write_into(&staged, |file| {
                file.seek(SeekFrom::End(0)).expect("seek");
                file.write_all(b"grown").expect("grow");
            });
        }
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Git(CustodyGitError::StdinLimit { .. })
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"], "index-pack never spawned");
}

// ---------------------------------------------------------------------------------------------
// Task 2: real strict failures
// ---------------------------------------------------------------------------------------------

/// Replaces the staged pack's bytes in place, and the recorded length and digest with them, as a
/// sealed capsule carrying those pack bytes would have reached phase G.
fn restage_pack(dest: &DestV1, verified: &mut VerifiedCapsuleV1, bytes: &[u8]) {
    let path = staged_path(dest, PACK_STAGED_V1);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .expect("the staged pack opens");
    std::io::Write::write_all(&mut file, bytes).expect("the replacement bytes");
    let one = verified
        .staged
        .iter_mut()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .expect("the staged pack");
    one.length = bytes.len() as u64;
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    one.sha256 = sha256;
}

#[test]
fn a_corrupt_pack_is_refused_by_real_strict_indexing() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, mut verified) = phase_v(&capsule.capsule);
    let original = std::fs::read(staged_path(&dest, PACK_STAGED_V1)).expect("the pack");
    let truncated = &original[..original.len() / 2];
    restage_pack(&dest, &mut verified, truncated);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(failure.cause, GitBootstrapErrorV1::StrictPack(_)),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init", "index-pack"]);
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::BootstrapInit));
}

/// A pack holding a strictly invalid object (a tree with a zero-padded filemode) is refused by
/// `index-pack --strict` with its message id, from a real pack.
#[test]
fn a_strictly_invalid_object_is_refused_with_its_message_id() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, mut verified) = phase_v(&capsule.capsule);
    // A scratch repository builds the bad tree and packs it.
    let scratch = tempfile::TempDir::new().expect("a scratch");
    let git_dir = scratch.path().join("scratch.git");
    std::fs::create_dir(&git_dir).expect("the scratch git directory");
    let _ = Command::new(lane_git_path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["init", "--bare", "-q", "--template="])
        .arg(&git_dir)
        .output()
        .expect("the scratch init");
    let blob = plumb(&git_dir, &["hash-object", "-w", "--stdin"], Some(b"x\n"));
    let blob_raw: Vec<u8> = (0..blob.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&blob[index..index + 2], 16).expect("hex"))
        .collect();
    let mut tree = b"0100644 f\0".to_vec();
    tree.extend_from_slice(&blob_raw);
    let tree_id = {
        use std::io::Write as _;
        let mut child = Command::new(lane_git_path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_DIR", &git_dir)
            .args(["hash-object", "-t", "tree", "--literally", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("hash-object starts");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(&tree)
            .expect("tree bytes");
        String::from_utf8(child.wait_with_output().expect("hash-object ends").stdout)
            .expect("an object id")
            .trim()
            .to_owned()
    };
    let pack = {
        use std::io::Write as _;
        let mut child = Command::new(lane_git_path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_DIR", &git_dir)
            .args(["pack-objects", "--stdout"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("pack-objects starts");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(format!("{tree_id}\n{blob}\n").as_bytes())
            .expect("object ids");
        child.wait_with_output().expect("pack-objects ends").stdout
    };
    assert!(pack.starts_with(b"PACK"));
    restage_pack(&dest, &mut verified, &pack);

    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init", "index-pack"]);
}

// ---------------------------------------------------------------------------------------------
// Task 2: one absolute deadline, no retry
// ---------------------------------------------------------------------------------------------

#[test]
fn an_expired_deadline_refuses_before_any_child_and_a_slow_child_times_out() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);

    // Already past: admission and the pins complete, and no child is ever started.
    let (dest, verified) = phase_v(&capsule.capsule);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = match bootstrap_git_objects_v1(
        verified,
        &runner,
        GitObjectFormatV1::Sha1,
        Instant::now(),
    ) {
        Ok(_) => panic!("an expired deadline must refuse"),
        Err(failure) => failure,
    };
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), Vec::<String>::new());
    assert_eq!(
        dest.work_entries(),
        ["git-bootstrap", "home", "plain", "xdg"]
    );

    // A child that outlives the one deadline is killed and reported; no retry follows.
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[("index-pack --strict --stdin", "cat > /dev/null; sleep 60")]);
    let (_area, runner) = route.runner();
    let started = Instant::now();
    let failure = match bootstrap_git_objects_v1(
        verified,
        &runner,
        GitObjectFormatV1::Sha1,
        Instant::now() + Duration::from_secs(8),
    ) {
        Ok(_) => panic!("a slow child must time out"),
        Err(failure) => failure,
    };
    assert!(
        started.elapsed() < Duration::from_secs(45),
        "the child was killed at the deadline"
    );
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Git(CustodyGitError::Timeout)
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init", "index-pack"], "no retry");
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::BootstrapInit));
    // The timed-out writing child was reconciled: it made nothing, so its three entries and its
    // bound are released down to what is present.
    assert_eq!(failure.ledger_used, base + tree_bytes(&dest.bootstrap()));
}

// ---------------------------------------------------------------------------------------------
// Task 2: the closed environment; ambient state cannot reach the children
// ---------------------------------------------------------------------------------------------

const AMBIENT_CHILD_V1: &str = "A2A_PASS_A_AMBIENT_CHILD";

/// The body that runs inside a re-executed test binary whose process environment carries planted
/// `GIT_*` and `HOME` state. In an ordinary run it does nothing, and the parent control below
/// requires the marker only the planted run prints.
#[test]
fn ambient_environment_child() {
    if std::env::var_os(AMBIENT_CHILD_V1).is_none() {
        return;
    }
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    // The source is made unavailable, and the ambient state is planted AFTER fixtures are built.
    std::fs::remove_dir_all(&capsule.source.git_dir).expect("the source is removed");
    let planted = [
        ("GIT_DIR", "/nonexistent-ambient/git-dir"),
        ("GIT_OBJECT_DIRECTORY", "/nonexistent-ambient/objects"),
        (
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "/nonexistent-ambient/alternates",
        ),
        ("GIT_INDEX_FILE", "/nonexistent-ambient/index"),
        ("GIT_WORK_TREE", "/nonexistent-ambient/worktree"),
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "core.hooksPath"),
        ("GIT_CONFIG_VALUE_0", "/nonexistent-ambient/hooks"),
        ("GIT_CONFIG_GLOBAL", "/nonexistent-ambient/gitconfig"),
        ("HOME", "/nonexistent-ambient/home"),
        ("XDG_CONFIG_HOME", "/nonexistent-ambient/xdg"),
    ];
    for (key, value) in planted {
        std::env::set_var(key, value);
    }
    let route = RouteV1::new(&[(
        "cat-file --batch-all-objects",
        "env | sort > \"$LOGFILE.env\"; exec \"$REAL\" \"$@\"",
    )]);
    let (_area, runner) = route.runner();
    let bootstrap = bootstrap(verified, &runner, GitObjectFormatV1::Sha1)
        .unwrap_or_else(|failure| panic!("the ambient bootstrap must succeed: {failure:?}"));
    let env = std::fs::read_to_string(format!("{}.env", route.log.display())).expect("the env log");
    assert!(
        !env.contains("nonexistent-ambient"),
        "an ambient value reached the child:\n{env}"
    );
    assert!(env.contains("GIT_DIR=git-bootstrap"), "{env}");
    assert!(
        env.contains("HOME=home") && env.contains("XDG_CONFIG_HOME=xdg"),
        "{env}"
    );
    assert!(env.contains("GIT_CONFIG_GLOBAL=/dev/null"), "{env}");
    for key in [
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_INDEX_FILE",
        "GIT_WORK_TREE",
        "GIT_CONFIG_COUNT",
    ] {
        assert!(
            !env.contains(&format!("{key}=")),
            "{key} reached the child:\n{env}"
        );
    }
    for run in &bootstrap.runs {
        assert!(!run.environment.contains_key("GIT_OBJECT_DIRECTORY"));
    }
    assert_eq!(dest.entries(), [".restore-work"]);
    println!("AMBIENT-CHILD-OK");
}

#[test]
fn planted_ambient_git_environment_cannot_reach_the_children() {
    if std::env::var_os(AMBIENT_CHILD_V1).is_some() {
        return;
    }
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            "custody_restore::git_tests::ambient_environment_child",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(AMBIENT_CHILD_V1, "1")
        .output()
        .expect("the ambient child runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("AMBIENT-CHILD-OK"),
        "the ambient child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------------------------
// Task 5: the closing gate, partial progress, and a second invocation
// ---------------------------------------------------------------------------------------------

/// Drift after each writing child: `plain/` is swapped in the child's post-exit callback, so the
/// gate refuses before the next step, with the completed step recorded and every byte kept.
#[test]
fn drift_after_each_writing_child_refuses_with_typed_progress() {
    for (label, completed) in [
        ("init --bare", Some(GitPlaneStepV1::Admission)),
        (
            "index-pack --strict --stdin",
            Some(GitPlaneStepV1::BootstrapInit),
        ),
    ] {
        let (dest, route, failure) = refused_after_child(label, |paths| {
            std::fs::rename(paths.work.join("plain"), paths.work.join("plain-away"))
                .expect("plain moves away");
            std::fs::create_dir(paths.work.join("plain")).expect("a replacement plain");
        });
        assert!(
            matches!(
                failure.cause,
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
            ),
            "{label}: {}",
            failure_cause(&failure)
        );
        assert_eq!(failure.last_completed, completed, "{label}");
        assert!(!failure.active_materialization_began);
        assert_eq!(
            route.children().last().map(String::as_str),
            Some(&label[..label.find(' ').unwrap_or(label.len())])
        );
        // Filesystem-visible state: the moved tree and the bootstrap are preserved.
        assert!(dest.work().join("plain-away").is_dir(), "{label}");
        assert!(dest.bootstrap().join("HEAD").is_file(), "{label}");
        assert!(!dest.path.join("repository").exists(), "{label}");
    }
}

/// The closing gate checks the root's identity LAST: a root swapped just before it refuses, with
/// every proof step recorded as complete.
#[test]
fn a_root_swapped_before_the_closing_gate_refuses_with_every_proof_recorded() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let root = dest.path.clone();
    let away = root.with_file_name("dest-away");
    let swap_away = away.clone();
    let _hook = install_git_hook_for_test(GitHookPointV1::BeforeFinalGate, move |_| {
        std::fs::rename(&root, &swap_away).expect("the root moves away");
        std::fs::create_dir(&root).expect("an empty root takes its name");
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Fsck));
    assert_eq!(route.children().len(), 6, "every proof child ran");
    assert_eq!(
        dest.entries(),
        Vec::<String>::new(),
        "the replacement root stays empty"
    );
    assert!(
        away.join(".restore-work/git-bootstrap/HEAD").is_file(),
        "the bootstrap is preserved"
    );
}

/// A mount that appears only for the closing gate's own census refuses, with every proof recorded.
#[test]
fn a_mount_appearing_for_the_closing_gate_census_refuses() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);

    // Learn the number of census calls in a clean run.
    let (_dest, verified) = phase_v(&capsule.capsule);
    let census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec()]);
    let _ = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    let total = census.calls();
    drop(census);
    assert!(total > 10, "{total} census calls");

    let (dest, verified) = phase_v(&capsule.capsule);
    let inside = dest
        .path
        .join(".restore-work/home")
        .as_os_str()
        .as_bytes()
        .to_vec();
    let _census = crate::custody_mounts::seam::install(Box::new(move |call| {
        if call == total {
            Ok(vec![b"/".to_vec(), inside.clone()])
        } else {
            Ok(vec![b"/".to_vec()])
        }
    }));
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::MountBoundary(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Fsck));
    assert_eq!(route.children().len(), 6);
}

/// A census that cannot list the mount points fails closed.
#[test]
fn a_failing_mount_census_fails_closed() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let _census = crate::custody_mounts::seam::install(Box::new(|_| {
        Err(crate::custody_mounts::CustodyMountErrorV1::MalformedLine { line: 1 })
    }));
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::MountBoundary(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_no_phase_g_effect(&dest);
}

/// The success control with every mutation seam enabled and nothing changed.
#[test]
fn a_success_control_with_every_seam_enabled_and_no_change_succeeds() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let dev = device_of(&dest.path);
    let hooked = std::rc::Rc::new(std::cell::Cell::new(0_usize));
    let mut guards = Vec::new();
    for point in [
        GitHookPointV1::AdmissionEntry,
        GitHookPointV1::BeforeBootstrapCreate,
        GitHookPointV1::BeforeChildCensus,
        GitHookPointV1::BeforeChild,
        GitHookPointV1::AfterChild,
        GitHookPointV1::CensusListed,
        GitHookPointV1::BeforeCensusOpen,
        GitHookPointV1::BeforeFinalGate,
    ] {
        let hooked = std::rc::Rc::clone(&hooked);
        guards.push(install_git_hook_for_test(point, move |_| {
            hooked.set(hooked.get() + 1)
        }));
    }
    guards.push(override_census_dev_for_test("objects/pack", dev));
    guards.push(override_census_dev_for_test("", dev));
    let census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec()]);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let bootstrap = bootstrap(verified, &runner, GitObjectFormatV1::Sha1)
        .unwrap_or_else(|failure| panic!("the control must succeed: {failure:?}"));
    assert_eq!(bootstrap.runs.len(), 6);
    assert!(hooked.get() > 20, "every seam ran: {}", hooked.get());
    assert!(census.calls() > 10);
    drop(guards);
}

/// A refused invocation leaves its created bytes, and a second phase V over the same destination
/// refuses without touching them.
#[test]
fn a_refused_second_invocation_cannot_overwrite_the_first_partial_state() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let route = RouteV1::new(&[("index-pack --strict --stdin", "cat > /dev/null; exit 1")]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(matches!(failure.cause, GitBootstrapErrorV1::StrictPack(_)));
    let before = snapshot(&dest.path);
    assert!(before.contains_key(".restore-work/git-bootstrap/HEAD"));

    let second = verify_and_stage_v1(
        &capsule.capsule,
        &dest.path,
        CustodyRestoreBudgetV1::ceiling(),
        frame_budget(),
        &FixtureOpenerV1::honest(),
    );
    assert!(
        matches!(second, Err(CustodyRestoreErrorV1::DestinationInvalid(_))),
        "{second:?}"
    );
    assert_eq!(
        snapshot(&dest.path),
        before,
        "the partial state is byte-identical"
    );
}

// ---------------------------------------------------------------------------------------------
// Exposed helpers keep their exporter behavior
// ---------------------------------------------------------------------------------------------

#[test]
fn the_exposed_budget_helpers_keep_their_exporter_formulas() {
    // SHA-1: trailer 40; index (20+8)*4 + 8*4 + 1072 + 40 = 1256; reverse 4*4 + 12 + 40 = 68.
    assert_eq!(
        index_pack_logical_bound(100, 4, 20).expect("a bound"),
        100 + 1256 + 68
    );
    // SHA-256: trailer 64; index 40*4 + 32 + 1072 + 64 = 1328; reverse 16 + 12 + 64 = 92.
    assert_eq!(
        index_pack_logical_bound(100, 4, 32).expect("a bound"),
        100 + 1328 + 92
    );
    assert!(
        index_pack_logical_bound(u64::MAX, 1, 20).is_err(),
        "overflow refuses"
    );

    let mut budget = GitDirectoryBudgetV1::for_init("git-bootstrap");
    assert_eq!(budget.directory, "git-bootstrap");
    assert_eq!(budget.entries, INIT_BARE_ENTRIES_V1);
    assert_eq!(budget.logical_bytes, INIT_BARE_LOGICAL_BYTES_V1);
    assert_eq!(
        budget.files.iter().map(String::as_str).collect::<Vec<_>>(),
        ["HEAD", "config"]
    );
    budget
        .admit_index_pack(&"a".repeat(40), 1000)
        .expect("admitted");
    assert_eq!(budget.entries, INIT_BARE_ENTRIES_V1 + INDEX_PACK_ENTRIES_V1);
    assert_eq!(budget.logical_bytes, INIT_BARE_LOGICAL_BYTES_V1 + 1000);
    for extension in ["pack", "idx", "rev"] {
        assert!(budget
            .files
            .contains(&format!("objects/pack/pack-{}.{extension}", "a".repeat(40))));
    }
    assert!(
        budget.admit_index_pack("b", u64::MAX).is_err(),
        "overflow refuses"
    );
}

// ---------------------------------------------------------------------------------------------
// Structural controls over the seam's own source
// ---------------------------------------------------------------------------------------------

/// The bootstrap measures through retained descriptors only. It must not call the exporter's
/// path-reopening walker, open any path with `std::fs`, route a caller object store, or write a
/// repository entry: pass A creates nothing under `repository/`. That invariant is function-scoped
/// (direct `bootstrap_git_objects_v1` never creates `repository/`), so the scan covers the bootstrap
/// implementation: the production source up to the pass B1a marker, where later passes' code begins. The
/// narrowing is guarded by `b1a_the_prevalidation_region_is_read_only_and_the_scan_split_is_unique`, and
/// the filesystem behavior by `b1a_the_direct_bootstrap_creates_no_repository_and_prevalidation_adds_none`.
#[test]
fn the_seam_never_uses_the_path_walker_a_path_open_an_object_store_route_or_repository() {
    let source = include_str!("custody_restore_git.rs");
    let bootstrap_implementation = source
        .split("// Pass B1a: retained active-frame prevalidation")
        .next()
        .expect("the bootstrap implementation");
    let production = bootstrap_implementation
        .split("// Test seams")
        .next()
        .expect("the production half")
        .to_owned()
        + bootstrap_implementation
            .split("// Pure helpers")
            .nth(1)
            .expect("the helpers and steps");
    for forbidden in [
        "remeasure_git_directory",
        "measure_git_directory",
        "std::fs::read",
        "std::fs::metadata",
        "std::fs::symlink_metadata",
        "std::fs::canonicalize",
        "File::open(",
        "OpenOptions",
        "read_dir(",
        "GitObjectStoreRouteV1",
        "object_store",
        "\"repository\"",
        ".git\"",
    ] {
        assert!(
            !production.contains(forbidden),
            "the production seam mentions `{forbidden}`"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// More controls: the pin-creation gate, the admitted pack, and the admitted files' identity
// ---------------------------------------------------------------------------------------------

/// Drift between HOME/XDG and the bootstrap's creation refuses before `git-bootstrap` exists.
#[test]
fn drift_after_home_and_xdg_refuses_before_the_bootstrap_is_created() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let paths = PathsV1::of(&dest);
    let _hook = install_git_hook_for_test(GitHookPointV1::BeforeBootstrapCreate, move |_| {
        std::fs::rename(paths.work.join("plain"), paths.work.join("plain-away"))
            .expect("plain moves away");
        std::fs::create_dir(paths.work.join("plain")).expect("a replacement plain");
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(
        dest.work_entries(),
        ["home", "plain", "plain-away", "xdg"],
        "no git-bootstrap was created"
    );
    assert_eq!(route.children(), Vec::<String>::new());
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Admission));
}

/// The pack `index-pack` wrote must be exactly the staged bytes: a child that appends to it is
/// caught when the admitted files are measured from their descriptors.
#[test]
fn an_indexed_pack_that_is_not_the_staged_bytes_refuses() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (_dest, verified) = phase_v(&capsule.capsule);
    let route = RouteV1::new(&[(
        "index-pack --strict --stdin",
        "\"$REAL\" \"$@\"; rc=$?; for f in git-bootstrap/objects/pack/*.pack; do echo x >> \"$f\"; done; exit $rc",
    )]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::VerificationInputMismatch
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init", "index-pack"]);
}

/// An admitted file replaced by another file of the same name and length after admission is an
/// identity change, caught by the next census.
#[test]
fn a_replaced_admitted_object_file_refuses_before_the_next_child() {
    let (outside_area, outside) = external_dir_with(&[]);
    // After admission opened the files, so the next census holds them to their retained identity.
    let (dest, route, failure) = refused_after_child("verify-pack", move |paths| {
        let pack_dir = paths.bootstrap.join("objects/pack");
        let idx = std::fs::read_dir(&pack_dir)
            .expect("the pack directory")
            .map(|entry| entry.expect("an entry").path())
            .find(|path| path.extension().is_some_and(|ext| ext == "idx"))
            .expect("the index");
        let bytes = std::fs::read(&idx).expect("the index bytes");
        std::fs::rename(&idx, outside.join("idx-away")).expect("the index moves away");
        std::fs::write(&idx, bytes).expect("a same-name, same-length replacement");
    });
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("no longer the admitted object file")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(
        route.children(),
        ["init", "index-pack", "verify-pack"],
        "cat-file must not start"
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::BootstrapInit));
    assert!(dest.bootstrap().join("objects/pack").is_dir());
    drop(outside_area);
}

// ---------------------------------------------------------------------------------------------
// Drift between children, inside a census, and in every retained chain
// ---------------------------------------------------------------------------------------------

/// Runs the Captured SHA-1 bootstrap with `plant` run at `point` of the child labelled `label`,
/// once, and returns the refusal with the destination and the child log.
fn refused_at(
    point: GitHookPointV1,
    label: &'static str,
    plant: impl Fn(&PathsV1) + 'static,
) -> (DestV1, RouteV1, GitBootstrapFailureV1) {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let paths = PathsV1::of(&dest);
    let done = std::cell::Cell::new(false);
    let _hook = install_git_hook_for_test(point, move |event| {
        if event.label == label && !done.replace(true) {
            plant(&paths);
        }
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    (dest, route, failure)
}

/// Like [`refused_at`], but acts once, at the first event whose path ends with `suffix`.
fn refused_at_path(
    point: GitHookPointV1,
    suffix: &'static str,
    plant: impl Fn(&PathsV1) + 'static,
) -> (DestV1, RouteV1, GitBootstrapFailureV1) {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let paths = PathsV1::of(&dest);
    let done = std::cell::Cell::new(false);
    let _hook = install_git_hook_for_test(point, move |event| {
        if event.path.ends_with(suffix) && !done.replace(true) {
            plant(&paths);
        }
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    (dest, route, failure)
}

/// A link or an extra name planted BETWEEN two children is caught by the next child's own census,
/// before it starts: the chain gate cannot see a file.
#[test]
fn an_entry_planted_between_children_stops_the_next_child() {
    let (_outside_area, outside) = external_dir_with(&["secret"]);
    let outside_before = snapshot(&outside);
    let target = outside.join("secret");
    let (dest, route, failure) = refused_at(
        GitHookPointV1::BeforeChildCensus,
        "index-pack --strict --stdin",
        move |paths| {
            let head = paths.bootstrap.join("HEAD");
            std::fs::remove_file(&head).expect("HEAD goes");
            std::os::unix::fs::symlink(&target, &head).expect("a HEAD link");
        },
    );
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { name }) if name == "HEAD"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"], "index-pack must not start");
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::BootstrapInit));
    assert!(std::fs::symlink_metadata(dest.bootstrap().join("HEAD")).is_ok_and(|m| m.is_symlink()));
    assert_eq!(snapshot(&outside), outside_before);

    let (_dest, route, failure) =
        refused_at(GitHookPointV1::BeforeChildCensus, "verify-pack", |paths| {
            std::fs::write(paths.bootstrap.join("objects/pack/stray"), b"x").expect("a stray file");
        });
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::UnexpectedEntry { name } if name == "objects/pack/stray"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(
        route.children(),
        ["init", "index-pack"],
        "verify-pack must not start"
    );
}

/// A listed entry swapped between its `fstatat` and its open is bound to the listing: an opened
/// file that is not the listed entry refuses, and a link in its place is refused by the no-follow
/// open, never followed.
#[test]
fn a_census_open_is_bound_to_the_listed_entry() {
    let (_outside_area, outside) = external_dir_with(&["secret"]);
    let outside_before = snapshot(&outside);

    // A new regular file takes HEAD's name between the listing and the open.
    let (dest, route, failure) = refused_at_path(
        GitHookPointV1::BeforeCensusOpen,
        "git-bootstrap/HEAD",
        |paths| {
            let head = paths.bootstrap.join("HEAD");
            std::fs::rename(&head, paths.bootstrap.join("HEAD-away")).expect("HEAD moves away");
            std::fs::write(&head, b"ref: refs/heads/other\n").expect("a new HEAD");
        },
    );
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("opened file is not the listed entry")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"]);
    assert!(
        dest.bootstrap().join("HEAD-away").is_file(),
        "the moved file is preserved"
    );

    // HEAD replaced by a symlink to an outside file between the listing and the open.
    let target = outside.join("secret");
    let (dest, route, failure) = refused_at_path(
        GitHookPointV1::BeforeCensusOpen,
        "git-bootstrap/HEAD",
        move |paths| {
            let head = paths.bootstrap.join("HEAD");
            std::fs::remove_file(&head).expect("HEAD goes");
            std::os::unix::fs::symlink(&target, &head).expect("a HEAD link");
        },
    );
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { name }) if name == "HEAD"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"]);
    assert!(std::fs::symlink_metadata(dest.bootstrap().join("HEAD")).is_ok_and(|m| m.is_symlink()));
    assert_eq!(
        snapshot(&outside),
        outside_before,
        "the outside file was not opened or touched"
    );
}

/// A directory swapped between its listing and its adoption is not adopted.
#[test]
fn a_directory_swapped_during_adoption_is_not_adopted() {
    let (outside_area, outside) = external_dir_with(&[]);
    let away = outside.join("info-away");
    let (dest, route, failure) = refused_at_path(
        GitHookPointV1::BeforeCensusOpen,
        "git-bootstrap/objects/info",
        move |paths| {
            let info = paths.bootstrap.join("objects/info");
            std::fs::rename(&info, &away).expect("info moves away");
            std::fs::create_dir(&info).expect("a replacement info");
        },
    );
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("changed while it was adopted")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"]);
    assert!(outside.join("info-away").is_dir() && dest.bootstrap().join("objects/info").is_dir());
    drop(outside_area);
}

/// A drift in the child's own pre-spawn callback stops that child, whichever retained chain moved.
#[test]
fn drift_in_the_pre_spawn_callback_stops_the_child() {
    type Swap = fn(&PathsV1);
    let rows: [(&str, &str, Swap, &[&str], &str); 5] = [
        (
            "plain",
            "init --bare",
            |p| {
                std::fs::rename(p.work.join("plain"), p.work.join("plain-away")).expect("away");
                std::fs::create_dir(p.work.join("plain")).expect("replacement");
            },
            &[],
            ".restore-work/plain",
        ),
        (
            "home",
            "init --bare",
            |p| {
                std::fs::rename(p.work.join("home"), p.work.join("home-away")).expect("away");
                std::fs::create_dir(p.work.join("home")).expect("replacement");
            },
            &[],
            ".restore-work/home",
        ),
        (
            "xdg",
            "init --bare",
            |p| {
                std::fs::rename(p.work.join("xdg"), p.work.join("xdg-away")).expect("away");
                std::fs::create_dir(p.work.join("xdg")).expect("replacement");
            },
            &[],
            ".restore-work/xdg",
        ),
        (
            "the bootstrap",
            "index-pack --strict --stdin",
            |p| {
                std::fs::rename(&p.bootstrap, p.work.join("git-bootstrap-away")).expect("away");
                std::fs::create_dir(&p.bootstrap).expect("replacement");
            },
            &["init"],
            ".restore-work/git-bootstrap",
        ),
        (
            "a nested directory",
            "verify-pack",
            |p| {
                std::fs::rename(
                    p.bootstrap.join("objects/info"),
                    p.bootstrap.join("objects/info-away"),
                )
                .expect("away");
                std::fs::create_dir(p.bootstrap.join("objects/info")).expect("replacement");
            },
            &["init", "index-pack"],
            "objects/info",
        ),
    ];
    for (row, label, plant, started, named) in rows {
        let (_dest, route, failure) = refused_at(GitHookPointV1::BeforeChild, label, plant);
        // A child whose tree was swapped before it spawned is also measured, and the census meets
        // the same swap: the gate's refusal stays the primary cause, beside the census's.
        let primary = match &failure.cause {
            GitBootstrapErrorV1::ChildAndReconcile { primary, .. } => &**primary,
            other => other,
        };
        assert!(
            matches!(primary,
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                    if detail.contains(named)),
            "{row}: {}",
            failure_cause(&failure)
        );
        assert_eq!(route.children(), started, "{row}: the child must not start");
    }
}

/// A staging subdirectory swapped at entry refuses at the first gate.
#[test]
fn a_swapped_staging_subdirectory_refuses_at_entry() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let plain = dest.work().join("plain");
    let _hook = install_git_hook_for_test(GitHookPointV1::AdmissionEntry, move |_| {
        std::fs::rename(plain.join("git"), plain.join("git-away")).expect("git moves away");
        std::fs::create_dir(plain.join("git")).expect("a replacement git");
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("plain/git")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(dest.work_entries(), ["plain"]);
}

/// A tree tampered just before the closing gate refuses, with every proof recorded.
#[test]
fn a_tree_tampered_before_the_closing_gate_refuses() {
    let (dest, route, failure) = refused_at(GitHookPointV1::BeforeFinalGate, "", |paths| {
        std::fs::write(paths.bootstrap.join("objects/info/late"), b"late").expect("a late file");
    });
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::UnexpectedEntry { name } if name == "objects/info/late"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.last_completed, Some(GitPlaneStepV1::Fsck));
    assert_eq!(route.children().len(), 6);
    assert!(
        dest.bootstrap().join("objects/info/late").is_file(),
        "the file is preserved"
    );
}

/// A file `git` expected to have written is missing after `index-pack`.
#[test]
fn an_indexed_store_missing_an_admitted_file_refuses() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (_dest, verified) = phase_v(&capsule.capsule);
    let route = RouteV1::new(&[(
        "index-pack --strict --stdin",
        "\"$REAL\" \"$@\"; rc=$?; rm -f git-bootstrap/objects/pack/*.rev; exit $rc",
    )]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::MissingEntry { name }
                if name.starts_with("objects/pack/pack-") && name.ends_with(".rev")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init", "index-pack"]);
}

/// A bootstrap file on another device refuses.
#[test]
fn a_bootstrap_file_on_another_device_refuses() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let dev = device_of(&dest.path);
    let _device = override_census_dev_for_test("HEAD", dev + 1);
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing { name })
                if name == "git-bootstrap/HEAD"),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"]);
}

/// A child that exits successfully but prints output the bootstrap refuses still has its
/// reservation reconciled: it made nothing, so the entries and the bound are released down to what
/// is present.
#[test]
fn a_successful_exit_with_unusable_output_still_reconciles_the_ledger() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[(
        "index-pack --strict --stdin",
        "cat > /dev/null; printf 'pack\\t%039d\\n' 0",
    )]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(failure.cause, GitBootstrapErrorV1::GitOutput(_)),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.ledger_used, base + tree_bytes(&dest.bootstrap()));
}

/// The census itself refuses a retained directory whose entry was replaced: the swap lands before
/// the next child's census, so only the census's own identity guard can name it (the gate's later
/// refusal says "retained staging directory", and this one must not be that).
#[test]
fn the_census_itself_refuses_a_replaced_retained_directory() {
    let (outside_area, outside) = external_dir_with(&[]);
    let away = outside.join("info-away");
    let (_dest, route, failure) = refused_at(
        GitHookPointV1::BeforeChildCensus,
        "verify-pack",
        move |paths| {
            std::fs::rename(paths.bootstrap.join("objects/info"), &away).expect("info moves away");
            std::fs::create_dir(paths.bootstrap.join("objects/info")).expect("a replacement info");
        },
    );
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("objects/info: the entry is no longer the retained directory")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(
        route.children(),
        ["init", "index-pack"],
        "verify-pack must not start"
    );
    drop(outside_area);
}

/// The gate checks `.restore-work/` and `plain/` itself, not only as a side effect of a staging
/// subdirectory's chain: with no staging subdirectory retained, a swapped `plain/` still refuses.
#[test]
fn the_gate_checks_work_and_plain_without_any_staging_subdirectory() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    verified.destination.staging.borrow_mut().clear();
    let work = dest.work();
    let _hook = install_git_hook_for_test(GitHookPointV1::AdmissionEntry, move |_| {
        std::fs::rename(work.join("plain"), work.join("plain-away")).expect("plain moves away");
        std::fs::create_dir(work.join("plain")).expect("a replacement plain");
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(dest.work_entries(), ["plain", "plain-away"]);
}

// ---------------------------------------------------------------------------------------------
// Review round 1: the closing gate rechecks bytes, failed children keep both failures, and the
// census measures every subtree a child can leave
// ---------------------------------------------------------------------------------------------

/// One byte of `path` changed in place: same inode, same length.
fn change_in_place(path: &Path) {
    use std::os::unix::fs::FileExt as _;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("an admitted file opens for the racer");
    let mut byte = [0_u8; 1];
    std::fs::File::open(path)
        .and_then(|reader| reader.read_exact_at(&mut byte, 10))
        .expect("the byte to change");
    file.write_all_at(&[!byte[0]], 10)
        .expect("one byte changed");
}

fn admitted_path(bootstrap: &Path, extension: &str) -> PathBuf {
    std::fs::read_dir(bootstrap.join("objects/pack"))
        .expect("the pack directory")
        .map(|entry| entry.expect("an entry").path())
        .find(|path| path.extension().is_some_and(|found| found == extension))
        .unwrap_or_else(|| panic!("an admitted .{extension} file"))
}

/// A same-length, same-inode change to an admitted file before the closing gate must not be
/// returned as a verified bootstrap: the closing gate re-measures the bytes of all three files.
#[test]
fn a_same_length_change_to_an_admitted_file_before_the_closing_gate_refuses() {
    for extension in ["pack", "idx", "rev"] {
        let (dest, route, failure) =
            refused_at(GitHookPointV1::BeforeFinalGate, "", move |paths| {
                change_in_place(&admitted_path(&paths.bootstrap, extension));
            });
        assert!(
            matches!(&failure.cause,
                GitBootstrapErrorV1::AdmittedObjectChanged { relative }
                    if relative.ends_with(&format!(".{extension}"))),
            ".{extension}: {}",
            failure_cause(&failure)
        );
        assert_eq!(
            failure.last_completed,
            Some(GitPlaneStepV1::Fsck),
            ".{extension}"
        );
        assert_eq!(
            route.children().len(),
            6,
            ".{extension}: every proof child ran"
        );
        assert!(!dest.path.join("repository").exists());
    }
}

/// The same change made inside the closing gate's own mount census is also caught, because the
/// census runs first and the byte re-measurement after it.
#[test]
fn a_same_length_change_inside_the_closing_census_refuses() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (_dest, verified) = phase_v(&capsule.capsule);
    let census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec()]);
    let _ = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    let total = census.calls();
    drop(census);

    for extension in ["pack", "idx"] {
        let (dest, verified) = phase_v(&capsule.capsule);
        let bootstrap = dest.bootstrap();
        let _census = crate::custody_mounts::seam::install(Box::new(move |call| {
            if call == total {
                change_in_place(&admitted_path(&bootstrap, extension));
            }
            Ok(vec![b"/".to_vec()])
        }));
        let route = RouteV1::recorder();
        let (_area, runner) = route.runner();
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        assert!(
            matches!(&failure.cause,
                GitBootstrapErrorV1::AdmittedObjectChanged { relative }
                    if relative.ends_with(&format!(".{extension}"))),
            ".{extension}: {}",
            failure_cause(&failure)
        );
        assert_eq!(
            failure.last_completed,
            Some(GitPlaneStepV1::Fsck),
            ".{extension}"
        );
    }
}

/// A failed writing child that also overran its reservation, or left bytes where nothing may be,
/// returns BOTH failures, and the ledger keeps the whole reservation.
#[test]
fn a_failed_child_that_overran_keeps_both_failures_and_the_reservation() {
    type Check = fn(&GitBootstrapErrorV1) -> bool;
    let rows: [(&str, &str, Check); 3] = [
        (
            "config grown past the reservation",
            "\"$REAL\" \"$@\"; head -c 8192 /dev/zero >> git-bootstrap/config; exit 3",
            |reconcile| {
                matches!(reconcile, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(detail))
                    if detail.contains("above its"))
            },
        ),
        (
            "a large file in an unexpected nested directory",
            "\"$REAL\" \"$@\"; mkdir git-bootstrap/objects/zz; head -c 8192 /dev/zero > git-bootstrap/objects/zz/big; exit 3",
            |reconcile| {
                matches!(reconcile, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(detail))
                    if detail.contains("above its"))
            },
        ),
        (
            "bytes written into HOME",
            "\"$REAL\" \"$@\"; head -c 100 /dev/zero > home/big; exit 3",
            |reconcile| {
                matches!(reconcile, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(detail))
                    if detail.contains("home and xdg hold 100 bytes"))
            },
        ),
    ];
    for (row, shell, check) in rows {
        let capsule = captured(CustodyGitObjectFormatV1::Sha1);
        let (dest, verified) = phase_v(&capsule.capsule);
        let (_, base) = pin_charge(&verified);
        let route = RouteV1::new(&[("init --bare", shell)]);
        let (_area, runner) = route.runner();
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        match &failure.cause {
            GitBootstrapErrorV1::ChildAndReconcile { primary, reconcile } => {
                assert!(
                    matches!(
                        **primary,
                        GitBootstrapErrorV1::GitChild {
                            command: "init --bare",
                            status: Some(3),
                            ..
                        }
                    ),
                    "{row}: primary {primary:?}"
                );
                assert!(check(reconcile), "{row}: reconcile {reconcile:?}");
            }
            other => panic!("{row}: the reconciliation failure was dropped: {other:?}"),
        }
        assert_eq!(
            failure.ledger_used,
            base + INIT_BARE_LOGICAL_BYTES_V1,
            "{row}"
        );
        assert_eq!(route.children(), ["init"], "{row}");
        assert!(dest.work().join("home").is_dir());
    }
}

/// A route that drifts while a child runs is reported, and the child's effects are still
/// reconciled: an overrun beside the drift is kept, and a clean tree is released.
#[test]
fn binary_drift_still_reconciles_the_child_and_keeps_both_failures() {
    let drift = "chmod u+w \"$0\"; echo '# drift' >> \"$0\"";
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);

    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[(
        "init --bare",
        &format!("\"$REAL\" \"$@\"; head -c 8192 /dev/zero >> git-bootstrap/config; {drift}"),
    )]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    match &failure.cause {
        GitBootstrapErrorV1::ChildAndReconcile { primary, reconcile } => {
            assert!(
                matches!(
                    **primary,
                    GitBootstrapErrorV1::Git(CustodyGitError::BinaryDrift(_))
                ),
                "primary {primary:?}"
            );
            assert!(
                matches!(&**reconcile, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(detail))
                    if detail.contains("above its")),
                "reconcile {reconcile:?}"
            );
        }
        other => panic!("the overrun beside the drift was dropped: {other:?}"),
    }
    assert_eq!(failure.ledger_used, base + INIT_BARE_LOGICAL_BYTES_V1);
    assert_eq!(route.children(), ["init"]);
    drop(dest);

    // Drift alone: the drift is the cause, and the clean tree's reservation is released.
    let (dest, verified) = phase_v(&capsule.capsule);
    let (_, base) = pin_charge(&verified);
    let route = RouteV1::new(&[("init --bare", &format!("\"$REAL\" \"$@\"; {drift}"))]);
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(
            failure.cause,
            GitBootstrapErrorV1::Git(CustodyGitError::BinaryDrift(_))
        ),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(failure.ledger_used, base + tree_bytes(&dest.bootstrap()));
}

/// An unexpected directory swapped between its listing and its measurement is not measured: the
/// no-follow open must be the listed entry, so the moved tree is never mistaken for the new one.
#[test]
fn a_swapped_unexpected_directory_is_not_measured() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let paths = PathsV1::of(&dest);
    let planted = paths.clone();
    let _after = install_git_hook_for_test(GitHookPointV1::AfterChild, move |event| {
        if event.label == "init --bare" {
            std::fs::create_dir(planted.bootstrap.join("objects/zz")).expect("an extra directory");
        }
    });
    let swapped = paths.clone();
    let done = std::cell::Cell::new(false);
    let _open = install_git_hook_for_test(GitHookPointV1::BeforeCensusOpen, move |event| {
        if event.path.ends_with("git-bootstrap/objects/zz") && !done.replace(true) {
            std::fs::rename(
                swapped.bootstrap.join("objects/zz"),
                swapped.bootstrap.join("objects/zz-away"),
            )
            .expect("zz moves away");
            std::fs::create_dir(swapped.bootstrap.join("objects/zz")).expect("a replacement zz");
        }
    });
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    assert!(
        matches!(&failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(detail))
                if detail.contains("changed while it was measured")),
        "{}",
        failure_cause(&failure)
    );
    assert_eq!(route.children(), ["init"]);
    assert!(
        dest.bootstrap().join("objects/zz-away").is_dir(),
        "the moved directory is preserved"
    );
}

// ---------------------------------------------------------------------------------------------
// Corrective round: complete entry/byte accounting and complete proof envelopes
//
// Every control runs a REAL child through a fixture route. The route's log is the child-start
// marker: each row prints `CHILD-START-MARKER` with the children that actually started BEFORE it
// judges anything, so a failing row can be told from a fixture or environment refusal. Rows never
// stop at the first defect: they collect every one.
// ---------------------------------------------------------------------------------------------

/// What one bootstrap attempt ran over, and the figures its controls compare against, learned
/// before the attempt.
struct RanV1 {
    dest: DestV1,
    route: RouteV1,
    /// The ledger after phase V.
    phase_v: u64,
    /// `phase_v` plus HOME, XDG, and the nine `for_init` entries.
    base: u64,
    pack_length: u64,
}

type OutcomeV1 = Result<VerifiedGitBootstrapV1, GitBootstrapFailureV1>;

/// One Captured SHA-1 bootstrap through a tampering route. `setup` may install seams over the
/// destination; its guards live until the attempt ends.
fn run_with(
    rules: &[(&str, &str)],
    timeout: Duration,
    setup: impl FnOnce(&DestV1) -> Vec<GitSeamGuardV1>,
) -> (RanV1, OutcomeV1) {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let (phase_v_used, base) = pin_charge(&verified);
    let pack_length = verified
        .staged
        .iter()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .map(|one| one.length)
        .expect("the staged pack");
    let _guards = setup(&dest);
    let route = RouteV1::new(rules);
    let (_area, runner) = route.runner();
    let outcome = bootstrap_git_objects_v1(
        verified,
        &runner,
        GitObjectFormatV1::Sha1,
        Instant::now() + timeout,
    );
    (
        RanV1 {
            dest,
            route,
            phase_v: phase_v_used,
            base,
            pack_length,
        },
        outcome,
    )
}

fn run(rules: &[(&str, &str)]) -> (RanV1, OutcomeV1) {
    run_with(rules, Duration::from_secs(120), |_| Vec::new())
}

/// Like [`run`] over an honest recording route, with `plant` run once at `point` of the child
/// labelled `label`.
fn run_hooked(
    point: GitHookPointV1,
    label: &'static str,
    plant: impl Fn(&PathsV1) + 'static,
) -> (RanV1, OutcomeV1) {
    run_with(&[], Duration::from_secs(120), move |dest| {
        let paths = PathsV1::of(dest);
        let done = std::cell::Cell::new(false);
        vec![install_git_hook_for_test(point, move |event| {
            if event.label == label && !done.replace(true) {
                plant(&paths);
            }
        })]
    })
}

/// What the one ledger must charge when a failed attempt ends.
#[derive(Clone, Copy, Debug)]
enum ChargeV1 {
    /// Exactly what the preserved tree holds: HOME, XDG, the bootstrap root, every entry beneath it,
    /// and its measured logical bytes, over phase V. The test measures this from the disk.
    Preserved,
    /// The reservation is kept whole: this exact figure.
    Exactly(u64),
}

fn preserved_charge(ran: &RanV1) -> u64 {
    let tree = snapshot(&ran.dest.bootstrap());
    ran.phase_v + (3 + tree.len() as u64) * ENTRY_V1 + tree_bytes(&ran.dest.bootstrap())
}

fn combined_with(
    error: &GitBootstrapErrorV1,
    primary: impl Fn(&GitBootstrapErrorV1) -> bool,
    reconcile: impl Fn(&GitBootstrapErrorV1) -> bool,
) -> bool {
    matches!(error, GitBootstrapErrorV1::ChildAndReconcile { primary: p, reconcile: r }
        if primary(p) && reconcile(r))
}

fn is_budget(error: &GitBootstrapErrorV1, needle: &str) -> bool {
    matches!(error, GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(detail))
        if detail.contains(needle))
}

fn is_git_child(error: &GitBootstrapErrorV1, command: &str, status: i32) -> bool {
    matches!(error, GitBootstrapErrorV1::GitChild { command: c, status: Some(s), .. }
        if *c == command && *s == status)
}

fn is_identity_changed(error: &GitBootstrapErrorV1) -> bool {
    matches!(
        error,
        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::IdentityChanged(_))
    )
}

fn is_stdout_cap(error: &GitBootstrapErrorV1) -> bool {
    matches!(
        error,
        GitBootstrapErrorV1::Git(CustodyGitError::StdoutLimit { .. })
    )
}

fn is_stderr_cap(error: &GitBootstrapErrorV1) -> bool {
    matches!(
        error,
        GitBootstrapErrorV1::Git(CustodyGitError::StderrLimit { .. })
    )
}

fn is_binary_drift(error: &GitBootstrapErrorV1) -> bool {
    matches!(
        error,
        GitBootstrapErrorV1::Git(CustodyGitError::BinaryDrift(_))
    )
}

fn is_timeout(error: &GitBootstrapErrorV1) -> bool {
    matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout))
}

/// A byte effect refused: beyond the byte reservation, or as a read-only child's change.
fn is_bytes_effect(error: &GitBootstrapErrorV1) -> bool {
    is_budget(error, "above its") || is_budget(error, "read-only")
}

/// Any of the shapes a read-only child's effect may be refused with.
fn is_effect_refusal(error: &GitBootstrapErrorV1) -> bool {
    matches!(error, GitBootstrapErrorV1::UnexpectedEntry { .. })
        || is_bytes_effect(error)
        || is_identity_changed(error)
}

/// Judges one attempt that must have been refused, and returns EVERY problem found.
fn judge(
    name: &str,
    ran: &RanV1,
    outcome: &OutcomeV1,
    children: &[&str],
    completed: Option<GitPlaneStepV1>,
    charge: ChargeV1,
    cause: impl Fn(&GitBootstrapErrorV1) -> bool,
) -> Vec<String> {
    let observed = ran.route.children();
    let reached = observed.len() >= children.len() && observed[..children.len()] == *children;
    println!(
        "CHILD-START-MARKER {name}: expected={children:?} observed={observed:?} reached={reached}"
    );
    let mut problems = Vec::new();
    if observed != children {
        // `reached=true`: every expected child started, and more did (the bootstrap went on past
        // the child). `reached=false`: a child that must have started did not: a fixture problem.
        problems.push(format!(
            "{name}: CHILD-LOG reached={reached}: expected exactly {children:?}, observed {observed:?}"
        ));
    }
    match outcome {
        Ok(_) => problems.push(format!("{name}: the bootstrap SUCCEEDED")),
        Err(failure) => {
            if !cause(&failure.cause) {
                problems.push(format!("{name}: cause {}", failure_cause(failure)));
            }
            if failure.last_completed != completed {
                problems.push(format!(
                    "{name}: completed {:?}, expected {completed:?}",
                    failure.last_completed
                ));
            }
            if failure.active_materialization_began {
                problems.push(format!("{name}: active materialization began"));
            }
            let expected = match charge {
                ChargeV1::Preserved => preserved_charge(ran),
                ChargeV1::Exactly(figure) => figure,
            };
            if failure.ledger_used != expected {
                problems.push(format!(
                    "{name}: ledger {} != {expected} ({charge:?})",
                    failure.ledger_used
                ));
            }
        }
    }
    problems
}

fn assert_no_problems(problems: Vec<String>) {
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// A table of one child's output tampering; each row must be refused, with that child's marker.
fn marked_table(
    child: &str,
    last_child: &str,
    last_completed: GitPlaneStepV1,
    needle: &str,
    rows: &[(&str, &str, ErrorCheckV1)],
) {
    let mut problems = Vec::new();
    for (row, shell, check) in rows {
        let (ran, outcome) = run(&[(needle, shell)]);
        let name = format!("{child}/{row}");
        let observed = ran.route.children();
        // The tampered child must have started. A refusal also stops there: it is the last child.
        let started = observed.iter().any(|one| one == last_child);
        let stopped_there = observed.last().map(String::as_str) == Some(last_child);
        println!(
            "CHILD-START-MARKER {name}: child={last_child} observed={observed:?} started={started}"
        );
        match &outcome {
            Ok(_) => problems.push(format!("{name}: SUCCEEDED (started={started})")),
            Err(failure) => {
                let ok = check(&failure.cause)
                    && failure.last_completed == Some(last_completed)
                    && !failure.active_materialization_began;
                if !started || !stopped_there || !ok {
                    problems.push(format!(
                        "{name}: started={started} stopped_there={stopped_there} cause {} completed {:?}",
                        failure_cause(failure),
                        failure.last_completed
                    ));
                }
            }
        }
    }
    assert_no_problems(problems);
}

/// A bootstrap that must SUCCEED through `rules`, with every one of its six children started.
fn succeeds_through(name: &str, rules: &[(&str, &str)]) -> (RanV1, VerifiedGitBootstrapV1) {
    let (ran, outcome) = run(rules);
    let observed = ran.route.children();
    println!("CHILD-START-MARKER {name}: observed={observed:?}");
    assert_eq!(observed.len(), 6, "{name}: every proof child must have run");
    match outcome {
        Ok(verified) => (ran, verified),
        Err(failure) => panic!("{name}: the control must succeed: {failure:?}"),
    }
}

/// The charge a clean bootstrap leaves above phase V, learned from a real run.
fn clean_charge() -> u64 {
    let (ran, verified) = succeeds_through("the clean control", &[]);
    ledger_used(&verified.verified) - ran.phase_v
}

const FLOOD_V1: &str = "head -c 100000 /dev/zero";
const DRIFT_V1: &str = "chmod u+w \"$0\"; echo '# drift' >> \"$0\"";

// --- Entries left behind by a failed writing child, in every domain ---------------------------

/// A failed `init` that leaves an entry ANYWHERE — zero-length files, directories, nested trees, in
/// the bootstrap, HOME, or XDG — returns BOTH failures and keeps the whole reservation charged.
#[test]
fn corrective_a_failed_init_that_leaves_entries_anywhere_keeps_both_failures_and_the_reservation() {
    let bootstrap_entries = |e: &GitBootstrapErrorV1| is_budget(e, "entries above its");
    let outside = |e: &GitBootstrapErrorV1| is_budget(e, "home and xdg hold");
    type Check<'a> = &'a dyn Fn(&GitBootstrapErrorV1) -> bool;
    let rows: [(&str, &str, &str, Check<'_>); 7] = [
        (
            "a zero-length extra in the bootstrap",
            "\"$REAL\" \"$@\"; : > git-bootstrap/extra; exit 3",
            "git-bootstrap/extra",
            &bootstrap_entries,
        ),
        (
            "a zero-length extra in a known directory",
            "\"$REAL\" \"$@\"; : > git-bootstrap/refs/heads/h; exit 3",
            "git-bootstrap/refs/heads/h",
            &bootstrap_entries,
        ),
        (
            "a nested directory and a file in it",
            "\"$REAL\" \"$@\"; mkdir git-bootstrap/objects/zz; : > git-bootstrap/objects/zz/f; exit 3",
            "git-bootstrap/objects/zz/f",
            &bootstrap_entries,
        ),
        (
            "a deep chain of empty directories",
            "\"$REAL\" \"$@\"; mkdir -p git-bootstrap/objects/a/b/c; exit 3",
            "git-bootstrap/objects/a/b/c",
            &bootstrap_entries,
        ),
        (
            "a zero-length extra in HOME",
            "\"$REAL\" \"$@\"; : > home/extra; exit 3",
            "home/extra",
            &outside,
        ),
        (
            "a zero-length extra in XDG",
            "\"$REAL\" \"$@\"; : > xdg/extra; exit 3",
            "xdg/extra",
            &outside,
        ),
        (
            "a nested directory and a file in XDG",
            "\"$REAL\" \"$@\"; mkdir xdg/d; : > xdg/d/f; exit 3",
            "xdg/d/f",
            &outside,
        ),
    ];
    let mut problems = Vec::new();
    for (row, shell, preserved, check) in rows {
        let (ran, outcome) = run(&[("init --bare", shell)]);
        problems.extend(judge(
            row,
            &ran,
            &outcome,
            &["init"],
            Some(GitPlaneStepV1::Admission),
            ChargeV1::Exactly(ran.base + INIT_BARE_LOGICAL_BYTES_V1),
            |cause| {
                combined_with(
                    cause,
                    |primary| is_git_child(primary, "init --bare", 3),
                    check,
                )
            },
        ));
        let on_disk = if let Some(inner) = preserved.strip_prefix("git-bootstrap/") {
            ran.dest.bootstrap().join(inner)
        } else {
            ran.dest.work().join(preserved)
        };
        if std::fs::symlink_metadata(&on_disk).is_err() {
            problems.push(format!("{row}: the partial bytes were not preserved"));
        }
    }
    assert_no_problems(problems);
}

/// A failed `index-pack` leaving zero, one, three, or four entries: the unused entry allowances are
/// released, exactly three stay charged, and a fourth is an overrun that keeps the whole
/// reservation and refuses beside the failure.
#[test]
fn corrective_a_failed_index_pack_releases_unused_entries_and_refuses_an_overrun() {
    let pack = "git-bootstrap/objects/pack";
    let needle = "index-pack --strict --stdin";
    let mut problems = Vec::new();
    for (row, shell) in [
        ("zero entries", "cat > /dev/null; exit 1".to_owned()),
        (
            "one entry",
            format!("cat > /dev/null; : > {pack}/pack-a.pack; exit 1"),
        ),
        (
            "three entries",
            format!(
                "cat > /dev/null; : > {pack}/pack-a.pack; : > {pack}/pack-a.idx; : > {pack}/pack-a.rev; exit 1"
            ),
        ),
    ] {
        let (ran, outcome) = run(&[(needle, &shell)]);
        problems.extend(judge(
            row,
            &ran,
            &outcome,
            &["init", "index-pack"],
            Some(GitPlaneStepV1::BootstrapInit),
            ChargeV1::Preserved,
            |cause| matches!(cause, GitBootstrapErrorV1::StrictPack(_)),
        ));
    }
    let four = format!(
        "cat > /dev/null; : > {pack}/pack-a.pack; : > {pack}/pack-a.idx; : > {pack}/pack-a.rev; : > {pack}/pack-a.keep; exit 1"
    );
    let (ran, outcome) = run(&[(needle, &four)]);
    let bound = index_pack_logical_bound(ran.pack_length, 4, 20).expect("a bound");
    let init_bytes = tree_bytes(&ran.dest.bootstrap());
    problems.extend(judge(
        "four entries",
        &ran,
        &outcome,
        &["init", "index-pack"],
        Some(GitPlaneStepV1::BootstrapInit),
        ChargeV1::Exactly(ran.base + init_bytes + INDEX_PACK_ENTRIES_V1 * ENTRY_V1 + bound),
        |cause| {
            combined_with(
                cause,
                |primary| matches!(primary, GitBootstrapErrorV1::StrictPack(_)),
                |reconcile| is_budget(reconcile, "entries above its"),
            )
        },
    ));
    assert_no_problems(problems);
}

/// The census counts raw entries, never lossily rendered names: two distinct non-UTF-8 names render
/// identically, and must still count twice. (Linux: other filesystems refuse such names.)
#[cfg(target_os = "linux")]
#[test]
fn corrective_entries_are_counted_by_enumeration_not_by_rendered_name() {
    // One entry removed and two entries that render alike added: the true total overruns the nine,
    // and a count keyed by the lossy name would not.
    let (ran, outcome) = run(&[(
        "init --bare",
        "\"$REAL\" \"$@\"; rmdir git-bootstrap/refs/tags; : > \"git-bootstrap/objects/$(printf '\\377')a\"; : > \"git-bootstrap/objects/$(printf '\\376')a\"; exit 3",
    )]);
    assert_no_problems(judge(
        "two entries that render alike",
        &ran,
        &outcome,
        &["init"],
        Some(GitPlaneStepV1::Admission),
        ChargeV1::Exactly(ran.base + INIT_BARE_LOGICAL_BYTES_V1),
        |cause| {
            combined_with(
                cause,
                |primary| is_git_child(primary, "init --bare", 3),
                |reconcile| is_budget(reconcile, "entries above its"),
            )
        },
    ));
}

/// What a child can leave that no census may infer anything from: the reservation stays whole and
/// the census's own refusal sits beside the child's failure.
#[test]
fn corrective_an_incomplete_census_keeps_the_whole_reservation_and_both_failures() {
    let many = "i=0; while [ $i -lt 300 ]; do : > git-bootstrap/objects/pack/n$i; i=$((i+1)); done";
    let rows: [(&str, String, ErrorCheckV1); 4] = [
        (
            "a special file",
            "\"$REAL\" \"$@\"; mkfifo git-bootstrap/f; exit 3".to_owned(),
            |e| matches!(e, GitBootstrapErrorV1::SpecialEntry { .. }),
        ),
        (
            "a symlink",
            "\"$REAL\" \"$@\"; ln -s /nonexistent git-bootstrap/l; exit 3".to_owned(),
            |e| {
                matches!(
                    e,
                    GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { .. })
                )
            },
        ),
        (
            "a tree nested past the depth bound",
            "\"$REAL\" \"$@\"; mkdir -p git-bootstrap/objects/z/d0/d1/d2/d3/d4/d5/d6/d7; exit 3"
                .to_owned(),
            |e| matches!(e, GitBootstrapErrorV1::CensusBound(_)),
        ),
        (
            "more names than the census bound",
            format!("\"$REAL\" \"$@\"; {many}; exit 3"),
            |e| matches!(e, GitBootstrapErrorV1::CensusBound(_)),
        ),
    ];
    let mut problems = Vec::new();
    for (row, shell, reconcile) in rows {
        let (ran, outcome) = run(&[("init --bare", &shell)]);
        problems.extend(judge(
            row,
            &ran,
            &outcome,
            &["init"],
            Some(GitPlaneStepV1::Admission),
            ChargeV1::Exactly(ran.base + INIT_BARE_LOGICAL_BYTES_V1),
            |cause| {
                combined_with(
                    cause,
                    |primary| is_git_child(primary, "init --bare", 3),
                    reconcile,
                )
            },
        ));
    }
    assert_no_problems(problems);
}

// --- Every outcome of a writing child settles its reservation exactly once ---------------------

/// Every post-spawn outcome of `init` and `index-pack` is reconciled once: an unused allowance is
/// released once and never below what the preserved state holds, and an overrun beside the failure
/// is kept whole.
#[test]
fn corrective_every_outcome_of_a_writing_child_reconciles_the_ledger_once() {
    let pack = "git-bootstrap/objects/pack";
    let mut problems = Vec::new();

    // init: a nonzero exit, a stdout cap, and a stderr cap, each over a tree the child never made.
    type Check = fn(&GitBootstrapErrorV1) -> bool;
    let rows: [(&str, String, Check); 4] = [
        (
            "init exits before creating anything",
            "exit 3".to_owned(),
            |e| is_git_child(e, "init --bare", 3),
        ),
        (
            "init exits after making only some directories",
            "mkdir git-bootstrap/objects git-bootstrap/objects/pack; exit 3".to_owned(),
            |e| is_git_child(e, "init --bare", 3),
        ),
        (
            "init prints past the stdout cap, then stops",
            "head -c 40000 /dev/zero".to_owned(),
            is_stdout_cap,
        ),
        (
            "init prints past the stderr cap, then stops",
            format!("{FLOOD_V1} >&2"),
            is_stderr_cap,
        ),
    ];
    for (row, shell, cause) in rows {
        let (ran, outcome) = run(&[("init --bare", &shell)]);
        problems.extend(judge(
            row,
            &ran,
            &outcome,
            &["init"],
            Some(GitPlaneStepV1::Admission),
            ChargeV1::Preserved,
            cause,
        ));
    }

    // init: a whole init, a stray entry, then a cap or a drift: the overrun is kept beside the
    // runner's error, and the reservation stays whole.
    let rows: [(&str, String, Check); 3] = [
        (
            "a stdout cap beside a stray entry",
            "\"$REAL\" \"$@\"; : > git-bootstrap/extra; head -c 40000 /dev/zero".to_owned(),
            is_stdout_cap,
        ),
        (
            "a stderr cap beside a stray entry",
            format!("\"$REAL\" \"$@\"; : > git-bootstrap/extra; {FLOOD_V1} >&2"),
            is_stderr_cap,
        ),
        (
            "binary drift beside a stray entry",
            format!("\"$REAL\" \"$@\"; : > git-bootstrap/extra; {DRIFT_V1}"),
            is_binary_drift,
        ),
    ];
    for (row, shell, runner_error) in rows {
        let (ran, outcome) = run(&[("init --bare", &shell)]);
        problems.extend(judge(
            row,
            &ran,
            &outcome,
            &["init"],
            Some(GitPlaneStepV1::Admission),
            ChargeV1::Exactly(ran.base + INIT_BARE_LOGICAL_BYTES_V1),
            |cause| combined_with(cause, runner_error, |r| is_budget(r, "entries above its")),
        ));
    }

    // index-pack: the same outcomes over three real files and a fourth that nothing reserved.
    let real_then = |effect: &str| format!("\"$REAL\" \"$@\" > /dev/null; {effect}");
    let rows: [(&str, String, Check); 4] = [
        (
            "a stdout cap beside a fourth entry",
            real_then(&format!(": > {pack}/extra; head -c 40000 /dev/zero")),
            is_stdout_cap,
        ),
        (
            "a stderr cap beside a fourth entry",
            real_then(&format!(": > {pack}/extra; {FLOOD_V1} >&2")),
            is_stderr_cap,
        ),
        (
            "binary drift beside a fourth entry",
            real_then(&format!(": > {pack}/extra; {DRIFT_V1}")),
            is_binary_drift,
        ),
        (
            "a status-zero record the parser refuses beside a fourth entry",
            real_then(&format!(": > {pack}/extra; printf 'pack\\t%039d\\n' 0")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
    ];
    for (row, shell, runner_error) in rows {
        let (ran, outcome) = run(&[("index-pack --strict --stdin", &shell)]);
        let bound = index_pack_logical_bound(ran.pack_length, 4, 20).expect("a bound");
        let init_bytes = ["HEAD", "config"]
            .iter()
            .map(|name| {
                std::fs::metadata(ran.dest.bootstrap().join(name))
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
            })
            .sum::<u64>();
        problems.extend(judge(
            row,
            &ran,
            &outcome,
            &["init", "index-pack"],
            Some(GitPlaneStepV1::BootstrapInit),
            ChargeV1::Exactly(ran.base + init_bytes + INDEX_PACK_ENTRIES_V1 * ENTRY_V1 + bound),
            |cause| combined_with(cause, runner_error, |r| is_budget(r, "entries above its")),
        ));
    }

    // index-pack: a status-zero record the parser refuses over three REAL files and nothing else:
    // the reservation settles down to exactly the preserved tree.
    let (ran, outcome) = run(&[(
        "index-pack --strict --stdin",
        &real_then("printf 'pack\\t%039d\\n' 0"),
    )]);
    problems.extend(judge(
        "a refused record over three real files",
        &ran,
        &outcome,
        &["init", "index-pack"],
        Some(GitPlaneStepV1::BootstrapInit),
        ChargeV1::Preserved,
        |cause| matches!(cause, GitBootstrapErrorV1::GitOutput(_)),
    ));
    assert_no_problems(problems);
}

/// Pre-spawn refusals settle too: the unused allowance is released, and an already-created
/// bootstrap root keeps its entry. A post-exit callback refusal settles what the child made.
#[test]
fn corrective_a_pre_spawn_or_post_exit_refusal_settles_the_reservation() {
    let mut problems = Vec::new();
    let swap_plain = |paths: &PathsV1| {
        std::fs::rename(paths.work.join("plain"), paths.work.join("plain-away"))
            .expect("plain moves away");
        std::fs::create_dir(paths.work.join("plain")).expect("a replacement plain");
    };

    // An expired deadline refuses before init starts: HOME, XDG, and the root are all that stay.
    let (ran, outcome) = run_with(&[], Duration::ZERO, |_| Vec::new());
    problems.extend(judge(
        "an expired deadline before init",
        &ran,
        &outcome,
        &[],
        Some(GitPlaneStepV1::Admission),
        ChargeV1::Exactly(ran.phase_v + 3 * ENTRY_V1),
        is_timeout,
    ));

    // The pre-spawn callback refuses each writing child.
    let (ran, outcome) = run_hooked(GitHookPointV1::BeforeChild, "init --bare", swap_plain);
    problems.extend(judge(
        "a pre-spawn callback refusal before init",
        &ran,
        &outcome,
        &[],
        Some(GitPlaneStepV1::Admission),
        ChargeV1::Exactly(ran.phase_v + 3 * ENTRY_V1),
        is_identity_changed,
    ));
    let (ran, outcome) = run_hooked(
        GitHookPointV1::BeforeChild,
        "index-pack --strict --stdin",
        swap_plain,
    );
    problems.extend(judge(
        "a pre-spawn callback refusal before index-pack",
        &ran,
        &outcome,
        &["init"],
        Some(GitPlaneStepV1::BootstrapInit),
        ChargeV1::Preserved,
        is_identity_changed,
    ));

    // The request cannot be built: the staged pack grew after admission, so index-pack never spawns.
    let (ran, outcome) = run_hooked(GitHookPointV1::AfterChild, "init --bare", |paths| {
        use std::io::{Seek as _, SeekFrom, Write as _};
        write_into(&paths.work.join("plain").join(PACK_STAGED_V1), |file| {
            file.seek(SeekFrom::End(0)).expect("seek");
            file.write_all(b"grown").expect("grow");
        });
    });
    problems.extend(judge(
        "a request that cannot be built for index-pack",
        &ran,
        &outcome,
        &["init"],
        Some(GitPlaneStepV1::BootstrapInit),
        ChargeV1::Preserved,
        |cause| {
            matches!(
                cause,
                GitBootstrapErrorV1::Git(CustodyGitError::StdinLimit { .. })
            )
        },
    ));

    // The post-exit callback refuses init, which also left an entry nothing reserved: both are kept.
    let (ran, outcome) = run_hooked(GitHookPointV1::AfterChild, "init --bare", |paths| {
        std::fs::write(paths.bootstrap.join("extra"), b"").expect("a stray entry");
        std::fs::rename(paths.work.join("plain"), paths.work.join("plain-away"))
            .expect("plain moves away");
        std::fs::create_dir(paths.work.join("plain")).expect("a replacement plain");
    });
    problems.extend(judge(
        "a post-exit callback refusal beside an overrun",
        &ran,
        &outcome,
        &["init"],
        Some(GitPlaneStepV1::Admission),
        ChargeV1::Exactly(ran.base + INIT_BARE_LOGICAL_BYTES_V1),
        |cause| {
            combined_with(cause, is_identity_changed, |reconcile| {
                is_budget(reconcile, "entries above its")
            })
        },
    ));

    // The post-exit callback refuses a child that made everything it reserved.
    for (label, children, completed) in [
        ("init --bare", vec!["init"], Some(GitPlaneStepV1::Admission)),
        (
            "index-pack --strict --stdin",
            vec!["init", "index-pack"],
            Some(GitPlaneStepV1::BootstrapInit),
        ),
    ] {
        let (ran, outcome) = run_hooked(GitHookPointV1::AfterChild, label, swap_plain);
        problems.extend(judge(
            &format!("a post-exit callback refusal after {label}"),
            &ran,
            &outcome,
            &children,
            completed,
            ChargeV1::Preserved,
            is_identity_changed,
        ));
    }
    assert_no_problems(problems);
}

/// A child that outlives the one deadline is killed, and what it left is settled: nothing it never
/// made stays charged.
#[test]
fn corrective_a_timed_out_writing_child_settles_its_reservation() {
    let mut problems = Vec::new();
    for (label, shell, children, completed) in [
        (
            "init --bare",
            "sleep 60",
            vec!["init"],
            Some(GitPlaneStepV1::Admission),
        ),
        (
            "index-pack --strict --stdin",
            "cat > /dev/null; sleep 60",
            vec!["init", "index-pack"],
            Some(GitPlaneStepV1::BootstrapInit),
        ),
    ] {
        let (ran, outcome) = run_with(&[(label, shell)], Duration::from_secs(8), |_| Vec::new());
        problems.extend(judge(
            &format!("a timeout in {label}"),
            &ran,
            &outcome,
            &children,
            completed,
            ChargeV1::Preserved,
            is_timeout,
        ));
    }
    assert_no_problems(problems);
}

// --- Create-new reservations: roll back only what was provably never created -------------------

/// A create-new that meets a planted entry created NOTHING, so exactly the entry it reserved is
/// released; for the bootstrap, the whole paired entry and byte reservation.
#[test]
fn corrective_a_refused_create_new_releases_only_what_it_never_created() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let mut problems = Vec::new();
    // The entries already created and charged when the slot is reached.
    for (slot, created_before) in [("home", 0_u64), ("xdg", 1), ("git-bootstrap", 2)] {
        for kind in [
            PlantV1::EmptyDirectory,
            PlantV1::RegularFile,
            PlantV1::Symlink,
        ] {
            let (dest, verified) = phase_v(&capsule.capsule);
            let (_outside_area, outside) = external_dir_with(&["decoy"]);
            plant(&dest.work().join(slot), kind, &outside);
            let used = ledger_used(&verified);
            let route = RouteV1::recorder();
            let (_area, runner) = route.runner();
            let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
            println!(
                "CHILD-START-MARKER create-new {slot} {kind:?}: observed={:?}",
                route.children()
            );
            let expected = used + created_before * ENTRY_V1;
            if failure.ledger_used != expected {
                problems.push(format!(
                    "{slot} {kind:?}: ledger {} != {expected} ({})",
                    failure.ledger_used,
                    failure_cause(&failure)
                ));
            }
            if !matches!(
                &failure.cause,
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::StagingCollision { .. })
            ) {
                problems.push(format!(
                    "{slot} {kind:?}: cause {}",
                    failure_cause(&failure)
                ));
            }
        }
    }
    assert_no_problems(problems);
}

/// A device refusal AFTER a successful create keeps the charge: the directory exists.
#[test]
fn corrective_a_device_refusal_after_creation_keeps_the_charge() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let mut problems = Vec::new();
    for (name, kept) in [
        (".restore-work/home", ENTRY_V1),
        (".restore-work/xdg", 2 * ENTRY_V1),
        (
            ".restore-work/git-bootstrap",
            (2 + INIT_BARE_ENTRIES_V1) * ENTRY_V1 + INIT_BARE_LOGICAL_BYTES_V1,
        ),
    ] {
        let (dest, verified) = phase_v(&capsule.capsule);
        let dev = device_of(&dest.path);
        let _device = override_created_dev_for_test(name, dev + 1);
        let used = ledger_used(&verified);
        let route = RouteV1::recorder();
        let (_area, runner) = route.runner();
        let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
        println!(
            "CHILD-START-MARKER device {name}: observed={:?}",
            route.children()
        );
        if failure.ledger_used != used + kept {
            problems.push(format!(
                "{name}: ledger {} != {} ({})",
                failure.ledger_used,
                used + kept,
                failure_cause(&failure)
            ));
        }
        if !matches!(
            &failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::DeviceCrossing { .. })
        ) {
            problems.push(format!("{name}: cause {}", failure_cause(&failure)));
        }
    }
    assert_no_problems(problems);
}

/// A refused SECOND reservation of a pair releases the first at once: nothing stays charged for a
/// directory or a child that never existed.
#[test]
fn corrective_a_refused_second_reservation_releases_the_first() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let learn = DestV1::new();
    let verified = verify_stage(&capsule.capsule, &learn, CustodyRestoreBudgetV1::ceiling());
    let phase_v_used = ledger_used(&verified);
    let pack_length = verified
        .staged
        .iter()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .map(|one| one.length)
        .expect("the staged pack");
    let _ = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    let init_bytes = ["HEAD", "config"]
        .iter()
        .map(|name| {
            std::fs::metadata(learn.bootstrap().join(name))
                .expect("an init file")
                .len()
        })
        .sum::<u64>();
    let bound = index_pack_logical_bound(pack_length, 4, 20).expect("a bound");
    let base = phase_v_used + (2 + INIT_BARE_ENTRIES_V1) * ENTRY_V1;
    let peak = base + init_bytes + INDEX_PACK_ENTRIES_V1 * ENTRY_V1 + bound;
    let mut problems = Vec::new();

    // init's entries fit but its 4 KiB of bytes do not: the nine entries go back.
    let dest = DestV1::new();
    let verified = verify_stage(
        &capsule.capsule,
        &dest,
        CustodyRestoreBudgetV1::new(base + INIT_BARE_LOGICAL_BYTES_V1 - 1).expect("a budget"),
    );
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    println!(
        "CHILD-START-MARKER second-reservation init: observed={:?}",
        route.children()
    );
    if failure.ledger_used != phase_v_used + 2 * ENTRY_V1
        || !matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(_))
        )
    {
        problems.push(format!(
            "init: ledger {} != {} ({})",
            failure.ledger_used,
            phase_v_used + 2 * ENTRY_V1,
            failure_cause(&failure)
        ));
    }

    // index-pack's three entries fit but its bound does not: the three entries go back, and init's
    // children are all that stay.
    let dest = DestV1::new();
    let verified = verify_stage(
        &capsule.capsule,
        &dest,
        CustodyRestoreBudgetV1::new(peak - 1).expect("a budget"),
    );
    let route = RouteV1::recorder();
    let (_area, runner) = route.runner();
    let failure = refusal(verified, &runner, GitObjectFormatV1::Sha1);
    println!(
        "CHILD-START-MARKER second-reservation index-pack: observed={:?}",
        route.children()
    );
    if route.children() != ["init"] {
        problems.push(format!("index-pack started: {:?}", route.children()));
    }
    let expected = base + init_bytes;
    if failure.ledger_used != expected
        || !matches!(
            failure.cause,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::Budget(_))
        )
    {
        problems.push(format!(
            "index-pack: ledger {} != {expected} ({})",
            failure.ledger_used,
            failure_cause(&failure)
        ));
    }
    assert_no_problems(problems);
}

// --- Nominally read-only children: a measured zero-effect baseline on EVERY outcome ------------

/// `(needle, command label, the children that have started by it, the last completed step before it)`.
type ReadOnlyV1 = (
    &'static str,
    &'static str,
    &'static [&'static str],
    GitPlaneStepV1,
);

const READ_ONLY_V1: [ReadOnlyV1; 4] = [
    (
        "verify-pack -v",
        "verify-pack",
        &["init", "index-pack", "verify-pack"],
        GitPlaneStepV1::BootstrapInit,
    ),
    (
        "cat-file --batch-all-objects",
        "cat-file --batch-all-objects",
        &["init", "index-pack", "verify-pack", "cat-file"],
        GitPlaneStepV1::Indexing,
    ),
    (
        "rev-list --objects",
        "rev-list --missing=print",
        &["init", "index-pack", "verify-pack", "cat-file", "rev-list"],
        GitPlaneStepV1::Inventory,
    ),
    (
        "fsck --strict",
        "fsck --strict",
        &[
            "init",
            "index-pack",
            "verify-pack",
            "cat-file",
            "rev-list",
            "fsck",
        ],
        GitPlaneStepV1::Closure,
    ),
];

const BYTES_EFFECT_V1: &str = "echo x >> git-bootstrap/HEAD";
const TRUNCATE_EFFECT_V1: &str = ": > git-bootstrap/HEAD";
const REPLACE_EFFECT_V1: &str =
    "cp git-bootstrap/config git-bootstrap/config.tmp && mv git-bootstrap/config.tmp git-bootstrap/config";
const ENTRY_EFFECT_V1: &str = ": > git-bootstrap/objects/info/stray";
const HOME_EFFECT_V1: &str = ": > home/stray";
const XDG_EFFECT_V1: &str = ": > xdg/stray";

/// Each of `verify-pack`, `cat-file`, `rev-list`, and `fsck` leaves an entry or bytes beneath a
/// returned result, a runner error, and a nonzero status: every effect is refused, and beside the
/// child's own failure. A child with no effect keeps its single failure.
#[test]
fn corrective_a_read_only_child_that_leaves_an_effect_is_refused_on_every_outcome() {
    let clean = clean_charge();
    let mut problems = Vec::new();
    for (needle, command, started, completed) in READ_ONLY_V1 {
        type Check<'a> = Box<dyn Fn(&GitBootstrapErrorV1) -> bool + 'a>;
        let rows: Vec<(String, String, Check<'_>)> = vec![
            (
                "a returned result that wrote bytes".to_owned(),
                format!("\"$REAL\" \"$@\"; {BYTES_EFFECT_V1}"),
                Box::new(is_bytes_effect),
            ),
            (
                "a returned result that truncated a file".to_owned(),
                format!("\"$REAL\" \"$@\"; {TRUNCATE_EFFECT_V1}"),
                Box::new(is_bytes_effect),
            ),
            (
                "a returned result that replaced a file by one of the same size".to_owned(),
                format!("\"$REAL\" \"$@\"; {REPLACE_EFFECT_V1}"),
                Box::new(is_bytes_effect),
            ),
            (
                "a runner error beside a truncated file".to_owned(),
                format!("\"$REAL\" \"$@\"; {TRUNCATE_EFFECT_V1}; {FLOOD_V1}"),
                Box::new(|e| combined_with(e, is_stdout_cap, is_bytes_effect)),
            ),
            (
                "a runner error beside a replaced file".to_owned(),
                format!("\"$REAL\" \"$@\"; {REPLACE_EFFECT_V1}; {FLOOD_V1}"),
                Box::new(|e| combined_with(e, is_stdout_cap, is_bytes_effect)),
            ),
            (
                "a nonzero status beside a replaced file".to_owned(),
                format!("\"$REAL\" \"$@\"; {REPLACE_EFFECT_V1}; exit 2"),
                Box::new(move |e| {
                    combined_with(e, |p| is_git_child(p, command, 2), is_bytes_effect)
                }),
            ),
            (
                "a returned result that left an entry".to_owned(),
                format!("\"$REAL\" \"$@\"; {ENTRY_EFFECT_V1}"),
                Box::new(is_effect_refusal),
            ),
            (
                "a returned result that left an entry in HOME".to_owned(),
                format!("\"$REAL\" \"$@\"; {HOME_EFFECT_V1}"),
                Box::new(is_effect_refusal),
            ),
            (
                "a runner error beside bytes".to_owned(),
                format!("\"$REAL\" \"$@\"; {BYTES_EFFECT_V1}; {FLOOD_V1}"),
                Box::new(|e| combined_with(e, is_stdout_cap, is_bytes_effect)),
            ),
            (
                "a runner error beside an entry".to_owned(),
                format!("\"$REAL\" \"$@\"; {ENTRY_EFFECT_V1}; {FLOOD_V1}"),
                Box::new(|e| combined_with(e, is_stdout_cap, is_effect_refusal)),
            ),
            (
                "a runner error beside an entry in XDG".to_owned(),
                format!("\"$REAL\" \"$@\"; {XDG_EFFECT_V1}; {FLOOD_V1}"),
                Box::new(|e| combined_with(e, is_stdout_cap, is_effect_refusal)),
            ),
            (
                "a stderr cap beside bytes".to_owned(),
                format!("\"$REAL\" \"$@\"; {BYTES_EFFECT_V1}; {FLOOD_V1} >&2"),
                Box::new(|e| combined_with(e, is_stderr_cap, is_bytes_effect)),
            ),
            (
                "a nonzero status beside bytes".to_owned(),
                format!("\"$REAL\" \"$@\"; {BYTES_EFFECT_V1}; exit 2"),
                Box::new(move |e| {
                    combined_with(e, |p| is_git_child(p, command, 2), is_bytes_effect)
                }),
            ),
            (
                "a nonzero status beside an entry".to_owned(),
                format!("\"$REAL\" \"$@\"; {ENTRY_EFFECT_V1}; exit 2"),
                Box::new(move |e| {
                    combined_with(e, |p| is_git_child(p, command, 2), is_effect_refusal)
                }),
            ),
            (
                "binary drift beside an entry".to_owned(),
                format!("\"$REAL\" \"$@\"; {ENTRY_EFFECT_V1}; {DRIFT_V1}"),
                Box::new(|e| combined_with(e, is_binary_drift, is_effect_refusal)),
            ),
            (
                "control: binary drift and no effect".to_owned(),
                format!("\"$REAL\" \"$@\"; {DRIFT_V1}"),
                Box::new(is_binary_drift),
            ),
            (
                "control: a runner error and no effect".to_owned(),
                format!("\"$REAL\" \"$@\"; {FLOOD_V1}"),
                Box::new(is_stdout_cap),
            ),
            (
                "control: a nonzero status and no effect".to_owned(),
                "\"$REAL\" \"$@\"; exit 2".to_owned(),
                Box::new(move |e| is_git_child(e, command, 2)),
            ),
        ];
        for (row, shell, check) in rows {
            let (ran, outcome) = run(&[(needle, &shell)]);
            problems.extend(judge(
                &format!("{command}: {row}"),
                &ran,
                &outcome,
                started,
                Some(completed),
                ChargeV1::Exactly(ran.phase_v + clean),
                &check,
            ));
        }
    }
    assert_no_problems(problems);
}

/// The same effect planted by a callback of a read-only child that the callback then refuses:
/// before the child spawns (it never starts) and after it exits. Both refusals sit beside the effect.
#[test]
fn corrective_a_refused_read_only_child_still_measures_its_effect() {
    let clean = clean_charge();
    let stray_and_swap = |paths: &PathsV1| {
        std::fs::write(paths.bootstrap.join("objects/info/stray"), b"").expect("a stray entry");
        std::fs::rename(paths.work.join("plain"), paths.work.join("plain-away"))
            .expect("plain moves away");
        std::fs::create_dir(paths.work.join("plain")).expect("a replacement plain");
    };
    let mut problems = Vec::new();
    for (_needle, command, started, completed) in READ_ONLY_V1 {
        for (point, children) in [
            (GitHookPointV1::BeforeChild, &started[..started.len() - 1]),
            (GitHookPointV1::AfterChild, started),
        ] {
            let (ran, outcome) = run_hooked(point, command, stray_and_swap);
            problems.extend(judge(
                &format!("{command} at {point:?}"),
                &ran,
                &outcome,
                children,
                Some(completed),
                ChargeV1::Exactly(ran.phase_v + clean),
                |cause| combined_with(cause, is_identity_changed, is_effect_refusal),
            ));
        }
    }
    assert_no_problems(problems);
}

/// What a read-only child can leave that no census can read: the census's refusal sits beside the
/// child's own failure, and the ledger keeps what it held.
#[test]
fn corrective_a_read_only_child_that_leaves_an_unreadable_effect_is_refused() {
    let clean = clean_charge();
    let mut problems = Vec::new();
    for (needle, command, started, completed) in READ_ONLY_V1 {
        for (row, effect, reconcile) in [
            (
                "a special file",
                "mkfifo git-bootstrap/objects/info/fifo",
                (|e: &GitBootstrapErrorV1| matches!(e, GitBootstrapErrorV1::SpecialEntry { .. }))
                    as fn(&GitBootstrapErrorV1) -> bool,
            ),
            (
                "a symlink",
                "ln -s /nonexistent git-bootstrap/objects/info/link",
                |e| {
                    matches!(
                        e,
                        GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::SymlinkRefused { .. })
                    )
                },
            ),
        ] {
            let shell = format!("\"$REAL\" \"$@\"; {effect}; exit 2");
            let (ran, outcome) = run(&[(needle, &shell)]);
            problems.extend(judge(
                &format!("{command}: {row}"),
                &ran,
                &outcome,
                started,
                Some(completed),
                ChargeV1::Exactly(ran.phase_v + clean),
                |cause| combined_with(cause, |p| is_git_child(p, command, 2), reconcile),
            ));
        }
    }
    assert_no_problems(problems);
}

// --- Exact proof envelopes ---------------------------------------------------------------------

#[test]
fn corrective_verify_pack_requires_one_terminal_complete_utf8_proof_record() {
    marked_table(
        "verify-pack",
        "verify-pack",
        GitPlaneStepV1::BootstrapInit,
        "verify-pack -v",
        &[
            (
                "the final LF removed",
                "out=$(\"$REAL\" \"$@\"); printf '%s' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "invalid UTF-8 before the proof",
                "printf '\\377\\n'; \"$REAL\" \"$@\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "the proof duplicated at the end",
                "out=$(\"$REAL\" \"$@\"); echo \"$out\"; echo \"$out\" | tail -n 1",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a proof-shaped row before the real proof",
                "echo 'other/objects/pack/pack-0000.pack: ok'; \"$REAL\" \"$@\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "the proof not last: a trailing row",
                "\"$REAL\" \"$@\"; echo 'trailing diagnostic'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a blank row before the proof",
                "echo; \"$REAL\" \"$@\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a whitespace-only row before the proof",
                "echo '   '; \"$REAL\" \"$@\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a whitespace-only row after the proof",
                "\"$REAL\" \"$@\"; echo '   '",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a CRLF proof row",
                "out=$(\"$REAL\" \"$@\"); printf '%s\\r\\n' \"$out\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "every row CRLF",
                "\"$REAL\" \"$@\" | while IFS= read -r l; do printf '%s\\r\\n' \"$l\"; done",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a carriage return on the first verbose row only",
                "out=$(\"$REAL\" \"$@\"); first=$(printf '%s\\n' \"$out\" | head -n 1); rest=$(printf '%s\\n' \"$out\" | tail -n +2); printf '%s\\r\\n%s\\n' \"$first\" \"$rest\"",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
        ],
    );
}

/// The valid control: real `verify-pack` output, verbose rows then the proof, is accepted.
#[test]
fn corrective_verify_pack_accepts_the_real_terminal_record() {
    let (_ran, verified) = succeeds_through("verify-pack control", &[]);
    assert_eq!(verified.runs[2].label, "verify-pack");
    assert!(verified.runs[2].stdout.length > 0);
}

#[test]
fn corrective_fsck_requires_byte_empty_stdout_and_one_complete_notice() {
    let notice = "notice: No default references";
    // stdout and the real exit status come from the real fsck; stderr is replaced.
    let replace = |stderr: &str| format!("\"$REAL\" \"$@\" 2>/dev/null; {stderr}");
    let strict_row = "error: object 0123456789abcdef0123456789abcdef01234567: zeroPaddedFilemode: contains zero-padded file modes";
    let rows: Vec<(&str, String, ErrorCheckV1)> = vec![
        (
            "stdout is whitespace",
            "\"$REAL\" \"$@\"; printf '   '".to_owned(),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "stdout is a lone newline",
            "\"$REAL\" \"$@\"; echo".to_owned(),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "the notice has no final LF",
            replace(&format!("printf '{notice}' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "the notice has leading whitespace",
            replace(&format!("printf ' {notice}\\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "the notice has trailing whitespace",
            replace(&format!("printf '{notice} \\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "the notice is CRLF",
            replace(&format!("printf '{notice}\\r\\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "the notice is duplicated",
            replace(&format!("printf '{notice}\\n{notice}\\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "blank rows surround the notice",
            replace(&format!("printf '\\n{notice}\\n\\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "a blank row follows the notice",
            replace(&format!("printf '{notice}\\n\\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "stderr is invalid UTF-8",
            replace(&format!("printf '\\377\\n{notice}\\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "stderr is only whitespace",
            replace("printf '  \\n' >&2"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "invalid UTF-8 ahead of a strict-object rejection refuses the whole stream",
            format!("printf '\\377\\n' >&2; echo '{strict_row}' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "a strict-object rejection and a nonzero status",
            format!("echo '{strict_row}' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        ),
        (
            "a strict-object rejection after the notice, status zero",
            replace(&format!("printf '{notice}\\n{strict_row}\\n' >&2")),
            |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        ),
        (
            "a nonzero status over a clean stream",
            "\"$REAL\" \"$@\"; exit 1".to_owned(),
            |e| {
                matches!(
                    e,
                    GitBootstrapErrorV1::GitChild {
                        command: "fsck --strict",
                        status: Some(1),
                        ..
                    }
                )
            },
        ),
    ];
    let borrowed: Vec<(&str, &str, ErrorCheckV1)> = rows
        .iter()
        .map(|(row, shell, check)| (*row, shell.as_str(), *check))
        .collect();
    marked_table(
        "fsck",
        "fsck",
        GitPlaneStepV1::Closure,
        "fsck --strict",
        &borrowed,
    );
}

/// The valid fsck controls: the real stderr, an empty stderr, and the exact LF-terminated notice.
#[test]
fn corrective_fsck_accepts_empty_stderr_and_the_exact_notice() {
    for (name, shell) in [
        ("the real stderr", "\"$REAL\" \"$@\"".to_owned()),
        ("an empty stderr", "\"$REAL\" \"$@\" 2>/dev/null".to_owned()),
        (
            "the exact notice, LF-terminated",
            "\"$REAL\" \"$@\" 2>/dev/null; printf 'notice: No default references\\n' >&2"
                .to_owned(),
        ),
    ] {
        let (_ran, verified) = succeeds_through(name, &[("fsck --strict", &shell)]);
        assert!(verified.closure.fsck_strict, "{name}");
    }
}

const CRLF_ALL_V1: &str =
    "\"$REAL\" \"$@\" | while IFS= read -r l; do printf '%s\\r\\n' \"$l\"; done";
const CRLF_LAST_V1: &str = "out=$(\"$REAL\" \"$@\"); printf '%s\\r\\n' \"$out\"";

#[test]
fn corrective_inventory_refuses_crlf_and_stray_whitespace_rows() {
    let crlf_all = CRLF_ALL_V1;
    let crlf_last = CRLF_LAST_V1;
    marked_table(
        "cat-file",
        "cat-file",
        GitPlaneStepV1::Indexing,
        "cat-file --batch-all-objects",
        &[
            ("every row is CRLF", crlf_all, |e| {
                matches!(e, GitBootstrapErrorV1::GitOutput(_))
            }),
            ("the last row is CRLF", crlf_last, |e| {
                matches!(e, GitBootstrapErrorV1::GitOutput(_))
            }),
            (
                "a whitespace-only row follows",
                "\"$REAL\" \"$@\"; echo ' '",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a tab separates the fields",
                "\"$REAL\" \"$@\" | awk 'NR==1{sub(/ /,\"\\t\")}1'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a leading space",
                "\"$REAL\" \"$@\" | awk 'NR==1{$0=\" \" $0}1'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
        ],
    );
}

#[test]
fn corrective_closure_refuses_crlf_and_stray_whitespace_rows() {
    let crlf_all = CRLF_ALL_V1;
    let crlf_last = CRLF_LAST_V1;
    marked_table(
        "rev-list",
        "rev-list",
        GitPlaneStepV1::Inventory,
        "rev-list --objects",
        &[
            ("every row is CRLF", crlf_all, |e| {
                matches!(e, GitBootstrapErrorV1::GitOutput(_))
            }),
            ("the last row is CRLF", crlf_last, |e| {
                matches!(e, GitBootstrapErrorV1::GitOutput(_))
            }),
            (
                "a whitespace-only row follows",
                "\"$REAL\" \"$@\"; echo ' '",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
            (
                "a trailing space",
                "\"$REAL\" \"$@\" | awk 'NR==1{$0=$0 \" \"}1'",
                |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
            ),
        ],
    );
}

// --- Success controls: exact totals, positions, and nothing under `repository/` ----------------

/// Empty holds 11 pass-A entries and Captured 14, for either object format; the ledger charges
/// exactly those entries and the logical bytes on disk; every returned descriptor is at offset 0.
#[test]
fn corrective_success_totals_count_every_pass_a_entry_for_every_shape() {
    use std::io::Seek as _;
    let mut problems = Vec::new();
    for (name, capsule_format, populated, caller, expected_entries) in [
        (
            "sha1 captured",
            CustodyGitObjectFormatV1::Sha1,
            true,
            GitObjectFormatV1::Sha1,
            14_u64,
        ),
        (
            "sha256 captured",
            CustodyGitObjectFormatV1::Sha256,
            true,
            GitObjectFormatV1::Sha256,
            14,
        ),
        (
            "empty over sha1",
            CustodyGitObjectFormatV1::Sha1,
            false,
            GitObjectFormatV1::Sha1,
            11,
        ),
        (
            "empty over sha256",
            CustodyGitObjectFormatV1::Sha1,
            false,
            GitObjectFormatV1::Sha256,
            11,
        ),
    ] {
        let capsule = CustomCapsuleV1::export(capsule_format, populated);
        let (dest, verified) = phase_v(&capsule.capsule);
        let used_before = ledger_used(&verified);
        let route = RouteV1::recorder();
        let (_area, runner) = route.runner();
        let bootstrap = bootstrap(verified, &runner, caller)
            .unwrap_or_else(|failure| panic!("{name}: must succeed: {failure:?}"));
        println!(
            "CHILD-START-MARKER success {name}: observed={:?}",
            route.children()
        );
        let on_disk = 3
            + snapshot(&dest.bootstrap()).len() as u64
            + snapshot(&dest.work().join("home")).len() as u64
            + snapshot(&dest.work().join("xdg")).len() as u64;
        if on_disk != expected_entries {
            problems.push(format!(
                "{name}: {on_disk} entries on disk, expected {expected_entries}"
            ));
        }
        let charged = ledger_used(&bootstrap.verified) - used_before;
        let expected = expected_entries * ENTRY_V1 + tree_bytes(&dest.bootstrap());
        if charged != expected {
            problems.push(format!("{name}: charged {charged}, expected {expected}"));
        }
        if dest.entries() != [".restore-work"] || dest.path.join("repository").exists() {
            problems.push(format!(
                "{name}: something exists outside the work directory"
            ));
        }
        for one in &bootstrap.verified.staged {
            if (&one.file).stream_position().expect("a position") != 0 {
                problems.push(format!("{name}: a staged descriptor is not at offset 0"));
            }
        }
        if let BootstrapObjectsV1::Captured { files, .. } = &bootstrap.objects {
            for file in files {
                if (&file.file).stream_position().expect("a position") != 0 {
                    problems.push(format!("{name}: {} is not at offset 0", file.relative));
                }
            }
        }
    }
    assert_no_problems(problems);
}

/// `index-pack` stderr is failure typing only, but on a nonzero status the WHOLE bounded diagnostic
/// is scanned for a strict-object id, not just its first lines; any classification still refuses.
#[test]
fn corrective_index_pack_scans_its_whole_diagnostic_for_a_strict_object_id() {
    let flood =
        "i=0; while [ $i -lt 100 ]; do echo \"hint: diagnostic line $i\" >&2; i=$((i+1)); done";
    let strict = "echo 'error: object 0123456789abcdef0123456789abcdef01234567: zeroPaddedFilemode: contains zero-padded file modes' >&2";
    let late = format!("cat > /dev/null; {flood}; {strict}; exit 128");
    let early = format!("cat > /dev/null; {strict}; {flood}; exit 128");
    let none = format!("cat > /dev/null; {flood}; exit 128");
    marked_table(
        "index-pack",
        "index-pack",
        GitPlaneStepV1::BootstrapInit,
        "index-pack --strict --stdin",
        &[
            (
                "a strict message after a hundred diagnostic lines",
                &late,
                |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
            ),
            (
                "a strict message before a hundred diagnostic lines",
                &early,
                |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
            ),
            (
                "a hundred diagnostic lines and no strict message",
                &none,
                |e| matches!(e, GitBootstrapErrorV1::StrictPack(_)),
            ),
        ],
    );
}

/// The release criterion is a COMPLETE census that fits, not names the census understands: a child
/// that exits nonzero after swapping a file for a differently named one leaves a tree of the same
/// entry count and fewer bytes, so the unused bytes are released although a name is unexpected.
#[test]
fn corrective_a_complete_census_releases_even_when_a_name_is_unexpected() {
    let (ran, outcome) = run(&[(
        "init --bare",
        "\"$REAL\" \"$@\"; rm git-bootstrap/config; : > git-bootstrap/other; exit 3",
    )]);
    assert_no_problems(judge(
        "a renamed file within the reservation",
        &ran,
        &outcome,
        &["init"],
        Some(GitPlaneStepV1::Admission),
        ChargeV1::Preserved,
        |cause| is_git_child(cause, "init --bare", 3),
    ));
}

/// Structural: a create-new rollback is conditioned on `TargetExists`, the one error whose
/// producing `mkdirat` provably created nothing. Every other error may follow a successful `mkdirat`
/// (the post-create identity, ownership, and emptiness guards), so it keeps its charge. The
/// post-create hooks that could drive such an error are private to `fs_custody.rs`, so this is a
/// source assertion, not a behavioral one.
#[test]
fn corrective_structural_create_new_rolls_back_only_on_target_exists() {
    fn squeezed(source: &str) -> String {
        source.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    assert!(
        squeezed(include_str!("custody_restore_git.rs")).contains(
            "if matches!(error, FsCustodyError::TargetExists(_)) { reserved.roll_back(&dest.ledger); }"
        ),
        "the bootstrap pair's rollback must be the TargetExists branch"
    );
    assert!(
        squeezed(include_str!("custody_restore.rs")).contains(
            "if matches!(error, FsCustodyError::TargetExists(_)) { ledger.borrow_mut().release(ENTRY_ALLOWANCE_BYTES_V1); }"
        ),
        "the staging entry's rollback must be the TargetExists branch"
    );
}

/// A census that refuses never infers absence: index-pack has already reserved its three entries and
/// its bound when its own pre-child census meets a stray entry, and the whole reservation stays
/// charged beside the census's refusal. (The child never started.)
#[test]
fn corrective_a_refused_pre_child_census_keeps_the_pending_reservation() {
    let (ran, outcome) = run_hooked(
        GitHookPointV1::BeforeChildCensus,
        "index-pack --strict --stdin",
        |paths| {
            std::fs::write(paths.bootstrap.join("objects/pack/stray"), b"").expect("a stray entry");
        },
    );
    let bound = index_pack_logical_bound(ran.pack_length, 4, 20).expect("a bound");
    let init_bytes = ["HEAD", "config"]
        .iter()
        .map(|name| {
            std::fs::metadata(ran.dest.bootstrap().join(name))
                .map(|metadata| metadata.len())
                .unwrap_or(0)
        })
        .sum::<u64>();
    assert_no_problems(judge(
        "a refused pre-child census",
        &ran,
        &outcome,
        &["init"],
        Some(GitPlaneStepV1::BootstrapInit),
        ChargeV1::Exactly(ran.base + init_bytes + INDEX_PACK_ENTRIES_V1 * ENTRY_V1 + bound),
        |cause| matches!(cause, GitBootstrapErrorV1::UnexpectedEntry { .. }),
    ));
}

/// A read-only child that outlives the one deadline is killed, and its effect is still measured:
/// the timeout sits beside the refusal. With NO effect the settlement's digest pass keeps the same
/// spent deadline, so the primary timeout stands beside the reconciliation's own timeout (round 5
/// corrected this row: it used to expect the primary timeout alone, which let the post-child digest
/// run past the deadline).
#[test]
fn corrective_a_timed_out_read_only_child_is_refused_beside_its_effect() {
    let clean = clean_charge();
    let mut problems = Vec::new();
    for (needle, command, started, completed) in READ_ONLY_V1 {
        if !["verify-pack", "fsck --strict"].contains(&command) {
            continue;
        }
        for (row, effect) in [
            ("a timeout beside an entry", ENTRY_EFFECT_V1),
            ("a timeout and no effect", ":"),
        ] {
            let shell = format!("\"$REAL\" \"$@\"; {effect}; sleep 60");
            let (ran, outcome) =
                run_with(&[(needle, &shell)], Duration::from_secs(8), |_| Vec::new());
            problems.extend(judge(
                &format!("{command}: {row}"),
                &ran,
                &outcome,
                started,
                Some(completed),
                ChargeV1::Exactly(ran.phase_v + clean),
                |cause| {
                    if effect == ":" {
                        combined_with(cause, is_timeout, is_timeout)
                    } else {
                        combined_with(cause, is_timeout, is_effect_refusal)
                    }
                },
            ));
        }
    }
    assert_no_problems(problems);
}

// ---------------------------------------------------------------------------------------------
// Round 4: a read-only child's baseline keeps EVERY regular file's complete non-atime facts and
// its content digest
//
// A same-inode, same-length write leaves the old triple (length, device, inode) untouched. Every row
// runs a REAL child through the fixture route and prints its CHILD-START-MARKER. The effect is made
// by the runner's post-exit callback of the child under test (`write_all_at`, `set_permissions`,
// `set_modified`) AFTER that child exited, so the settlement of that very child is what must see it.
// These rows are runtime-causal: each ends in the bootstrap's actual outcome. Source-text checks
// are never offered for them.
// ---------------------------------------------------------------------------------------------

const ROUND4_FILES_V1: [&str; 2] = ["HEAD", "config"];

/// Timestamps advance in ticks. Let one pass, so a change made now cannot share a tick with the
/// file's last change on a coarse clock.
fn let_the_clock_tick() {
    std::thread::sleep(Duration::from_millis(25));
}

/// One byte overwritten with `write_all_at`: the SAME inode and the SAME length, proven here.
fn rewrite_one_byte(path: &Path) {
    use std::os::unix::fs::MetadataExt as _;
    let_the_clock_tick();
    let before = std::fs::metadata(path).expect("metadata before");
    change_in_place(path);
    let after = std::fs::metadata(path).expect("metadata after");
    assert_eq!(
        (before.ino(), before.len()),
        (after.ino(), after.len()),
        "{}: the effect must keep the inode and the length",
        path.display()
    );
}

/// Content untouched; a permission bit toggled (mode and ctime move).
fn toggle_a_mode_bit(path: &Path) {
    let_the_clock_tick();
    let mode = std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode();
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode((mode ^ 0o004) & 0o7777),
    )
    .expect("a mode change");
}

/// Content untouched; mtime moved (and with it ctime).
fn move_mtime(path: &Path) {
    let_the_clock_tick();
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("the file opens")
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(86_400))
        .expect("an mtime change");
}

/// Content, size, mode, and mtime untouched; only ctime moves: the SAME mode set again.
fn reset_the_same_mode(path: &Path) {
    let_the_clock_tick();
    let permissions = std::fs::metadata(path).expect("metadata").permissions();
    std::fs::set_permissions(path, permissions).expect("the same mode, set again");
}

/// Like [`run_with`] over `rules`, with `effect` run on `file` once, in the post-exit callback of
/// the child labelled `command`.
fn run_with_effect(
    rules: &[(&str, &str)],
    command: &'static str,
    file: &'static str,
    effect: fn(&Path),
) -> (RanV1, OutcomeV1) {
    run_with(rules, Duration::from_secs(120), move |dest| {
        let paths = PathsV1::of(dest);
        let done = std::cell::Cell::new(false);
        vec![install_git_hook_for_test(
            GitHookPointV1::AfterChild,
            move |event| {
                if event.label == command && !done.replace(true) {
                    effect(&paths.bootstrap.join(file));
                }
            },
        )]
    })
}

/// A same-inode, same-length write to `HEAD` or `config` after each read-only child, on a returned
/// result, a runner error, and a nonzero status: refused every time, beside the child's own failure.
/// The clean control is [`clean_charge`]'s real, unwritten run of all six children.
#[test]
fn round4_a_same_inode_same_length_write_after_a_read_only_child_is_refused_on_every_outcome() {
    let clean = clean_charge();
    let flood = format!("\"$REAL\" \"$@\"; {FLOOD_V1}");
    let nonzero = "\"$REAL\" \"$@\"; exit 2".to_owned();
    let mut problems = Vec::new();
    for (needle, command, started, completed) in READ_ONLY_V1 {
        for file in ROUND4_FILES_V1 {
            type Check<'a> = Box<dyn Fn(&GitBootstrapErrorV1) -> bool + 'a>;
            type Rules<'a> = Vec<(&'a str, &'a str)>;
            let rows: Vec<(&str, Rules<'_>, Check<'_>)> = vec![
                ("a returned result", vec![], Box::new(is_bytes_effect)),
                (
                    "a runner error",
                    vec![(needle, flood.as_str())],
                    Box::new(|e| combined_with(e, is_stdout_cap, is_bytes_effect)),
                ),
                (
                    "a nonzero status",
                    vec![(needle, nonzero.as_str())],
                    Box::new(move |e| {
                        combined_with(e, |p| is_git_child(p, command, 2), is_bytes_effect)
                    }),
                ),
            ];
            for (row, rules, check) in rows {
                let (ran, outcome) = run_with_effect(&rules, command, file, rewrite_one_byte);
                problems.extend(judge(
                    &format!("{command}: {file} rewritten in place beside {row}"),
                    &ran,
                    &outcome,
                    started,
                    Some(completed),
                    ChargeV1::Exactly(ran.phase_v + clean),
                    &check,
                ));
            }
        }
    }
    assert_no_problems(problems);
}

/// A change to a baseline file's metadata ALONE (content, length, and inode intact) is an effect:
/// the digest cannot see it, so the metadata comparison must.
#[test]
fn round4_a_metadata_only_change_to_a_baseline_file_is_refused() {
    let clean = clean_charge();
    type Effect = (&'static str, fn(&Path));
    let effects: [Effect; 3] = [
        ("its mode bits", toggle_a_mode_bit),
        ("its mtime", move_mtime),
        ("its ctime alone", reset_the_same_mode),
    ];
    let flood = format!("\"$REAL\" \"$@\"; {FLOOD_V1}");
    let mut problems = Vec::new();
    for (_needle, command, started, completed) in READ_ONLY_V1 {
        for file in ROUND4_FILES_V1 {
            for (what, effect) in effects {
                let (ran, outcome) = run_with_effect(&[], command, file, effect);
                problems.extend(judge(
                    &format!("{command}: {file} {what} changed beside a returned result"),
                    &ran,
                    &outcome,
                    started,
                    Some(completed),
                    ChargeV1::Exactly(ran.phase_v + clean),
                    is_bytes_effect,
                ));
            }
        }
    }
    // The runner-error shape, once, for the final child.
    let (needle, command, started, completed) = READ_ONLY_V1[3];
    for file in ROUND4_FILES_V1 {
        for (what, effect) in effects {
            let (ran, outcome) =
                run_with_effect(&[(needle, flood.as_str())], command, file, effect);
            problems.extend(judge(
                &format!("{command}: {file} {what} changed beside a runner error"),
                &ran,
                &outcome,
                started,
                Some(completed),
                ChargeV1::Exactly(ran.phase_v + clean),
                |e| combined_with(e, is_stdout_cap, is_bytes_effect),
            ));
        }
    }
    assert_no_problems(problems);
}

/// The negative control: a child that only READS its baseline files changes their atime and nothing
/// else, and that is allowed. The atimes are set to the epoch first, so a read has something to move
/// (a `noatime` mount moves nothing, and the control then holds trivially: the marker says which).
#[test]
fn round4_an_atime_only_read_of_a_baseline_file_is_allowed() {
    use std::os::unix::fs::MetadataExt as _;
    let reads = "\"$REAL\" \"$@\" && cat git-bootstrap/HEAD git-bootstrap/config > /dev/null";
    let (ran, outcome) = run_with(
        &[("fsck --strict", reads)],
        Duration::from_secs(120),
        |dest| {
            let paths = PathsV1::of(dest);
            vec![install_git_hook_for_test(
                GitHookPointV1::BeforeChildCensus,
                move |event| {
                    if event.label == "fsck --strict" {
                        for name in ROUND4_FILES_V1 {
                            std::fs::File::open(paths.bootstrap.join(name))
                                .expect("the file opens")
                                .set_times(
                                    std::fs::FileTimes::new().set_accessed(std::time::UNIX_EPOCH),
                                )
                                .expect("an old atime");
                        }
                    }
                },
            )]
        },
    );
    let observed = ran.route.children();
    println!("CHILD-START-MARKER the atime-only control: observed={observed:?}");
    assert_eq!(observed.len(), 6, "every proof child must have run");
    let moved = ROUND4_FILES_V1.map(|name| {
        std::fs::metadata(ran.dest.bootstrap().join(name))
            .expect("metadata")
            .atime()
            != 0
    });
    println!("ATIME-EVIDENCE HEAD and config atime moved off the epoch: {moved:?}");
    match outcome {
        Ok(_) => {}
        Err(failure) => panic!("a read that moved only atime was refused: {failure:?}"),
    }
}

/// The baseline's digest pass honours the ONE absolute deadline: once it has passed, no read-only
/// child starts, and the refusal is the deadline's. The runner is never entered (its pre-spawn
/// callback never runs), so the refusal comes from the baseline itself.
#[test]
fn round4_the_baseline_digest_honours_the_one_deadline_before_a_read_only_child_starts() {
    let clean = clean_charge();
    let entered = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = entered.clone();
    let (ran, outcome) = run_with(&[], Duration::from_secs(8), move |_dest| {
        let expire = install_git_hook_for_test(GitHookPointV1::BeforeChildCensus, |event| {
            if event.label == "verify-pack" {
                std::thread::sleep(Duration::from_secs(9));
            }
        });
        let enter = install_git_hook_for_test(GitHookPointV1::BeforeChild, move |event| {
            if event.label == "verify-pack" {
                seen.set(true);
            }
        });
        vec![expire, enter]
    });
    println!(
        "CHILD-START-MARKER the expired baseline digest: runner entered={}",
        entered.get()
    );
    let mut problems = judge(
        "the expired baseline digest",
        &ran,
        &outcome,
        &["init", "index-pack"],
        Some(GitPlaneStepV1::BootstrapInit),
        ChargeV1::Exactly(ran.phase_v + clean),
        is_timeout,
    );
    if entered.get() {
        problems.push("the runner was entered after the deadline had passed".to_owned());
    }
    assert_no_problems(problems);
}

// --- Isolating each guard: a clock that did not tick -------------------------------------------
//
// On a filesystem with a fine clock a write moves mtime and ctime, so the metadata comparison alone
// refuses it and the digest is never what decides. These rows stall the census's clocks (a
// coarse-timestamp filesystem: a write within one tick leaves every timestamp as it was), so that
// exactly ONE guard is left to decide. They need the stall seam, which does not exist on the
// pre-round-4 source: their causality is shown by a mutation that removes that one guard.

/// Like [`run_with_effect`], with `stalled` clocks in the census for the whole attempt.
fn run_with_stalled_effect(
    rules: &[(&str, &str)],
    command: &'static str,
    file: &'static str,
    effect: fn(&Path),
    stalled: StalledClocksV1,
) -> (RanV1, OutcomeV1) {
    run_with(rules, Duration::from_secs(120), move |dest| {
        let paths = PathsV1::of(dest);
        let done = std::cell::Cell::new(false);
        vec![
            stall_census_clocks_for_test(stalled),
            install_git_hook_for_test(GitHookPointV1::AfterChild, move |event| {
                if event.label == command && !done.replace(true) {
                    effect(&paths.bootstrap.join(file));
                }
            }),
        ]
    })
}

const BOTH_CLOCKS_V1: StalledClocksV1 = StalledClocksV1 {
    mtime: true,
    ctime: true,
};

/// With both clocks stalled, a same-length rewrite in place leaves length, device, inode, mode, and
/// both timestamps as they were: ONLY the digest can refuse it, on a returned result and on a runner
/// error alike.
#[test]
fn round4_the_digest_alone_refuses_a_rewrite_whose_clocks_did_not_advance() {
    let clean = clean_charge();
    let flood = format!("\"$REAL\" \"$@\"; {FLOOD_V1}");
    let mut problems = Vec::new();
    for (needle, command, started, completed) in READ_ONLY_V1 {
        for file in ROUND4_FILES_V1 {
            let (ran, outcome) =
                run_with_stalled_effect(&[], command, file, rewrite_one_byte, BOTH_CLOCKS_V1);
            problems.extend(judge(
                &format!("{command}: {file} rewritten, clocks stalled, a returned result"),
                &ran,
                &outcome,
                started,
                Some(completed),
                ChargeV1::Exactly(ran.phase_v + clean),
                is_bytes_effect,
            ));
            let (ran, outcome) = run_with_stalled_effect(
                &[(needle, flood.as_str())],
                command,
                file,
                rewrite_one_byte,
                BOTH_CLOCKS_V1,
            );
            problems.extend(judge(
                &format!("{command}: {file} rewritten, clocks stalled, a runner error"),
                &ran,
                &outcome,
                started,
                Some(completed),
                ChargeV1::Exactly(ran.phase_v + clean),
                |e| combined_with(e, is_stdout_cap, is_bytes_effect),
            ));
        }
    }
    assert_no_problems(problems);
}

/// With both clocks stalled, a mode change (content, length, inode intact) is refused by the mode
/// comparison alone; with only ctime stalled, an mtime change is refused by the mtime comparison
/// alone. The remaining field, ctime, is isolated by the unstalled row of
/// [`round4_a_metadata_only_change_to_a_baseline_file_is_refused`] ("its ctime alone").
#[test]
fn round4_the_mode_and_mtime_comparisons_each_refuse_alone() {
    let clean = clean_charge();
    type Row = (&'static str, fn(&Path), StalledClocksV1);
    let rows: [Row; 2] = [
        ("its mode alone", toggle_a_mode_bit, BOTH_CLOCKS_V1),
        (
            "its mtime alone",
            move_mtime,
            StalledClocksV1 {
                mtime: false,
                ctime: true,
            },
        ),
    ];
    let mut problems = Vec::new();
    for (_needle, command, started, completed) in [READ_ONLY_V1[0], READ_ONLY_V1[3]] {
        for file in ROUND4_FILES_V1 {
            for (what, effect, stalled) in rows {
                let (ran, outcome) = run_with_stalled_effect(&[], command, file, effect, stalled);
                problems.extend(judge(
                    &format!("{command}: {file} {what} changed beside a returned result"),
                    &ran,
                    &outcome,
                    started,
                    Some(completed),
                    ChargeV1::Exactly(ran.phase_v + clean),
                    is_bytes_effect,
                ));
            }
        }
    }
    assert_no_problems(problems);
}

/// The negative controls for the isolation rows: stalled clocks alone refuse nothing, and a ctime
/// that only "moves" while stalled (the same mode set again) is invisible to every remaining guard,
/// because nothing else changed.
#[test]
fn round4_stalled_clocks_refuse_nothing_by_themselves() {
    type Effect = fn(&Path);
    let cases: [(&str, Option<Effect>); 2] = [
        ("stalled clocks and no effect", None),
        (
            "stalled clocks and the same mode set again",
            Some(reset_the_same_mode),
        ),
    ];
    for (name, effect) in cases {
        let (ran, outcome) = match effect {
            None => run_with(&[], Duration::from_secs(120), |_| {
                vec![stall_census_clocks_for_test(BOTH_CLOCKS_V1)]
            }),
            Some(effect) => {
                run_with_stalled_effect(&[], "fsck --strict", "HEAD", effect, BOTH_CLOCKS_V1)
            }
        };
        let observed = ran.route.children();
        println!("CHILD-START-MARKER {name}: observed={observed:?}");
        assert_eq!(observed.len(), 6, "{name}: every proof child must have run");
        if let Err(failure) = outcome {
            panic!("{name}: the control must succeed: {failure:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Round 5: every digest descriptor is bound to its census fact; every digest keeps the ONE absolute
// deadline; a strict fsck row cannot mask the rest of its stream
//
// Every row runs REAL children through the fixture route and prints its CHILD-START-MARKER. The
// races are made by test hooks that fire at points the pre-round-5 source already has (a census
// listing, a child's callbacks, the closing gate), with the census's clocks stalled so that the
// metadata comparison cannot see a same-length rewrite: the digest descriptor's own identity is what
// is left to decide.
// ---------------------------------------------------------------------------------------------

/// Which of a read-only child's two digest passes a race targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DigestPassV1 {
    /// Taken right after the baseline census, before the child starts.
    Baseline,
    /// Taken right after the settlement census, after the child exited.
    PostChild,
}

/// How a baseline file is touched between its census and the digest's open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubstitutionV1 {
    /// A NEW inode takes the name: the measured inode is parked beyond the destination.
    Alternate,
    /// The measured inode leaves its name and returns to it at once: the same inode, no substitution.
    SameInodeBack,
}

/// What one race did, shared between the hooks of one attempt.
#[derive(Default)]
struct RaceV1 {
    armed: std::cell::Cell<bool>,
    swapped: std::cell::Cell<bool>,
    restored: std::cell::Cell<bool>,
    rewritten: std::cell::Cell<bool>,
    /// The bytes the alternate inode holds.
    alternate: std::cell::RefCell<Vec<u8>>,
    measured_inode: std::cell::Cell<u64>,
    alternate_inode: std::cell::Cell<u64>,
}

/// The measured inode of `target` parked at `parked`, and a NEW regular file with `bytes` and the
/// measured file's mode at `target`: the same name, another inode, and (with the clocks stalled) a
/// census fact that differs from the measured one in its inode alone.
fn swap_in_alternate(target: &Path, parked: &Path, bytes: &[u8], race: &RaceV1) {
    use std::os::unix::fs::MetadataExt as _;
    let measured = std::fs::metadata(target).expect("the measured file");
    race.measured_inode.set(measured.ino());
    std::fs::rename(target, parked).expect("the measured inode is parked");
    std::fs::write(target, bytes).expect("the alternate inode");
    std::fs::set_permissions(
        target,
        std::fs::Permissions::from_mode(measured.permissions().mode() & 0o7777),
    )
    .expect("the measured mode");
    race.alternate_inode
        .set(std::fs::metadata(target).expect("the alternate").ino());
}

/// One Captured SHA-1 bootstrap whose baseline file `file` is touched between a census and the
/// digest's open (`how`), for the digest `pass` of the child labelled `command`.
///
/// `Alternate` after the child (`PostChild`): the child's post-exit callback rewrites the measured
/// inode in place (same inode, same length, stalled clocks), the settlement census then measures it,
/// and the original bytes in another inode take its name before the digest opens it; the rewritten
/// inode returns to its name once the closing gate is about to run. Before the child (`Baseline`):
/// bytes that differ in one place sit in the other inode when the baseline digest opens it, the
/// measured inode returns before the child starts, and the child's post-exit callback then writes
/// those very bytes into it in place.
fn run_with_substitution(
    command: &'static str,
    file: &'static str,
    pass: DigestPassV1,
    how: SubstitutionV1,
) -> (RanV1, OutcomeV1, std::rc::Rc<RaceV1>) {
    let race = std::rc::Rc::new(RaceV1::default());
    let shared = race.clone();
    let (ran, outcome) = run_with(&[], Duration::from_secs(120), move |dest| {
        let paths = PathsV1::of(dest);
        let parking = dest
            .path
            .parent()
            .expect("the destination's area")
            .join("parked");
        std::fs::create_dir(&parking).expect("a parking directory beside the destination");
        let target = paths.bootstrap.join(file);
        let parked = parking.join(file);
        let mut guards = vec![stall_census_clocks_for_test(BOTH_CLOCKS_V1)];

        // The substitution: after the root's regular files were measured, before any digest opens them.
        {
            let (race, target, parked) = (shared.clone(), target.clone(), parked.clone());
            guards.push(install_git_hook_for_test(
                GitHookPointV1::CensusListed,
                move |event| {
                    if race.armed.get()
                        && !race.swapped.get()
                        && event.path.ends_with("git-bootstrap/objects/pack")
                    {
                        match how {
                            SubstitutionV1::Alternate => {
                                swap_in_alternate(
                                    &target,
                                    &parked,
                                    &race.alternate.borrow(),
                                    &race,
                                );
                            }
                            SubstitutionV1::SameInodeBack => {
                                std::fs::rename(&target, &parked).expect("the inode is parked");
                                std::fs::rename(&parked, &target).expect("the inode returns");
                            }
                        }
                        race.swapped.set(true);
                    }
                },
            ));
        }
        // Each hook owns its own clones; the scopes keep them from shadowing one another.
        match pass {
            DigestPassV1::PostChild => {
                {
                    let (race, target) = (shared.clone(), target.clone());
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::AfterChild,
                        move |event| {
                            // Arming happens for every substitution; only an alternate inode comes
                            // with a rewrite of the measured one.
                            if event.label == command
                                && !race.armed.replace(true)
                                && how == SubstitutionV1::Alternate
                            {
                                *race.alternate.borrow_mut() =
                                    std::fs::read(&target).expect("the original bytes");
                                rewrite_one_byte(&target);
                            }
                        },
                    ));
                }
                {
                    let (race, target, parked) = (shared.clone(), target.clone(), parked.clone());
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::BeforeFinalGate,
                        move |_| {
                            if race.swapped.get()
                                && how == SubstitutionV1::Alternate
                                && !race.restored.replace(true)
                            {
                                std::fs::rename(&parked, &target)
                                    .expect("the rewritten inode returns to its name");
                            }
                        },
                    ));
                }
            }
            DigestPassV1::Baseline => {
                {
                    let (race, target) = (shared.clone(), target.clone());
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::BeforeChildCensus,
                        move |event| {
                            if event.label == command && !race.armed.replace(true) {
                                // One byte differs, and stays printable ASCII inside a name Git still
                                // accepts (HEAD's branch name, a config key): Git keeps reading it.
                                let at = if file == "HEAD" { 16 } else { 10 };
                                let mut bytes = std::fs::read(&target).expect("the measured bytes");
                                bytes[at] = if bytes[at] == b'x' { b'y' } else { b'x' };
                                *race.alternate.borrow_mut() = bytes;
                            }
                        },
                    ));
                }
                {
                    let (race, target, parked) = (shared.clone(), target.clone(), parked.clone());
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::BeforeChild,
                        move |event| {
                            if event.label == command
                                && race.swapped.get()
                                && how == SubstitutionV1::Alternate
                                && !race.restored.replace(true)
                            {
                                std::fs::rename(&parked, &target)
                                    .expect("the measured inode returns before the child");
                            }
                        },
                    ));
                }
                {
                    let (race, target) = (shared.clone(), target.clone());
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::AfterChild,
                        move |event| {
                            if event.label == command
                                && race.restored.get()
                                && !race.rewritten.replace(true)
                            {
                                // The alternate's bytes, written into the measured inode itself:
                                // the same inode and the same length.
                                use std::os::unix::fs::FileExt as _;
                                std::fs::OpenOptions::new()
                                    .write(true)
                                    .open(&target)
                                    .expect("the measured inode opens for the racer")
                                    .write_all_at(&race.alternate.borrow(), 0)
                                    .expect("the alternate's bytes are written in place");
                            }
                        },
                    ));
                }
            }
        }
        guards
    });
    (ran, outcome, race)
}

/// The final bytes of `file`, and whether they are the bytes the alternate inode held.
fn final_state(ran: &RanV1, file: &str, race: &RaceV1) -> (Vec<u8>, bool) {
    let bytes = std::fs::read(ran.dest.bootstrap().join(file)).expect("the file's final bytes");
    let is_alternate = bytes == *race.alternate.borrow();
    (bytes, is_alternate)
}

/// A digest that opens another inode than the one the census measured is refused, on every
/// read-only child and on both digest passes, for both baseline files: a custody refusal, with the
/// child's own marker. Without the descriptor's binding the digest hashes the alternate inode's
/// bytes, the digests agree, and a bootstrap whose HEAD or config holds the child's changed bytes
/// is returned as verified.
#[test]
fn round5_a_digest_opened_on_a_substituted_inode_is_refused() {
    let clean = clean_charge();
    let mut problems = Vec::new();
    for (_needle, command, started, completed) in READ_ONLY_V1 {
        for file in ROUND4_FILES_V1 {
            for pass in [DigestPassV1::PostChild, DigestPassV1::Baseline] {
                let name = format!("{command}: {file} substituted before the {pass:?} digest");
                let (ran, outcome, race) =
                    run_with_substitution(command, file, pass, SubstitutionV1::Alternate);
                // A baseline refusal comes before the child under test starts.
                let children = match pass {
                    DigestPassV1::PostChild => started,
                    DigestPassV1::Baseline => &started[..started.len() - 1],
                };
                let (bytes, is_alternate) = final_state(&ran, file, &race);
                println!(
                    "SWAP-MARKER {name}: swapped={} restored={} rewritten={} measured_inode={} alternate_inode={} final_len={} final_is_the_alternate={is_alternate}",
                    race.swapped.get(),
                    race.restored.get(),
                    race.rewritten.get(),
                    race.measured_inode.get(),
                    race.alternate_inode.get(),
                    bytes.len()
                );
                if !race.swapped.get() || race.measured_inode.get() == race.alternate_inode.get() {
                    problems.push(format!("{name}: the fixture never made the substitution"));
                    continue;
                }
                if outcome.is_ok() && !is_alternate {
                    problems.push(format!(
                        "{name}: the verified bootstrap holds {file} bytes that no digest ever read"
                    ));
                }
                problems.extend(judge(
                    &name,
                    &ran,
                    &outcome,
                    children,
                    Some(completed),
                    ChargeV1::Exactly(ran.phase_v + clean),
                    is_identity_changed,
                ));
            }
        }
    }
    assert_no_problems(problems);
}

/// The negative control: the measured inode renamed away and back between the census and the open is
/// the SAME inode, so nothing is refused: the binding refuses an inode that is not the measured one,
/// not a name that was touched. No byte is written, so every row must verify.
#[test]
fn round5_the_measured_inode_renamed_away_and_back_is_not_a_substitution() {
    for command in [READ_ONLY_V1[0].1, READ_ONLY_V1[3].1] {
        for pass in [DigestPassV1::PostChild, DigestPassV1::Baseline] {
            for file in ROUND4_FILES_V1 {
                let name =
                    format!("{command}: {file} renamed away and back before the {pass:?} digest");
                let (ran, outcome, race) =
                    run_with_substitution(command, file, pass, SubstitutionV1::SameInodeBack);
                let observed = ran.route.children();
                println!(
                    "CHILD-START-MARKER {name}: observed={observed:?} swapped={}",
                    race.swapped.get()
                );
                assert!(race.swapped.get(), "{name}: the fixture never renamed");
                assert_eq!(observed.len(), 6, "{name}: every proof child must have run");
                if let Err(failure) = outcome {
                    panic!("{name}: the control must succeed: {failure:?}");
                }
            }
        }
    }
}

/// The one deadline of the near-expiry rows.
const NEAR_EXPIRY_V1: Duration = Duration::from_secs(10);

/// One Captured SHA-1 bootstrap in which the final `fsck` EXITS WITHIN the deadline, and the clock
/// then reaches `until` (measured from the attempt's start) in the child's post-exit callback, before
/// the child's settlement runs: the digest pass of that settlement is what meets the deadline.
fn run_with_the_final_fsck_settling_at(until: Duration) -> (RanV1, OutcomeV1, Instant) {
    let begun = Instant::now();
    let (ran, outcome) = run_with(&[], NEAR_EXPIRY_V1, move |_dest| {
        vec![install_git_hook_for_test(
            GitHookPointV1::AfterChild,
            move |event| {
                if event.label == "fsck --strict" {
                    std::thread::sleep((begun + until).saturating_duration_since(Instant::now()));
                }
            },
        )]
    });
    (ran, outcome, begun + NEAR_EXPIRY_V1)
}

/// A final `fsck` that finishes before the deadline can never make the bootstrap succeed AFTER it:
/// the settlement's digest pass keeps the one absolute deadline, so a deadline that passes before it
/// refuses with a timeout. The child's own exit was clean, so the timeout stands alone: it is not a
/// child failure. Without that deadline the digests run to the end and a verified bootstrap is
/// returned after the deadline.
#[test]
fn round5_a_final_fsck_that_finishes_before_the_deadline_cannot_succeed_after_it() {
    let clean = clean_charge();
    let (_needle, _command, started, completed) = READ_ONLY_V1[3];
    let (ran, outcome, deadline) =
        run_with_the_final_fsck_settling_at(NEAR_EXPIRY_V1 + Duration::from_secs(1));
    let after = Instant::now().saturating_duration_since(deadline);
    println!(
        "DEADLINE-MARKER the final fsck crossing: returned {}ms after the deadline",
        after.as_millis()
    );
    let mut problems = judge(
        "the final fsck crossing the deadline",
        &ran,
        &outcome,
        started,
        Some(completed),
        ChargeV1::Exactly(ran.phase_v + clean),
        is_timeout,
    );
    if outcome.is_ok() {
        problems.push(format!(
            "success was returned {}ms after the deadline",
            after.as_millis()
        ));
    }
    assert_no_problems(problems);
}

/// The with-headroom control: the same final `fsck` and the same hook, settling well inside the
/// deadline, succeeds with every child started, and returns before the deadline.
#[test]
fn round5_a_final_fsck_settling_with_headroom_succeeds_before_the_deadline() {
    let (ran, outcome, deadline) = run_with_the_final_fsck_settling_at(Duration::from_millis(500));
    let observed = ran.route.children();
    println!(
        "CHILD-START-MARKER the headroom control: observed={observed:?} returned_before_the_deadline={}",
        Instant::now() < deadline
    );
    assert_eq!(observed.len(), 6, "every proof child must have run");
    match outcome {
        Ok(verified) => assert!(verified.closure.fsck_strict),
        Err(failure) => panic!("the headroom control must succeed: {failure:?}"),
    }
    assert!(
        Instant::now() < deadline,
        "the control must finish inside its deadline"
    );
}

/// `fsck` stderr rows, each a real final child with its marker: a recognized strict-object row never
/// masks an unclassified row or a repeated notice anywhere in the stream (the WHOLE stream is
/// classified before the strict rejection is chosen), and the controls that stay as they were:
/// the notice and a strict row, a lone strict row, two strict rows, and each malformed stream alone.
#[test]
fn round5_fsck_classifies_the_whole_stream_before_it_selects_a_strict_rejection() {
    let notice = "notice: No default references";
    let strict = "error: object 0123456789abcdef0123456789abcdef01234567: zeroPaddedFilemode: contains zero-padded file modes";
    let second = "error: object 0123456789abcdef0123456789abcdef01234567: badTimezone: invalid author/committer line - bad time zone";
    let rows: Vec<(&str, String, ErrorCheckV1)> = vec![
        (
            "a strict row, then an unclassified row",
            format!("echo '{strict}' >&2; echo 'unclassified trailing output' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "an unclassified row, then a strict row",
            format!("echo 'unclassified leading output' >&2; echo '{strict}' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "an unclassified row on both sides of a strict row",
            format!("printf 'before\\n{strict}\\nafter\\n' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "a strict row, then an unclassified row, status zero",
            format!("echo '{strict}' >&2; echo 'unclassified trailing output' >&2; exit 0"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "a notice on both sides of a strict row",
            format!("printf '{notice}\\n{strict}\\n{notice}\\n' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "a duplicated notice, then a strict row",
            format!("printf '{notice}\\n{notice}\\n{strict}\\n' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "a strict row, then a duplicated notice",
            format!("printf '{strict}\\n{notice}\\n{notice}\\n' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "a strict row, then a duplicated notice, status zero",
            format!("printf '{strict}\\n{notice}\\n{notice}\\n' >&2; exit 0"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "control: the notice, then a strict row",
            format!("printf '{notice}\\n{strict}\\n' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        ),
        (
            "control: a strict row, then the notice",
            format!("printf '{strict}\\n{notice}\\n' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        ),
        (
            "control: two recognized strict rows",
            format!("printf '{strict}\\n{second}\\n' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        ),
        (
            "control: a strict row alone",
            format!("echo '{strict}' >&2; exit 1"),
            |e| matches!(e, GitBootstrapErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        ),
        (
            "control: an unclassified row alone",
            "echo 'unclassified output' >&2; exit 1".to_owned(),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "control: a duplicated notice alone",
            format!("printf '{notice}\\n{notice}\\n' >&2; exit 0"),
            |e| matches!(e, GitBootstrapErrorV1::GitOutput(_)),
        ),
        (
            "control: a nonzero status over a clean stream",
            "\"$REAL\" \"$@\"; exit 1".to_owned(),
            |e| {
                matches!(
                    e,
                    GitBootstrapErrorV1::GitChild {
                        command: "fsck --strict",
                        status: Some(1),
                        ..
                    }
                )
            },
        ),
    ];
    let borrowed: Vec<(&str, &str, ErrorCheckV1)> = rows
        .iter()
        .map(|(row, shell, check)| (*row, shell.as_str(), *check))
        .collect();
    marked_table(
        "fsck",
        "fsck",
        GitPlaneStepV1::Closure,
        "fsck --strict",
        &borrowed,
    );
}

// --- Isolating each descriptor check: the new digest seam ---------------------------------------
//
// These rows need `DigestOpened` and `DigestHashed`, two test hook points inside the digest that do not
// exist on the pre-round-5 source: they cannot compile there, and NO base RED is claimed for them.
// Their causality is shown by a mutation that removes exactly one guard, with the rows below as the
// unchanged selected tests. The clocks are stalled for every row, so the descriptor's own checks are
// what is left to decide, one at a time.

/// What a hook does to a file while the digest holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InReadV1 {
    /// A mode bit toggled between the census and the open, and restored once the file is hashed: only
    /// the descriptor's check BEFORE the read sees it.
    ModeBeforeOpen,
    /// A mode bit toggled once the file is hashed: only the check AFTER the read sees it.
    ModeAfterRead,
    /// One byte short while it is read, its length restored once it is hashed: only the byte count
    /// sees it.
    ShortRead,
    /// One byte long while it is read, its length restored once it is hashed: only the byte count
    /// sees it.
    LongRead,
}

#[derive(Default)]
struct InReadStateV1 {
    armed: std::cell::Cell<bool>,
    /// The race was made (the mode toggled, the file shortened or grown).
    made: std::cell::Cell<bool>,
    /// The file was put back after the read.
    restored: std::cell::Cell<bool>,
    mode: std::cell::Cell<u32>,
    length: std::cell::Cell<u64>,
}

fn set_length(path: &Path, length: u64) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("the file opens for the racer")
        .set_len(length)
        .expect("the file's length is set");
}

/// One Captured SHA-1 bootstrap, clocks stalled, in which `file` is raced (`kind`) during the digest
/// `pass` of the child labelled `command`.
fn run_with_digest_race(
    command: &'static str,
    file: &'static str,
    pass: DigestPassV1,
    kind: InReadV1,
) -> (RanV1, OutcomeV1, std::rc::Rc<InReadStateV1>) {
    use std::io::Write as _;
    let state = std::rc::Rc::new(InReadStateV1::default());
    let shared = state.clone();
    let (ran, outcome) = run_with(&[], Duration::from_secs(120), move |_dest| {
        let mut guards = vec![stall_census_clocks_for_test(BOTH_CLOCKS_V1)];
        // Arm for the next digest of this child: its baseline, or its settlement.
        {
            let state = shared.clone();
            let point = match pass {
                DigestPassV1::Baseline => GitHookPointV1::BeforeChildCensus,
                DigestPassV1::PostChild => GitHookPointV1::AfterChild,
            };
            guards.push(install_git_hook_for_test(point, move |event| {
                if event.label == command {
                    state.armed.set(true);
                }
            }));
        }
        match kind {
            InReadV1::ModeBeforeOpen => {
                {
                    let state = shared.clone();
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::CensusListed,
                        move |event| {
                            if state.armed.get()
                                && !state.made.get()
                                && event.path.ends_with("git-bootstrap/objects/pack")
                            {
                                let path = event
                                    .path
                                    .parent()
                                    .and_then(Path::parent)
                                    .expect("the bootstrap")
                                    .join(file);
                                state.mode.set(
                                    std::fs::metadata(&path)
                                        .expect("the file")
                                        .permissions()
                                        .mode(),
                                );
                                toggle_a_mode_bit(&path);
                                state.made.set(true);
                            }
                        },
                    ));
                }
                {
                    let state = shared.clone();
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::DigestHashed,
                        move |event| {
                            if state.made.get()
                                && event.path.ends_with(file)
                                && !state.restored.replace(true)
                            {
                                std::fs::set_permissions(
                                    event.path,
                                    std::fs::Permissions::from_mode(state.mode.get() & 0o7777),
                                )
                                .expect("the mode is put back");
                            }
                        },
                    ));
                }
            }
            InReadV1::ModeAfterRead => {
                let state = shared.clone();
                guards.push(install_git_hook_for_test(
                    GitHookPointV1::DigestHashed,
                    move |event| {
                        if state.armed.get()
                            && event.path.ends_with(file)
                            && !state.made.replace(true)
                        {
                            toggle_a_mode_bit(event.path);
                        }
                    },
                ));
            }
            InReadV1::ShortRead | InReadV1::LongRead => {
                {
                    let state = shared.clone();
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::DigestOpened,
                        move |event| {
                            if state.armed.get()
                                && event.path.ends_with(file)
                                && !state.made.replace(true)
                            {
                                let length = std::fs::metadata(event.path).expect("the file").len();
                                state.length.set(length);
                                if kind == InReadV1::ShortRead {
                                    set_length(event.path, length - 1);
                                } else {
                                    std::fs::OpenOptions::new()
                                        .append(true)
                                        .open(event.path)
                                        .expect("the file opens for appending")
                                        .write_all(b"x")
                                        .expect("one byte is appended");
                                }
                            }
                        },
                    ));
                }
                {
                    let state = shared.clone();
                    guards.push(install_git_hook_for_test(
                        GitHookPointV1::DigestHashed,
                        move |event| {
                            if state.made.get()
                                && event.path.ends_with(file)
                                && !state.restored.replace(true)
                            {
                                set_length(event.path, state.length.get());
                            }
                        },
                    ));
                }
            }
        }
        guards
    });
    (ran, outcome, state)
}

/// Each descriptor check refuses ALONE, on its own row: the check before the read (a mode that differs
/// between the census and the open), the check after the read (a mode that changes once it is hashed),
/// and the exact byte count (a file one byte short or one byte long while it is read, its length
/// restored afterwards, so the closing `fstat` agrees). Every refusal is a custody refusal. A baseline
/// refusal comes before the child under test starts; the byte count is isolated on the baseline pass,
/// where no digest comparison could refuse the wrong-length read in its place.
#[test]
fn round5_each_descriptor_check_refuses_alone() {
    let clean = clean_charge();
    let mut problems = Vec::new();
    type Row = (InReadV1, DigestPassV1);
    let rows: [Row; 6] = [
        (InReadV1::ModeBeforeOpen, DigestPassV1::Baseline),
        (InReadV1::ModeBeforeOpen, DigestPassV1::PostChild),
        (InReadV1::ModeAfterRead, DigestPassV1::Baseline),
        (InReadV1::ModeAfterRead, DigestPassV1::PostChild),
        (InReadV1::ShortRead, DigestPassV1::Baseline),
        (InReadV1::LongRead, DigestPassV1::Baseline),
    ];
    for (_needle, command, started, completed) in [READ_ONLY_V1[0], READ_ONLY_V1[3]] {
        for file in ROUND4_FILES_V1 {
            for (kind, pass) in rows {
                let name = format!("{command}: {file} {kind:?} during the {pass:?} digest");
                let (ran, outcome, state) = run_with_digest_race(command, file, pass, kind);
                let children = match pass {
                    DigestPassV1::PostChild => started,
                    DigestPassV1::Baseline => &started[..started.len() - 1],
                };
                println!(
                    "DIGEST-RACE-MARKER {name}: made={} restored={}",
                    state.made.get(),
                    state.restored.get()
                );
                if !state.made.get() {
                    problems.push(format!("{name}: the fixture never made the race"));
                    continue;
                }
                problems.extend(judge(
                    &name,
                    &ran,
                    &outcome,
                    children,
                    Some(completed),
                    ChargeV1::Exactly(ran.phase_v + clean),
                    is_identity_changed,
                ));
            }
        }
    }
    assert_no_problems(problems);
}

/// The stable controls: hooks that only OBSERVE change nothing and every digest check passes, with the
/// seam firing once per file per digest (four read-only children, a baseline and a settlement digest
/// each, five files: HEAD, config, and the pack's three); and an atime-only read made inside a digest,
/// on a mount that moves atime, is allowed with the REAL clocks (a read changes no other fact).
#[test]
fn round5_a_stable_file_and_an_atime_only_read_pass_every_descriptor_check() {
    use std::os::unix::fs::MetadataExt as _;
    let opened = std::rc::Rc::new(std::cell::Cell::new(0_usize));
    let hashed = std::rc::Rc::new(std::cell::Cell::new(0_usize));
    let (seen_open, seen_hash) = (opened.clone(), hashed.clone());
    let (ran, outcome) = run_with(&[], Duration::from_secs(120), move |_dest| {
        vec![
            install_git_hook_for_test(GitHookPointV1::DigestOpened, move |_| {
                seen_open.set(seen_open.get() + 1);
            }),
            install_git_hook_for_test(GitHookPointV1::DigestHashed, move |_| {
                seen_hash.set(seen_hash.get() + 1);
            }),
        ]
    });
    let observed = ran.route.children();
    println!(
        "CHILD-START-MARKER the stable control: observed={observed:?} digest opens={} hashes={}",
        opened.get(),
        hashed.get()
    );
    assert_eq!(observed.len(), 6, "every proof child must have run");
    if let Err(failure) = outcome {
        panic!("the stable control must succeed: {failure:?}");
    }
    assert_eq!((opened.get(), hashed.get()), (40, 40));

    // The atime-only read: the real clocks, atimes set to the epoch before each baseline census, and a
    // second descriptor's read of HEAD and config at every digest open.
    let (ran, outcome) = run_with(&[], Duration::from_secs(120), move |dest| {
        let paths = PathsV1::of(dest);
        let old = paths.clone();
        vec![
            install_git_hook_for_test(GitHookPointV1::BeforeChildCensus, move |event| {
                // Only the read-only children: `init` runs before the files exist.
                if READ_ONLY_V1
                    .iter()
                    .any(|(_, command, _, _)| *command == event.label)
                {
                    for name in ROUND4_FILES_V1 {
                        std::fs::File::open(old.bootstrap.join(name))
                            .expect("the file opens")
                            .set_times(
                                std::fs::FileTimes::new().set_accessed(std::time::UNIX_EPOCH),
                            )
                            .expect("an old atime");
                    }
                }
            }),
            install_git_hook_for_test(GitHookPointV1::DigestOpened, move |event| {
                if ROUND4_FILES_V1
                    .iter()
                    .any(|name| event.path.ends_with(name))
                {
                    std::fs::read(event.path).expect("the racer's read");
                }
            }),
        ]
    });
    let observed = ran.route.children();
    let moved = ROUND4_FILES_V1.map(|name| {
        std::fs::metadata(ran.dest.bootstrap().join(name))
            .expect("metadata")
            .atime()
            != 0
    });
    println!(
        "CHILD-START-MARKER the atime-only control: observed={observed:?} ATIME-EVIDENCE moved off the epoch: {moved:?}"
    );
    assert_eq!(observed.len(), 6, "every proof child must have run");
    if let Err(failure) = outcome {
        panic!("a read that moved only atime was refused: {failure:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Round 6: the ONE absolute deadline holds through every late validation step
//
// The runner enforces the deadline only while a child starts and waits, and the digest checks it before
// each read. Everything after the final child's result (its settlement, the post-child success gate,
// the staged rewinds, and the closing gate: mount census, exact tree, admitted-file rehash, every
// retained chain) is slow validation whose clock the base never looked at: if it passed there, the
// bootstrap still returned a verified value. Each row below makes ONE such step slow (a hook or the
// mount-census seam sleeps until the deadline has passed) in a real run with all SIX children started.
//
// STAGE A rows use only hook points and seams that exist on the pristine base b3aa6afd, so they compile
// there and are behavioral RED there (the bootstrap SUCCEEDS after the deadline). Every row prints its
// CHILD-START-MARKER and DEADLINE-MARKER.
// ---------------------------------------------------------------------------------------------

/// The one deadline of the round-6 rows. A clean bootstrap takes well under a second, so the five
/// children before the injection finish far inside it; under extreme load a row fails with a different,
/// visible cause (its marker prints the children), never passes wrongly.
const ROUND6_DEADLINE_V1: Duration = Duration::from_secs(6);

/// A slow step that returns after `deadline`: sleep until it has passed, plus a margin.
fn pass_the_deadline(deadline: Instant) {
    std::thread::sleep(
        deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(50),
    );
}

/// Like [`run_with`], with the ABSOLUTE `deadline` chosen by the caller (so a hook can sleep to exactly
/// it). `setup` runs after phase V and before the route is built; whatever it returns lives until the
/// attempt ends.
fn run_before(
    rules: &[(&str, &str)],
    deadline: Instant,
    setup: impl FnOnce(&DestV1) -> Vec<Box<dyn std::any::Any>>,
) -> (RanV1, OutcomeV1) {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let (phase_v_used, base) = pin_charge(&verified);
    let pack_length = verified
        .staged
        .iter()
        .find(|one| one.role == CustodyCapsuleArtifactRoleV1::GitObjectPack)
        .map(|one| one.length)
        .expect("the staged pack");
    let _guards = setup(&dest);
    let route = RouteV1::new(rules);
    let (_area, runner) = route.runner();
    let outcome = bootstrap_git_objects_v1(verified, &runner, GitObjectFormatV1::Sha1, deadline);
    (
        RanV1 {
            dest,
            route,
            phase_v: phase_v_used,
            base,
            pack_length,
        },
        outcome,
    )
}

/// The mount-census calls one clean bootstrap makes after phase V: the LAST is the closing gate's, the one
/// before it is the final child's post-child success gate's.
fn clean_mount_census_calls() -> usize {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let (_dest, verified) = phase_v(&capsule.capsule);
    let census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec()]);
    let _ = must_bootstrap(verified, GitObjectFormatV1::Sha1);
    census.calls()
}

/// A late validation step a Stage A row makes slow, each reachable through a seam the base has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LateV1 {
    /// The `BeforeFinalGate` hook: everything before the closing gate is done.
    BeforeFinalGate,
    /// The first directory the CLOSING gate's exact-tree census lists.
    ClosingTreeCensus,
    /// The mount census inside the closing gate (the run's last census call).
    ClosingMountCensus,
    /// The mount census inside the final `fsck`'s post-child success gate (the run's last-but-one call).
    SuccessGateMountCensus,
    /// The final `fsck`'s post-child digest has just read its LAST file (`.rev`) to its end.
    FinalDigestRead,
}

impl LateV1 {
    const ALL: [LateV1; 5] = [
        Self::BeforeFinalGate,
        Self::ClosingTreeCensus,
        Self::ClosingMountCensus,
        Self::SuccessGateMountCensus,
        Self::FinalDigestRead,
    ];

    /// The step `last_completed` names when the deadline is caught right after this one: the final
    /// `fsck` has not completed whole while its own success gate is still running.
    fn completed(self) -> GitPlaneStepV1 {
        match self {
            Self::SuccessGateMountCensus | Self::FinalDigestRead => GitPlaneStepV1::Closure,
            Self::BeforeFinalGate | Self::ClosingTreeCensus | Self::ClosingMountCensus => {
                GitPlaneStepV1::Fsck
            }
        }
    }
}

/// How slow an injected step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DelayV1 {
    /// It returns after the deadline.
    PastTheDeadline,
    /// A short, harmless delay that leaves most of the deadline: the with-headroom control.
    Brief,
}

#[derive(Default)]
struct LateStateV1 {
    /// The final `fsck`'s post-exit callback ran (its post-child settlement is next).
    after_final_child: std::cell::Cell<bool>,
    /// The closing gate's turn: `BeforeFinalGate` ran.
    closing: std::cell::Cell<bool>,
    /// Every injection that ran, in the state it names (after the final child / in the closing gate).
    fired: std::cell::RefCell<Vec<LateV1>>,
    /// Every injection that ran OUT of its place (a fixture that drifted).
    misplaced: std::cell::RefCell<Vec<LateV1>>,
}

impl LateStateV1 {
    /// Whether `point` has not run yet; records that it now has, and where.
    fn first(&self, point: LateV1, in_place: bool) -> bool {
        if self.fired.borrow().contains(&point) {
            return false;
        }
        self.fired.borrow_mut().push(point);
        if !in_place {
            self.misplaced.borrow_mut().push(point);
        }
        true
    }
}

/// One Captured SHA-1 bootstrap with every step in `points` slowed by `delay`. `mount_calls` is
/// [`clean_mount_census_calls`].
fn run_late(
    points: &[LateV1],
    delay: DelayV1,
    mount_calls: usize,
    deadline: Instant,
) -> (RanV1, OutcomeV1, std::rc::Rc<LateStateV1>) {
    let state = std::rc::Rc::new(LateStateV1::default());
    let shared = state.clone();
    let points = points.to_vec();
    let slow = move || match delay {
        DelayV1::PastTheDeadline => pass_the_deadline(deadline),
        DelayV1::Brief => std::thread::sleep(Duration::from_millis(100)),
    };
    let (ran, outcome) = run_before(&[], deadline, move |_dest| {
        let mut guards: Vec<Box<dyn std::any::Any>> = Vec::new();
        {
            let state = shared.clone();
            guards.push(Box::new(install_git_hook_for_test(
                GitHookPointV1::AfterChild,
                move |event| {
                    if event.label == "fsck --strict" {
                        state.after_final_child.set(true);
                    }
                },
            )));
        }
        {
            let state = shared.clone();
            let inject = points.contains(&LateV1::BeforeFinalGate);
            guards.push(Box::new(install_git_hook_for_test(
                GitHookPointV1::BeforeFinalGate,
                move |_| {
                    state.closing.set(true);
                    if inject && state.first(LateV1::BeforeFinalGate, true) {
                        slow();
                    }
                },
            )));
        }
        if points.contains(&LateV1::ClosingTreeCensus) {
            let state = shared.clone();
            guards.push(Box::new(install_git_hook_for_test(
                GitHookPointV1::CensusListed,
                move |_| {
                    if state.closing.get() && state.first(LateV1::ClosingTreeCensus, true) {
                        slow();
                    }
                },
            )));
        }
        if points.contains(&LateV1::FinalDigestRead) {
            let state = shared.clone();
            guards.push(Box::new(install_git_hook_for_test(
                GitHookPointV1::DigestHashed,
                move |event| {
                    if state.after_final_child.get()
                        && event.path.extension().is_some_and(|found| found == "rev")
                        && state.first(LateV1::FinalDigestRead, true)
                    {
                        slow();
                    }
                },
            )));
        }
        let at_closing = points.contains(&LateV1::ClosingMountCensus);
        let at_success_gate = points.contains(&LateV1::SuccessGateMountCensus);
        if at_closing || at_success_gate {
            let state = shared.clone();
            guards.push(Box::new(crate::custody_mounts::seam::install(Box::new(
                move |call| {
                    if at_closing && call == mount_calls {
                        let in_place = state.closing.get() && state.after_final_child.get();
                        if state.first(LateV1::ClosingMountCensus, in_place) {
                            slow();
                        }
                    } else if at_success_gate && call + 1 == mount_calls {
                        let in_place = !state.closing.get() && state.after_final_child.get();
                        if state.first(LateV1::SuccessGateMountCensus, in_place) {
                            slow();
                        }
                    }
                    Ok(vec![b"/".to_vec()])
                },
            ))));
        }
        guards
    });
    (ran, outcome, state)
}

/// The six children a complete run starts, in order.
fn six_children() -> &'static [&'static str] {
    READ_ONLY_V1[3].2
}

/// One Stage A row: `point` made slow past the deadline must be refused with a typed timeout, with every
/// one of the SIX children started, the step `point` names as the last completed, and the ledger exactly
/// as a clean run leaves it. Returns every problem found.
fn late_row(point: LateV1, mount_calls: usize, clean: u64) -> Vec<String> {
    let deadline = Instant::now() + ROUND6_DEADLINE_V1;
    let (ran, outcome, state) = run_late(&[point], DelayV1::PastTheDeadline, mount_calls, deadline);
    let after = Instant::now().saturating_duration_since(deadline);
    println!(
        "DEADLINE-MARKER {point:?}: fired={:?} misplaced={:?} returned {}ms after the deadline",
        state.fired.borrow(),
        state.misplaced.borrow(),
        after.as_millis()
    );
    let mut problems = judge(
        &format!("{point:?} spent past the deadline"),
        &ran,
        &outcome,
        six_children(),
        Some(point.completed()),
        ChargeV1::Exactly(ran.phase_v + clean),
        is_timeout,
    );
    if *state.fired.borrow() != [point] || !state.misplaced.borrow().is_empty() {
        problems.push(format!(
            "{point:?}: the injected slow step never ran, once, in its place"
        ));
    }
    if outcome.is_ok() {
        problems.push(format!(
            "{point:?}: a verified bootstrap was returned {}ms after the deadline",
            after.as_millis()
        ));
    }
    problems
}

/// The deadline passes at the `BeforeFinalGate` hook with all six children started: the closing gate must
/// refuse it with a typed timeout and never return a verified bootstrap. On the base it returns one.
#[test]
fn round6_a_deadline_spent_before_the_final_gate_cannot_succeed() {
    let clean = clean_charge();
    let mount_calls = clean_mount_census_calls();
    assert_no_problems(late_row(LateV1::BeforeFinalGate, mount_calls, clean));
}

/// The deadline passes while the closing gate's exact-tree census is listing a directory.
#[test]
fn round6_a_deadline_spent_inside_the_closing_tree_census_cannot_succeed() {
    let clean = clean_charge();
    let mount_calls = clean_mount_census_calls();
    assert_no_problems(late_row(LateV1::ClosingTreeCensus, mount_calls, clean));
}

/// The deadline passes inside the closing gate's mount census (the slow platform call).
#[test]
fn round6_a_deadline_spent_inside_the_closing_mount_census_cannot_succeed() {
    let clean = clean_charge();
    let mount_calls = clean_mount_census_calls();
    assert_no_problems(late_row(LateV1::ClosingMountCensus, mount_calls, clean));
}

/// The deadline passes inside the final child's post-child success gate (its mount census), after its
/// settlement's digests all ran: the success arm must not return the child's result.
#[test]
fn round6_a_deadline_spent_inside_the_final_childs_success_gate_cannot_succeed() {
    let clean = clean_charge();
    let mount_calls = clean_mount_census_calls();
    assert_no_problems(late_row(LateV1::SuccessGateMountCensus, mount_calls, clean));
}

/// The deadline passes right after the final child's post-child digest read its LAST file to its end,
/// the one moment the per-read check can never see: nothing may progress toward success past it.
#[test]
fn round6_a_deadline_spent_after_the_final_digest_read_cannot_succeed() {
    let clean = clean_charge();
    let mount_calls = clean_mount_census_calls();
    assert_no_problems(late_row(LateV1::FinalDigestRead, mount_calls, clean));
}

/// The with-headroom control: ALL FIVE injections run together, each a short delay that leaves most of
/// the deadline. Every injection must have run once, in place (so the rows above are not refused for a
/// fixture reason), every one of the six children must have started, and the bootstrap must SUCCEED before
/// the deadline. On the base AND on the correction this passes.
#[test]
fn round6_with_headroom_every_late_validation_step_succeeds_before_the_deadline() {
    let mount_calls = clean_mount_census_calls();
    let deadline = Instant::now() + ROUND6_DEADLINE_V1;
    let (ran, outcome, state) = run_late(&LateV1::ALL, DelayV1::Brief, mount_calls, deadline);
    let observed = ran.route.children();
    println!(
        "CHILD-START-MARKER the headroom control: observed={observed:?} fired={:?} misplaced={:?} returned_before_the_deadline={}",
        state.fired.borrow(),
        state.misplaced.borrow(),
        Instant::now() < deadline
    );
    assert_eq!(observed.len(), 6, "every proof child must have run");
    let mut fired = state.fired.borrow().clone();
    fired.sort_by_key(|point| format!("{point:?}"));
    let mut every = LateV1::ALL.to_vec();
    every.sort_by_key(|point| format!("{point:?}"));
    assert_eq!(fired, every, "every injection must have run once");
    assert!(
        state.misplaced.borrow().is_empty(),
        "every injection ran in its place"
    );
    match outcome {
        Ok(verified) => assert!(verified.closure.fsck_strict),
        Err(failure) => panic!("the headroom control must succeed: {failure:?}"),
    }
    assert!(
        Instant::now() < deadline,
        "the control must finish inside its deadline"
    );
}

// ---------------------------------------------------------------------------------------------
// Round 6, STAGE B: every late-validation guard, one by one
//
// These rows need `GitHookPointV1::DeadlineObserved`, a test hook inside the one guard function that
// every late-validation step calls right after it returns. It does not exist on the pristine base, so
// these rows cannot compile there and NO base RED is claimed for them (Stage A above is the base RED).
// Their causality is shown on the correction by (a) the seam-only control, the correction with the single
// clock comparison inside the guard removed, and (b) one mutation per guard. Each row makes the step
// BEFORE one guard slow past the deadline and requires the typed timeout, the step it names, the exact
// ledger, all the children that had started, and that NO later guard ever ran (the run halted AT it).
// ---------------------------------------------------------------------------------------------

/// What the guard hook does when it is the guard with index `at` (counted over the whole run).
#[derive(Debug)]
enum AtGuardV1 {
    /// Nothing: the clean control records every guard.
    Observe,
    /// The step before that guard returned after the deadline.
    PassTheDeadline { at: usize, label: &'static str },
    /// The destination root is replaced, and the clock is NOT spent: at the guard immediately before the
    /// root's identity check, the identity check must still be the last, and still refuse.
    SwapTheRoot { at: usize, label: &'static str },
}

#[derive(Default)]
struct GuardLogV1 {
    /// Every guard that read the clock, in order, by step label.
    events: std::cell::RefCell<Vec<&'static str>>,
    /// How many guards had run when the final `fsck`'s post-exit callback ran.
    final_child_at: std::cell::Cell<Option<usize>>,
    /// The label of the guard the injection found at its index.
    injected: std::cell::Cell<Option<&'static str>>,
}

/// One Captured SHA-1 bootstrap whose every late-validation guard is logged, with `at` done at one guard.
fn run_guarded(
    rules: &[(&str, &str)],
    deadline: Instant,
    at: AtGuardV1,
) -> (RanV1, OutcomeV1, std::rc::Rc<GuardLogV1>) {
    let log = std::rc::Rc::new(GuardLogV1::default());
    let shared = log.clone();
    let (ran, outcome) = run_before(rules, deadline, move |dest| {
        let mut guards: Vec<Box<dyn std::any::Any>> = Vec::new();
        {
            let log = shared.clone();
            guards.push(Box::new(install_git_hook_for_test(
                GitHookPointV1::AfterChild,
                move |event| {
                    if event.label == "fsck --strict" {
                        log.final_child_at.set(Some(log.events.borrow().len()));
                    }
                },
            )));
        }
        let root = dest.path.clone();
        let away = root.with_file_name("dest-away-round6");
        let log = shared.clone();
        guards.push(Box::new(install_git_hook_for_test(
            GitHookPointV1::DeadlineObserved,
            move |event| {
                let index = log.events.borrow().len();
                log.events.borrow_mut().push(event.label);
                match &at {
                    AtGuardV1::Observe => {}
                    AtGuardV1::PassTheDeadline { at, label } => {
                        if index == *at {
                            log.injected.set(Some(event.label));
                            if event.label == *label {
                                pass_the_deadline(deadline);
                            }
                        }
                    }
                    AtGuardV1::SwapTheRoot { at, label } => {
                        if index == *at {
                            log.injected.set(Some(event.label));
                            if event.label == *label {
                                std::fs::rename(&root, &away).expect("the root moves away");
                                std::fs::create_dir(&root).expect("an empty root takes its name");
                            }
                        }
                    }
                }
            },
        )));
        guards
    });
    (ran, outcome, log)
}

/// The guards one clean run makes, and the filesystem facts the expected sequence is derived from (never
/// from the production code under test).
struct GuardShapeV1 {
    events: Vec<&'static str>,
    final_child_at: usize,
    /// Retained staging directories beneath `plain/`.
    staging_dirs: usize,
    /// The bootstrap root and every directory beneath it.
    bootstrap_dirs: usize,
    /// Every entry beneath the bootstrap root.
    bootstrap_entries: usize,
    /// Staged descriptors the final rewind goes through.
    staged: usize,
}

impl GuardShapeV1 {
    /// The chain steps of one sweep: the work and plain chain, each staging directory, HOME, XDG, and
    /// every bootstrap directory.
    fn chains(&self) -> usize {
        1 + self.staging_dirs + 2 + self.bootstrap_dirs
    }

    /// The census steps of the closing gate: before each directory and before each entry.
    fn census_steps(&self) -> usize {
        self.bootstrap_dirs + self.bootstrap_entries
    }

    /// The whole sequence a clean run must log, derived from the facts above: one success gate for each
    /// of its `read_only` children, the rewinds, and the closing gate over `admitted` object files.
    fn expected(&self, read_only: usize, admitted: usize) -> Vec<&'static str> {
        let chains = self.chains();
        let mut expected = Vec::new();
        for _ in 0..read_only {
            expected.push("gate");
            expected.extend(std::iter::repeat_n("chain", chains));
            expected.push("root");
        }
        expected.extend(std::iter::repeat_n("rewind", self.staged));
        expected.extend(["closing", "closing-census"]);
        expected.extend(std::iter::repeat_n("census", self.census_steps()));
        expected.push("closing-tree");
        expected.extend(std::iter::repeat_n("rehash", admitted));
        expected.extend(std::iter::repeat_n("chain", chains));
        expected.push("root");
        expected
    }

    /// Where each part starts in `events`: the first read-only child's gate, the final child's gate, the
    /// rewinds, and the closing gate.
    fn starts(&self) -> [usize; 4] {
        let group = self.chains() + 2;
        [
            0,
            self.final_child_at,
            self.final_child_at + group,
            self.final_child_at + group + self.staged,
        ]
    }
}

fn clean_guard_shape() -> GuardShapeV1 {
    let deadline = Instant::now() + ROUND6_DEADLINE_V1;
    let (ran, outcome, log) = run_guarded(&[], deadline, AtGuardV1::Observe);
    let verified = match outcome {
        Ok(verified) => verified,
        Err(failure) => panic!("the clean control must succeed: {failure:?}"),
    };
    let directories = |root: &Path| {
        snapshot(root)
            .values()
            .filter(|bytes| bytes.is_none())
            .count()
    };
    let bootstrap = ran.dest.bootstrap();
    let events = log.events.borrow().clone();
    GuardShapeV1 {
        events,
        final_child_at: log.final_child_at.get().expect("the final fsck ran"),
        staging_dirs: directories(&ran.dest.work().join("plain")),
        bootstrap_dirs: directories(&bootstrap) + 1,
        bootstrap_entries: snapshot(&bootstrap).len(),
        staged: verified.verified.staged.len(),
    }
}

/// The guard sequence of a clean run is EXACTLY the one derived from the filesystem: every guard of the
/// success path runs, once per step and per loop iteration, in order, and the LAST guard is the one
/// immediately before the root's identity check, with nothing observed after it. Every guard site has a
/// row below; this pins that none is missing, doubled, or moved.
#[test]
fn round6_the_late_validation_guards_run_in_the_documented_order() {
    let shape = clean_guard_shape();
    println!(
        "GUARD-MARKER staging_dirs={} bootstrap_dirs={} entries={} staged={} chains={} census_steps={} events={} final_child_at={}",
        shape.staging_dirs,
        shape.bootstrap_dirs,
        shape.bootstrap_entries,
        shape.staged,
        shape.chains(),
        shape.census_steps(),
        shape.events.len(),
        shape.final_child_at
    );
    assert!(
        shape.staging_dirs >= 1 && shape.staged >= 2,
        "the fixture must exercise the per-staging-directory and per-rewind loops"
    );
    // Captured: verify-pack, cat-file, rev-list and fsck are read-only; the pack, index and reverse index
    // are the three admitted files.
    assert_eq!(shape.events, shape.expected(4, 3));
    assert_eq!(shape.events.last(), Some(&"root"), "the root guard is last");
}

/// An Empty object database runs the same guards over its two read-only children and NO admitted file:
/// there is no rehash guard, and the closing gate still ends at the root's guard.
#[test]
fn round6_an_empty_object_database_logs_the_same_guards_without_a_rehash() {
    let capsule = empty(CustodyGitObjectFormatV1::Sha1);
    let (dest, verified) = phase_v(&capsule.capsule);
    let log = std::rc::Rc::new(GuardLogV1::default());
    let (seen, final_child) = (log.clone(), log.clone());
    let _observe = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        seen.events.borrow_mut().push(event.label);
    });
    let _final = install_git_hook_for_test(GitHookPointV1::AfterChild, move |event| {
        if event.label == "fsck --strict" {
            final_child
                .final_child_at
                .set(Some(final_child.events.borrow().len()));
        }
    });
    let (_area, runner) = lane_runner();
    let bootstrap = bootstrap(verified, &runner, GitObjectFormatV1::Sha1)
        .unwrap_or_else(|failure| panic!("the Empty control must succeed: {failure:?}"));
    let bootstrap_tree = dest.bootstrap();
    let directories = |root: &Path| {
        snapshot(root)
            .values()
            .filter(|bytes| bytes.is_none())
            .count()
    };
    let shape = GuardShapeV1 {
        events: log.events.borrow().clone(),
        final_child_at: log.final_child_at.get().expect("the final fsck ran"),
        staging_dirs: directories(&dest.work().join("plain")),
        bootstrap_dirs: directories(&bootstrap_tree) + 1,
        bootstrap_entries: snapshot(&bootstrap_tree).len(),
        staged: bootstrap.verified.staged.len(),
    };
    println!(
        "GUARD-MARKER the Empty control: staging_dirs={} bootstrap_dirs={} entries={} staged={} events={}",
        shape.staging_dirs,
        shape.bootstrap_dirs,
        shape.bootstrap_entries,
        shape.staged,
        shape.events.len()
    );
    assert_eq!(shape.events, shape.expected(2, 0));
    assert!(!shape.events.contains(&"rehash"));
    assert_eq!(shape.events.last(), Some(&"root"));
}

/// One Stage B row's expectation: the children that had started, and the last completed step.
fn guard_row_expectation(
    index: usize,
    starts: &[usize; 4],
) -> (&'static [&'static str], GitPlaneStepV1) {
    if index < starts[1] {
        (
            &["init", "index-pack", "verify-pack"],
            GitPlaneStepV1::BootstrapInit,
        )
    } else if index < starts[2] {
        (six_children(), GitPlaneStepV1::Closure)
    } else {
        (six_children(), GitPlaneStepV1::Fsck)
    }
}

/// The guards one row per guard label and per loop end isolates, as `(index, label)`.
fn guard_rows(shape: &GuardShapeV1) -> Vec<(usize, &'static str)> {
    let [first_gate, final_gate, rewinds, closing] = shape.starts();
    let chains = shape.chains();
    let census = closing + 2;
    let tree = census + shape.census_steps();
    let rehash = tree + 1;
    let sweep = rehash + 3;
    let mut rows = vec![
        // The first read-only child's success gate: the same guard code runs for each of the four.
        (first_gate, "gate"),
        // The final child's success gate: before its census, right after it (the sweep's first step),
        // the sweep's last chain step, and immediately before the root's identity.
        (final_gate, "gate"),
        (final_gate + 1, "chain"),
        (final_gate + chains, "chain"),
        (final_gate + chains + 1, "root"),
        // The staged rewinds.
        (rewinds, "rewind"),
        (rewinds + shape.staged - 1, "rewind"),
        // The closing gate, in order.
        (closing, "closing"),
        (closing + 1, "closing-census"),
        (census, "census"),
        (tree - 1, "census"),
        (tree, "closing-tree"),
        (rehash, "rehash"),
        (rehash + 2, "rehash"),
        (sweep, "chain"),
        (sweep + chains - 1, "chain"),
        (sweep + chains, "root"),
    ];
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// Every guard of the success path halts the run ALONE: the step before it is made slow past the deadline,
/// and the run must end with a typed timeout AT that guard: the children that had started, the step it
/// names as last completed, the ledger exactly as a clean run leaves it, and NO later guard run (so nothing
/// further was done toward a verified result, and the root's identity was never reached).
#[test]
fn round6_each_late_validation_guard_halts_the_run_alone() {
    let clean = clean_charge();
    let shape = clean_guard_shape();
    let starts = shape.starts();
    let mut problems = Vec::new();
    for (index, label) in guard_rows(&shape) {
        let name = format!("guard #{index} `{label}`");
        let deadline = Instant::now() + ROUND6_DEADLINE_V1;
        let (ran, outcome, log) = run_guarded(
            &[],
            deadline,
            AtGuardV1::PassTheDeadline { at: index, label },
        );
        let events = log.events.borrow().clone();
        println!(
            "GUARD-MARKER {name}: found={:?} guards_run={} last={:?} returned {}ms after the deadline",
            log.injected.get(),
            events.len(),
            events.last(),
            Instant::now().saturating_duration_since(deadline).as_millis()
        );
        if log.injected.get() != Some(label) {
            problems.push(format!(
                "{name}: the fixture found {:?} at that index",
                log.injected.get()
            ));
            continue;
        }
        let (children, completed) = guard_row_expectation(index, &starts);
        problems.extend(judge(
            &name,
            &ran,
            &outcome,
            children,
            Some(completed),
            ChargeV1::Exactly(ran.phase_v + clean),
            is_timeout,
        ));
        if events.len() != index + 1 {
            problems.push(format!(
                "{name}: {} guards ran after it, and the run must halt at it",
                events.len().saturating_sub(index + 1)
            ));
        }
    }
    assert_no_problems(problems);
}

/// The root's identity stays the LAST fallible gate: with the clock NOT spent, the root replaced at the
/// closing gate's last guard (the one immediately before the identity check) is refused by that check, with
/// every proof recorded; and the same swap at the success gate's last guard is refused there. Neither is a
/// timeout, and neither is a success. (A check that moved AFTER the identity would not run first.)
#[test]
fn round6_the_root_identity_is_still_the_last_fallible_gate_after_its_guard() {
    let clean = clean_charge();
    let shape = clean_guard_shape();
    let starts = shape.starts();
    let chains = shape.chains();
    let census = starts[3] + 2;
    let tree = census + shape.census_steps();
    let mut problems = Vec::new();
    for (index, completed) in [
        (starts[1] + chains + 1, GitPlaneStepV1::Closure),
        (tree + 1 + 3 + chains, GitPlaneStepV1::Fsck),
    ] {
        let deadline = Instant::now() + ROUND6_DEADLINE_V1;
        let (ran, outcome, log) = run_guarded(
            &[],
            deadline,
            AtGuardV1::SwapTheRoot {
                at: index,
                label: "root",
            },
        );
        println!(
            "GUARD-MARKER the root swapped at guard #{index}: found={:?} guards_run={}",
            log.injected.get(),
            log.events.borrow().len()
        );
        let name = format!("the root swapped at guard #{index}");
        if log.injected.get() != Some("root") {
            problems.push(format!(
                "{name}: the fixture found {:?}",
                log.injected.get()
            ));
            continue;
        }
        problems.extend(judge(
            &name,
            &ran,
            &outcome,
            six_children(),
            Some(completed),
            ChargeV1::Exactly(ran.phase_v + clean),
            is_identity_changed,
        ));
        if log.events.borrow().len() != index + 1 {
            problems.push(format!("{name}: a guard ran after the root's guard"));
        }
    }
    assert_no_problems(problems);
}

/// The guards belong to the SUCCESS path only: a final child that FAILED is settled exactly as before (its
/// failure, its effect and its ledger, never held to the success gate), so no success-gate, rewind or closing
/// guard ever runs for it. The earlier three children's success gates are all that were observed.
#[test]
fn round6_a_failed_final_child_is_never_held_to_the_success_gate_guards() {
    let clean = clean_charge();
    let shape = clean_guard_shape();
    let group = shape.chains() + 2;
    let deadline = Instant::now() + ROUND6_DEADLINE_V1;
    let (ran, outcome, log) = run_guarded(
        &[("fsck --strict", "\"$REAL\" \"$@\"; exit 2")],
        deadline,
        AtGuardV1::Observe,
    );
    let events = log.events.borrow().clone();
    println!(
        "GUARD-MARKER the failed final child: guards_run={} (three success gates are {})",
        events.len(),
        3 * group
    );
    let problems = judge(
        "a failed final fsck",
        &ran,
        &outcome,
        six_children(),
        Some(GitPlaneStepV1::Closure),
        ChargeV1::Exactly(ran.phase_v + clean),
        |cause| is_git_child(cause, "fsck --strict", 2),
    );
    assert_no_problems(problems);
    assert_eq!(events.len(), 3 * group, "no guard ran for the failed child");
}

/// The body of the function or method that starts with `signature`, to the closing brace that has the
/// signature line's own indentation.
fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` is in the production source"));
    let line_start = source[..start].rfind('\n').map_or(0, |newline| newline + 1);
    let indent = " ".repeat(start - line_start);
    let rest = &source[start..];
    let end = rest
        .find(&format!("\n{indent}}}\n"))
        .expect("the function ends");
    &rest[..end]
}

/// STRUCTURAL ONLY (it reads the production source, so it is evidence of order and shape, never of
/// behavior; the behavioral evidence is every row above). The late-validation order is fixed in the
/// source: the admitted rehash streams under the deadline and never through the unbounded hash; the
/// closing gate is census, tree, rehash, then the sweep as its TAIL expression; the sweep's guard is
/// followed by nothing but the root's identity, which is the last fallible call, with only a tail
/// expression after it; the success arm uses the deadline gate; the runner callbacks stay unobserved;
/// and nothing fallible follows the closing gate but infallible construction.
#[test]
fn round6_the_late_validation_order_is_fixed_in_the_source() {
    let source = include_str!("custody_restore_git.rs");
    let at = |body: &str, needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("`{needle}` is in `{body}`"))
    };

    let rehash = body_of(source, "fn remeasure_admitted(");
    assert!(rehash.contains("stream_digest(") && rehash.contains("observe_deadline("));
    assert!(
        !rehash.contains("hash_prefix("),
        "the admitted rehash is never an unbounded read"
    );

    let closing = body_of(source, "fn closing_gate(");
    let order = [
        at(closing, "refuse_mounts_within("),
        at(closing, "reconcile_paced("),
        at(closing, "remeasure_admitted("),
        at(closing, "sweep_chains("),
    ];
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    assert!(
        !closing[order[3]..].contains(';'),
        "the sweep is the closing gate's tail expression"
    );

    let sweep = body_of(source, "fn sweep_chains(");
    let guard = at(sweep, "pace(\"root\")?;");
    let identity = at(sweep, "pinned_root_unchanged(");
    assert!(
        sweep[guard + "pace(\"root\")?;".len()..identity]
            .trim()
            .is_empty(),
        "the guard is the last statement before the root's identity"
    );
    assert!(
        !sweep[identity..].contains(';'),
        "the root's identity is the last fallible call: only a tail expression"
    );

    let settle = body_of(source, "fn settle_read_only<");
    assert!(settle.contains("self.deadline_gate()?") && !settle.contains("self.gate()?"));

    let gate = body_of(source, "fn gate(&self)");
    assert!(
        gate.contains("gate_parts(") && !gate.contains("observe_deadline("),
        "the runner callbacks' gate is not held to the deadline"
    );
    assert!(source.contains("match this.gate()"));

    let steps = body_of(source, "fn run_steps(");
    let tail = &steps[at(steps, "work.closing_gate()?;") + "work.closing_gate()?;".len()..];
    assert!(
        !tail.contains('?'),
        "only infallible construction follows the closing gate"
    );
    let construct = body_of(source, "fn into_result(");
    assert!(
        !construct.contains('?'),
        "the success value is built infallibly"
    );
}

// ---------------------------------------------------------------------------------------------
// Pass B1a: retained active-frame prevalidation
//
// Every row drives `prevalidate_active_frames_v1` over a REAL retained phase-V value: the rich
// capsule's two plan-produced frames, or hand-encoded frames (built with the real frame encoder) that
// stand where a sealed capsule's staged payloads would, beside a real phase-V capsule and its real
// ledger, pins, and destination. Prevalidation takes no runner, so no Git child can run; every refusal
// row also proves that the destination tree and the one ledger are exactly what they were.
//
// Pass A's controls above are unchanged. The mutation matrix in `.git/a2a-evidence/2b3b-pass-b1a/` maps
// each guard to the rows that turn red without it.
// ---------------------------------------------------------------------------------------------

type ClassV1 = CustodyCoverageClassV1;
const REFS_V1: ClassV1 = CustodyCoverageClassV1::RefsAndHead;
const LOGS_V1: ClassV1 = CustodyCoverageClassV1::StashAndReflogs;
const HEAD_BYTES_V1: &[u8] = b"ref: refs/heads/main\n";
const DIGEST_BYTES_V1: usize = 32;

/// One entry of a hand-built frame.
#[derive(Clone, Copy)]
enum EntryV1<'a> {
    Dir,
    File(&'a [u8]),
    Link(&'a [u8]),
}

type EntriesV1<'a> = [(&'a [u8], EntryV1<'a>)];

fn dir(path: &'static str) -> (&'static [u8], EntryV1<'static>) {
    (path.as_bytes(), EntryV1::Dir)
}

fn file(path: &'static str, content: &'static [u8]) -> (&'static [u8], EntryV1<'static>) {
    (path.as_bytes(), EntryV1::File(content))
}

fn link(path: &'static str, target: &'static [u8]) -> (&'static [u8], EntryV1<'static>) {
    (path.as_bytes(), EntryV1::Link(target))
}

/// A frame's bytes, from the real encoder, with the entries in canonical order whatever order they are
/// given in. A `/` or NUL cannot be in a component, but every other byte can: that is the codec's own
/// rule, so every spelling below is encodable.
fn encode_frame(class: ClassV1, generation: &str, entries: &EntriesV1<'_>) -> Vec<u8> {
    let mut ordered: Vec<(CustodyFramePathV1, EntryV1<'_>)> = entries
        .iter()
        .map(|(path, entry)| {
            (
                CustodyFramePathV1::from_bytes(path).expect("a frame path"),
                *entry,
            )
        })
        .collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let header = CustodyFrameHeaderV1::new(class, generation).expect("a frame header");
    let mut bytes = Vec::new();
    let mut encoder =
        CustodyFrameEncoderV1::new(&mut bytes, &header, frame_budget()).expect("an encoder");
    for (path, entry) in ordered {
        match entry {
            EntryV1::Dir => encoder.directory(&path, 0o755).expect("a directory"),
            EntryV1::File(content) => {
                let mut reader: &[u8] = content;
                encoder
                    .regular_file(&path, 0o644, content.len() as u64, &mut reader)
                    .expect("a regular file");
            }
            EntryV1::Link(target) => encoder.symlink(&path, target).expect("a symlink"),
        }
    }
    encoder.finish().expect("the trailer");
    bytes
}

/// `entries` plus a regular `HEAD`, unless they already name one.
fn with_head<'a>(entries: &EntriesV1<'a>) -> Vec<(&'a [u8], EntryV1<'a>)> {
    let mut all = entries.to_vec();
    if !all.iter().any(|(path, _)| *path == b"HEAD") {
        all.push((b"HEAD", EntryV1::File(HEAD_BYTES_V1)));
    }
    all
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    sha256
}

/// Stage `bytes` as the one Captured payload of `class`, through a fresh unlinked descriptor, and set
/// the manifest's row for it: exactly what phase V would hold for a capsule carrying those bytes.
fn stage_frame(verified: &mut VerifiedCapsuleV1, class: ClassV1, bytes: &[u8]) {
    use std::io::{Seek as _, Write as _};
    verified.control.manifest = manifest_with_row(class, CustodyStateClassV1::Captured, verified);
    verified
        .staged
        .retain(|one| one.role != CustodyCapsuleArtifactRoleV1::CoveragePayload(class));
    let mut file = tempfile::tempfile().expect("an unlinked staging descriptor");
    file.write_all(bytes).expect("the frame bytes");
    file.seek(SeekFrom::Start(0)).expect("rewind");
    verified.staged.push(StagedPlaintextV1 {
        name: LosslessPathV1::from_bytes(format!("test/{class:?}.frame").into_bytes()),
        role: CustodyCapsuleArtifactRoleV1::CoveragePayload(class),
        file,
        length: bytes.len() as u64,
        sha256: sha256_bytes(bytes),
    });
}

fn staged_index(verified: &VerifiedCapsuleV1, class: ClassV1) -> usize {
    verified
        .staged
        .iter()
        .position(|one| one.role == CustodyCapsuleArtifactRoleV1::CoveragePayload(class))
        .expect("the staged payload")
}

/// One real phase-V value with its destination.
struct CaseV1 {
    dest: DestV1,
    verified: VerifiedCapsuleV1,
}

impl CaseV1 {
    /// A real all-Empty phase-V capsule whose manifest and staged payloads name `frames` (class, bytes).
    fn with_bytes(capsule: &CustomCapsuleV1, frames: &[(ClassV1, Vec<u8>)]) -> Self {
        let (dest, mut verified) = phase_v(&capsule.capsule);
        for (class, bytes) in frames {
            stage_frame(&mut verified, *class, bytes);
        }
        Self { dest, verified }
    }

    fn generation(capsule: &CustomCapsuleV1) -> String {
        let (_dest, verified) = phase_v(&capsule.capsule);
        verified.control.manifest.generation_id().to_owned()
    }

    /// A refs frame (with `HEAD` added) and, if given, a logs frame, both well formed for the generation.
    fn with_entries(
        capsule: &CustomCapsuleV1,
        refs: &EntriesV1<'_>,
        logs: Option<&EntriesV1<'_>>,
    ) -> Self {
        let generation = Self::generation(capsule);
        let mut frames = vec![(
            REFS_V1,
            encode_frame(REFS_V1, &generation, &with_head(refs)),
        )];
        if let Some(logs) = logs {
            frames.push((LOGS_V1, encode_frame(LOGS_V1, &generation, logs)));
        }
        Self::with_bytes(capsule, &frames)
    }

    fn refs_only(capsule: &CustomCapsuleV1, refs: &EntriesV1<'_>) -> Self {
        Self::with_entries(capsule, refs, None)
    }

    fn run(&mut self) -> Result<ActiveFramePlanV1, GitBootstrapErrorV1> {
        prevalidate(&mut self.verified, far_deadline())
    }

    /// Runs, and requires the refusal to leave no trace: not on the ledger, not in the destination tree,
    /// and no `repository/` entry.
    fn refusal(&mut self, row: &str) -> GitBootstrapErrorV1 {
        let ledger = ledger_used(&self.verified);
        let tree = snapshot(&self.dest.path);
        let error = self.run().expect_err(row);
        assert_eq!(ledger_used(&self.verified), ledger, "{row}: the ledger");
        assert_eq!(snapshot(&self.dest.path), tree, "{row}: a byte was written");
        assert_eq!(
            self.dest.entries(),
            [".restore-work"],
            "{row}: no repository exists"
        );
        error
    }

    fn fault(&mut self, row: &str, class: ClassV1, fault: ActiveFrameFaultV1) {
        match self.refusal(row) {
            GitBootstrapErrorV1::ActiveFrame {
                class: found_class,
                fault: found,
            } => assert_eq!((found_class, found), (class, fault), "{row}"),
            other => panic!("{row}: {other:?}"),
        }
    }

    fn accepted(&mut self, row: &str) -> ActiveFramePlanV1 {
        let ledger = ledger_used(&self.verified);
        let tree = snapshot(&self.dest.path);
        let plan = self
            .run()
            .unwrap_or_else(|error| panic!("{row}: must prevalidate: {error:?}"));
        assert_eq!(ledger_used(&self.verified), ledger, "{row}: the ledger");
        assert_eq!(snapshot(&self.dest.path), tree, "{row}: a byte was written");
        assert_eq!(self.dest.entries(), [".restore-work"], "{row}");
        assert!(
            self.verified.staged.iter().all(|one| position(one) == 0),
            "{row}: every staged descriptor is back at offset 0"
        );
        plan
    }
}

fn frames_capsule() -> CustomCapsuleV1 {
    empty(CustodyGitObjectFormatV1::Sha1)
}

fn path_string(bytes: &[u8]) -> String {
    lossy(bytes)
}

/// The test's own decode of one retained frame: `(entries, directories, regular files, content bytes)`.
/// It does not use the seam, and leaves the descriptor at offset 0.
fn independent_summary(
    file: &std::fs::File,
    class: ClassV1,
    generation: &str,
) -> (u64, u64, u64, u64) {
    use std::io::Seek as _;
    let mut handle = file.try_clone().expect("a duplicate descriptor");
    handle.seek(SeekFrom::Start(0)).expect("rewind");
    let header = CustodyFrameHeaderV1::new(class, generation).expect("a header");
    let (mut entries, mut directories, mut files, mut content_bytes) = (0, 0, 0, 0);
    {
        let reader = BufReader::new(&handle);
        let mut decoder =
            CustodyFrameDecoderV1::new(reader, &header, frame_budget()).expect("a decoder");
        while let Some(entry) = decoder.next_entry().expect("a whole frame") {
            entries += 1;
            match entry {
                CustodyFrameEntryV1::Directory { .. } => directories += 1,
                CustodyFrameEntryV1::Regular {
                    length,
                    mut content,
                    ..
                } => {
                    files += 1;
                    content_bytes += length;
                    std::io::copy(&mut content, &mut std::io::sink()).expect("its content");
                }
                CustodyFrameEntryV1::Symlink { .. } => {}
            }
        }
    }
    handle.seek(SeekFrom::Start(0)).expect("rewind");
    (entries, directories, files, content_bytes)
}

fn position(one: &StagedPlaintextV1) -> u64 {
    use std::io::Seek as _;
    (&one.file).stream_position().expect("a position")
}

/// The last step a flow that ran pass A has completed: B1a is called after it.
const B1A_COMPLETED_V1: Option<GitPlaneStepV1> = Some(GitPlaneStepV1::BootstrapGate);

/// Holds a refusal to pass A's failure shape: no active materialization began, the caller's last completed
/// step is carried unchanged, and the ledger figure is the one ledger's, exactly as it was before the call.
fn require_failure_shape(
    failure: GitBootstrapFailureV1,
    completed: Option<GitPlaneStepV1>,
    ledger: u64,
) -> GitBootstrapErrorV1 {
    assert!(
        !failure.active_materialization_began,
        "prevalidation writes nothing, so no materialization began: {:?}",
        failure.cause
    );
    assert_eq!(failure.last_completed, completed, "{:?}", failure.cause);
    assert_eq!(failure.ledger_used, ledger, "{:?}", failure.cause);
    failure.cause
}

/// The real entry point after pass A, with the refusal's cause returned and its shape checked.
fn prevalidate(
    verified: &mut VerifiedCapsuleV1,
    deadline: Instant,
) -> Result<ActiveFramePlanV1, GitBootstrapErrorV1> {
    let ledger = ledger_used(verified);
    prevalidate_active_frames_v1(verified, frame_budget(), deadline, B1A_COMPLETED_V1)
        .map_err(|failure| require_failure_shape(failure, B1A_COMPLETED_V1, ledger))
}

// ---------------------------------------------------------------------------------------------
// Successful controls: the real frames, and the whole master allowlist
// ---------------------------------------------------------------------------------------------

/// The rich capsule's two real frames prevalidate. The plan names each by class and staged index with the
/// staged length and digest, and its counts equal the test's own decode. Nothing is written, no ledger
/// entry is charged, every staged descriptor is back at offset 0, and a second run returns the same plan.
#[test]
fn b1a_the_rich_capsules_real_frames_prevalidate_and_change_nothing() {
    let fixture = PlanFixtureV1::rich_clone();
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    let (dest, mut verified) = phase_v(&fixture.capsule_dir());
    let ledger = ledger_used(&verified);
    let tree = snapshot(&dest.path);

    let plan = prevalidate(&mut verified, far_deadline()).expect("the real frames prevalidate");
    // Checked before the test's own decode, which shares (and rewinds) each descriptor's offset.
    assert!(
        verified.staged.iter().all(|one| position(one) == 0),
        "every staged descriptor is back at offset 0"
    );
    let generation = verified.control.manifest.generation_id().to_owned();
    assert_eq!(
        plan.frames
            .iter()
            .map(|frame| frame.class)
            .collect::<Vec<_>>(),
        [REFS_V1, LOGS_V1]
    );
    for frame in &plan.frames {
        let staged = &verified.staged[frame.staged];
        assert_eq!(
            staged.role,
            CustodyCapsuleArtifactRoleV1::CoveragePayload(frame.class)
        );
        assert_eq!((frame.length, frame.sha256), (staged.length, staged.sha256));
        let (entries, directories, files, content_bytes) =
            independent_summary(&staged.file, frame.class, &generation);
        assert_eq!(
            (
                frame.entries,
                frame.directories,
                frame.regular_files,
                frame.content_bytes
            ),
            (entries, directories, files, content_bytes),
            "{:?}",
            frame.class
        );
        assert!(frame.entries > 1 && frame.regular_files > 0);
        assert_eq!(frame.entries, frame.directories + frame.regular_files);
    }
    assert_eq!(ledger_used(&verified), ledger);
    assert_eq!(snapshot(&dest.path), tree);
    assert_eq!(dest.entries(), [".restore-work"]);

    let again = prevalidate(&mut verified, far_deadline()).expect("a second run");
    assert_eq!(again, plan, "the same frames give the same plan");
}

/// Every exactly-allowed name in its actual class is admitted, in both frames, with the counts the test
/// knows, and the plan's retained facts bind the frames.
#[test]
fn b1a_the_master_allowlist_is_admitted_in_its_actual_classes() {
    let capsule = frames_capsule();
    let refs: &[(&[u8], EntryV1<'_>)] = &[
        (b"HEAD", EntryV1::File(HEAD_BYTES_V1)),
        (
            b"ORIG_HEAD",
            EntryV1::File(b"0123456789012345678901234567890123456789\n"),
        ),
        (b"FETCH_HEAD", EntryV1::File(b"fetched\n")),
        (b"packed-refs", EntryV1::File(b"# pack-refs with: peeled\n")),
        (b"refs", EntryV1::Dir),
        (b"refs/heads", EntryV1::Dir),
        (b"refs/heads/feature", EntryV1::Dir),
        (b"refs/heads/feature/x", EntryV1::File(b"oid\n")),
        (b"refs/heads/main", EntryV1::File(b"oid\n")),
        (b"refs/tags", EntryV1::Dir),
        (b"refs/tags/v1.0", EntryV1::File(b"oid\n")),
        (b"refs/stash", EntryV1::File(b"oid\n")),
    ];
    let logs: &[(&[u8], EntryV1<'_>)] = &[
        (b"logs", EntryV1::Dir),
        (b"logs/HEAD", EntryV1::File(b"log\n")),
        (b"logs/refs", EntryV1::Dir),
        (b"logs/refs/heads", EntryV1::Dir),
        (b"logs/refs/heads/main", EntryV1::File(b"log\n")),
        (b"logs/refs/stash", EntryV1::File(b"log\n")),
    ];
    let mut case = CaseV1::with_entries(&capsule, refs, Some(logs));
    let plan = case.accepted("the whole allowlist");
    assert_eq!(plan.frames.len(), 2);
    let refs_frame = &plan.frames[0];
    assert_eq!(
        (
            refs_frame.class,
            refs_frame.entries,
            refs_frame.directories,
            refs_frame.regular_files
        ),
        (REFS_V1, 12, 4, 8)
    );
    assert_eq!(
        refs_frame.content_bytes,
        (HEAD_BYTES_V1.len() + 41 + 8 + 25 + 4 * 4) as u64
    );
    let logs_frame = &plan.frames[1];
    assert_eq!(
        (
            logs_frame.class,
            logs_frame.entries,
            logs_frame.directories,
            logs_frame.regular_files,
            logs_frame.content_bytes
        ),
        (LOGS_V1, 6, 3, 3, 4 * 3)
    );
    for frame in &plan.frames {
        let staged = &case.verified.staged[frame.staged];
        assert_eq!((frame.length, frame.sha256), (staged.length, staged.sha256));
    }

    // A refs frame that is only a `HEAD` (an unborn repository) is whole; B1b owns its grammar.
    let mut only_head = CaseV1::refs_only(&capsule, &[]);
    let plan = only_head.accepted("HEAD alone");
    assert_eq!(
        (
            plan.frames.len(),
            plan.frames[0].entries,
            plan.frames[0].regular_files
        ),
        (1, 1, 1)
    );
}

/// Pass A still accepts the existing all-Empty capsule for both caller formats and reports no active
/// frame; complete G's first rule then refuses it with the typed Empty-refs refusal. The refusal leaves the
/// original ledger, every pin, and the destination exactly as Pass A left them, and creates no repository.
#[test]
fn b1a_an_all_empty_capsule_is_accepted_by_pass_a_and_refused_by_the_complete_g_rule() {
    for format in [
        CustodyGitObjectFormatV1::Sha1,
        CustodyGitObjectFormatV1::Sha256,
    ] {
        let capsule = empty(format);
        let (dest, verified) = phase_v(&capsule.capsule);
        let mut accepted = must_bootstrap(verified, core_format(format));
        assert!(accepted.active_frames.is_empty(), "{format:?}");
        assert_eq!(
            accepted.provenance,
            GitFormatProvenanceV1::CallerSelectedEmpty
        );

        let ledger = ledger_used(&accepted.verified);
        let tree = snapshot(&dest.path);
        let pins: Vec<String> = accepted.directories.keys().cloned().collect();
        let error = prevalidate(&mut accepted.verified, far_deadline())
            .expect_err("complete G refuses Empty refs");
        assert!(
            matches!(error, GitBootstrapErrorV1::EmptyRefsAndHead),
            "{format:?}: {error:?}"
        );
        assert_eq!(ledger_used(&accepted.verified), ledger, "{format:?}");
        assert_eq!(snapshot(&dest.path), tree, "{format:?}: a byte was written");
        assert_eq!(
            dest.entries(),
            [".restore-work"],
            "{format:?}: no repository"
        );
        assert_eq!(
            accepted.directories.keys().cloned().collect::<Vec<_>>(),
            pins,
            "{format:?}: the pins are retained"
        );

        // The rule is independent of Pass A: the same refusal on a fresh phase-V value.
        let (dest, mut verified) = phase_v(&capsule.capsule);
        let ledger = ledger_used(&verified);
        let error = prevalidate(&mut verified, far_deadline())
            .expect_err("Empty refs refuse without Pass A");
        assert!(
            matches!(error, GitBootstrapErrorV1::EmptyRefsAndHead),
            "{error:?}"
        );
        assert_eq!(ledger_used(&verified), ledger);
        assert_eq!(dest.entries(), [".restore-work"]);
        assert_eq!(dest.work_entries(), ["plain"]);
    }
}

/// Empty refs refuse even beside a Captured reflog frame, and before ANY frame is read.
#[test]
fn b1a_empty_refs_refuse_before_any_frame_is_read() {
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let logs = encode_frame(LOGS_V1, &generation, &[(b"logs", EntryV1::Dir)]);
    let mut case = CaseV1::with_bytes(&capsule, &[(LOGS_V1, logs)]);
    let labels = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
    let seen = labels.clone();
    let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        seen.borrow_mut().push(event.label);
    });
    let error = case.refusal("Empty refs beside a Captured reflog frame");
    assert!(
        matches!(error, GitBootstrapErrorV1::EmptyRefsAndHead),
        "{error:?}"
    );
    assert!(
        !labels
            .borrow()
            .iter()
            .any(|label| label.starts_with("frame-")),
        "no frame was rehashed or decoded: {:?}",
        labels.borrow()
    );
}

/// Impossible and unsupported coverage states refuse through the B1a entry as they do through Pass A,
/// for both active classes, and are never downgraded to Empty.
#[test]
fn b1a_active_coverage_states_that_cannot_be_restored_refuse() {
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    for class in [REFS_V1, LOGS_V1] {
        let frame = || encode_frame(class, &generation, &with_head(&[]));
        // Captured with no staged payload.
        let (dest, mut verified) = phase_v(&capsule.capsule);
        verified.control.manifest =
            manifest_with_row(class, CustodyStateClassV1::Captured, &verified);
        let error = prevalidate(&mut verified, far_deadline())
            .expect_err("a Captured class without its payload");
        assert!(
            matches!(&error, GitBootstrapErrorV1::UnsupportedCoverage { class: found, detail }
                if *found == class && detail.contains("exactly one")),
            "{class:?}: {error:?}"
        );
        assert_eq!(dest.entries(), [".restore-work"]);

        // Captured with two staged payloads.
        let mut case = CaseV1::with_bytes(&capsule, &[(class, frame())]);
        let twin = StagedPlaintextV1 {
            name: LosslessPathV1::from_bytes(b"test/twin".to_vec()),
            role: CustodyCapsuleArtifactRoleV1::CoveragePayload(class),
            file: case.verified.staged[staged_index(&case.verified, class)]
                .file
                .try_clone()
                .expect("a duplicate descriptor"),
            length: 1,
            sha256: [0; 32],
        };
        case.verified.staged.push(twin);
        let error = case.refusal("two payloads");
        assert!(
            matches!(&error, GitBootstrapErrorV1::UnsupportedCoverage { class: found, detail }
                if *found == class && detail.contains("exactly one")),
            "{class:?}: {error:?}"
        );

        // Empty with a staged payload (the refs class is Captured so only this class is wrong).
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, with_head_frame(&generation))]);
        if class == LOGS_V1 {
            stage_frame(&mut case.verified, LOGS_V1, &frame());
            case.verified.control.manifest =
                manifest_with_row(LOGS_V1, CustodyStateClassV1::Empty, &case.verified);
        } else {
            case.verified.control.manifest =
                manifest_with_row(REFS_V1, CustodyStateClassV1::Empty, &case.verified);
        }
        let error = case.refusal("Empty with a payload");
        assert!(
            matches!(&error, GitBootstrapErrorV1::UnsupportedCoverage { class: found, detail }
                if *found == class && detail.contains("Empty active class")),
            "{class:?}: {error:?}"
        );

        // Neither Captured nor Empty.
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, with_head_frame(&generation))]);
        case.verified.control.manifest =
            manifest_with_row(class, CustodyStateClassV1::Unresolved, &case.verified);
        let error = case.refusal("Unresolved");
        assert!(
            matches!(&error, GitBootstrapErrorV1::UnsupportedCoverage { class: found, .. }
                if *found == class),
            "{class:?}: {error:?}"
        );
    }
}

fn with_head_frame(generation: &str) -> Vec<u8> {
    encode_frame(REFS_V1, generation, &with_head(&[]))
}

// ---------------------------------------------------------------------------------------------
// Exactly one HEAD, the exact allowlist, class ownership, types, symlinks
// ---------------------------------------------------------------------------------------------

/// A Captured refs class holds exactly one `HEAD` regular file. Absent, a directory, or a symlink
/// refuses, with no repository and no byte written, and nothing is synthesized or copied from the
/// bootstrap.
#[test]
fn b1a_captured_refs_need_exactly_one_head_regular_file() {
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let refs_frame = |entries: &EntriesV1<'_>| encode_frame(REFS_V1, &generation, entries);

    let none_at_all = refs_frame(&[]);
    let no_head = refs_frame(&[(b"refs", EntryV1::Dir), (b"refs/heads", EntryV1::Dir)]);
    let only_orig = refs_frame(&[(b"ORIG_HEAD", EntryV1::File(b"x\n"))]);
    for (row, bytes) in [
        ("an empty frame", none_at_all),
        ("refs without a HEAD", no_head),
        ("ORIG_HEAD is not HEAD", only_orig),
    ] {
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, bytes)]);
        case.fault(row, REFS_V1, ActiveFrameFaultV1::MissingHead);
    }

    let mut case = CaseV1::with_bytes(
        &capsule,
        &[(REFS_V1, refs_frame(&[(b"HEAD", EntryV1::Dir)]))],
    );
    case.fault(
        "HEAD as a directory",
        REFS_V1,
        ActiveFrameFaultV1::WrongType {
            path: "HEAD".into(),
            expected: ActiveEntryKindV1::Regular,
        },
    );

    let mut case = CaseV1::with_bytes(
        &capsule,
        &[(
            REFS_V1,
            refs_frame(&[(b"HEAD", EntryV1::Link(b"refs/heads/main"))]),
        )],
    );
    case.fault(
        "HEAD as a symlink",
        REFS_V1,
        ActiveFrameFaultV1::Symlink {
            path: "HEAD".into(),
        },
    );

    // The control: a HEAD regular file is exactly what is needed, whatever it says.
    let mut case = CaseV1::with_bytes(
        &capsule,
        &[(REFS_V1, refs_frame(&[(b"HEAD", EntryV1::File(b""))]))],
    );
    case.accepted("an empty HEAD regular file is a B1b grammar question, not B1a's");
}

/// The exact master allowlist and nothing else: a name another class owns, an unsupported name, a lock
/// name, or an unknown name refuses. Each row is a frame whose `HEAD` is fine, so only the named entry
/// can be the cause.
#[test]
fn b1a_only_the_exact_allowlist_is_admitted_per_class() {
    let capsule = frames_capsule();
    let foreign = |path: &str, owner: ClassV1| ActiveFrameFaultV1::ForeignClass {
        path: path.into(),
        owner,
    };
    let unsupported = |path: &str| ActiveFrameFaultV1::Unsupported { path: path.into() };
    let config = ClassV1::GitConfigurationAndHooks;
    let operation = ClassV1::InProgressGitOperations;
    type RowV1 = (
        &'static str,
        Vec<(&'static [u8], EntryV1<'static>)>,
        ActiveFrameFaultV1,
    );
    let refs_rows: Vec<RowV1> = vec![
        (
            "config",
            vec![file("config", b"[core]\n")],
            foreign("config", config),
        ),
        (
            "hooks",
            vec![dir("hooks"), file("hooks/pre-commit", b"#!\n")],
            foreign("hooks", config),
        ),
        ("info", vec![dir("info")], foreign("info", config)),
        (
            "objects",
            vec![dir("objects")],
            foreign("objects", ClassV1::ObjectDatabase),
        ),
        (
            "an alternates file",
            vec![
                dir("objects"),
                dir("objects/info"),
                file("objects/info/alternates", b"/elsewhere\n"),
            ],
            foreign("objects", ClassV1::ObjectDatabase),
        ),
        (
            "MERGE_HEAD",
            vec![file("MERGE_HEAD", b"x\n")],
            foreign("MERGE_HEAD", operation),
        ),
        (
            "rebase-merge",
            vec![dir("rebase-merge")],
            foreign("rebase-merge", operation),
        ),
        (
            "CHERRY_PICK_HEAD",
            vec![file("CHERRY_PICK_HEAD", b"x\n")],
            foreign("CHERRY_PICK_HEAD", operation),
        ),
        (
            "BISECT_LOG",
            vec![file("BISECT_LOG", b"x\n")],
            foreign("BISECT_LOG", operation),
        ),
        (
            "index",
            vec![file("index", b"x")],
            foreign("index", ClassV1::Index),
        ),
        (
            "bridge evidence",
            vec![dir("a2a-bridge")],
            foreign("a2a-bridge", ClassV1::BridgeEvidence),
        ),
        (
            "logs inside the refs frame",
            vec![dir("logs")],
            foreign("logs", LOGS_V1),
        ),
        (
            "shallow",
            vec![file("shallow", b"x\n")],
            unsupported("shallow"),
        ),
        (
            "a lock file",
            vec![file("HEAD.lock", b"")],
            unsupported("HEAD.lock"),
        ),
        (
            "an unknown name",
            vec![file("surprise", b"")],
            unsupported("surprise"),
        ),
        (
            "linked worktrees",
            vec![dir("worktrees")],
            foreign("worktrees", ClassV1::LinkedWorktrees),
        ),
    ];
    for (row, entries, fault) in refs_rows {
        let mut case = CaseV1::refs_only(&capsule, &entries);
        case.fault(row, REFS_V1, fault);
    }

    let logs_rows: Vec<RowV1> = vec![
        (
            "HEAD inside the logs frame",
            vec![file("HEAD", b"x\n")],
            foreign("HEAD", REFS_V1),
        ),
        (
            "refs inside the logs frame",
            vec![dir("refs")],
            foreign("refs", REFS_V1),
        ),
        (
            "packed-refs inside the logs frame",
            vec![file("packed-refs", b"")],
            foreign("packed-refs", REFS_V1),
        ),
        (
            "config inside the logs frame",
            vec![file("config", b"")],
            foreign("config", config),
        ),
        (
            "shallow inside the logs frame",
            vec![file("shallow", b"")],
            unsupported("shallow"),
        ),
        (
            "an unknown name inside the logs frame",
            vec![dir("surprise")],
            unsupported("surprise"),
        ),
    ];
    for (row, entries, fault) in logs_rows {
        let mut case = CaseV1::with_entries(&capsule, &[], Some(&entries));
        case.fault(row, LOGS_V1, fault);
    }
}

/// Each top-level allowlisted name has its own entry type: files are regular, `refs` and `logs` are
/// directories.
#[test]
fn b1a_top_level_names_must_have_their_class_type() {
    let capsule = frames_capsule();
    let wrong = |path: &str, expected| ActiveFrameFaultV1::WrongType {
        path: path.into(),
        expected,
    };
    for name in ["ORIG_HEAD", "FETCH_HEAD", "packed-refs"] {
        let mut case = CaseV1::refs_only(&capsule, &[(name.as_bytes(), EntryV1::Dir)]);
        case.fault(name, REFS_V1, wrong(name, ActiveEntryKindV1::Regular));
    }
    let mut case = CaseV1::refs_only(&capsule, &[(b"refs", EntryV1::File(b"x"))]);
    case.fault(
        "refs as a regular file",
        REFS_V1,
        wrong("refs", ActiveEntryKindV1::Directory),
    );
    let mut case = CaseV1::with_entries(&capsule, &[], Some(&[(b"logs", EntryV1::File(b"x"))]));
    case.fault(
        "logs as a regular file",
        LOGS_V1,
        wrong("logs", ActiveEntryKindV1::Directory),
    );
}

/// A symlink is refused wherever the frame carries one: at a top-level name, under `refs/`, and under
/// `logs/`. A valid frame (its trailer verifies) holding a `refs` symlink refuses before any write.
#[test]
fn b1a_symlinks_are_refused_wherever_the_frame_carries_them() {
    let capsule = frames_capsule();
    let link = |path: &str| ActiveFrameFaultV1::Symlink { path: path.into() };
    let mut case = CaseV1::refs_only(&capsule, &[(b"refs", EntryV1::Link(b"/elsewhere"))]);
    case.fault("a refs symlink", REFS_V1, link("refs"));
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/main", EntryV1::Link(b"../../elsewhere")),
        ],
    );
    case.fault("a link beneath refs", REFS_V1, link("refs/heads/main"));
    let mut case = CaseV1::refs_only(&capsule, &[(b"packed-refs", EntryV1::Link(b"x"))]);
    case.fault("a packed-refs symlink", REFS_V1, link("packed-refs"));
    let mut case = CaseV1::with_entries(
        &capsule,
        &[],
        Some(&[
            (b"logs", EntryV1::Dir),
            (b"logs/HEAD", EntryV1::Link(b"/etc/passwd")),
        ]),
    );
    case.fault("a link beneath logs", LOGS_V1, link("logs/HEAD"));
    // A symlink under a name no class owns is still a symlink first.
    let mut case = CaseV1::refs_only(&capsule, &[(b"config", EntryV1::Link(b"x"))]);
    case.fault("a config symlink", REFS_V1, link("config"));
}

// ---------------------------------------------------------------------------------------------
// Portable spelling and collisions
// ---------------------------------------------------------------------------------------------

/// The portable component rule: printable ASCII `0x21..=0x7e`, no backslash, no trailing dot. Every
/// other spelling refuses with its own reason, never normalized: NFC and NFD, a lone invalid byte,
/// a backslash, a trailing dot or space, a control byte, an interior space, DEL.
#[test]
fn b1a_the_portable_spelling_rule_refuses_every_other_component() {
    let capsule = frames_capsule();
    let nfc: &[u8] = "caf\u{e9}".as_bytes();
    let nfd: &[u8] = "cafe\u{301}".as_bytes();
    assert_ne!(nfc, nfd);
    let refused: Vec<(&str, &[u8], PortableFaultV1)> = vec![
        ("NFC", nfc, PortableFaultV1::NonAscii),
        ("NFD", nfd, PortableFaultV1::NonAscii),
        ("a lone 0xff", b"a\xff", PortableFaultV1::NonAscii),
        ("a truncated sequence", b"a\xc3", PortableFaultV1::NonAscii),
        ("a backslash", b"a\\b", PortableFaultV1::Backslash),
        ("a leading backslash", b"\\a", PortableFaultV1::Backslash),
        ("a trailing dot", b"a.", PortableFaultV1::TrailingDot),
        ("two trailing dots", b"a..", PortableFaultV1::TrailingDot),
        ("a trailing space", b"a ", PortableFaultV1::Unprintable),
        ("an interior space", b"a b", PortableFaultV1::Unprintable),
        ("a tab", b"a\tb", PortableFaultV1::Unprintable),
        ("a control byte", b"a\x01b", PortableFaultV1::Unprintable),
        ("DEL", b"a\x7fb", PortableFaultV1::Unprintable),
    ];
    for (row, name, reason) in refused {
        let mut path = b"refs/heads/".to_vec();
        path.extend_from_slice(name);
        let mut case = CaseV1::refs_only(
            &capsule,
            &[
                (b"refs", EntryV1::Dir),
                (b"refs/heads", EntryV1::Dir),
                (&path, EntryV1::File(b"oid\n")),
            ],
        );
        case.fault(
            row,
            REFS_V1,
            ActiveFrameFaultV1::NonPortable {
                path: path_string(&path),
                reason,
            },
        );
    }
    // The same rule governs the logs class and every component, not only the last.
    let mut case = CaseV1::with_entries(
        &capsule,
        &[],
        Some(&[(b"logs", EntryV1::Dir), (b"logs/a ", EntryV1::Dir)]),
    );
    case.fault(
        "a logs component",
        LOGS_V1,
        ActiveFrameFaultV1::NonPortable {
            path: "logs/a ".into(),
            reason: PortableFaultV1::Unprintable,
        },
    );
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/d.", EntryV1::Dir),
            (b"refs/d./x", EntryV1::File(b"oid\n")),
        ],
    );
    case.fault(
        "a directory component",
        REFS_V1,
        ActiveFrameFaultV1::NonPortable {
            path: "refs/d.".into(),
            reason: PortableFaultV1::TrailingDot,
        },
    );

    // Controls: every other admitted byte, including punctuation and an interior or leading dot.
    for name in [
        "a.b", ".hidden", "a-b_c", "x~1", "A", "0", "a@b", "a{b}", "~",
    ] {
        let mut path = b"refs/heads/".to_vec();
        path.extend_from_slice(name.as_bytes());
        let mut case = CaseV1::refs_only(
            &capsule,
            &[
                (b"refs", EntryV1::Dir),
                (b"refs/heads", EntryV1::Dir),
                (&path, EntryV1::File(b"oid\n")),
            ],
        );
        let _ = case.accepted(name);
    }
}

/// Two distinct spellings that share the ASCII-lowercased key refuse, in either order and at any depth,
/// in either class. The key lowercases only `A..Z`: `@` and the backtick, which differ by 0x20 outside
/// the letters, stay distinct, and so do `a/b` and `ab`.
#[test]
fn b1a_ascii_case_collisions_refuse_and_distinct_spellings_do_not() {
    let capsule = frames_capsule();
    let collision = |path: &str, with: &str| ActiveFrameFaultV1::Collision {
        path: path.into(),
        with: with.into(),
    };
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/A", EntryV1::File(b"1\n")),
            (b"refs/heads/a", EntryV1::File(b"2\n")),
        ],
    );
    // Canonical order puts `A` (0x41) before `a` (0x61).
    case.fault(
        "A/a files",
        REFS_V1,
        collision("refs/heads/a", "refs/heads/A"),
    );
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/Heads", EntryV1::Dir),
            (b"refs/Heads/x", EntryV1::File(b"1\n")),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/x", EntryV1::File(b"2\n")),
        ],
    );
    case.fault(
        "case-differing directories",
        REFS_V1,
        collision("refs/heads", "refs/Heads"),
    );
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/tags", EntryV1::Dir),
            (b"refs/tags/V1", EntryV1::File(b"1\n")),
            (b"refs/tags/v1", EntryV1::File(b"2\n")),
        ],
    );
    case.fault("V1/v1", REFS_V1, collision("refs/tags/v1", "refs/tags/V1"));
    let mut case = CaseV1::with_entries(
        &capsule,
        &[],
        Some(&[
            (b"logs", EntryV1::Dir),
            (b"logs/refs", EntryV1::Dir),
            (b"logs/refs/Main", EntryV1::File(b"1\n")),
            (b"logs/refs/main", EntryV1::File(b"2\n")),
        ]),
    );
    case.fault(
        "reflog names",
        LOGS_V1,
        collision("logs/refs/main", "logs/refs/Main"),
    );
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/MiXeD", EntryV1::File(b"1\n")),
            (b"refs/heads/mixed", EntryV1::File(b"2\n")),
        ],
    );
    case.fault(
        "mixed case",
        REFS_V1,
        collision("refs/heads/mixed", "refs/heads/MiXeD"),
    );

    // Distinct spellings, and bytes that differ by 0x20 outside the letters, are different keys.
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/a", EntryV1::File(b"1\n")),
            (b"refs/heads/b", EntryV1::File(b"2\n")),
            (b"refs/heads/@", EntryV1::File(b"3\n")),
            (b"refs/heads/`", EntryV1::File(b"4\n")),
            (b"refs/heads/[", EntryV1::File(b"5\n")),
            (b"refs/heads/{", EntryV1::File(b"6\n")),
            (b"refs/heads/ab", EntryV1::File(b"7\n")),
            (b"refs/heads/a.b", EntryV1::File(b"8\n")),
            (b"refs/heads/a_b", EntryV1::File(b"9\n")),
            (b"refs/heads/a-b", EntryV1::File(b"0\n")),
        ],
    );
    let plan = case.accepted("distinct spellings");
    assert_eq!(
        plan.frames[0].regular_files,
        10 + 1,
        "ten branches and HEAD"
    );
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/a", EntryV1::Dir),
            (b"refs/a/b", EntryV1::File(b"1\n")),
            (b"refs/ab", EntryV1::File(b"2\n")),
        ],
    );
    let _ = case.accepted("component boundaries are part of the key");
}

// ---------------------------------------------------------------------------------------------
// The aggregate table, driven directly (the one table every frame of a restore feeds)
// ---------------------------------------------------------------------------------------------

/// The aggregate path table refuses what only the aggregate can see: the same path twice, a spelling
/// collision across entries that did not come from one frame, a parent that is absent, a regular file, or
/// spelled differently, an inactive class, and an overrun of its own bound; and its `finish` requires a
/// regular `HEAD`.
#[test]
fn b1a_the_aggregate_table_refuses_what_only_the_aggregate_sees() {
    use ActiveEntryKindV1::{Directory, Regular, Symlink};
    let bound = 1 << 20;
    let admit = |table: &mut ActivePathTableV1, class, path: &str, kind| {
        table.admit(class, path.as_bytes(), kind)
    };

    let mut table = ActivePathTableV1::new(bound);
    admit(&mut table, REFS_V1, "refs", Directory).expect("refs");
    assert_eq!(
        admit(&mut table, REFS_V1, "refs", Directory),
        Err(ActiveFrameFaultV1::Duplicate {
            path: "refs".into()
        }),
        "the same path from a second frame"
    );
    admit(&mut table, REFS_V1, "refs/heads", Directory).expect("refs/heads");
    admit(&mut table, REFS_V1, "refs/heads/A", Regular).expect("refs/heads/A");
    assert_eq!(
        admit(&mut table, REFS_V1, "refs/heads/a", Regular),
        Err(ActiveFrameFaultV1::Collision {
            path: "refs/heads/a".into(),
            with: "refs/heads/A".into()
        }),
        "a collision with an entry of an earlier frame"
    );
    assert_eq!(
        admit(&mut table, REFS_V1, "refs/tags/x", Regular),
        Err(ActiveFrameFaultV1::Conflict {
            path: "refs/tags/x".into()
        }),
        "an absent parent"
    );
    admit(&mut table, REFS_V1, "packed-refs", Regular).expect("packed-refs");
    assert_eq!(
        admit(&mut table, REFS_V1, "packed-refs/x", Regular),
        Err(ActiveFrameFaultV1::Conflict {
            path: "packed-refs/x".into()
        }),
        "a regular file is not a parent"
    );
    assert_eq!(
        admit(&mut table, REFS_V1, "refs/Heads/x", Regular),
        Err(ActiveFrameFaultV1::Conflict {
            path: "refs/Heads/x".into()
        }),
        "a parent spelled differently is not the parent"
    );
    assert_eq!(
        admit(&mut table, REFS_V1, "refs/heads/s", Symlink),
        Err(ActiveFrameFaultV1::Symlink {
            path: "refs/heads/s".into()
        })
    );
    assert_eq!(
        admit(&mut table, ClassV1::ObjectDatabase, "objects", Directory),
        Err(ActiveFrameFaultV1::InactiveClass {
            class: ClassV1::ObjectDatabase
        })
    );
    // The refusals admitted nothing: the earlier entries are the whole table, and there is no HEAD.
    assert_eq!(table.entries(), 4);
    assert_eq!(table.finish(), Err(ActiveFrameFaultV1::MissingHead));
    admit(&mut table, REFS_V1, "HEAD", Regular).expect("HEAD");
    assert_eq!(table.finish(), Ok(()));

    // A HEAD from the wrong class is not the refs class's HEAD.
    let mut table = ActivePathTableV1::new(bound);
    assert_eq!(
        admit(&mut table, LOGS_V1, "HEAD", Regular),
        Err(ActiveFrameFaultV1::ForeignClass {
            path: "HEAD".into(),
            owner: REFS_V1
        })
    );
    assert_eq!(table.finish(), Err(ActiveFrameFaultV1::MissingHead));

    // Its own bound: 4 entries' keys and spellings do not fit in a handful of bytes.
    let mut table = ActivePathTableV1::new(64);
    let mut refused = None;
    for path in ["HEAD", "ORIG_HEAD", "FETCH_HEAD", "packed-refs"] {
        if let Err(fault) = admit(&mut table, REFS_V1, path, Regular) {
            refused = Some(fault);
            break;
        }
    }
    assert_eq!(
        refused,
        Some(ActiveFrameFaultV1::PathTableBound { bound: 64 })
    );
}

/// The bound is a parameter of the entry point, so its refusal is exercised at a small size through the
/// whole decode: a frame whose paths do not fit refuses with the table bound, after the frame verified.
#[test]
fn b1a_the_path_table_bound_refuses_through_the_whole_decode() {
    let capsule = frames_capsule();
    let mut entries: Vec<(Vec<u8>, EntryV1<'static>)> = vec![
        (b"refs".to_vec(), EntryV1::Dir),
        (b"refs/heads".to_vec(), EntryV1::Dir),
    ];
    for index in 0..64 {
        entries.push((
            format!("refs/heads/branch-{index:03}").into_bytes(),
            EntryV1::File(b"oid\n"),
        ));
    }
    let borrowed: Vec<(&[u8], EntryV1<'_>)> = entries
        .iter()
        .map(|(path, entry)| (path.as_slice(), *entry))
        .collect();
    let mut case = CaseV1::refs_only(&capsule, &borrowed);
    // The default bound holds the frame.
    let _ = case.accepted("the default bound");
    let ledger = ledger_used(&case.verified);
    let failure = prevalidate_active_frames_bounded_v1(
        &mut case.verified,
        frame_budget(),
        far_deadline(),
        B1A_COMPLETED_V1,
        1024,
    )
    .expect_err("a small bound refuses");
    let error = require_failure_shape(failure, B1A_COMPLETED_V1, ledger);
    assert!(
        matches!(
            error,
            GitBootstrapErrorV1::ActiveFrame {
                class: REFS_V1,
                fault: ActiveFrameFaultV1::PathTableBound { bound: 1024 }
            }
        ),
        "{error:?}"
    );
    assert_eq!(ledger_used(&case.verified), ledger);
}

// ---------------------------------------------------------------------------------------------
// Complete decode: the trailer, the header, and the staged bytes
// ---------------------------------------------------------------------------------------------

fn flip(bytes: &mut [u8], index: usize) {
    bytes[index] ^= 0xff;
}

/// A whole frame is decoded through its verified trailer before anything is judged final. Every damaged
/// trailer, content digest, header binding, or trailing byte refuses with `FrameInvalid`, even though the
/// provisional entries before the damage were fine, and even when an entry fault came first: the frame's
/// integrity outranks what it says. Controls: the same frames undamaged succeed.
#[test]
fn b1a_a_failed_trailer_after_provisional_entries_refuses_the_whole_frame() {
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let good_entries: &[(&[u8], EntryV1<'_>)] = &[
        (b"refs", EntryV1::Dir),
        (b"refs/heads", EntryV1::Dir),
        (b"refs/heads/main", EntryV1::File(b"oid\n")),
    ];
    let good = encode_frame(REFS_V1, &generation, &with_head(good_entries));
    let invalid = |class| {
        move |error: &GitBootstrapErrorV1| {
            matches!(
                error,
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { class: found })
                    if *found == class
            )
        }
    };

    let mut damaged: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut flipped_digest = good.clone();
    flip(&mut flipped_digest, good.len() - 1);
    damaged.push(("the frame digest's last byte", flipped_digest));
    let mut flipped_count = good.clone();
    flip(&mut flipped_count, good.len() - (DIGEST_BYTES_V1 + 8 + 8));
    damaged.push(("the trailer's entry count", flipped_count));
    let mut flipped_total = good.clone();
    flip(&mut flipped_total, good.len() - (DIGEST_BYTES_V1 + 8));
    damaged.push(("the trailer's content total", flipped_total));
    let mut flipped_tag = good.clone();
    flip(&mut flipped_tag, good.len() - (DIGEST_BYTES_V1 + 8 + 8 + 1));
    damaged.push(("the trailer's tag", flipped_tag));
    damaged.push(("a truncated trailer", good[..good.len() - 8].to_vec()));
    damaged.push(("no trailer at all", good[..good.len() - 49].to_vec()));
    let mut trailing = good.clone();
    trailing.push(0);
    damaged.push(("a byte after the trailer", trailing));
    let content_at = good
        .windows(HEAD_BYTES_V1.len())
        .position(|window| window == HEAD_BYTES_V1)
        .expect("the HEAD content");
    let mut flipped_content = good.clone();
    flip(&mut flipped_content, content_at);
    damaged.push(("a HEAD content byte", flipped_content));
    damaged.push((
        "another generation's header",
        encode_frame(REFS_V1, "another-generation", &with_head(good_entries)),
    ));
    damaged.push((
        "another class's header",
        encode_frame(LOGS_V1, &generation, &with_head(good_entries)),
    ));
    let mut with_link = encode_frame(
        REFS_V1,
        &generation,
        &with_head(&[(b"refs", EntryV1::Dir), (b"refs/l", EntryV1::Link(b"x"))]),
    );
    let last = with_link.len() - 1;
    flip(&mut with_link, last);
    damaged.push(("an entry fault and then a bad trailer", with_link));
    for (row, bytes) in damaged {
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, bytes)]);
        let error = case.refusal(row);
        assert!(invalid(REFS_V1)(&error), "{row}: {error:?}");
    }

    // The logs class is whole-decoded too.
    let logs_good = encode_frame(
        LOGS_V1,
        &generation,
        &[
            (b"logs", EntryV1::Dir),
            (b"logs/HEAD", EntryV1::File(b"log\n")),
        ],
    );
    let mut bad_logs = logs_good.clone();
    flip(&mut bad_logs, logs_good.len() - 1);
    let refs_ok = encode_frame(REFS_V1, &generation, &with_head(&[]));
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, refs_ok.clone()), (LOGS_V1, bad_logs)]);
    let error = case.refusal("a bad logs trailer");
    assert!(invalid(LOGS_V1)(&error), "{error:?}");

    // Controls: the undamaged frames.
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, good.clone()), (LOGS_V1, logs_good)]);
    let plan = case.accepted("undamaged frames");
    assert_eq!(plan.frames.len(), 2);
}

/// Staged bytes or length that differ from the verified plaintext refuse before decoding, and name the
/// staged frame: a changed byte, an appended byte, a truncation, an emptied file. Either class.
#[test]
fn b1a_staged_frame_byte_or_length_drift_refuses() {
    use std::os::unix::fs::FileExt as _;
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let refs = encode_frame(REFS_V1, &generation, &with_head(&[]));
    let logs = encode_frame(LOGS_V1, &generation, &[(b"logs", EntryV1::Dir)]);
    type Edit = fn(&std::fs::File);
    let edits: [(&str, Edit); 4] = [
        ("a byte changed", |file| {
            file.write_all_at(&[0xff], 12).expect("change a byte");
        }),
        ("a byte appended", |file| {
            let length = file.metadata().expect("metadata").len();
            file.write_all_at(b"x", length).expect("append");
        }),
        ("truncated", |file| {
            let length = file.metadata().expect("metadata").len();
            file.set_len(length - 1).expect("truncate");
        }),
        ("emptied", |file| file.set_len(0).expect("empty")),
    ];
    for class in [REFS_V1, LOGS_V1] {
        for (row, edit) in edits {
            let mut case = CaseV1::with_bytes(
                &capsule,
                &[(REFS_V1, refs.clone()), (LOGS_V1, logs.clone())],
            );
            let index = staged_index(&case.verified, class);
            edit(&case.verified.staged[index].file);
            let name = lossy(case.verified.staged[index].name.as_bytes());
            let error = case.refusal(&format!("{class:?}: {row}"));
            assert!(
                matches!(&error, GitBootstrapErrorV1::StagedInputChanged { name: found }
                    if *found == name),
                "{class:?}: {row}: {error:?}"
            );
        }
    }
}

/// A frame replaced between its rehash and its decode (with another well-formed frame, so only the
/// verified staged digest can tell) is refused: the decoded bytes are bound to the verified plaintext.
#[test]
fn b1a_a_frame_changed_between_the_rehash_and_the_decode_is_refused() {
    use std::os::unix::fs::FileExt as _;
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    // The replacement is shorter: the decode reads it whole, so only the verified digest can tell.
    let refs = encode_frame(
        REFS_V1,
        &generation,
        &with_head(&[(b"refs", EntryV1::Dir), (b"refs/heads", EntryV1::Dir)]),
    );
    let other = encode_frame(REFS_V1, &generation, &with_head(&[]));
    assert!(other.len() < refs.len());
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, refs)]);
    let handle = case.verified.staged[staged_index(&case.verified, REFS_V1)]
        .file
        .try_clone()
        .expect("a duplicate descriptor");
    let swapped = std::cell::Cell::new(false);
    let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "frame-rewind" && !swapped.replace(true) {
            handle.set_len(0).expect("empty the frame");
            handle.write_all_at(&other, 0).expect("another frame");
        }
    });
    let error = case.refusal("a frame swapped after its rehash");
    assert!(
        matches!(error, GitBootstrapErrorV1::StagedInputChanged { .. }),
        "{error:?}"
    );
}

/// A frame assembled byte by byte, with a correct trailer and digest, for structures the encoder will
/// not emit: each record is already encoded.
fn raw_frame(
    class_code: u8,
    generation: &str,
    records: &[Vec<u8>],
    entries: u64,
    total: u64,
) -> Vec<u8> {
    let mut bytes = b"a2a-cfr1".to_vec();
    bytes.push(1);
    bytes.push(class_code);
    bytes.extend_from_slice(&(generation.len() as u16).to_le_bytes());
    bytes.extend_from_slice(generation.as_bytes());
    for record in records {
        bytes.extend_from_slice(record);
    }
    bytes.push(0xff);
    bytes.extend_from_slice(&entries.to_le_bytes());
    bytes.extend_from_slice(&total.to_le_bytes());
    let frame_digest = sha256_bytes(&bytes);
    bytes.extend_from_slice(&frame_digest);
    bytes
}

fn raw_record(tag: u8, path: &[u8], tail: &[u8]) -> Vec<u8> {
    let mut record = vec![tag];
    record.extend_from_slice(&(path.len() as u16).to_le_bytes());
    record.extend_from_slice(path);
    record.extend_from_slice(tail);
    record
}

fn raw_file(path: &[u8], content: &[u8]) -> Vec<u8> {
    let mut tail = 0o644_u16.to_le_bytes().to_vec();
    tail.extend_from_slice(&(content.len() as u64).to_le_bytes());
    tail.extend_from_slice(content);
    tail.extend_from_slice(&sha256_bytes(content));
    raw_record(2, path, &tail)
}

/// A regular file is never a parent: a hand-built frame with a correct trailer whose entry sits under a
/// regular file is refused whole (by the codec's own structure rule, before the aggregate table sees it),
/// and so is one whose parent directory is simply absent. The aggregate table's own parent rule is
/// exercised directly above.
#[test]
fn b1a_a_hand_built_frame_with_a_file_parent_or_a_missing_parent_is_refused() {
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let whole = |records: &[Vec<u8>], total: u64| {
        raw_frame(1, &generation, records, records.len() as u64, total)
    };

    // Control: the same builder, with entries that are in order and under a directory, is whole.
    let control = whole(
        &[
            raw_file(b"HEAD", b"ref: refs/heads/main\n"),
            raw_record(1, b"refs", &0o755_u16.to_le_bytes()),
            raw_file(b"refs/x", b"1"),
        ],
        21 + 1,
    );
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, control)]);
    let plan = case.accepted("the hand-built control");
    assert_eq!(plan.frames[0].entries, 3);

    let under_a_file = whole(&[raw_file(b"HEAD", b"x"), raw_file(b"HEAD/x", b"y")], 2);
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, under_a_file)]);
    let error = case.refusal("an entry under a regular file");
    assert!(
        matches!(
            error,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { class: REFS_V1 })
        ),
        "{error:?}"
    );

    let orphan = whole(
        &[raw_file(b"HEAD", b"x"), raw_file(b"refs/heads/main", b"y")],
        2,
    );
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, orphan)]);
    let error = case.refusal("an entry whose parents are absent");
    assert!(
        matches!(
            error,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { class: REFS_V1 })
        ),
        "{error:?}"
    );
}

/// Every refusal of the real entry point is in pass A's failure shape: no active materialization began,
/// the caller's `completed` step is carried unchanged (whatever it is), and the ledger figure is the one
/// ledger's. The causes span every refusal family: Empty refs, an entry fault, a bad trailer, drift, and
/// a spent deadline. Nothing exists under `repository/`.
#[test]
fn b1a_every_refusal_is_in_the_failure_shape_that_began_no_materialization() {
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let bad_trailer = {
        let mut bytes = encode_frame(REFS_V1, &generation, &with_head(&[]));
        let last = bytes.len() - 1;
        flip(&mut bytes, last);
        bytes
    };
    let linked = encode_frame(
        REFS_V1,
        &generation,
        &with_head(&[(b"refs", EntryV1::Link(b"x"))]),
    );
    for completed in [
        None,
        Some(GitPlaneStepV1::Admission),
        Some(GitPlaneStepV1::BootstrapGate),
    ] {
        let mut rows: Vec<(&str, CaseV1, Instant)> = vec![
            (
                "empty refs",
                CaseV1::with_bytes(&capsule, &[]),
                far_deadline(),
            ),
            (
                "an entry fault",
                CaseV1::with_bytes(&capsule, &[(REFS_V1, linked.clone())]),
                far_deadline(),
            ),
            (
                "a bad trailer",
                CaseV1::with_bytes(&capsule, &[(REFS_V1, bad_trailer.clone())]),
                far_deadline(),
            ),
        ];
        let spent = Instant::now();
        std::thread::sleep(Duration::from_millis(5));
        rows.push((
            "a spent deadline",
            CaseV1::with_entries(&capsule, &[], None),
            spent,
        ));
        for (row, mut case, deadline) in rows {
            let ledger = ledger_used(&case.verified);
            let failure = prevalidate_active_frames_v1(
                &mut case.verified,
                frame_budget(),
                deadline,
                completed,
            )
            .expect_err(row);
            let _ = require_failure_shape(failure, completed, ledger);
            assert_eq!(
                case.dest.entries(),
                [".restore-work"],
                "{row}: no repository"
            );
        }
    }
    // The control: success carries no failure at all.
    let mut case = CaseV1::with_entries(&capsule, &[], None);
    prevalidate_active_frames_v1(&mut case.verified, frame_budget(), far_deadline(), None)
        .expect("a whole refs frame");
}

/// A frame that grows after its rehash is read only one byte past its verified length, never whole: the
/// decoder's source is bounded, so a frame that is not the verified plaintext is refused having consumed
/// at most that one extra byte.
#[test]
fn b1a_a_frame_that_grows_after_its_rehash_is_never_read_past_one_extra_byte() {
    use std::os::unix::fs::FileExt as _;
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let bytes = encode_frame(REFS_V1, &generation, &with_head(&[]));
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, bytes)]);
    let index = staged_index(&case.verified, REFS_V1);
    let handle = case.verified.staged[index]
        .file
        .try_clone()
        .expect("a duplicate descriptor");
    let length = case.verified.staged[index].length;
    let grown = std::cell::Cell::new(false);
    let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "frame-rewind" && !grown.replace(true) {
            handle
                .write_all_at(&vec![0_u8; 1024 * 1024], length)
                .expect("grow the frame");
        }
    });
    let error = case.refusal("a frame grown after its rehash");
    assert!(
        matches!(
            error,
            GitBootstrapErrorV1::StagedInputChanged { .. }
                | GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid {
                    class: REFS_V1
                })
        ),
        "{error:?}"
    );
    assert_eq!(
        position(&case.verified.staged[index]),
        length + 1,
        "one byte past the verified length, and no more"
    );
}

/// The caller's frame budget bounds the null-sink decode: a frame with more entries than the budget
/// allows, or more bytes, is refused whole, and the same frame under a sufficient budget is whole.
#[test]
fn b1a_the_callers_frame_budget_bounds_the_decode() {
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let bytes = encode_frame(
        REFS_V1,
        &generation,
        &with_head(&[
            dir("refs"),
            dir("refs/heads"),
            file("refs/heads/a", b"1\n"),
            file("refs/heads/b", b"2\n"),
            file("refs/heads/c", b"3\n"),
        ]),
    );
    let length = bytes.len() as u64;
    let run = |budget: CustodyFrameBudgetV1| {
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, bytes.clone())]);
        let ledger = ledger_used(&case.verified);
        let outcome = prevalidate_active_frames_bounded_v1(
            &mut case.verified,
            budget,
            far_deadline(),
            B1A_COMPLETED_V1,
            1 << 20,
        );
        assert_eq!(ledger_used(&case.verified), ledger);
        outcome.map_err(|failure| failure.cause)
    };
    let sufficient = CustodyFrameBudgetV1::new(6, length).expect("a budget");
    assert!(run(sufficient).is_ok(), "exactly enough entries and bytes");
    for (row, budget) in [
        (
            "one entry short",
            CustodyFrameBudgetV1::new(5, length).expect("a budget"),
        ),
        (
            "one byte short",
            CustodyFrameBudgetV1::new(6, length - 1).expect("a budget"),
        ),
    ] {
        let error = run(budget).expect_err(row);
        assert!(
            matches!(
                error,
                GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid {
                    class: REFS_V1
                })
            ),
            "{row}: {error:?}"
        );
    }
}

/// The paced rehash, directly: it rewinds before and after, checks length and digest, never reads a
/// grown file whole, and an expired deadline refuses before a byte is read or a descriptor is moved.
#[test]
fn b1a_the_paced_rehash_checks_rewinds_and_reads_a_bounded_prefix() {
    use std::io::{Seek as _, Write as _};
    use std::os::unix::fs::FileExt as _;
    let capsule = frames_capsule();
    let generation = CaseV1::generation(&capsule);
    let bytes = encode_frame(REFS_V1, &generation, &with_head(&[]));
    let fresh = || {
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, bytes.clone())]);
        let index = staged_index(&case.verified, REFS_V1);
        (case.verified.staged.swap_remove(index), case)
    };

    // A descriptor left mid-file is rewound first; it ends at offset 0.
    let (mut one, _case) = fresh();
    one.file.seek(SeekFrom::Start(7)).expect("seek");
    reverify_staged_v1(&mut one, far_deadline()).expect("an unchanged frame");
    assert_eq!(position(&one), 0);

    // A same-length change and a truncation refuse by digest and length.
    let (mut one, _case) = fresh();
    one.file.write_all_at(&[0xff], 12).expect("change");
    assert!(matches!(
        reverify_staged_v1(&mut one, far_deadline()),
        Err(GitBootstrapErrorV1::StagedInputChanged { .. })
    ));
    let (mut one, _case) = fresh();
    one.file.set_len(one.length - 1).expect("truncate");
    assert!(matches!(
        reverify_staged_v1(&mut one, far_deadline()),
        Err(GitBootstrapErrorV1::StagedInputChanged { .. })
    ));

    // A grown file is read only one byte past its recorded length, never whole.
    let (mut one, _case) = fresh();
    one.file.seek(SeekFrom::End(0)).expect("seek");
    one.file
        .write_all(&vec![0_u8; 4 * 1024 * 1024])
        .expect("grow it");
    assert!(matches!(
        reverify_staged_v1(&mut one, far_deadline()),
        Err(GitBootstrapErrorV1::StagedInputChanged { .. })
    ));
    assert_eq!(position(&one), one.length + 1, "one byte past the length");

    // An expired deadline refuses before any read or rewind.
    let (mut one, _case) = fresh();
    one.file.seek(SeekFrom::Start(5)).expect("seek");
    let spent = Instant::now();
    std::thread::sleep(Duration::from_millis(5));
    assert!(matches!(
        reverify_staged_v1(&mut one, spent),
        Err(GitBootstrapErrorV1::Git(CustodyGitError::Timeout))
    ));
    assert_eq!(position(&one), 5, "no read and no rewind happened");
}

/// Retained descriptors are the only input: every staged leaf renamed away, and then unlinked, is still
/// read from its descriptor.
#[test]
fn b1a_renamed_and_unlinked_staged_frames_are_still_read_from_their_descriptors() {
    let fixture = PlanFixtureV1::rich_clone();
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    let (dest, mut verified) = phase_v(&fixture.capsule_dir());
    let whole = prevalidate(&mut verified, far_deadline()).expect("the unmoved frames");

    let plain = dest.work().join("plain");
    let mut renamed = 0_usize;
    for (relative, bytes) in snapshot(&plain) {
        if bytes.is_some() {
            let from = plain.join(&relative);
            let to = from.with_file_name(format!("renamed-{renamed}"));
            std::fs::rename(&from, &to).expect("a staged leaf is renamed");
            renamed += 1;
        }
    }
    assert_eq!(renamed, verified.staged.len());
    let moved = prevalidate(&mut verified, far_deadline())
        .expect("renamed frames are read from their descriptors");
    assert_eq!(moved, whole);

    for (relative, bytes) in snapshot(&plain) {
        if bytes.is_some() {
            std::fs::remove_file(plain.join(&relative)).expect("a staged leaf is unlinked");
        }
    }
    let unlinked = prevalidate(&mut verified, far_deadline())
        .expect("unlinked frames are read from their descriptors");
    assert_eq!(unlinked, whole);
}

// ---------------------------------------------------------------------------------------------
// The one deadline paces every step: reads, entries, rewinds, and the success
// ---------------------------------------------------------------------------------------------

/// The one deadline of the pacing rows. A clean prevalidation of these small frames takes milliseconds.
const B1A_DEADLINE_V1: Duration = Duration::from_secs(3);

fn two_frame_case(capsule: &CustomCapsuleV1) -> CaseV1 {
    CaseV1::with_entries(
        capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/main", EntryV1::File(b"oid\n")),
        ],
        Some(&[
            (b"logs", EntryV1::Dir),
            (b"logs/HEAD", EntryV1::File(b"log\n")),
        ]),
    )
}

/// The guards of a clean two-frame run, in order, by label.
const CLEAN_GUARDS_V1: [&str; 10] = [
    "frames-entry",
    "frame-rehash",
    "frame-rewind",
    "frame-decoded",
    "frame-rewound",
    "frame-rehash",
    "frame-rewind",
    "frame-decoded",
    "frame-rewound",
    "frames-final",
];

fn record_guards() -> (
    std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    GitSeamGuardV1,
) {
    let labels = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
    let seen = labels.clone();
    let hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        seen.borrow_mut().push(event.label);
    });
    (labels, hook)
}

/// A clean run reads the clock at exactly the documented guards, in order, so no step is unobserved.
#[test]
fn b1a_the_guards_of_a_clean_run_are_the_documented_ones_in_order() {
    let capsule = frames_capsule();
    let mut case = two_frame_case(&capsule);
    let (labels, _hook) = record_guards();
    let _ = case.accepted("a clean two-frame run");
    assert_eq!(*labels.borrow(), CLEAN_GUARDS_V1);
}

/// Each guard halts the run alone: its step made slow past the deadline is refused with a typed timeout,
/// the run stops AT that guard (no later guard ever ran), nothing is written, and the ledger is as it
/// was. A brief delay at every guard, with headroom, still succeeds.
#[test]
fn b1a_each_guard_halts_the_run_alone_and_headroom_succeeds() {
    let capsule = frames_capsule();
    let mut distinct: Vec<&'static str> = Vec::new();
    for label in CLEAN_GUARDS_V1 {
        if !distinct.contains(&label) {
            distinct.push(label);
        }
    }
    for target in distinct {
        let mut case = two_frame_case(&capsule);
        let deadline = Instant::now() + B1A_DEADLINE_V1;
        let labels = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let (seen, slow) = (labels.clone(), fired.clone());
        let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
            seen.borrow_mut().push(event.label);
            if event.label == target && !slow.replace(true) {
                pass_the_deadline(deadline);
            }
        });
        let ledger = ledger_used(&case.verified);
        let tree = snapshot(&case.dest.path);
        let error = prevalidate(&mut case.verified, deadline).expect_err(target);
        assert!(
            matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
            "{target}: {error:?}"
        );
        assert!(fired.get(), "{target}: the slow step never ran");
        assert_eq!(
            labels.borrow().last().copied(),
            Some(target),
            "{target}: the run halted AT its guard: {:?}",
            labels.borrow()
        );
        assert_eq!(ledger_used(&case.verified), ledger, "{target}");
        assert_eq!(snapshot(&case.dest.path), tree, "{target}");
        assert_eq!(case.dest.entries(), [".restore-work"], "{target}");
    }

    let mut case = two_frame_case(&capsule);
    let deadline = Instant::now() + B1A_DEADLINE_V1;
    let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, |_| {
        std::thread::sleep(Duration::from_millis(40));
    });
    prevalidate(&mut case.verified, deadline).expect("ten brief delays leave most of the deadline");
}

/// A deadline already spent refuses before the first read: no label past the entry guard, and every
/// staged descriptor is exactly where it was left.
#[test]
fn b1a_a_spent_deadline_refuses_before_any_frame_is_touched() {
    let capsule = frames_capsule();
    let mut case = two_frame_case(&capsule);
    for one in &case.verified.staged {
        use std::io::Seek as _;
        (&one.file)
            .seek(SeekFrom::Start(3))
            .expect("park the descriptor");
    }
    let spent = Instant::now();
    std::thread::sleep(Duration::from_millis(5));
    let (labels, _hook) = record_guards();
    let error = prevalidate(&mut case.verified, spent).expect_err("a spent deadline");
    assert!(
        matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
        "{error:?}"
    );
    assert_eq!(*labels.borrow(), ["frames-entry"]);
    assert!(case.verified.staged.iter().all(|one| position(one) == 3));
}

/// A frame of several entries whose deadline passes while one entry is being judged stops AT that entry:
/// the per-entry guard keeps the later entries from being decoded.
#[test]
fn b1a_the_deadline_is_observed_before_every_decoded_entry() {
    let capsule = frames_capsule();
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/a", EntryV1::File(b"1\n")),
            (b"refs/heads/b", EntryV1::File(b"2\n")),
            (b"refs/heads/c", EntryV1::File(b"3\n")),
            (b"refs/heads/d", EntryV1::File(b"4\n")),
        ],
    );
    let deadline = Instant::now() + B1A_DEADLINE_V1;
    let entries = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let seen = entries.clone();
    let _hook = install_git_hook_for_test(GitHookPointV1::FrameEntry, move |event| {
        seen.borrow_mut()
            .push(event.path.to_string_lossy().into_owned());
        if event.path == Path::new("refs/heads/a") {
            pass_the_deadline(deadline);
        }
    });
    let error = prevalidate(&mut case.verified, deadline)
        .expect_err("the deadline passed while judging an entry");
    assert!(
        matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
        "{error:?}"
    );
    assert_eq!(
        *entries.borrow(),
        ["HEAD", "refs", "refs/heads", "refs/heads/a"],
        "no entry after the one that spent the deadline was decoded"
    );
}

/// Reads are paced too: a large regular file whose entry spends the deadline is not read past the one
/// buffer the decoder had already taken. The descriptor stops within one buffer of where it was.
#[test]
fn b1a_no_frame_byte_is_read_after_the_deadline() {
    let capsule = frames_capsule();
    let big = vec![b'x'; 300 * 1024];
    let mut case = CaseV1::refs_only(
        &capsule,
        &[
            (b"refs", EntryV1::Dir),
            (b"refs/heads", EntryV1::Dir),
            (b"refs/heads/big", EntryV1::File(&big)),
        ],
    );
    let deadline = Instant::now() + B1A_DEADLINE_V1;
    // The rehash reads the whole frame once; the decode must then stop after its first buffer. The
    // hook marks the decode: it runs at an entry, which only the decode reaches.
    let _hook = install_git_hook_for_test(GitHookPointV1::FrameEntry, move |event| {
        if event.path == Path::new("refs/heads/big") {
            pass_the_deadline(deadline);
        }
    });
    let error = prevalidate(&mut case.verified, deadline)
        .expect_err("the deadline passed before the content was read");
    assert!(
        matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
        "{error:?}"
    );
    let one = &case.verified.staged[staged_index(&case.verified, REFS_V1)];
    assert!(one.length > 2 * HASH_BUFFER_BYTES_V1 as u64);
    assert!(
        position(one) <= HASH_BUFFER_BYTES_V1 as u64,
        "the decode read {} bytes of {}: past the deadline",
        position(one),
        one.length
    );
}

// ---------------------------------------------------------------------------------------------
// Bounded private metadata for the later second decode
// ---------------------------------------------------------------------------------------------

/// The entry digest binds class, kind, path, and length, in order: any one difference changes it, and
/// the same entries give the same digest.
#[test]
fn b1a_the_entry_digest_binds_class_kind_path_length_and_order() {
    use ActiveEntryKindV1::{Directory, Regular};
    let digest_of = |class, entries: &[(ActiveEntryKindV1, &str, u64)]| {
        let mut digest = ActiveEntryDigestV1::new(class);
        for (kind, path, length) in entries {
            digest.record(*kind, path.as_bytes(), *length);
        }
        digest.finish()
    };
    let base = [(Directory, "refs", 0), (Regular, "refs/x", 3)];
    let reference = digest_of(REFS_V1, &base);
    assert_eq!(reference, digest_of(REFS_V1, &base), "deterministic");
    let variants: [(&str, [u8; 32]); 6] = [
        ("class", digest_of(LOGS_V1, &base)),
        (
            "kind",
            digest_of(REFS_V1, &[(Directory, "refs", 0), (Directory, "refs/x", 3)]),
        ),
        (
            "path",
            digest_of(REFS_V1, &[(Directory, "refs", 0), (Regular, "refs/y", 3)]),
        ),
        (
            "length",
            digest_of(REFS_V1, &[(Directory, "refs", 0), (Regular, "refs/x", 4)]),
        ),
        (
            "order",
            digest_of(REFS_V1, &[(Regular, "refs/x", 3), (Directory, "refs", 0)]),
        ),
        ("count", digest_of(REFS_V1, &base[..1])),
    ];
    for (what, other) in variants {
        assert_ne!(other, reference, "{what}");
    }
    // A path boundary is part of the record: `ab` + `c` is not `a` + `bc`.
    assert_ne!(
        digest_of(REFS_V1, &[(Regular, "ab", 0), (Regular, "c", 0)]),
        digest_of(REFS_V1, &[(Regular, "a", 0), (Regular, "bc", 0)]),
    );
    // And the path is length-prefixed: one path that embeds the bytes of the next record (its content
    // length and the following kind, as the digest lays them out) is not those two records. Without the
    // prefix the two lists would hash the same bytes.
    let embedded = "a\0\0\0\0\0\0\0\0\u{2}b";
    assert_ne!(
        digest_of(REFS_V1, &[(Regular, "a", 0), (Regular, "b", 0)]),
        digest_of(REFS_V1, &[(Regular, embedded, 0)]),
    );
}

/// The plan's facts for a second decode are those of the frames: different frames give different
/// entry digests, and the same frame staged again gives the same.
#[test]
fn b1a_the_plan_is_reproducible_and_distinguishes_frames() {
    let capsule = frames_capsule();
    let one = |entries: &EntriesV1<'_>| {
        CaseV1::refs_only(&capsule, entries)
            .accepted("a frame")
            .frames[0]
            .clone()
    };
    let a = one(&[dir("refs"), file("refs/x", b"1\n")]);
    let again = one(&[dir("refs"), file("refs/x", b"1\n")]);
    let content = one(&[dir("refs"), file("refs/x", b"2\n")]);
    let name = one(&[dir("refs"), file("refs/y", b"1\n")]);
    assert_eq!(a, again);
    // Content is bound by the staged digest, the names and shapes by the entry digest.
    assert_ne!(a.sha256, content.sha256);
    assert_eq!(a.entries_sha256, content.entries_sha256);
    assert_ne!(a.entries_sha256, name.entries_sha256);
}

// ---------------------------------------------------------------------------------------------
// Pass A's no-repository invariant is function-scoped, and B1a's seam writes nothing
// ---------------------------------------------------------------------------------------------

/// Direct `bootstrap_git_objects_v1` never creates `repository/`, for every capsule shape: it leaves only
/// `.restore-work/` in the destination, and nothing named `repository` anywhere beneath it. B1a's
/// prevalidation run afterwards on the same retained value adds no entry either.
#[test]
fn b1a_the_direct_bootstrap_creates_no_repository_and_prevalidation_adds_none() {
    let fixture = PlanFixtureV1::rich_clone();
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    let rich = fixture.capsule_dir();
    let captured_capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let empty_sha256 = empty(CustodyGitObjectFormatV1::Sha256);
    // Only the rich capsule carries active frames; the other two have Empty refs.
    for (row, capsule, format, frames) in [
        ("rich", rich, GitObjectFormatV1::Sha1, true),
        (
            "captured",
            captured_capsule.capsule.clone(),
            GitObjectFormatV1::Sha1,
            false,
        ),
        (
            "empty",
            empty_sha256.capsule.clone(),
            GitObjectFormatV1::Sha256,
            false,
        ),
    ] {
        let (dest, verified) = phase_v(&capsule);
        let mut bootstrap = must_bootstrap(verified, format);
        assert_eq!(dest.entries(), [".restore-work"], "{row}");
        let after_bootstrap = snapshot(&dest.path);
        assert!(
            after_bootstrap
                .keys()
                .all(|name| !name.contains("repository")),
            "{row}: {:?}",
            after_bootstrap.keys().collect::<Vec<_>>()
        );
        let ledger = ledger_used(&bootstrap.verified);
        let outcome = prevalidate(&mut bootstrap.verified, far_deadline());
        match (frames, outcome) {
            (true, Ok(plan)) => assert_eq!(plan.frames.len(), 2, "{row}"),
            (false, Err(GitBootstrapErrorV1::EmptyRefsAndHead)) => {}
            (_, other) => panic!("{row}: {other:?}"),
        }
        assert_eq!(ledger_used(&bootstrap.verified), ledger, "{row}");
        assert_eq!(snapshot(&dest.path), after_bootstrap, "{row}");
        assert_eq!(dest.entries(), [".restore-work"], "{row}");
    }
}

const B1A_MARKER_V1: &str = "// Pass B1a: retained active-frame prevalidation";

/// STRUCTURAL ONLY. Pass A's whole-module forbidden-name scan is narrowed to the bootstrap
/// implementation (its production source up to the B1a marker), because later passes create the
/// repository in this file; the marker is unique and the narrowing leaves a nonempty region before and
/// after it. B1a's own region is then held to a read-only vocabulary: no path open, no walker, no
/// repository or create-new write, no ledger charge, no Git runner, no object-store route, no unsafe.
#[test]
fn b1a_the_prevalidation_region_is_read_only_and_the_scan_split_is_unique() {
    let source = include_str!("custody_restore_git.rs");
    assert_eq!(source.matches(B1A_MARKER_V1).count(), 1, "one marker");
    let (before, region) = source.split_once(B1A_MARKER_V1).expect("the marker");
    assert!(before.contains("pub(crate) fn bootstrap_git_objects_v1("));
    assert!(region.contains("pub(crate) fn prevalidate_active_frames_v1("));
    assert!(
        !before.contains("prevalidate_active_frames_v1("),
        "Pass A's region does not call the prevalidation"
    );
    let code: String = region
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // The one ledger is only READ, once, for the failure's figure: it is never charged or released.
    assert_eq!(
        code.matches(".ledger").count(),
        1,
        "one access to the ledger"
    );
    assert!(code.contains(".ledger.borrow().used()"));
    for forbidden in [
        "std::fs",
        "File::open(",
        "OpenOptions",
        "read_dir(",
        "create_new",
        "create_staging_directory",
        "write_all",
        "GitRunnerV1",
        "GitRunRequestV1",
        "GitObjectStoreRouteV1",
        "object_store",
        "borrow_mut",
        "ReservedPairV1",
        "OwnedChargeV1",
        "ScratchLedgerV1",
        ".reserve(",
        ".release(",
        "roll_back",
        "unsafe",
        "\"repository\"",
        "remeasure_git_directory",
        "measure_git_directory",
        "walk",
        "Command",
    ] {
        assert!(
            !code.contains(forbidden),
            "the prevalidation region mentions `{forbidden}`"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Pass B1b: pure refs agreement and inert config
//
// The grammar, agreement, and limit rows drive `RefsBuilderV1` over byte slices and readers: the same
// code that later runs over retained final descriptors, fed here by the test. Objects are named by one
// fill character (`a`..`e`) so a row reads as the refs it describes. The production rows below them run
// the real entry point over REAL retained phase-V values and frames. Nothing here writes a byte, runs
// Git on the code under test, or charges the ledger.
//
// Pass A's and B1a's controls above are unchanged. The mutation matrix in
// `.git/a2a-evidence/2b3b-pass-b1b/` maps each guard to the rows that turn red without it.
// ---------------------------------------------------------------------------------------------

use crate::custody_seal::{CustodyOriginalRefV1, CustodyRefTargetV1};

type KindV1 = CustodyGitObjectKindV1;

/// The pure rows' object pool: `a` and `b` are commits, `c` an annotated tag, `d` a tree, `e` a blob,
/// and `f` an object of unknown kind.
const POOL_V1: [(char, KindV1); 6] = [
    ('a', KindV1::Commit),
    ('b', KindV1::Commit),
    ('c', KindV1::Tag),
    ('d', KindV1::Tree),
    ('e', KindV1::Blob),
    ('f', KindV1::Unknown),
];
const BOTH_FORMATS_V1: [GitObjectFormatV1; 2] =
    [GitObjectFormatV1::Sha1, GitObjectFormatV1::Sha256];

fn oid_len(format: GitObjectFormatV1) -> usize {
    match format {
        GitObjectFormatV1::Sha1 => 40,
        GitObjectFormatV1::Sha256 => 64,
    }
}

fn oid_of(fill: char, format: GitObjectFormatV1) -> String {
    fill.to_string().repeat(oid_len(format))
}

fn manifest_format_of(format: GitObjectFormatV1) -> CustodyGitObjectFormatV1 {
    match format {
        GitObjectFormatV1::Sha1 => CustodyGitObjectFormatV1::Sha1,
        GitObjectFormatV1::Sha256 => CustodyGitObjectFormatV1::Sha256,
    }
}

fn pool_object(fill: char, format: GitObjectFormatV1) -> CustodyOriginalObjectV1 {
    let kind = POOL_V1
        .iter()
        .find(|(found, _)| *found == fill)
        .expect("a pooled object")
        .1;
    CustodyOriginalObjectV1::new(manifest_format_of(format), oid_of(fill, format), kind)
        .expect("a pooled object")
}

/// What a manifest ref says its target is.
#[derive(Clone, Copy)]
enum WantV1 {
    Direct(char),
    Symbolic(&'static str),
    Unborn,
}

fn want_ref(format: GitObjectFormatV1, name: &str, want: WantV1) -> CustodyOriginalRefV1 {
    let target = match want {
        WantV1::Direct(fill) => CustodyRefTargetV1::Direct(pool_object(fill, format)),
        WantV1::Symbolic(target) => {
            CustodyRefTargetV1::Symbolic(LosslessPathV1::from_bytes(target.as_bytes().to_vec()))
        }
        WantV1::Unborn => CustodyRefTargetV1::Unborn,
    };
    CustodyOriginalRefV1::new(LosslessPathV1::from_bytes(name.as_bytes().to_vec()), target)
        .expect("a manifest ref")
}

/// A manifest over the pool (or over no objects) and `refs`.
fn pool_manifest(
    format: GitObjectFormatV1,
    pooled: bool,
    refs: &[(&str, WantV1)],
) -> CustodyManifestV1 {
    let objects = if pooled {
        POOL_V1
            .iter()
            .map(|(fill, _)| pool_object(*fill, format))
            .collect()
    } else {
        vec![]
    };
    let state = if pooled {
        CustodyStateClassV1::Captured
    } else {
        CustodyStateClassV1::Empty
    };
    CustodyManifestV1::new(
        "unit-b1b",
        "run-b1b",
        "materialization-b1b",
        GENERATION_V1,
        coverage_rows(state),
        refs.iter()
            .map(|(name, want)| want_ref(format, name, *want))
            .collect(),
        objects,
        vec![],
        vec![],
    )
    .expect("a pooled manifest")
}

type FileV1 = (&'static str, Vec<u8>);
type ManifestRefsV1 = Vec<(&'static str, WantV1)>;

fn direct_file(fill: char, format: GitObjectFormatV1) -> Vec<u8> {
    format!("{}\n", oid_of(fill, format)).into_bytes()
}

fn symbolic_file(target: &str) -> Vec<u8> {
    format!("ref: {target}\n").into_bytes()
}

fn file_of(path: &'static str, content: Vec<u8>) -> FileV1 {
    (path, content)
}

/// An attached `HEAD` on `refs/heads/main`, which is commit `a`.
fn base_files(format: GitObjectFormatV1) -> Vec<FileV1> {
    vec![
        file_of("HEAD", symbolic_file("refs/heads/main")),
        file_of("refs/heads/main", direct_file('a', format)),
    ]
}

/// `files` into a builder, then the agreement with a manifest of `refs`.
fn pure_run(
    format: GitObjectFormatV1,
    pooled: bool,
    files: &[FileV1],
    refs: &[(&str, WantV1)],
    limits: RefsLimitsV1,
) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
    let mut builder = RefsBuilderV1::new(format, limits, far_deadline());
    for (path, content) in files {
        let mut reader: &[u8] = content;
        builder.admit_file(path.as_bytes(), &mut reader)?;
    }
    builder.finish(&pool_manifest(format, pooled, refs))
}

fn pure(
    format: GitObjectFormatV1,
    files: &[FileV1],
    refs: &[(&str, WantV1)],
) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
    pure_run(format, true, files, refs, RefsLimitsV1::v1())
}

fn refs_fault(error: GitBootstrapErrorV1) -> RefsFaultV1 {
    match error {
        GitBootstrapErrorV1::Refs(fault) => fault,
        other => panic!("a refs fault, not {other:?}"),
    }
}

fn fault_of(result: Result<RefsAgreementV1, GitBootstrapErrorV1>, row: &str) -> RefsFaultV1 {
    refs_fault(result.expect_err(row))
}

fn row_fault(path: &str, why: RowFaultV1) -> RefsFaultV1 {
    RefsFaultV1::Row {
        path: path.to_owned(),
        why,
    }
}

fn name_fault(path: &str, why: RefNameFaultV1) -> RefsFaultV1 {
    RefsFaultV1::RefName {
        path: path.to_owned(),
        why,
    }
}

// ---------------------------------------------------------------------------------------------
// Ref names
// ---------------------------------------------------------------------------------------------

/// The ref-name rule: every name is under `refs/`, has no empty component, follows the portable ASCII
/// spelling rule, and Git's check-ref-format rules (no leading dot, no `.lock` suffix, no `..`, none of
/// `~ ^ : ? * [`, no `@{`). Each refusal is its own typed reason, and a valid neighbour passes.
#[test]
fn b1b_ref_names_follow_the_git_rules_and_the_portable_rule() {
    for valid in [
        "refs/heads/main",
        "refs/tags/v1.0",
        "refs/remotes/origin/HEAD",
        "refs/stash",
        "refs/heads/feature/deep/er",
        "refs/heads/a-b_c",
        "refs/heads/a.b",
        "refs/heads/@x",
        "refs/heads/lock",
        "refs/notes/commits",
        "refs/heads/x.locked",
    ] {
        assert_eq!(check_ref_name_v1(valid.as_bytes()), Ok(()), "{valid}");
    }
    let table: &[(&[u8], RefNameFaultV1)] = &[
        (b"refs", RefNameFaultV1::NotUnderRefs),
        (b"heads/main", RefNameFaultV1::NotUnderRefs),
        (b"HEAD", RefNameFaultV1::NotUnderRefs),
        (b"", RefNameFaultV1::NotUnderRefs),
        (b"refs/", RefNameFaultV1::EmptyComponent),
        (b"refs//x", RefNameFaultV1::EmptyComponent),
        (b"refs/heads/", RefNameFaultV1::EmptyComponent),
        (b"refs/heads/.hidden", RefNameFaultV1::LeadingDot),
        (b"refs/.x/y", RefNameFaultV1::LeadingDot),
        (b"refs/heads/x.lock", RefNameFaultV1::LockSuffix),
        (b"refs/x.lock/y", RefNameFaultV1::LockSuffix),
        (b"refs/heads/a..b", RefNameFaultV1::DoubleDot),
        (b"refs/heads/a~1", RefNameFaultV1::ForbiddenByte),
        (b"refs/heads/a^", RefNameFaultV1::ForbiddenByte),
        (b"refs/heads/a:b", RefNameFaultV1::ForbiddenByte),
        (b"refs/heads/a?", RefNameFaultV1::ForbiddenByte),
        (b"refs/heads/a*", RefNameFaultV1::ForbiddenByte),
        (b"refs/heads/a[", RefNameFaultV1::ForbiddenByte),
        (b"refs/heads/a@{b", RefNameFaultV1::AtBrace),
        (
            b"refs/heads/a.",
            RefNameFaultV1::Portable(PortableFaultV1::TrailingDot),
        ),
        (
            b"refs/heads/a b",
            RefNameFaultV1::Portable(PortableFaultV1::Unprintable),
        ),
        (
            b"refs/heads/a\tb",
            RefNameFaultV1::Portable(PortableFaultV1::Unprintable),
        ),
        (
            b"refs/heads/a\x7f",
            RefNameFaultV1::Portable(PortableFaultV1::Unprintable),
        ),
        (
            b"refs/heads/a\\b",
            RefNameFaultV1::Portable(PortableFaultV1::Backslash),
        ),
        (
            "refs/heads/h\u{e9}".as_bytes(),
            RefNameFaultV1::Portable(PortableFaultV1::NonAscii),
        ),
        (
            b"refs/heads/\xff",
            RefNameFaultV1::Portable(PortableFaultV1::NonAscii),
        ),
    ];
    for (name, why) in table {
        assert_eq!(
            check_ref_name_v1(name),
            Err(why.clone()),
            "{}",
            String::from_utf8_lossy(name)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Loose refs, HEAD, and ORIG_HEAD
// ---------------------------------------------------------------------------------------------

/// HEAD, ORIG_HEAD, and a loose ref parse in both formats: a direct ref is exactly the format's
/// lowercase hex and one LF, a symbolic ref exactly `ref: ` and a full ref name and one LF.
#[test]
fn b1b_exact_loose_refs_parse_in_both_formats() {
    for format in BOTH_FORMATS_V1 {
        let agreement = pure(
            format,
            &[
                file_of("HEAD", symbolic_file("refs/heads/main")),
                file_of("ORIG_HEAD", direct_file('b', format)),
                file_of("refs/heads/main", direct_file('a', format)),
                file_of("refs/heads/alias", symbolic_file("refs/heads/main")),
                file_of("refs/tags/v1", direct_file('c', format)),
                file_of("refs/heads/deep/er", direct_file('b', format)),
            ],
            &[],
        )
        .unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(
            agreement.head,
            HeadStateV1::Symbolic {
                target: "refs/heads/main".into(),
                resolved: Some(oid_of('a', format)),
            }
        );
        assert_eq!(agreement.loose_refs, 4, "{format:?}");
        assert_eq!(agreement.symbolic_refs, 1, "{format:?}");
        assert_eq!(agreement.effective_refs, 4, "{format:?}");
        assert_eq!(agreement.orig_head, Some(oid_of('b', format)));
        assert_eq!((agreement.packed_refs, agreement.peeled_rows), (0, 0));
        // A detached HEAD is the same grammar.
        let detached = pure(format, &[file_of("HEAD", direct_file('a', format))], &[])
            .unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(
            detached.head,
            HeadStateV1::Detached {
                oid: oid_of('a', format)
            }
        );
    }
}

/// Every malformed loose ref refuses with its own typed reason, for HEAD and for a loose ref alike:
/// nothing is trimmed, lowercased, or defaulted.
#[test]
fn b1b_malformed_loose_refs_refuse_with_typed_reasons() {
    for format in BOTH_FORMATS_V1 {
        let good = oid_of('a', format);
        let other_width = oid_of(
            'a',
            if oid_len(format) == 40 {
                GitObjectFormatV1::Sha256
            } else {
                GitObjectFormatV1::Sha1
            },
        );
        let upper = good.to_uppercase();
        let rows: Vec<(&str, Vec<u8>, RowFaultV1)> = vec![
            ("empty", b"".to_vec(), RowFaultV1::EmptyFile),
            (
                "no newline",
                good.clone().into_bytes(),
                RowFaultV1::PartialLine,
            ),
            (
                "no newline, symbolic",
                b"ref: refs/heads/main".to_vec(),
                RowFaultV1::PartialLine,
            ),
            ("lone newline", b"\n".to_vec(), RowFaultV1::ObjectId),
            (
                "crlf",
                format!("{good}\r\n").into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "trailing space",
                format!("{good} \n").into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "leading space",
                format!(" {good}\n").into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "uppercase",
                format!("{upper}\n").into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "the other format's width",
                format!("{other_width}\n").into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "one hex digit short",
                format!("{}\n", &good[1..]).into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "a non-hex digit",
                format!("g{}\n", &good[1..]).into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "a NUL",
                format!("{}\0\n", &good[1..]).into_bytes(),
                RowFaultV1::ObjectId,
            ),
            (
                "a second line",
                format!("{good}\n{good}\n").into_bytes(),
                RowFaultV1::ExtraLine,
            ),
            (
                "a trailing blank line",
                format!("{good}\n\n").into_bytes(),
                RowFaultV1::ExtraLine,
            ),
            (
                "a second, partial line",
                format!("{good}\nx").into_bytes(),
                RowFaultV1::ExtraLine,
            ),
            (
                "ref: without its space",
                b"ref:refs/heads/main\n".to_vec(),
                RowFaultV1::ObjectId,
            ),
            (
                "a symbolic ref with two spaces",
                b"ref:  refs/heads/main\n".to_vec(),
                RowFaultV1::SymbolicTarget(RefNameFaultV1::NotUnderRefs),
            ),
            (
                "a symbolic target with a trailing space",
                b"ref: refs/heads/main \n".to_vec(),
                RowFaultV1::SymbolicTarget(RefNameFaultV1::Portable(PortableFaultV1::Unprintable)),
            ),
            (
                "a symbolic target with a CR",
                b"ref: refs/heads/main\r\n".to_vec(),
                RowFaultV1::SymbolicTarget(RefNameFaultV1::Portable(PortableFaultV1::Unprintable)),
            ),
            (
                "a symbolic target outside refs/",
                b"ref: HEAD\n".to_vec(),
                RowFaultV1::SymbolicTarget(RefNameFaultV1::NotUnderRefs),
            ),
            (
                "an empty symbolic target",
                b"ref: \n".to_vec(),
                RowFaultV1::SymbolicTarget(RefNameFaultV1::NotUnderRefs),
            ),
            (
                "a symbolic target with ..",
                b"ref: refs/heads/a..b\n".to_vec(),
                RowFaultV1::SymbolicTarget(RefNameFaultV1::DoubleDot),
            ),
            (
                "a symbolic target ending .lock",
                b"ref: refs/heads/x.lock\n".to_vec(),
                RowFaultV1::SymbolicTarget(RefNameFaultV1::LockSuffix),
            ),
        ];
        for path in ["HEAD", "refs/heads/x"] {
            for (row, content, why) in &rows {
                let result = pure(format, &[file_of(path, content.clone())], &[]);
                assert_eq!(
                    fault_of(result, row),
                    row_fault(path, why.clone()),
                    "{format:?} {path}: {row}"
                );
            }
        }
    }
}

/// ORIG_HEAD is only ever a direct object id: a symbolic ORIG_HEAD refuses, and so does every malformed
/// spelling.
#[test]
fn b1b_orig_head_is_a_direct_object_id_only() {
    for format in BOTH_FORMATS_V1 {
        let mut files = base_files(format);
        files.push(file_of("ORIG_HEAD", symbolic_file("refs/heads/main")));
        assert_eq!(
            fault_of(pure(format, &files, &[]), "symbolic ORIG_HEAD"),
            row_fault("ORIG_HEAD", RowFaultV1::ObjectId)
        );
        for (row, content, why) in [
            ("empty", b"".to_vec(), RowFaultV1::EmptyFile),
            (
                "no newline",
                oid_of('a', format).into_bytes(),
                RowFaultV1::PartialLine,
            ),
            (
                "two lines",
                format!("{0}\n{0}\n", oid_of('a', format)).into_bytes(),
                RowFaultV1::ExtraLine,
            ),
        ] {
            let mut files = base_files(format);
            files.push(file_of("ORIG_HEAD", content));
            assert_eq!(
                fault_of(pure(format, &files, &[]), row),
                row_fault("ORIG_HEAD", why),
                "{row}"
            );
        }
    }
}

/// Which paths are refs files: the allowlist routes only HEAD, ORIG_HEAD, FETCH_HEAD, packed-refs,
/// refs/**, and logs/HEAD or logs/refs/**. Any other file is refused, whatever it holds, and a ref
/// file under a forbidden name is refused by its name. A path may be admitted once.
#[test]
fn b1b_only_refs_files_are_admitted_and_each_once() {
    let format = GitObjectFormatV1::Sha1;
    for path in [
        "config",
        "index",
        "shallow",
        "MERGE_HEAD",
        "hooks/pre-commit",
        "objects/pack/x",
        "info/refs",
        "refs",
        "logs",
        "logs/foo",
        "logs/refs",
        "refs.lock",
        "head",
        "HEAD/x",
        "packed-refs/x",
        "",
    ] {
        let mut builder = RefsBuilderV1::new(format, RefsLimitsV1::v1(), far_deadline());
        let mut reader: &[u8] = b"anything\n";
        let error = builder
            .admit_file(path.as_bytes(), &mut reader)
            .expect_err(path);
        assert_eq!(
            refs_fault(error),
            RefsFaultV1::UnexpectedFile { path: path.into() },
            "{path}"
        );
    }
    for (path, why) in [
        ("refs/heads/x.lock", RefNameFaultV1::LockSuffix),
        ("refs/heads/.x", RefNameFaultV1::LeadingDot),
        ("refs/heads/a..b", RefNameFaultV1::DoubleDot),
        (
            "refs/heads/a b",
            RefNameFaultV1::Portable(PortableFaultV1::Unprintable),
        ),
    ] {
        let mut files = base_files(format);
        files.push(file_of(path, direct_file('a', format)));
        assert_eq!(
            fault_of(pure(format, &files, &[]), path),
            name_fault(path, why),
            "{path}"
        );
    }
    for path in [
        "HEAD",
        "ORIG_HEAD",
        "FETCH_HEAD",
        "packed-refs",
        "refs/heads/main",
        "logs/HEAD",
        "logs/refs/heads/main",
    ] {
        let mut builder = RefsBuilderV1::new(format, RefsLimitsV1::v1(), far_deadline());
        let content: &[u8] = match path {
            "HEAD" => b"ref: refs/heads/main\n",
            "packed-refs" | "FETCH_HEAD" | "logs/HEAD" | "logs/refs/heads/main" => b"",
            _ => b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        };
        let mut reader: &[u8] = content;
        builder
            .admit_file(path.as_bytes(), &mut reader)
            .expect("the first admission");
        let mut again: &[u8] = content;
        let error = builder
            .admit_file(path.as_bytes(), &mut again)
            .expect_err(path);
        assert_eq!(
            refs_fault(error),
            RefsFaultV1::Duplicate { path: path.into() },
            "{path}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// packed-refs
// ---------------------------------------------------------------------------------------------

const PACKED_HEADER_V1: &str = "# pack-refs with: peeled fully-peeled sorted \n";

fn packed(format: GitObjectFormatV1, header: &str, rows: &[(&str, char, Option<char>)]) -> Vec<u8> {
    let mut text = header.to_owned();
    for (name, fill, peeled) in rows {
        text.push_str(&format!("{} {name}\n", oid_of(*fill, format)));
        if let Some(peeled) = peeled {
            text.push_str(&format!("^{}\n", oid_of(*peeled, format)));
        }
    }
    text.into_bytes()
}

/// A real-shaped packed-refs file parses in both formats: the header line, sorted rows, a peeled row
/// under its annotated tag, a packed-only ref, and a ref that a loose one also names (the loose one wins).
#[test]
fn b1b_packed_refs_parse_with_peeled_rows_and_loose_precedence() {
    for format in BOTH_FORMATS_V1 {
        let mut files = vec![file_of("HEAD", symbolic_file("refs/heads/main"))];
        files.push(file_of(
            "packed-refs",
            packed(
                format,
                PACKED_HEADER_V1,
                &[
                    ("refs/heads/main", 'a', None),
                    ("refs/heads/other", 'b', None),
                    ("refs/tags/v1", 'c', Some('a')),
                    ("refs/tags/v2", 'a', None),
                ],
            ),
        ));
        // A loose ref hides the packed one of the same name.
        files.push(file_of("refs/heads/other", direct_file('a', format)));
        let agreement = pure(
            format,
            &files,
            &[
                ("HEAD", WantV1::Symbolic("refs/heads/main")),
                ("refs/heads/main", WantV1::Direct('a')),
                ("refs/heads/other", WantV1::Direct('a')),
                ("refs/tags/v1", WantV1::Direct('c')),
            ],
        )
        .unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(agreement.packed_refs, 4, "{format:?}");
        assert_eq!(agreement.peeled_rows, 1, "{format:?}");
        assert_eq!(agreement.loose_refs, 1, "{format:?}");
        assert_eq!(agreement.effective_refs, 4, "{format:?}");
        assert_eq!(agreement.manifest_refs, 4, "{format:?}");
        assert_eq!(
            agreement.head,
            HeadStateV1::Symbolic {
                target: "refs/heads/main".into(),
                resolved: Some(oid_of('a', format)),
            }
        );
    }
}

/// Packed-refs headers: the exact Git header, any subset of its three traits, or none at all, and an
/// empty file or header-only file is a valid packed-refs with no rows.
#[test]
fn b1b_packed_refs_headers_and_empty_files() {
    let format = GitObjectFormatV1::Sha1;
    let rows = [("refs/heads/main", 'a', None)];
    for header in [
        PACKED_HEADER_V1,
        "# pack-refs with: peeled fully-peeled sorted\n",
        "# pack-refs with: peeled\n",
        "# pack-refs with: sorted\n",
        "# pack-refs with:\n",
        "# pack-refs with: \n",
        "",
    ] {
        let mut files = vec![file_of("HEAD", symbolic_file("refs/heads/main"))];
        files.push(file_of("packed-refs", packed(format, header, &rows)));
        pure(format, &files, &[])
            .unwrap_or_else(|error| panic!("header {header:?} must parse: {error:?}"));
    }
    for content in [&b""[..], b"# pack-refs with: peeled fully-peeled sorted \n"] {
        let mut files = vec![file_of("HEAD", symbolic_file("refs/heads/main"))];
        files.push(file_of("packed-refs", content.to_vec()));
        let agreement = pure(format, &files, &[]).expect("a packed-refs with no rows");
        assert_eq!(agreement.packed_refs, 0);
        assert_eq!(agreement.effective_refs, 0);
    }
}

/// Every malformed packed-refs refuses with its own typed reason.
#[test]
fn b1b_malformed_packed_refs_refuse_with_typed_reasons() {
    for format in BOTH_FORMATS_V1 {
        let a = oid_of('a', format);
        let c = oid_of('c', format);
        let b = oid_of('b', format);
        let header = PACKED_HEADER_V1;
        let rows: Vec<(&str, String, RefsFaultV1)> = vec![
            (
                "an unknown trait",
                "# pack-refs with: peeled bogus \n".into(),
                row_fault("packed-refs", RowFaultV1::PackedHeader),
            ),
            (
                "a repeated trait",
                "# pack-refs with: sorted sorted \n".into(),
                row_fault("packed-refs", RowFaultV1::PackedHeader),
            ),
            (
                "a header without the colon",
                "# pack-refs with peeled\n".into(),
                row_fault("packed-refs", RowFaultV1::PackedHeader),
            ),
            (
                "a header glued to its traits",
                "# pack-refs with:peeled\n".into(),
                row_fault("packed-refs", RowFaultV1::PackedHeader),
            ),
            (
                "a comment that is not the header",
                format!("# comment\n{a} refs/heads/main\n"),
                row_fault("packed-refs", RowFaultV1::PackedHeader),
            ),
            (
                "a second header",
                format!("{header}{a} refs/heads/main\n{header}"),
                row_fault("packed-refs", RowFaultV1::PackedHeader),
            ),
            (
                "a header after a row",
                format!("{a} refs/heads/main\n{header}"),
                row_fault("packed-refs", RowFaultV1::PackedHeader),
            ),
            (
                "a row without a name",
                format!("{header}{a}\n"),
                row_fault("packed-refs", RowFaultV1::PackedRow),
            ),
            (
                "a row with two spaces",
                format!("{header}{a}  refs/heads/main\n"),
                name_fault(" refs/heads/main", RefNameFaultV1::NotUnderRefs),
            ),
            (
                "a row with a bad object id",
                format!("{header}{} refs/heads/main\n", a.to_uppercase()),
                row_fault("packed-refs", RowFaultV1::ObjectId),
            ),
            (
                "a row with a short object id",
                format!("{header}{} refs/heads/main\n", &a[1..]),
                row_fault("packed-refs", RowFaultV1::ObjectId),
            ),
            (
                "a row with a bad name",
                format!("{header}{a} refs/heads/x.lock\n"),
                name_fault("refs/heads/x.lock", RefNameFaultV1::LockSuffix),
            ),
            (
                "a row outside refs/",
                format!("{header}{a} HEAD\n"),
                name_fault("HEAD", RefNameFaultV1::NotUnderRefs),
            ),
            (
                "a row with a CR",
                format!("{header}{a} refs/heads/main\r\n"),
                name_fault(
                    "refs/heads/main\r",
                    RefNameFaultV1::Portable(PortableFaultV1::Unprintable),
                ),
            ),
            (
                "a missing final newline",
                format!("{header}{a} refs/heads/main"),
                row_fault("packed-refs", RowFaultV1::PartialLine),
            ),
            (
                "a blank line",
                format!("{header}\n{a} refs/heads/main\n"),
                row_fault("packed-refs", RowFaultV1::PackedRow),
            ),
            (
                "a peeled row first",
                format!("{header}^{a}\n{a} refs/heads/main\n"),
                row_fault("packed-refs", RowFaultV1::PeeledOrphan),
            ),
            (
                "two peeled rows",
                format!("{header}{c} refs/tags/v1\n^{a}\n^{b}\n"),
                row_fault("packed-refs", RowFaultV1::PeeledOrphan),
            ),
            (
                "a peeled row with a bad object id",
                format!("{header}{c} refs/tags/v1\n^{}\n", a.to_uppercase()),
                row_fault("packed-refs", RowFaultV1::ObjectId),
            ),
            (
                "an empty peeled row",
                format!("{header}{c} refs/tags/v1\n^\n"),
                row_fault("packed-refs", RowFaultV1::ObjectId),
            ),
            (
                "rows out of order under the sorted trait",
                format!("{header}{a} refs/heads/b\n{a} refs/heads/a\n"),
                row_fault("packed-refs", RowFaultV1::PackedOrder),
            ),
            (
                "a repeated name under the sorted trait",
                format!("{header}{a} refs/heads/a\n{a} refs/heads/a\n"),
                row_fault("packed-refs", RowFaultV1::PackedOrder),
            ),
            (
                "a repeated name without the sorted trait",
                format!("# pack-refs with: peeled \n{a} refs/heads/a\n{b} refs/heads/a\n"),
                row_fault("packed-refs", RowFaultV1::PackedDuplicate),
            ),
        ];
        for (row, content, expected) in rows {
            let files = vec![
                file_of("HEAD", symbolic_file("refs/heads/main")),
                file_of("packed-refs", content.into_bytes()),
            ];
            assert_eq!(
                fault_of(pure(format, &files, &[]), row),
                expected,
                "{format:?}: {row}"
            );
        }
        // Out of order without the sorted trait is fine: only the header promises an order.
        let files = vec![
            file_of("HEAD", symbolic_file("refs/heads/main")),
            file_of(
                "packed-refs",
                format!("# pack-refs with: peeled \n{a} refs/heads/b\n{a} refs/heads/a\n")
                    .into_bytes(),
            ),
        ];
        pure(format, &files, &[]).expect("unsorted rows without the sorted trait");
    }
}

// ---------------------------------------------------------------------------------------------
// FETCH_HEAD and reflogs
// ---------------------------------------------------------------------------------------------

fn fetch_head(format: GitObjectFormatV1, rows: &[(char, &str, &str)]) -> Vec<u8> {
    rows.iter()
        .map(|(fill, flag, note)| format!("{}\t{flag}\t{note}\n", oid_of(*fill, format)))
        .collect::<String>()
        .into_bytes()
}

/// FETCH_HEAD rows are `<oid> TAB (empty | not-for-merge) TAB <note> LF`; the first row's object is the
/// ref's value, and every row's object must be in the inventory. Empty is a valid FETCH_HEAD.
#[test]
fn b1b_fetch_head_rows_parse_and_the_first_row_is_the_value() {
    for format in BOTH_FORMATS_V1 {
        let mut files = base_files(format);
        files.push(file_of(
            "FETCH_HEAD",
            fetch_head(
                format,
                &[
                    ('b', "", "branch 'main' of https://example.invalid/r"),
                    (
                        'a',
                        "not-for-merge",
                        "branch 'side' of https://example.invalid/r",
                    ),
                    (
                        'c',
                        "not-for-merge",
                        "tag 'v1' of https://example.invalid/r",
                    ),
                    ('d', "", ""),
                ],
            ),
        ));
        let agreement = pure(format, &files, &[("FETCH_HEAD", WantV1::Direct('b'))])
            .unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(agreement.fetch_head_rows, 4);
        assert_eq!(agreement.manifest_refs, 1);
        // The second row's object is not the ref's value.
        assert_eq!(
            fault_of(
                pure(format, &files, &[("FETCH_HEAD", WantV1::Direct('a'))]),
                "the second row"
            ),
            RefsFaultV1::OriginalMismatch {
                name: "FETCH_HEAD".into(),
                expected: format!("direct {}", oid_of('a', format)),
                found: format!("direct {}", oid_of('b', format)),
            }
        );
        let mut empty_files = base_files(format);
        empty_files.push(file_of("FETCH_HEAD", Vec::new()));
        let agreement = pure(format, &empty_files, &[]).expect("an empty FETCH_HEAD");
        assert_eq!(agreement.fetch_head_rows, 0);
        // An empty FETCH_HEAD has no value for the manifest's FETCH_HEAD.
        assert_eq!(
            fault_of(
                pure(format, &empty_files, &[("FETCH_HEAD", WantV1::Direct('a'))]),
                "no first row"
            ),
            RefsFaultV1::MissingOriginal {
                name: "FETCH_HEAD".into()
            }
        );
    }
}

#[test]
fn b1b_malformed_fetch_head_refuses_with_typed_reasons() {
    for format in BOTH_FORMATS_V1 {
        let a = oid_of('a', format);
        let rows: Vec<(&str, String, RowFaultV1)> = vec![
            (
                "a bad flag",
                format!("{a}\tmerge\tnote\n"),
                RowFaultV1::FetchHeadRow,
            ),
            (
                "no tabs",
                format!("{a} not-for-merge note\n"),
                RowFaultV1::FetchHeadRow,
            ),
            ("one tab", format!("{a}\t\n"), RowFaultV1::FetchHeadRow),
            ("a bare oid", format!("{a}\n"), RowFaultV1::FetchHeadRow),
            (
                "an upper-case oid",
                format!("{}\t\tnote\n", a.to_uppercase()),
                RowFaultV1::ObjectId,
            ),
            (
                "a short oid",
                format!("{}\t\tnote\n", &a[1..]),
                RowFaultV1::ObjectId,
            ),
            (
                "no final newline",
                format!("{a}\t\tnote"),
                RowFaultV1::PartialLine,
            ),
            (
                "a blank row",
                format!("{a}\t\tnote\n\n"),
                RowFaultV1::FetchHeadRow,
            ),
            (
                "a NUL in the note",
                format!("{a}\t\tno\0te\n"),
                RowFaultV1::FetchHeadRow,
            ),
        ];
        for (row, content, why) in rows {
            let mut files = base_files(format);
            files.push(file_of("FETCH_HEAD", content.into_bytes()));
            assert_eq!(
                fault_of(pure(format, &files, &[]), row),
                row_fault("FETCH_HEAD", why),
                "{format:?}: {row}"
            );
        }
    }
}

fn log_row(format: GitObjectFormatV1, old: char, new: char, tail: &str) -> String {
    format!("{} {} {tail}\n", oid_of(old, format), oid_of(new, format))
}

const IDENT_V1: &str = "A U Thor <author@example.invalid> 1700000000 +0000";

/// Reflogs are `logs/HEAD` and `logs/refs/<ref name>`: each row is two object ids, an identity ending in
/// `>`, a timestamp, a zone, and an optional TAB message; the object ids need not be in the inventory (a
/// reflog may name an object that is gone), but they are the format's width. An empty log is valid.
#[test]
fn b1b_reflogs_parse_streaming_and_count_their_rows() {
    for format in BOTH_FORMATS_V1 {
        let mut files = base_files(format);
        let head_log = [
            log_row(
                format,
                '0',
                'a',
                &format!("{IDENT_V1}\tcommit (initial): one"),
            ),
            log_row(format, 'a', 'b', &format!("{IDENT_V1}\tcheckout: moving")),
            log_row(format, 'b', 'a', IDENT_V1),
            log_row(
                format,
                '0',
                '9',
                "N <n@e> 0 -1200\tmessage with <angle> brackets and a\u{e9}",
            ),
        ]
        .concat();
        files.push(file_of("logs/HEAD", head_log.into_bytes()));
        files.push(file_of(
            "logs/refs/heads/main",
            log_row(format, '0', 'a', IDENT_V1).into_bytes(),
        ));
        files.push(file_of("logs/refs/stash", Vec::new()));
        let agreement =
            pure(format, &files, &[]).unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(agreement.reflog_files, 3, "{format:?}");
        assert_eq!(agreement.reflog_rows, 5, "{format:?}");
    }
}

#[test]
fn b1b_malformed_reflogs_refuse_with_typed_reasons() {
    for format in BOTH_FORMATS_V1 {
        let (a, b) = (oid_of('a', format), oid_of('b', format));
        let rows: Vec<(&str, String, RowFaultV1)> = vec![
            (
                "one object id",
                format!("{a} {IDENT_V1}\n"),
                RowFaultV1::LogRow,
            ),
            (
                "an upper-case old id",
                format!("{} {b} {IDENT_V1}\n", a.to_uppercase()),
                RowFaultV1::LogRow,
            ),
            (
                "a short new id",
                format!("{a} {} {IDENT_V1}\n", &b[1..]),
                RowFaultV1::LogRow,
            ),
            (
                "two spaces after the old id",
                format!("{a}  {b} {IDENT_V1}\n"),
                RowFaultV1::LogRow,
            ),
            (
                "an identity without an address",
                format!("{a} {b} A U Thor 1700000000 +0000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "an address that never opens",
                format!("{a} {b} A U Thor> 1700000000 +0000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "an address that never closes",
                format!("{a} {b} A U Thor <author@example.invalid 1700000000 +0000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "no identity",
                format!("{a} {b} 1700000000 +0000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "a timestamp that is not digits",
                format!("{a} {b} A <a@e> 17x0000000 +0000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "a missing timestamp",
                format!("{a} {b} A <a@e> +0000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "a zone without a sign",
                format!("{a} {b} A <a@e> 1700000000 0000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "a short zone",
                format!("{a} {b} A <a@e> 1700000000 +000\n"),
                RowFaultV1::LogRow,
            ),
            (
                "a zone with letters",
                format!("{a} {b} A <a@e> 1700000000 +00x0\n"),
                RowFaultV1::LogRow,
            ),
            (
                "a blank row",
                format!("{a} {b} {IDENT_V1}\n\n"),
                RowFaultV1::LogRow,
            ),
            (
                "a NUL in the message",
                format!("{a} {b} {IDENT_V1}\tme\0ssage\n"),
                RowFaultV1::LogRow,
            ),
            (
                "no final newline",
                format!("{a} {b} {IDENT_V1}"),
                RowFaultV1::PartialLine,
            ),
        ];
        for (row, content, why) in rows {
            let mut files = base_files(format);
            files.push(file_of("logs/HEAD", content.into_bytes()));
            assert_eq!(
                fault_of(pure(format, &files, &[]), row),
                row_fault("logs/HEAD", why),
                "{format:?}: {row}"
            );
        }
    }
}

/// A reflog's path is judged as a ref name: `logs/refs/heads/x.lock` and `logs/refs/a..b` refuse, and
/// only `logs/HEAD` and `logs/refs/**` are reflogs.
#[test]
fn b1b_reflog_paths_are_ref_names() {
    let format = GitObjectFormatV1::Sha1;
    for (path, name, why) in [
        (
            "logs/refs/heads/x.lock",
            "refs/heads/x.lock",
            RefNameFaultV1::LockSuffix,
        ),
        ("logs/refs/a..b", "refs/a..b", RefNameFaultV1::DoubleDot),
        (
            "logs/refs/heads/a b",
            "refs/heads/a b",
            RefNameFaultV1::Portable(PortableFaultV1::Unprintable),
        ),
    ] {
        let mut files = base_files(format);
        files.push(file_of(path, Vec::new()));
        assert_eq!(
            fault_of(pure(format, &files, &[]), path),
            name_fault(name, why),
            "{path}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Agreement: HEAD states, precedence, resolution, kinds, targets, and the manifest
// ---------------------------------------------------------------------------------------------

/// Attached, detached, and unborn HEAD agree with the manifest in both formats; an unborn HEAD needs no
/// object at all (an Empty database), and `Unborn` and `Symbolic` both describe it.
#[test]
fn b1b_attached_detached_and_unborn_heads_agree_with_the_manifest() {
    for format in BOTH_FORMATS_V1 {
        let attached = pure(
            format,
            &base_files(format),
            &[
                ("HEAD", WantV1::Symbolic("refs/heads/main")),
                ("refs/heads/main", WantV1::Direct('a')),
            ],
        )
        .unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(
            attached.head,
            HeadStateV1::Symbolic {
                target: "refs/heads/main".into(),
                resolved: Some(oid_of('a', format)),
            }
        );
        assert_eq!((attached.manifest_refs, attached.direct_targets), (2, 1));

        let detached = pure(
            format,
            &[file_of("HEAD", direct_file('a', format))],
            &[("HEAD", WantV1::Direct('a'))],
        )
        .unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(
            detached.head,
            HeadStateV1::Detached {
                oid: oid_of('a', format)
            }
        );
        assert_eq!((detached.manifest_refs, detached.direct_targets), (1, 1));

        // Unborn: an Empty database, a HEAD naming a branch that does not exist.
        let unborn_files = [file_of("HEAD", symbolic_file("refs/heads/main"))];
        for refs in [
            &[][..],
            &[("HEAD", WantV1::Symbolic("refs/heads/main"))][..],
            &[("HEAD", WantV1::Unborn)][..],
        ] {
            let unborn = pure_run(format, false, &unborn_files, refs, RefsLimitsV1::v1())
                .unwrap_or_else(|error| panic!("{format:?} {}: {error:?}", refs.len()));
            assert_eq!(
                unborn.head,
                HeadStateV1::Symbolic {
                    target: "refs/heads/main".into(),
                    resolved: None,
                }
            );
            assert_eq!((unborn.direct_targets, unborn.effective_refs), (0, 0));
        }
        // Detached HEAD in an Empty database has no object to name: absent, not unborn.
        assert_eq!(
            fault_of(
                pure_run(
                    format,
                    false,
                    &[file_of("HEAD", direct_file('a', format))],
                    &[],
                    RefsLimitsV1::v1()
                ),
                "an empty inventory"
            ),
            RefsFaultV1::TargetAbsent {
                name: "HEAD".into(),
                oid: oid_of('a', format)
            }
        );
        // No HEAD at all is not an unborn HEAD: exactly one valid HEAD is required.
        assert_eq!(
            fault_of(
                pure(
                    format,
                    &[file_of("refs/heads/main", direct_file('a', format))],
                    &[]
                ),
                "no HEAD"
            ),
            RefsFaultV1::MissingHead
        );
    }
}

/// A loose ref hides a packed ref of the same name (direct or symbolic); a packed-only ref is a ref and
/// can be HEAD's target. A packed row that a loose ref hides still names an object that must exist.
#[test]
fn b1b_loose_refs_win_over_packed_refs_and_packed_only_refs_count() {
    let format = GitObjectFormatV1::Sha1;
    let header = PACKED_HEADER_V1;
    let rows = |extra: &[(&str, char, Option<char>)]| packed(format, header, extra);
    let head = file_of("HEAD", symbolic_file("refs/heads/main"));

    let mut files = vec![
        head.clone(),
        file_of("packed-refs", rows(&[("refs/heads/main", 'a', None)])),
        file_of("refs/heads/main", direct_file('b', format)),
    ];
    pure(format, &files, &[("refs/heads/main", WantV1::Direct('b'))]).expect("the loose value");
    assert_eq!(
        fault_of(
            pure(format, &files, &[("refs/heads/main", WantV1::Direct('a'))]),
            "the packed value is hidden"
        ),
        RefsFaultV1::OriginalMismatch {
            name: "refs/heads/main".into(),
            expected: format!("direct {}", oid_of('a', format)),
            found: format!("direct {}", oid_of('b', format)),
        }
    );
    // HEAD resolves through the loose value, not the packed one.
    let agreement = pure(format, &files, &[]).expect("loose over packed");
    assert_eq!(
        agreement.head,
        HeadStateV1::Symbolic {
            target: "refs/heads/main".into(),
            resolved: Some(oid_of('b', format)),
        }
    );

    // Packed only: HEAD resolves through the packed row.
    files = vec![
        head.clone(),
        file_of("packed-refs", rows(&[("refs/heads/main", 'a', None)])),
    ];
    let agreement = pure(format, &files, &[("refs/heads/main", WantV1::Direct('a'))])
        .expect("a packed-only ref");
    assert_eq!(
        agreement.head,
        HeadStateV1::Symbolic {
            target: "refs/heads/main".into(),
            resolved: Some(oid_of('a', format)),
        }
    );
    assert_eq!(agreement.loose_refs, 0);

    // A loose symbolic ref hides a packed direct one.
    files = vec![
        head.clone(),
        file_of(
            "packed-refs",
            rows(&[
                ("refs/heads/alias", 'a', None),
                ("refs/heads/main", 'a', None),
            ]),
        ),
        file_of("refs/heads/alias", symbolic_file("refs/heads/main")),
    ];
    pure(
        format,
        &files,
        &[("refs/heads/alias", WantV1::Symbolic("refs/heads/main"))],
    )
    .expect("the loose symbolic ref wins");
    assert_eq!(
        fault_of(
            pure(format, &files, &[("refs/heads/alias", WantV1::Direct('a'))]),
            "the packed direct value is hidden"
        ),
        RefsFaultV1::OriginalMismatch {
            name: "refs/heads/alias".into(),
            expected: format!("direct {}", oid_of('a', format)),
            found: "symbolic refs/heads/main".into(),
        }
    );

    // A packed row that a loose ref hides is not an active target: its stale object (here absent from the
    // inventory, as after `pack-refs`, a branch advance, and a prune) is not required.
    files = vec![
        head,
        file_of("packed-refs", rows(&[("refs/heads/hidden", '7', None)])),
        file_of("refs/heads/hidden", direct_file('a', format)),
    ];
    let agreement = pure(
        format,
        &files,
        &[("refs/heads/hidden", WantV1::Direct('a'))],
    )
    .expect("a shadowed stale packed row");
    assert_eq!((agreement.packed_refs, agreement.effective_refs), (1, 1));
    assert_eq!(
        agreement.direct_targets, 1,
        "only the loose value is active"
    );
}

/// A shadowed packed row is judged structurally and nothing more: its object, its kind, and its peeled
/// value (absent, of the wrong kind, a tag without a peel under `fully-peeled`) are never consulted, while
/// every grammar rule, order, and name rule still applies to it, a row nothing shadows is still held to
/// all of them, and a different name shadows nothing.
#[test]
fn b1b_a_shadowed_packed_row_is_not_an_active_target() {
    for format in BOTH_FORMATS_V1 {
        let missing = '7';
        let head = file_of("HEAD", symbolic_file("refs/heads/main"));
        let header = PACKED_HEADER_V1;
        let loose = |path: &'static str, fill: char| file_of(path, direct_file(fill, format));
        let rows = |list: &[(&str, char, Option<char>)]| {
            file_of("packed-refs", packed(format, header, list))
        };
        // Stale objects the inventory lacks, a wrong kind under a branch, a tag with no peeled row, a peeled
        // value that is absent and one that is itself a tag: all behind loose refs of the same names.
        let stale = rows(&[
            ("refs/heads/absent", missing, None),
            ("refs/heads/blob", 'e', None),
            ("refs/tags/bare", 'c', None),
            ("refs/tags/gone", 'c', Some(missing)),
            ("refs/tags/nontag", 'a', Some('b')),
            ("refs/tags/tagged", 'c', Some('c')),
        ]);
        let shadows = vec![
            head.clone(),
            stale.clone(),
            loose("refs/heads/absent", 'a'),
            loose("refs/heads/blob", 'b'),
            loose("refs/tags/bare", 'c'),
            loose("refs/tags/gone", 'c'),
            loose("refs/tags/tagged", 'c'),
            // A loose SYMBOLIC ref hides a packed one too.
            file_of("refs/tags/nontag", symbolic_file("refs/heads/absent")),
        ];
        let agreement = pure(format, &shadows, &[])
            .unwrap_or_else(|error| panic!("{format:?}: the stale rows are shadowed: {error:?}"));
        assert_eq!(agreement.packed_refs, 6, "{format:?}");
        assert_eq!(agreement.peeled_rows, 3, "{format:?}");
        assert_eq!(agreement.effective_refs, 6, "{format:?}");
        // The active direct objects are only the loose ones: a, b, and c.
        assert_eq!(agreement.direct_targets, 3, "{format:?}");

        // Nothing shadows a row of another name, and an unshadowed row is held to every closure rule.
        let kind_fault = |name: &str, oid: char, found: &'static str| RefsFaultV1::TargetKind {
            name: name.into(),
            oid: oid_of(oid, format),
            found,
            expected: "commit",
        };
        let rejected: Vec<(&str, Vec<FileV1>, RefsFaultV1)> = vec![
            (
                "an absent object, shadowed by another name",
                vec![
                    head.clone(),
                    rows(&[("refs/heads/absent", missing, None)]),
                    loose("refs/heads/absent-not", 'a'),
                ],
                RefsFaultV1::TargetAbsent {
                    name: "refs/heads/absent".into(),
                    oid: oid_of(missing, format),
                },
            ),
            (
                "an absent object, shadowed only by its prefix",
                vec![
                    head.clone(),
                    rows(&[("refs/heads/absent", missing, None)]),
                    loose("refs/heads", 'a'),
                ],
                // `refs/heads` as a ref conflicts with the packed name under it: structure is judged first.
                RefsFaultV1::Conflict {
                    name: "refs/heads/absent".into(),
                    with: "refs/heads".into(),
                },
            ),
            (
                "a branch on a blob",
                vec![head.clone(), rows(&[("refs/heads/blob", 'e', None)])],
                kind_fault("refs/heads/blob", 'e', "blob"),
            ),
            (
                "a tag with no peeled row under fully-peeled",
                vec![head.clone(), rows(&[("refs/tags/bare", 'c', None)])],
                RefsFaultV1::PeelMissing {
                    name: "refs/tags/bare".into(),
                },
            ),
            (
                "an absent peeled value",
                vec![
                    head.clone(),
                    rows(&[("refs/tags/gone", 'c', Some(missing))]),
                ],
                RefsFaultV1::TargetAbsent {
                    name: "refs/tags/gone".into(),
                    oid: oid_of(missing, format),
                },
            ),
            (
                "a peeled value that is a tag",
                vec![head.clone(), rows(&[("refs/tags/tagged", 'c', Some('c'))])],
                RefsFaultV1::PeeledIsTag {
                    name: "refs/tags/tagged".into(),
                },
            ),
            (
                "a peeled row under a commit",
                vec![head.clone(), rows(&[("refs/tags/nontag", 'a', Some('b'))])],
                RefsFaultV1::PeeledNotTag {
                    name: "refs/tags/nontag".into(),
                },
            ),
        ];
        for (row, files, expected) in rejected {
            assert_eq!(
                fault_of(pure(format, &files, &[]), row),
                expected,
                "{format:?}: {row}"
            );
        }

        // The grammar of a shadowed row is still judged: a malformed row, a bad name, a repeated name, and
        // rows out of order under `sorted` refuse even when a loose ref shadows every one of them.
        let a = oid_of('a', format);
        let bad: Vec<(&str, String, RefsFaultV1)> = vec![
            (
                "a bad object id",
                format!("{header}{} refs/heads/main\n", a.to_uppercase()),
                row_fault("packed-refs", RowFaultV1::ObjectId),
            ),
            (
                "a bad name",
                format!("{header}{a} refs/heads/main.lock\n"),
                name_fault("refs/heads/main.lock", RefNameFaultV1::LockSuffix),
            ),
            (
                "rows out of order",
                format!("{header}{a} refs/heads/main\n{a} refs/heads/a\n"),
                row_fault("packed-refs", RowFaultV1::PackedOrder),
            ),
            (
                "an orphan peeled row",
                format!("{header}^{a}\n{a} refs/heads/main\n"),
                row_fault("packed-refs", RowFaultV1::PeeledOrphan),
            ),
        ];
        for (row, content, expected) in bad {
            let files = vec![
                head.clone(),
                file_of("packed-refs", content.into_bytes()),
                loose("refs/heads/main", 'a'),
                loose("refs/heads/a", 'a'),
            ];
            assert_eq!(
                fault_of(pure(format, &files, &[]), row),
                expected,
                "{format:?}: {row}"
            );
        }

        // Case collisions and conflicts are still judged across shadowed names.
        let files = vec![
            head.clone(),
            rows(&[("refs/heads/Main", missing, None)]),
            loose("refs/heads/main", 'a'),
        ];
        assert_eq!(
            fault_of(pure(format, &files, &[]), "a shadowed collision"),
            RefsFaultV1::Collision {
                name: "refs/heads/main".into(),
                with: "refs/heads/Main".into(),
            },
            "{format:?}"
        );
    }
}

/// `n` symbolic refs starting at HEAD, each naming the next, the last naming `last`.
fn chain_files(n: usize, last: &'static str, format: GitObjectFormatV1) -> Vec<FileV1> {
    let names: Vec<&'static str> = (1..n)
        .map(|index| &*Box::leak(format!("refs/heads/s{index}").into_boxed_str()))
        .collect();
    let mut files = Vec::new();
    for index in 0..n {
        let target = names.get(index).copied().unwrap_or(last);
        let path = if index == 0 { "HEAD" } else { names[index - 1] };
        files.push(file_of(path, symbolic_file(target)));
    }
    files.push(file_of("refs/heads/main", direct_file('a', format)));
    files
}

/// A chain may follow at most five symbolic refs (Git's own bound): five, ending in a value or in an
/// unborn branch, resolve; six refuse. Ending in an absent ref is unborn only for HEAD or a manifest
/// `Unborn`, so the unborn chain declares each hop `Unborn`.
#[test]
fn b1b_symbolic_chains_are_bounded_at_five() {
    for format in BOTH_FORMATS_V1 {
        let agreement = pure(format, &chain_files(5, "refs/heads/main", format), &[])
            .unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(
            agreement.head,
            HeadStateV1::Symbolic {
                target: "refs/heads/s1".into(),
                resolved: Some(oid_of('a', format)),
            }
        );
        assert_eq!(agreement.symbolic_refs, 4);
        assert_eq!(
            fault_of(
                pure(format, &chain_files(6, "refs/heads/main", format), &[]),
                "six symbolic refs"
            ),
            RefsFaultV1::TooDeep {
                name: "HEAD".into()
            }
        );
        // The same bound for an unborn tail.
        let unborn = |n: usize| {
            let mut files = chain_files(n, "refs/heads/gone", format);
            files.pop();
            let refs: Vec<(&str, WantV1)> = std::iter::once(("HEAD", WantV1::Unborn))
                .chain((1..n).map(|index| {
                    (
                        &*Box::leak(format!("refs/heads/s{index}").into_boxed_str()),
                        WantV1::Unborn,
                    )
                }))
                .collect();
            pure(format, &files, &refs)
        };
        let agreement = unborn(5).unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert!(matches!(
            agreement.head,
            HeadStateV1::Symbolic { resolved: None, .. }
        ));
        assert_eq!(
            fault_of(unborn(6), "six symbolic refs ending unborn"),
            RefsFaultV1::TooDeep {
                name: "HEAD".into()
            }
        );
    }
}

/// Cycles fail closed, whatever their shape: HEAD in a loop, a self loop, a loop that HEAD is not in, a
/// ring short enough to close within the bound (a cycle) and one too long to (too deep).
#[test]
fn b1b_symbolic_cycles_fail_closed() {
    let format = GitObjectFormatV1::Sha1;
    let main = file_of("refs/heads/main", direct_file('a', format));
    let cycle = |name: &str| RefsFaultV1::Cycle { name: name.into() };

    let files = vec![
        file_of("HEAD", symbolic_file("refs/heads/a")),
        file_of("refs/heads/a", symbolic_file("refs/heads/b")),
        file_of("refs/heads/b", symbolic_file("refs/heads/a")),
    ];
    assert_eq!(
        fault_of(pure(format, &files, &[]), "HEAD in a loop"),
        cycle("HEAD")
    );

    let files = vec![
        file_of("HEAD", symbolic_file("refs/heads/main")),
        main.clone(),
        file_of("refs/heads/self", symbolic_file("refs/heads/self")),
    ];
    assert_eq!(
        fault_of(pure(format, &files, &[]), "a self loop"),
        cycle("refs/heads/self")
    );

    let files = vec![
        file_of("HEAD", symbolic_file("refs/heads/main")),
        main.clone(),
        file_of("refs/heads/a", symbolic_file("refs/heads/b")),
        file_of("refs/heads/b", symbolic_file("refs/heads/a")),
    ];
    assert_eq!(
        fault_of(pure(format, &files, &[]), "a loop beside HEAD"),
        cycle("refs/heads/a")
    );

    // A loop through a packed ref cannot exist (packed refs are direct), so a symbolic loop in loose refs
    // is the only kind; rings of six and seven close at, and past, the bound.
    for (ring, expected) in [
        (6, cycle("refs/heads/r1")),
        (
            7,
            RefsFaultV1::TooDeep {
                name: "refs/heads/r1".into(),
            },
        ),
    ] {
        let name = |index: usize| -> &'static str {
            Box::leak(format!("refs/heads/r{index}").into_boxed_str())
        };
        let mut files = vec![
            file_of("HEAD", symbolic_file("refs/heads/main")),
            main.clone(),
        ];
        for index in 1..=ring {
            let next = if index == ring { 1 } else { index + 1 };
            files.push(file_of(name(index), symbolic_file(name(next))));
        }
        assert_eq!(
            fault_of(pure(format, &files, &[]), &format!("a ring of {ring}")),
            expected,
            "a ring of {ring}"
        );
    }
}

/// Only HEAD may be unborn on its own; any other symbolic ref whose chain ends in a missing ref is
/// refused as dangling unless the manifest says it is `Unborn`.
#[test]
fn b1b_dangling_symbolic_refs_refuse_unless_head_or_declared_unborn() {
    for format in BOTH_FORMATS_V1 {
        let mut files = base_files(format);
        files.push(file_of(
            "refs/remotes/origin/HEAD",
            symbolic_file("refs/remotes/origin/main"),
        ));
        assert_eq!(
            fault_of(pure(format, &files, &[]), "a dangling remote HEAD"),
            RefsFaultV1::Dangling {
                name: "refs/remotes/origin/HEAD".into(),
                target: "refs/remotes/origin/main".into(),
            }
        );
        // A manifest that says it is unborn makes it so; a manifest that merely says it is symbolic does
        // not, because the target is still absent.
        pure(
            format,
            &files,
            &[("refs/remotes/origin/HEAD", WantV1::Unborn)],
        )
        .expect("declared unborn");
        assert_eq!(
            fault_of(
                pure(
                    format,
                    &files,
                    &[(
                        "refs/remotes/origin/HEAD",
                        WantV1::Symbolic("refs/remotes/origin/main")
                    )],
                ),
                "declared symbolic only"
            ),
            RefsFaultV1::Dangling {
                name: "refs/remotes/origin/HEAD".into(),
                target: "refs/remotes/origin/main".into(),
            }
        );
        // The target may be absent after more than one hop: the last name is the absent one.
        let files = vec![
            file_of("HEAD", symbolic_file("refs/heads/main")),
            file_of("refs/heads/main", direct_file('a', format)),
            file_of("refs/heads/x", symbolic_file("refs/heads/y")),
            file_of("refs/heads/y", symbolic_file("refs/heads/z")),
        ];
        assert_eq!(
            fault_of(pure(format, &files, &[]), "two hops to nothing"),
            RefsFaultV1::Dangling {
                name: "refs/heads/x".into(),
                target: "refs/heads/z".into(),
            }
        );
    }
}

/// Kind rules: `refs/heads/**`, a detached or resolved HEAD, and ORIG_HEAD must name a commit; other refs
/// may name any known kind; an object of unknown kind is refused wherever it is named.
#[test]
fn b1b_object_kinds_are_validated() {
    for format in BOTH_FORMATS_V1 {
        let kind_fault = |name: &str, fill: char, found: &'static str| RefsFaultV1::TargetKind {
            name: name.into(),
            oid: oid_of(fill, format),
            found,
            expected: "commit",
        };
        for (fill, found) in [('c', "tag"), ('d', "tree"), ('e', "blob")] {
            let mut files = base_files(format);
            files.push(file_of("refs/heads/x", direct_file(fill, format)));
            assert_eq!(
                fault_of(pure(format, &files, &[]), "a head"),
                kind_fault("refs/heads/x", fill, found)
            );
            assert_eq!(
                fault_of(
                    pure(format, &[file_of("HEAD", direct_file(fill, format))], &[]),
                    "a detached HEAD"
                ),
                kind_fault("HEAD", fill, found)
            );
            let mut files = base_files(format);
            files.push(file_of("ORIG_HEAD", direct_file(fill, format)));
            assert_eq!(
                fault_of(pure(format, &files, &[]), "ORIG_HEAD"),
                kind_fault("ORIG_HEAD", fill, found)
            );
            // A symbolic HEAD or head resolves to a non-commit through a tag ref.
            let files = vec![
                file_of("HEAD", symbolic_file("refs/tags/t")),
                file_of("refs/tags/t", direct_file(fill, format)),
            ];
            assert_eq!(
                fault_of(pure(format, &files, &[]), "HEAD resolving to a non-commit"),
                kind_fault("HEAD", fill, found)
            );
            let mut files = base_files(format);
            files.push(file_of("refs/tags/t", direct_file(fill, format)));
            files.push(file_of("refs/heads/alias", symbolic_file("refs/tags/t")));
            assert_eq!(
                fault_of(
                    pure(format, &files, &[]),
                    "an alias resolving to a non-commit"
                ),
                kind_fault("refs/heads/alias", fill, found)
            );
        }
        // Any known kind is a fine tag, note, or custom ref target.
        let mut files = base_files(format);
        for (path, fill) in [
            ("refs/tags/a", 'a'),
            ("refs/tags/c", 'c'),
            ("refs/tags/d", 'd'),
            ("refs/tags/e", 'e'),
            ("refs/notes/commits", 'a'),
            ("refs/custom/x", 'e'),
        ] {
            files.push(file_of(path, direct_file(fill, format)));
        }
        pure(format, &files, &[]).expect("known kinds under non-branch refs");
        // An unknown kind is refused wherever it is named.
        let mut files = base_files(format);
        files.push(file_of("refs/tags/u", direct_file('f', format)));
        assert_eq!(
            fault_of(pure(format, &files, &[]), "an unknown kind"),
            RefsFaultV1::TargetKind {
                name: "refs/tags/u".into(),
                oid: oid_of('f', format),
                found: "unknown",
                expected: "a known kind",
            }
        );
    }
}

/// Every active direct target, from every place one can come from, must be in the inventory, even when
/// the manifest lists none of the refs. The same ref is accepted once its object is in the inventory.
#[test]
fn b1b_every_extra_direct_target_must_be_in_the_inventory() {
    for format in BOTH_FORMATS_V1 {
        let missing = oid_of('7', format);
        let absent = |name: &str| RefsFaultV1::TargetAbsent {
            name: name.into(),
            oid: missing.clone(),
        };
        let header = PACKED_HEADER_V1;
        let loose = |path: &'static str, fill: char| file_of(path, direct_file(fill, format));
        let cases: Vec<(&str, Vec<FileV1>, &str)> = vec![
            (
                "a loose ref",
                vec![loose("refs/heads/x", '7')],
                "refs/heads/x",
            ),
            (
                "a loose tag",
                vec![loose("refs/tags/t", '7')],
                "refs/tags/t",
            ),
            (
                "a packed row",
                vec![file_of(
                    "packed-refs",
                    packed(format, header, &[("refs/heads/p", '7', None)]),
                )],
                "refs/heads/p",
            ),
            (
                "a peeled row",
                vec![file_of(
                    "packed-refs",
                    packed(format, header, &[("refs/tags/v1", 'c', Some('7'))]),
                )],
                "refs/tags/v1",
            ),
            ("ORIG_HEAD", vec![loose("ORIG_HEAD", '7')], "ORIG_HEAD"),
            (
                "a FETCH_HEAD row",
                vec![file_of(
                    "FETCH_HEAD",
                    fetch_head(
                        format,
                        &[('a', "", "first"), ('7', "not-for-merge", "second")],
                    ),
                )],
                "FETCH_HEAD",
            ),
        ];
        for (row, extra, name) in cases {
            let mut files = base_files(format);
            files.extend(extra);
            assert_eq!(
                fault_of(pure(format, &files, &[]), row),
                absent(name),
                "{format:?}: {row}"
            );
        }
        // With an empty inventory, any direct ref is absent.
        assert_eq!(
            fault_of(
                pure_run(
                    format,
                    false,
                    &[
                        file_of("HEAD", symbolic_file("refs/heads/main")),
                        loose("refs/heads/main", 'a')
                    ],
                    &[],
                    RefsLimitsV1::v1()
                ),
                "an empty inventory"
            ),
            RefsFaultV1::TargetAbsent {
                name: "refs/heads/main".into(),
                oid: oid_of('a', format)
            }
        );
        // And the same refs pass when their objects are present.
        let mut files = base_files(format);
        files.extend([
            loose("refs/heads/x", 'b'),
            loose("refs/tags/t", 'c'),
            loose("ORIG_HEAD", 'a'),
            file_of(
                "packed-refs",
                packed(format, header, &[("refs/tags/v1", 'c', Some('a'))]),
            ),
            file_of(
                "FETCH_HEAD",
                fetch_head(format, &[('a', "", "n"), ('e', "", "n")]),
            ),
        ]);
        let agreement = pure(format, &files, &[]).expect("every target present");
        // a (main/ORIG_HEAD/peeled/FETCH_HEAD), b, c, e.
        assert_eq!(agreement.direct_targets, 4, "{format:?}");
        assert_eq!(agreement.manifest_refs, 0);
    }
}

/// Peeled rows are checked against object kinds: only a tag has a peeled value, which is never a tag
/// itself; under the `fully-peeled` trait every tag row needs one, and under `peeled` alone it does not.
#[test]
fn b1b_peeled_rows_are_checked_against_object_kinds() {
    let format = GitObjectFormatV1::Sha1;
    let run = |header: &str, rows: &[(&str, char, Option<char>)]| {
        let files = vec![
            file_of("HEAD", symbolic_file("refs/heads/main")),
            file_of("packed-refs", packed(format, header, rows)),
        ];
        pure(format, &files, &[])
    };
    run(PACKED_HEADER_V1, &[("refs/tags/v1", 'c', Some('a'))]).expect("a tag and its peeled value");
    run(PACKED_HEADER_V1, &[("refs/tags/v1", 'c', Some('d'))]).expect("a tag of a tree");
    assert_eq!(
        fault_of(
            run(PACKED_HEADER_V1, &[("refs/heads/x", 'a', Some('b'))]),
            "a peeled value under a commit"
        ),
        RefsFaultV1::PeeledNotTag {
            name: "refs/heads/x".into()
        }
    );
    assert_eq!(
        fault_of(
            run(PACKED_HEADER_V1, &[("refs/tags/v1", 'c', Some('c'))]),
            "a tag peeled to a tag"
        ),
        RefsFaultV1::PeeledIsTag {
            name: "refs/tags/v1".into()
        }
    );
    assert_eq!(
        fault_of(
            run(PACKED_HEADER_V1, &[("refs/tags/v1", 'c', None)]),
            "fully-peeled without a peeled row"
        ),
        RefsFaultV1::PeelMissing {
            name: "refs/tags/v1".into()
        }
    );
    run(
        "# pack-refs with: peeled sorted \n",
        &[("refs/tags/v1", 'c', None)],
    )
    .expect("peeled alone does not require a row");
    run("", &[("refs/tags/v1", 'c', None)]).expect("no header, no promise");
    // A non-tag row without a peeled row is the ordinary case.
    run(PACKED_HEADER_V1, &[("refs/heads/x", 'a', None)]).expect("a commit with no peeled row");
}

/// Every manifest ref is cross-checked by name and by target kind; the refs the manifest does not list
/// are still checked, and `Unborn` means a symbolic ref whose chain ends in a missing ref.
#[test]
fn b1b_every_manifest_ref_is_cross_checked_by_name_and_target() {
    for format in BOTH_FORMATS_V1 {
        let mut files = base_files(format);
        files.extend([
            file_of("refs/heads/side", direct_file('b', format)),
            file_of("refs/tags/v1", direct_file('c', format)),
            file_of("refs/heads/alias", symbolic_file("refs/heads/main")),
            file_of("ORIG_HEAD", direct_file('b', format)),
        ]);
        let all: Vec<(&str, WantV1)> = vec![
            ("HEAD", WantV1::Symbolic("refs/heads/main")),
            ("ORIG_HEAD", WantV1::Direct('b')),
            ("refs/heads/alias", WantV1::Symbolic("refs/heads/main")),
            ("refs/heads/main", WantV1::Direct('a')),
            ("refs/heads/side", WantV1::Direct('b')),
            ("refs/tags/v1", WantV1::Direct('c')),
        ];
        let agreement =
            pure(format, &files, &all).unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(agreement.manifest_refs, 6);
        // A subset: the unlisted refs are still restored and still valid.
        let subset = pure(format, &files, &all[..1]).expect("a subset");
        assert_eq!(subset.manifest_refs, 1);
        assert_eq!(subset.effective_refs, 4);

        let with = |name: &'static str, want: WantV1| -> Vec<(&'static str, WantV1)> {
            let mut refs: Vec<_> = all.iter().filter(|(n, _)| *n != name).cloned().collect();
            refs.push((name, want));
            refs
        };
        let mismatch =
            |name: &str, expected: String, found: String| RefsFaultV1::OriginalMismatch {
                name: name.into(),
                expected,
                found,
            };
        let (a, b) = (oid_of('a', format), oid_of('b', format));
        let table: Vec<(&str, ManifestRefsV1, RefsFaultV1)> = vec![
            (
                "a missing original ref",
                with("refs/heads/ghost", WantV1::Direct('a')),
                RefsFaultV1::MissingOriginal {
                    name: "refs/heads/ghost".into(),
                },
            ),
            (
                "a missing pseudoref",
                with("FETCH_HEAD", WantV1::Direct('a')),
                RefsFaultV1::MissingOriginal {
                    name: "FETCH_HEAD".into(),
                },
            ),
            (
                "a different object",
                with("refs/heads/side", WantV1::Direct('a')),
                mismatch(
                    "refs/heads/side",
                    format!("direct {a}"),
                    format!("direct {b}"),
                ),
            ),
            (
                "a different ORIG_HEAD",
                with("ORIG_HEAD", WantV1::Direct('a')),
                mismatch("ORIG_HEAD", format!("direct {a}"), format!("direct {b}")),
            ),
            (
                "direct, but symbolic",
                with("refs/heads/alias", WantV1::Direct('a')),
                mismatch(
                    "refs/heads/alias",
                    format!("direct {a}"),
                    "symbolic refs/heads/main".into(),
                ),
            ),
            (
                "symbolic, but direct",
                with("refs/heads/side", WantV1::Symbolic("refs/heads/main")),
                mismatch(
                    "refs/heads/side",
                    "symbolic refs/heads/main".into(),
                    format!("direct {b}"),
                ),
            ),
            (
                "another symbolic target",
                with("HEAD", WantV1::Symbolic("refs/heads/side")),
                mismatch(
                    "HEAD",
                    "symbolic refs/heads/side".into(),
                    "symbolic refs/heads/main".into(),
                ),
            ),
            (
                "an attached HEAD that the manifest calls detached",
                with("HEAD", WantV1::Direct('a')),
                mismatch(
                    "HEAD",
                    format!("direct {a}"),
                    "symbolic refs/heads/main".into(),
                ),
            ),
            (
                "unborn, but direct",
                with("refs/heads/main", WantV1::Unborn),
                mismatch("refs/heads/main", "unborn".into(), format!("direct {a}")),
            ),
            (
                "unborn, but the chain resolves",
                with("HEAD", WantV1::Unborn),
                mismatch("HEAD", "unborn".into(), "symbolic refs/heads/main".into()),
            ),
        ];
        for (row, refs, expected) in table {
            assert_eq!(
                fault_of(pure(format, &files, &refs), row),
                expected,
                "{format:?}: {row}"
            );
        }
        // A detached HEAD that the manifest calls attached.
        assert_eq!(
            fault_of(
                pure(
                    format,
                    &[file_of("HEAD", direct_file('a', format))],
                    &[("HEAD", WantV1::Symbolic("refs/heads/main"))]
                ),
                "a detached HEAD called attached"
            ),
            mismatch(
                "HEAD",
                "symbolic refs/heads/main".into(),
                format!("direct {a}")
            )
        );
    }
}

/// A manifest ref with a name or a symbolic target that is not a valid ref name is refused by name, and
/// the three pseudorefs are the only names that need not be under `refs/`.
#[test]
fn b1b_manifest_ref_names_and_targets_are_validated() {
    let format = GitObjectFormatV1::Sha1;
    let files = base_files(format);
    for (name, why) in [
        ("refs/heads/x.lock", RefNameFaultV1::LockSuffix),
        ("heads/main", RefNameFaultV1::NotUnderRefs),
        ("head", RefNameFaultV1::NotUnderRefs),
        ("MERGE_HEAD", RefNameFaultV1::NotUnderRefs),
        ("refs/heads/a..b", RefNameFaultV1::DoubleDot),
    ] {
        assert_eq!(
            fault_of(pure(format, &files, &[(name, WantV1::Direct('a'))]), name),
            RefsFaultV1::ManifestName {
                name: name.into(),
                why
            },
            "{name}"
        );
    }
    for (target, why) in [
        ("main", RefNameFaultV1::NotUnderRefs),
        ("HEAD", RefNameFaultV1::NotUnderRefs),
        ("refs/heads/x.lock", RefNameFaultV1::LockSuffix),
    ] {
        assert_eq!(
            fault_of(
                pure(format, &files, &[("HEAD", WantV1::Symbolic(target))]),
                target
            ),
            RefsFaultV1::ManifestTarget {
                name: "HEAD".into(),
                target: target.into(),
                why
            },
            "{target}"
        );
    }
    // A manifest object in another format than the caller's is the inventory's format refusal.
    let mut builder = RefsBuilderV1::new(format, RefsLimitsV1::v1(), far_deadline());
    for (path, content) in &files {
        let mut reader: &[u8] = content;
        builder
            .admit_file(path.as_bytes(), &mut reader)
            .expect("a file");
    }
    let error = builder
        .finish(&pool_manifest(GitObjectFormatV1::Sha256, true, &[]))
        .expect_err("a SHA-256 manifest for a SHA-1 caller");
    assert!(
        matches!(error, GitBootstrapErrorV1::FormatMismatch(_)),
        "{error:?}"
    );
}

/// Structure: a ref and a ref under it (in any mix of loose and packed) conflict, spellings that differ
/// only by ASCII case collide, and the same name loose and packed is one ref, not a collision.
#[test]
fn b1b_ref_names_that_conflict_or_collide_refuse() {
    let format = GitObjectFormatV1::Sha1;
    let header = PACKED_HEADER_V1;
    let head = file_of("HEAD", symbolic_file("refs/heads/main"));
    let loose = |path: &'static str| file_of(path, direct_file('a', format));
    let rows =
        |list: &[(&str, char, Option<char>)]| file_of("packed-refs", packed(format, header, list));
    let conflict = |name: &str, with: &str| RefsFaultV1::Conflict {
        name: name.into(),
        with: with.into(),
    };
    let collision = |name: &str, with: &str| RefsFaultV1::Collision {
        name: name.into(),
        with: with.into(),
    };
    assert_eq!(
        fault_of(
            pure(
                format,
                &[
                    head.clone(),
                    loose("refs/heads/a"),
                    rows(&[("refs/heads/a/b", 'a', None)])
                ],
                &[]
            ),
            "loose over packed"
        ),
        conflict("refs/heads/a/b", "refs/heads/a")
    );
    assert_eq!(
        fault_of(
            pure(
                format,
                &[
                    head.clone(),
                    loose("refs/heads/a/b"),
                    rows(&[("refs/heads/a", 'a', None)])
                ],
                &[]
            ),
            "packed over loose"
        ),
        conflict("refs/heads/a/b", "refs/heads/a")
    );
    assert_eq!(
        fault_of(
            pure(
                format,
                &[
                    head.clone(),
                    rows(&[("refs/heads/a", 'a', None), ("refs/heads/a/b", 'a', None)])
                ],
                &[]
            ),
            "within packed"
        ),
        conflict("refs/heads/a/b", "refs/heads/a")
    );
    assert_eq!(
        fault_of(
            pure(
                format,
                &[
                    head.clone(),
                    loose("refs/heads/Main"),
                    rows(&[("refs/heads/main", 'a', None)])
                ],
                &[]
            ),
            "case, loose and packed"
        ),
        collision("refs/heads/main", "refs/heads/Main")
    );
    assert_eq!(
        fault_of(
            pure(
                format,
                &[head.clone(), loose("refs/heads/A"), loose("refs/heads/a")],
                &[]
            ),
            "case, both loose"
        ),
        collision("refs/heads/a", "refs/heads/A")
    );
    assert_eq!(
        fault_of(
            pure(
                format,
                &[
                    head.clone(),
                    rows(&[("refs/heads/A", 'a', None), ("refs/heads/a", 'a', None)])
                ],
                &[]
            ),
            "case, both packed"
        ),
        collision("refs/heads/a", "refs/heads/A")
    );
    // The same name in both is one ref, and a prefix that is not a path component is no conflict.
    pure(
        format,
        &[
            head.clone(),
            loose("refs/heads/a"),
            rows(&[("refs/heads/a", 'a', None), ("refs/heads/ab", 'a', None)]),
        ],
        &[],
    )
    .expect("the same name loose and packed, and a sibling that shares a prefix");
}

// ---------------------------------------------------------------------------------------------
// Limits and the one deadline, on the pure builder
// ---------------------------------------------------------------------------------------------

fn limits_with(edit: impl FnOnce(&mut RefsLimitsV1)) -> RefsLimitsV1 {
    let mut limits = RefsLimitsV1::v1();
    edit(&mut limits);
    limits
}

fn limit_fault(what: RefsLimitKindV1, bound: u64) -> RefsFaultV1 {
    RefsFaultV1::Limit { what, bound }
}

/// Runs `pure_run` under `limits` over `files` with an attached-HEAD manifest-free state.
fn limited(
    format: GitObjectFormatV1,
    files: &[FileV1],
    limits: RefsLimitsV1,
) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
    pure_run(format, true, files, &[], limits)
}

/// The documented v1 limits, and the retained-cost formula the accounting rows rely on.
#[test]
fn b1b_the_v1_limits_and_the_cost_formula_are_the_documented_ones() {
    let limits = RefsLimitsV1::v1();
    assert_eq!(limits.line_bytes, 8 * 1024);
    assert_eq!(limits.loose_file_bytes, 16 * 1024);
    assert_eq!(limits.packed_file_bytes, 64 * 1024 * 1024);
    assert_eq!(limits.fetch_head_bytes, 1024 * 1024);
    assert_eq!(limits.log_file_bytes, 64 * 1024 * 1024);
    assert_eq!(limits.entries, 262_144);
    assert_eq!(limits.retained_bytes, 64 * 1024 * 1024);
    // Two copies of the name (the map key and its case key), the value, and a fixed allowance.
    assert_eq!(refs_entry_cost_v1(0, 0), 64);
    assert_eq!(refs_entry_cost_v1(15, 40), 64 + 2 * 15 + 40);
    assert!(limits.line_bytes < limits.loose_file_bytes);
}

/// Each per-file byte cap is exact: a file of exactly the cap parses, one byte more is refused with the
/// typed limit and the cap, for every kind of file and in both formats.
#[test]
fn b1b_each_file_byte_cap_is_exact() {
    for format in BOTH_FORMATS_V1 {
        let direct = direct_file('a', format);
        let symbolic = symbolic_file("refs/heads/main");
        let packed_bytes = packed(
            format,
            PACKED_HEADER_V1,
            &[
                ("refs/heads/main", 'a', None),
                ("refs/tags/v1", 'c', Some('a')),
            ],
        );
        let fetch = fetch_head(format, &[('a', "", "note"), ('b', "not-for-merge", "n")]);
        let log = [
            log_row(format, '0', 'a', IDENT_V1),
            log_row(format, 'a', 'b', &format!("{IDENT_V1}\tmessage")),
        ]
        .concat()
        .into_bytes();
        type Edit = fn(&mut RefsLimitsV1, u64);
        let rows: Vec<(&str, FileV1, RefsLimitKindV1, Edit)> = vec![
            (
                "HEAD",
                file_of("HEAD", symbolic.clone()),
                RefsLimitKindV1::LooseFile,
                |limits, bound| limits.loose_file_bytes = bound,
            ),
            (
                "a detached HEAD",
                file_of("HEAD", direct.clone()),
                RefsLimitKindV1::LooseFile,
                |limits, bound| limits.loose_file_bytes = bound,
            ),
            (
                "ORIG_HEAD",
                file_of("ORIG_HEAD", direct.clone()),
                RefsLimitKindV1::LooseFile,
                |limits, bound| limits.loose_file_bytes = bound,
            ),
            (
                "a loose ref",
                file_of("refs/heads/x", direct.clone()),
                RefsLimitKindV1::LooseFile,
                |limits, bound| limits.loose_file_bytes = bound,
            ),
            (
                "packed-refs",
                file_of("packed-refs", packed_bytes),
                RefsLimitKindV1::PackedFile,
                |limits, bound| limits.packed_file_bytes = bound,
            ),
            (
                "FETCH_HEAD",
                file_of("FETCH_HEAD", fetch),
                RefsLimitKindV1::FetchHeadFile,
                |limits, bound| limits.fetch_head_bytes = bound,
            ),
            (
                "a reflog",
                file_of("logs/HEAD", log),
                RefsLimitKindV1::LogFile,
                |limits, bound| limits.log_file_bytes = bound,
            ),
        ];
        for (row, file, what, edit) in rows {
            let length = file.1.len() as u64;
            let others = if file.0 == "HEAD" {
                vec![]
            } else {
                vec![file_of("HEAD", symbolic.clone())]
            };
            let run = |bound: u64| {
                let mut files = others.clone();
                files.push(file.clone());
                limited(format, &files, limits_with(|limits| edit(limits, bound)))
            };
            // The HEAD file itself must also be valid for the manifest-free finish.
            run(length).unwrap_or_else(|error| panic!("{format:?} {row}: at the cap: {error:?}"));
            assert_eq!(
                fault_of(run(length - 1), row),
                limit_fault(what, length - 1),
                "{format:?} {row}: one byte over"
            );
        }
    }
}

/// The line cap is exact too, for every line-structured file: a line of exactly the cap (without its
/// LF) parses, one byte longer is refused.
#[test]
fn b1b_the_line_cap_is_exact() {
    for format in BOTH_FORMATS_V1 {
        let width = oid_len(format) as u64;
        let head = file_of("HEAD", symbolic_file("refs/heads/main"));
        let head_line = "ref: refs/heads/main".len() as u64;
        // Each row: the files, and the length of the one line the cap is lowered onto.
        let rows: Vec<(&str, Vec<FileV1>, u64)> = vec![
            (
                "a direct loose ref",
                vec![
                    head.clone(),
                    file_of("refs/heads/x", direct_file('a', format)),
                ],
                width,
            ),
            ("a symbolic HEAD", vec![head.clone()], head_line),
            (
                "a packed row",
                vec![
                    head.clone(),
                    file_of(
                        "packed-refs",
                        packed(format, "", &[("refs/heads/x", 'a', None)]),
                    ),
                ],
                width + 1 + "refs/heads/x".len() as u64,
            ),
            (
                "a FETCH_HEAD row",
                vec![
                    head.clone(),
                    file_of("FETCH_HEAD", fetch_head(format, &[('a', "", "note")])),
                ],
                width + 2 + "note".len() as u64,
            ),
            (
                "a reflog row",
                vec![
                    head.clone(),
                    file_of(
                        "logs/HEAD",
                        log_row(format, '0', 'a', IDENT_V1).into_bytes(),
                    ),
                ],
                2 * width + 2 + IDENT_V1.len() as u64,
            ),
        ];
        for (row, files, cap) in rows {
            let run = |bound: u64| {
                limited(
                    format,
                    &files,
                    limits_with(|limits| {
                        limits.line_bytes = bound;
                    }),
                )
            };
            run(cap).unwrap_or_else(|error| panic!("{format:?} {row}: at the cap: {error:?}"));
            assert_eq!(
                fault_of(run(cap - 1), row),
                limit_fault(RefsLimitKindV1::Line, cap - 1),
                "{format:?} {row}"
            );
        }
    }
}

/// The record cap counts every admitted record: each loose file (HEAD and ORIG_HEAD included), each
/// packed row and peeled row, each FETCH_HEAD row, and each reflog file.
#[test]
fn b1b_the_entry_cap_counts_every_admitted_record() {
    for format in BOTH_FORMATS_V1 {
        let files = vec![
            file_of("HEAD", symbolic_file("refs/heads/main")), // 1
            file_of("ORIG_HEAD", direct_file('a', format)),    // 2
            file_of("refs/heads/main", direct_file('a', format)), // 3
            file_of(
                "packed-refs",
                packed(
                    format,
                    PACKED_HEADER_V1,
                    &[
                        ("refs/heads/p", 'a', None),
                        ("refs/tags/v1", 'c', Some('a')),
                    ],
                ),
            ), // 4, 5, and the peeled row 6
            file_of(
                "FETCH_HEAD",
                fetch_head(format, &[('a', "", "n"), ('b', "", "n")]),
            ), // 7, 8
            file_of("logs/HEAD", Vec::new()),                  // 9
            file_of("logs/refs/heads/main", Vec::new()),       // 10
        ];
        let run =
            |bound: u64| limited(format, &files, limits_with(|limits| limits.entries = bound));
        let agreement = run(10).unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!(agreement.reflog_files, 2);
        assert_eq!(
            fault_of(run(9), "one record over"),
            limit_fault(RefsLimitKindV1::Entries, 9),
            "{format:?}"
        );
        assert_eq!(
            fault_of(run(0), "no records"),
            limit_fault(RefsLimitKindV1::Entries, 0),
            "{format:?}"
        );
    }
}

/// The retained-bytes cap is exact: the sum of every retained record's cost parses, one byte less is
/// refused. A reflog retains only its name (its rows stream by and are never kept).
#[test]
fn b1b_the_retained_cap_is_exact_and_reflogs_retain_only_their_names() {
    for format in BOTH_FORMATS_V1 {
        let width = oid_len(format);
        let files = vec![
            file_of("HEAD", symbolic_file("refs/heads/main")),
            file_of("refs/heads/main", direct_file('a', format)),
            file_of("refs/heads/alias", symbolic_file("refs/heads/main")),
            file_of("ORIG_HEAD", direct_file('b', format)),
            file_of(
                "packed-refs",
                packed(
                    format,
                    PACKED_HEADER_V1,
                    &[
                        ("refs/heads/p", 'a', None),
                        ("refs/tags/v1", 'c', Some('a')),
                    ],
                ),
            ),
            file_of(
                "FETCH_HEAD",
                fetch_head(format, &[('a', "", "n"), ('b', "", "n")]),
            ),
            file_of(
                "logs/HEAD",
                log_row(format, '0', 'a', IDENT_V1).into_bytes(),
            ),
        ];
        let expected = refs_entry_cost_v1("HEAD".len(), "refs/heads/main".len())
            + refs_entry_cost_v1("refs/heads/main".len(), width)
            + refs_entry_cost_v1("refs/heads/alias".len(), "refs/heads/main".len())
            + refs_entry_cost_v1("ORIG_HEAD".len(), width)
            + refs_entry_cost_v1("refs/heads/p".len(), width)
            + refs_entry_cost_v1("refs/tags/v1".len(), 2 * width)
            + 2 * refs_entry_cost_v1(0, width)
            + refs_entry_cost_v1("logs/HEAD".len(), 0);
        let run = |bound: u64| {
            limited(
                format,
                &files,
                limits_with(|limits| limits.retained_bytes = bound),
            )
        };
        run(expected).unwrap_or_else(|error| panic!("{format:?}: at the sum: {error:?}"));
        assert_eq!(
            fault_of(run(expected - 1), "one byte less"),
            limit_fault(RefsLimitKindV1::Retained, expected - 1),
            "{format:?}"
        );
    }
}

/// A reader that serves `data` forever (or until `data` is `None`'s generator ends) and counts what it
/// served: bounded admission means a hostile stream stops at the cap, never at the end of the stream.
struct EndlessV1<F: FnMut(&mut [u8]) -> usize> {
    fill: F,
    served: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<F: FnMut(&mut [u8]) -> usize> std::io::Read for EndlessV1<F> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = (self.fill)(buffer);
        self.served.set(self.served.get() + count as u64);
        Ok(count)
    }
}

/// Never-ending inputs stop at their cap with a typed refusal after reading at most the cap plus one
/// byte, never buffering the stream: a line with no LF, an endless list of valid packed rows, FETCH_HEAD
/// rows, and reflog rows.
#[test]
fn b1b_endless_inputs_stop_at_their_caps() {
    for format in BOTH_FORMATS_V1 {
        let served = std::rc::Rc::new(std::cell::Cell::new(0_u64));
        let builder = |limits: RefsLimitsV1| {
            served.set(0);
            RefsBuilderV1::new(format, limits, far_deadline())
        };
        // No LF, ever: the line cap stops it before the loose-file cap.
        let mut x = EndlessV1 {
            fill: |buffer: &mut [u8]| {
                buffer.fill(b'x');
                buffer.len()
            },
            served: served.clone(),
        };
        let limits = RefsLimitsV1::v1();
        let error = builder(limits)
            .admit_file(b"refs/heads/x", &mut x)
            .expect_err("an endless line");
        assert_eq!(
            refs_fault(error),
            limit_fault(RefsLimitKindV1::Line, limits.line_bytes)
        );
        assert!(
            served.get() <= limits.loose_file_bytes + 1,
            "{}",
            served.get()
        );
        // With the line cap out of the way, the loose-file cap itself stops it.
        served.set(0);
        let limits = limits_with(|limits| {
            limits.line_bytes = 1 << 30;
            limits.loose_file_bytes = 100;
        });
        let error = builder(limits)
            .admit_file(b"HEAD", &mut x)
            .expect_err("an endless loose file");
        assert_eq!(
            refs_fault(error),
            limit_fault(RefsLimitKindV1::LooseFile, 100)
        );
        assert!(served.get() <= 101, "{}", served.get());

        // Endless valid rows, one family at a time, each stopped by its file cap.
        type RowFn = Box<dyn Fn(u64) -> String>;
        let families: Vec<(&str, &[u8], RowFn, RefsLimitKindV1)> = vec![
            (
                "packed rows",
                &b"packed-refs"[..],
                Box::new(move |index| format!("{} refs/heads/n{index}\n", oid_of('a', format))),
                RefsLimitKindV1::PackedFile,
            ),
            (
                "FETCH_HEAD rows",
                &b"FETCH_HEAD"[..],
                Box::new(move |_| format!("{}\t\tnote\n", oid_of('a', format))),
                RefsLimitKindV1::FetchHeadFile,
            ),
            (
                "reflog rows",
                &b"logs/HEAD"[..],
                Box::new(move |_| log_row(format, '0', 'a', IDENT_V1)),
                RefsLimitKindV1::LogFile,
            ),
        ];
        for (family, path, row, what) in families {
            let cap = 64 * 1024_u64;
            let limits = limits_with(|limits| {
                limits.packed_file_bytes = cap;
                limits.fetch_head_bytes = cap;
                limits.log_file_bytes = cap;
            });
            let mut index = 0_u64;
            let mut pending: Vec<u8> = Vec::new();
            let mut stream = EndlessV1 {
                fill: |buffer: &mut [u8]| {
                    while pending.len() < buffer.len() {
                        pending.extend_from_slice(row(index).as_bytes());
                        index += 1;
                    }
                    let count = buffer.len();
                    buffer.copy_from_slice(&pending[..count]);
                    pending.drain(..count);
                    count
                },
                served: served.clone(),
            };
            let error = builder(limits)
                .admit_file(path, &mut stream)
                .expect_err(family);
            assert_eq!(refs_fault(error), limit_fault(what, cap), "{family}");
            assert!(served.get() <= cap + 1, "{family}: served {}", served.get());
        }
    }
}

/// A read error from the source is an `Unreadable` refusal naming the file, never a partial success.
#[test]
fn b1b_a_failing_source_is_unreadable_not_partial() {
    struct FailingV1;
    impl std::io::Read for FailingV1 {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("a failing source"))
        }
    }
    for path in [
        "HEAD",
        "refs/heads/x",
        "ORIG_HEAD",
        "FETCH_HEAD",
        "packed-refs",
        "logs/HEAD",
    ] {
        let mut builder =
            RefsBuilderV1::new(GitObjectFormatV1::Sha1, RefsLimitsV1::v1(), far_deadline());
        let error = builder
            .admit_file(path.as_bytes(), &mut FailingV1)
            .expect_err(path);
        assert_eq!(
            refs_fault(error),
            RefsFaultV1::Unreadable { path: path.into() },
            "{path}"
        );
    }
}

/// A source that hands out `data` in small reads and counts them; `on_read` runs before each one.
struct SlowV1<'a> {
    data: &'a [u8],
    reads: std::rc::Rc<std::cell::Cell<usize>>,
    on_read: Box<dyn Fn(usize) + 'a>,
}

impl std::io::Read for SlowV1<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        (self.on_read)(self.reads.get());
        self.reads.set(self.reads.get() + 1);
        let count = buffer.len().min(16).min(self.data.len());
        buffer[..count].copy_from_slice(&self.data[..count]);
        self.data = &self.data[count..];
        Ok(count)
    }
}

/// The one deadline is checked before every read of every file kind: a deadline already spent refuses
/// before the first read, and a deadline that passes during one read stops the next. No byte is
/// requested from a source after the deadline.
#[test]
fn b1b_no_source_read_follows_the_deadline() {
    let format = GitObjectFormatV1::Sha1;
    let rows = packed(
        format,
        PACKED_HEADER_V1,
        &[
            ("refs/heads/a", 'a', None),
            ("refs/heads/b", 'a', None),
            ("refs/heads/c", 'a', None),
        ],
    );
    let log = [
        log_row(format, '0', 'a', IDENT_V1),
        log_row(format, 'a', 'b', IDENT_V1),
        log_row(format, 'b', 'a', IDENT_V1),
    ]
    .concat()
    .into_bytes();
    let fetch = fetch_head(
        format,
        &[('a', "", "one"), ('b', "", "two"), ('a', "", "three")],
    );
    let files: Vec<(&[u8], Vec<u8>)> = vec![
        (&b"HEAD"[..], symbolic_file("refs/heads/main")),
        (&b"refs/heads/x"[..], direct_file('a', format)),
        (&b"packed-refs"[..], rows),
        (&b"FETCH_HEAD"[..], fetch),
        (&b"logs/HEAD"[..], log),
    ];
    for (path, data) in &files {
        let label = String::from_utf8_lossy(path).into_owned();
        // Already spent: refused before the first read.
        let reads = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        let spent = Instant::now();
        std::thread::sleep(Duration::from_millis(5));
        let mut source = SlowV1 {
            data,
            reads: reads.clone(),
            on_read: Box::new(|_| {}),
        };
        let mut builder = RefsBuilderV1::new(format, RefsLimitsV1::v1(), spent);
        let error = builder.admit_file(path, &mut source).expect_err(&label);
        assert!(
            matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
            "{label}: {error:?}"
        );
        assert_eq!(reads.get(), 0, "{label}: a read after a spent deadline");

        // Passes during the first read: the second is never requested.
        let reads = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut source = SlowV1 {
            data,
            reads: reads.clone(),
            on_read: Box::new(move |index| {
                if index == 0 {
                    pass_the_deadline(deadline);
                }
            }),
        };
        let mut builder = RefsBuilderV1::new(format, RefsLimitsV1::v1(), deadline);
        let error = builder.admit_file(path, &mut source).expect_err(&label);
        assert!(
            matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
            "{label}: {error:?}"
        );
        assert_eq!(reads.get(), 1, "{label}: a read after the deadline");
    }
}

/// A spent deadline refuses ahead of everything else the builder does with a file, including routing it:
/// a path that is no refs file at all is a timeout, not an unexpected file, and nothing is read.
#[test]
fn b1b_a_spent_deadline_refuses_before_a_file_is_routed() {
    let spent = Instant::now();
    std::thread::sleep(Duration::from_millis(5));
    for path in ["config", "HEAD", "refs/heads/x", "logs/HEAD"] {
        let reads = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        let mut source = SlowV1 {
            data: b"x\n",
            reads: reads.clone(),
            on_read: Box::new(|_| {}),
        };
        let mut builder = RefsBuilderV1::new(GitObjectFormatV1::Sha1, RefsLimitsV1::v1(), spent);
        let error = builder
            .admit_file(path.as_bytes(), &mut source)
            .expect_err(path);
        assert!(
            matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
            "{path}: {error:?}"
        );
        assert_eq!(reads.get(), 0, "{path}");
    }
}

/// The agreement's own phases each observe the deadline: a clean run reads the clock at exactly the
/// documented guards, in order; each guard halts the run alone, AT that guard; and brief delays at every
/// guard, with headroom, still succeed.
#[test]
fn b1b_the_agreement_phases_each_observe_the_deadline() {
    const GUARDS: [&str; 4] = [
        "refs-structure",
        "refs-targets",
        "refs-resolved",
        "refs-manifest",
    ];
    let format = GitObjectFormatV1::Sha1;
    let files = base_files(format);
    let refs = [
        ("HEAD", WantV1::Symbolic("refs/heads/main")),
        ("refs/heads/main", WantV1::Direct('a')),
    ];
    let run = |deadline: Instant| {
        let mut builder = RefsBuilderV1::new(format, RefsLimitsV1::v1(), deadline);
        for (path, content) in &files {
            let mut reader: &[u8] = content;
            builder.admit_file(path.as_bytes(), &mut reader)?;
        }
        builder.finish(&pool_manifest(format, true, &refs))
    };
    let (labels, hook) = record_guards();
    run(far_deadline()).expect("a clean run");
    drop(hook);
    assert_eq!(*labels.borrow(), GUARDS);

    for target in GUARDS {
        let deadline = Instant::now() + B1A_DEADLINE_V1;
        let labels = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let (seen, slow) = (labels.clone(), fired.clone());
        let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
            seen.borrow_mut().push(event.label);
            if event.label == target && !slow.replace(true) {
                pass_the_deadline(deadline);
            }
        });
        let error = run(deadline).expect_err(target);
        assert!(
            matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
            "{target}: {error:?}"
        );
        assert!(fired.get(), "{target}: the slow step never ran");
        assert_eq!(
            labels.borrow().last().copied(),
            Some(target),
            "{target}: the run halted AT its guard: {:?}",
            labels.borrow()
        );
    }
    let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, |_| {
        std::thread::sleep(Duration::from_millis(40));
    });
    run(Instant::now() + B1A_DEADLINE_V1).expect("brief delays leave most of the deadline");
}

// ---------------------------------------------------------------------------------------------
// The inert config encoder
// ---------------------------------------------------------------------------------------------

const CONFIG_SHA1_V1: &[u8] = b"[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tfilemode = true\n\tlogallrefupdates = true\n\tfsmonitor = false\n";
const CONFIG_SHA256_V1: &[u8] = b"[core]\n\trepositoryformatversion = 1\n\tbare = false\n\tfilemode = true\n\tlogallrefupdates = true\n\tfsmonitor = false\n[extensions]\n\tobjectformat = sha256\n";

/// The encoder's bytes are exactly the master's ordered keys, in both formats: `[core]`
/// repositoryformatversion (0 or 1), bare, filemode, logallrefupdates, fsmonitor; SHA-256 additionally
/// `[extensions]` objectformat. LF line ends, tab indentation, `key = value`, one trailing LF.
#[test]
fn b1b_the_inert_config_is_the_golden_bytes_in_both_formats() {
    assert_eq!(
        encode_inert_config_v1(GitObjectFormatV1::Sha1),
        CONFIG_SHA1_V1,
        "{}",
        String::from_utf8_lossy(&encode_inert_config_v1(GitObjectFormatV1::Sha1))
    );
    assert_eq!(
        encode_inert_config_v1(GitObjectFormatV1::Sha256),
        CONFIG_SHA256_V1,
        "{}",
        String::from_utf8_lossy(&encode_inert_config_v1(GitObjectFormatV1::Sha256))
    );
    // Deterministic: the same bytes every time, and the two formats differ.
    for format in BOTH_FORMATS_V1 {
        assert_eq!(
            encode_inert_config_v1(format),
            encode_inert_config_v1(format)
        );
    }
    assert_ne!(
        encode_inert_config_v1(GitObjectFormatV1::Sha1),
        encode_inert_config_v1(GitObjectFormatV1::Sha256)
    );
}

/// The shape rules hold byte for byte, and nothing behavior-affecting is in either encoding: no include,
/// remote, filter, hooks path, credential helper, alias, or any key beyond the documented ones.
#[test]
fn b1b_the_inert_config_has_only_the_documented_shape_and_no_behavior_keys() {
    for format in BOTH_FORMATS_V1 {
        let bytes = encode_inert_config_v1(format);
        let text = std::str::from_utf8(&bytes).expect("UTF-8");
        assert!(
            text.ends_with('\n') && !text.ends_with("\n\n"),
            "one trailing LF"
        );
        assert!(!bytes.contains(&b'\r'), "LF line ends only");
        let mut keys = Vec::new();
        for line in text.lines() {
            if line.starts_with('[') {
                assert!(line.ends_with(']'));
            } else {
                let rest = line.strip_prefix('\t').expect("a tab-indented key");
                assert!(!rest.starts_with(' ') && !rest.starts_with('\t'));
                let (key, value) = rest.split_once(" = ").expect("`key = value`");
                assert!(!key.is_empty() && !value.is_empty());
                assert!(!value.contains(' ') && !value.contains('\t'));
                keys.push(key.to_owned());
            }
        }
        let expected: &[&str] = match format {
            GitObjectFormatV1::Sha1 => &[
                "repositoryformatversion",
                "bare",
                "filemode",
                "logallrefupdates",
                "fsmonitor",
            ],
            GitObjectFormatV1::Sha256 => &[
                "repositoryformatversion",
                "bare",
                "filemode",
                "logallrefupdates",
                "fsmonitor",
                "objectformat",
            ],
        };
        assert_eq!(keys, expected, "{format:?}");
        let lowered = text.to_ascii_lowercase();
        for forbidden in [
            "include",
            "remote",
            "filter",
            "hookspath",
            "credential",
            "alias",
            "url",
            "pager",
            "editor",
            "sshcommand",
            "gpg",
            "diff",
            "merge",
            "worktree",
            "alternate",
            "core.fsmonitor = true",
        ] {
            assert!(
                !lowered.contains(forbidden),
                "{format:?} mentions {forbidden}"
            );
        }
    }
}

/// The golden bytes mean what they say to real Git: `git config --list` reads back exactly the
/// documented keys, and a repository whose config is replaced by them reports the right object format,
/// is not bare, and has its git directory where it was put. (Fixture plumbing, not the restore's effect
/// seam; the repository here is a scratch one.)
#[test]
fn b1b_real_git_reads_the_inert_config_back() {
    let git = |dir: &Path, args: &[&str]| -> String {
        let output = Command::new(lane_git_path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("the fixture Git child runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF-8")
            .trim_end()
            .to_owned()
    };
    for (format, flag, listed, name) in [
        (
            GitObjectFormatV1::Sha1,
            "--object-format=sha1",
            "core.repositoryformatversion=0\ncore.bare=false\ncore.filemode=true\ncore.logallrefupdates=true\ncore.fsmonitor=false",
            "sha1",
        ),
        (
            GitObjectFormatV1::Sha256,
            "--object-format=sha256",
            "core.repositoryformatversion=1\ncore.bare=false\ncore.filemode=true\ncore.logallrefupdates=true\ncore.fsmonitor=false\nextensions.objectformat=sha256",
            "sha256",
        ),
    ] {
        let area = tempfile::TempDir::new().expect("a scratch area");
        let area_path = area.path().canonicalize().expect("a canonical area");
        let repository = area_path.join("repository");
        std::fs::create_dir(&repository).expect("the repository");
        git(&area_path, &["init", "--bare", "-q", "--template=", flag, "repository/.git"]);
        let config = repository.join(".git/config");
        std::fs::write(&config, encode_inert_config_v1(format)).expect("the encoded config");
        assert_eq!(git(&repository, &["config", "--file", ".git/config", "--list"]), listed);
        assert_eq!(git(&repository, &["rev-parse", "--show-object-format"]), name);
        assert_eq!(git(&repository, &["rev-parse", "--is-bare-repository"]), "false");
        assert_eq!(git(&repository, &["rev-parse", "--git-dir"]), ".git");
    }
}

// ---------------------------------------------------------------------------------------------
// The retained entry point: real phase-V values, real frames, the one capsule, ledger, and deadline
// ---------------------------------------------------------------------------------------------

/// The failure shape of the retained agreement is B1a's: no materialization began, the caller's
/// completed step and the one ledger's charge are carried.
fn agree_with(
    verified: &mut VerifiedCapsuleV1,
    plan: &ActiveFramePlanV1,
    format: GitObjectFormatV1,
    deadline: Instant,
    limits: RefsLimitsV1,
) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
    let ledger = ledger_used(verified);
    agree_active_refs_bounded_v1(
        verified,
        plan,
        format,
        frame_budget(),
        deadline,
        B1A_COMPLETED_V1,
        limits,
    )
    .map_err(|failure| require_failure_shape(failure, B1A_COMPLETED_V1, ledger))
}

fn agree(
    verified: &mut VerifiedCapsuleV1,
    plan: &ActiveFramePlanV1,
    format: GitObjectFormatV1,
) -> Result<RefsAgreementV1, GitBootstrapErrorV1> {
    agree_with(verified, plan, format, far_deadline(), RefsLimitsV1::v1())
}

/// The refs the manifest of `verified` lists, replaced on the retained value (as the frames are staged).
fn set_refs(verified: &mut VerifiedCapsuleV1, refs: Vec<CustodyOriginalRefV1>) {
    let base = &verified.control.manifest;
    verified.control.manifest = CustodyManifestV1::new(
        base.unit_id(),
        base.run_id(),
        base.materialization_id(),
        base.generation_id(),
        base.coverage().to_vec(),
        refs,
        base.original_objects().to_vec(),
        base.dependencies().to_vec(),
        base.exclusions().to_vec(),
    )
    .expect("a manifest with refs");
}

fn real_object(capsule: &CustomCapsuleV1, kind: KindV1) -> CustodyOriginalObjectV1 {
    capsule
        .source
        .objects
        .iter()
        .find(|object| object.kind() == kind)
        .expect("a real object of that kind")
        .clone()
}

fn direct_ref(name: &str, object: &CustodyOriginalObjectV1) -> CustodyOriginalRefV1 {
    CustodyOriginalRefV1::new(
        LosslessPathV1::from_bytes(name.as_bytes().to_vec()),
        CustodyRefTargetV1::Direct(object.clone()),
    )
    .expect("a manifest ref")
}

fn symbolic_ref(name: &str, target: &str) -> CustodyOriginalRefV1 {
    CustodyOriginalRefV1::new(
        LosslessPathV1::from_bytes(name.as_bytes().to_vec()),
        CustodyRefTargetV1::Symbolic(LosslessPathV1::from_bytes(target.as_bytes().to_vec())),
    )
    .expect("a manifest ref")
}

fn fb<'a>(path: &'static str, content: &'a [u8]) -> (&'a [u8], EntryV1<'a>) {
    (path.as_bytes(), EntryV1::File(content))
}

fn db(path: &'static str) -> (&'static [u8], EntryV1<'static>) {
    (path.as_bytes(), EntryV1::Dir)
}

/// The real state of a Captured capsule: an attached `HEAD` on a loose `refs/heads/main` (the real commit),
/// `ORIG_HEAD`, a packed `refs/heads/side` and `refs/tags/v1` (the real annotated tag, peeled to the
/// real commit), and a `logs/HEAD` and `logs/refs/heads/main` reflog.
struct RealStateV1 {
    refs_frame: Vec<u8>,
    logs_frame: Vec<u8>,
    manifest_refs: Vec<CustodyOriginalRefV1>,
}

fn real_state(capsule: &CustomCapsuleV1, format: GitObjectFormatV1) -> RealStateV1 {
    let generation = CaseV1::generation(capsule);
    let commit = real_object(capsule, KindV1::Commit);
    let tag = real_object(capsule, KindV1::Tag);
    let (commit_id, tag_id) = (commit.object_id().to_owned(), tag.object_id().to_owned());
    let loose = format!("{commit_id}\n");
    let packed_text = format!(
        "{PACKED_HEADER_V1}{commit_id} refs/heads/side\n{tag_id} refs/tags/v1\n^{commit_id}\n"
    );
    let zero = "0".repeat(oid_len(format));
    let log = format!("{zero} {commit_id} {IDENT_V1}\tbranch: Created from main\n");
    let refs_frame = encode_frame(
        REFS_V1,
        &generation,
        &[
            fb("HEAD", b"ref: refs/heads/main\n"),
            fb("ORIG_HEAD", loose.as_bytes()),
            fb("packed-refs", packed_text.as_bytes()),
            db("refs"),
            db("refs/heads"),
            fb("refs/heads/main", loose.as_bytes()),
            db("refs/tags"),
        ],
    );
    let logs_frame = encode_frame(
        LOGS_V1,
        &generation,
        &[
            db("logs"),
            fb("logs/HEAD", log.as_bytes()),
            db("logs/refs"),
            db("logs/refs/heads"),
            fb("logs/refs/heads/main", log.as_bytes()),
        ],
    );
    let manifest_refs = vec![
        symbolic_ref("HEAD", "refs/heads/main"),
        direct_ref("ORIG_HEAD", &commit),
        direct_ref("refs/heads/main", &commit),
        direct_ref("refs/heads/side", &commit),
        direct_ref("refs/tags/v1", &tag),
    ];
    RealStateV1 {
        refs_frame,
        logs_frame,
        manifest_refs,
    }
}

fn real_case(capsule: &CustomCapsuleV1, format: GitObjectFormatV1) -> CaseV1 {
    let state = real_state(capsule, format);
    let mut case = CaseV1::with_bytes(
        capsule,
        &[(REFS_V1, state.refs_frame), (LOGS_V1, state.logs_frame)],
    );
    set_refs(&mut case.verified, state.manifest_refs);
    case
}

/// Real SHA-1 and SHA-256 capsules (real Git objects, a real pack, a real phase-V value) carry the
/// real-shaped refs through the WHOLE chain: pass A's bootstrap proves the objects with real Git, B1a
/// prevalidates the frames and returns its plan, and B1b's agreement over those same retained
/// descriptors, the one ledger, and the one deadline accepts every manifest ref. B1b writes, charges, and
/// reserves nothing.
#[test]
fn b1b_real_objects_and_frames_agree_through_the_whole_chain_in_both_formats() {
    for cformat in [
        CustodyGitObjectFormatV1::Sha1,
        CustodyGitObjectFormatV1::Sha256,
    ] {
        let format = core_format(cformat);
        let capsule = captured(cformat);
        let CaseV1 { dest, verified } = real_case(&capsule, format);
        let mut boot = must_bootstrap(verified, format);
        assert_eq!(
            boot.provenance,
            GitFormatProvenanceV1::CapsuleObjectsAndPack
        );
        let plan = prevalidate(&mut boot.verified, far_deadline()).expect("B1a admits the frames");
        let ledger = ledger_used(&boot.verified);
        let tree = snapshot(&dest.path);

        let agreement = agree(&mut boot.verified, &plan, format)
            .unwrap_or_else(|error| panic!("{cformat:?}: {error:?}"));
        let commit = real_object(&capsule, KindV1::Commit).object_id().to_owned();
        assert_eq!(
            agreement.head,
            HeadStateV1::Symbolic {
                target: "refs/heads/main".into(),
                resolved: Some(commit.clone()),
            }
        );
        assert_eq!(agreement.orig_head, Some(commit));
        assert_eq!(
            (
                agreement.loose_refs,
                agreement.packed_refs,
                agreement.peeled_rows,
                agreement.effective_refs,
                agreement.manifest_refs,
                agreement.reflog_files,
                agreement.reflog_rows,
                agreement.direct_targets,
            ),
            (1, 2, 1, 3, 5, 2, 2, 2),
            "{cformat:?}"
        );
        // Nothing was written, charged, or left mid-read.
        assert_eq!(ledger_used(&boot.verified), ledger, "{cformat:?}");
        assert_eq!(snapshot(&dest.path), tree, "{cformat:?}");
        assert_eq!(dest.entries(), [".restore-work"], "{cformat:?}");
        assert!(boot
            .verified
            .staged
            .iter()
            .filter(|one| matches!(one.role, CustodyCapsuleArtifactRoleV1::CoveragePayload(_)))
            .all(|one| position(one) == 0));
        // The same retained value agrees again, to the same result.
        assert_eq!(
            agree(&mut boot.verified, &plan, format).expect("a second agreement"),
            agreement
        );
    }
}

/// The success control: Empty objects (no pack, an empty inventory) with a Captured, valid, UNBORN HEAD
/// agree for BOTH caller formats, and the bootstrap reports caller-selected provenance.
#[test]
fn b1b_empty_objects_with_a_captured_unborn_head_agree_for_both_caller_formats() {
    for cformat in [
        CustodyGitObjectFormatV1::Sha1,
        CustodyGitObjectFormatV1::Sha256,
    ] {
        let format = core_format(cformat);
        let capsule = empty(cformat);
        let case = CaseV1::refs_only(&capsule, &[db("refs"), db("refs/heads"), db("refs/tags")]);
        let CaseV1 { dest, verified } = case;
        let mut boot = must_bootstrap(verified, format);
        assert_eq!(boot.provenance, GitFormatProvenanceV1::CallerSelectedEmpty);
        assert!(matches!(boot.objects, BootstrapObjectsV1::Empty));
        assert_eq!(boot.closure.inventory_objects, 0);
        let plan = prevalidate(&mut boot.verified, far_deadline()).expect("B1a admits the frame");
        let tree = snapshot(&dest.path);
        let agreement = agree(&mut boot.verified, &plan, format)
            .unwrap_or_else(|error| panic!("{cformat:?}: {error:?}"));
        assert_eq!(
            agreement.head,
            HeadStateV1::Symbolic {
                target: "refs/heads/main".into(),
                resolved: None,
            }
        );
        assert_eq!(
            (
                agreement.effective_refs,
                agreement.direct_targets,
                agreement.manifest_refs
            ),
            (0, 0, 0)
        );
        assert_eq!(snapshot(&dest.path), tree);
        // With a manifest that lists the unborn HEAD, either way, it still agrees.
        for head in [
            symbolic_ref("HEAD", "refs/heads/main"),
            CustodyOriginalRefV1::new(
                LosslessPathV1::from_bytes(b"HEAD".to_vec()),
                CustodyRefTargetV1::Unborn,
            )
            .expect("an unborn ref"),
        ] {
            set_refs(&mut boot.verified, vec![head]);
            let agreement = agree(&mut boot.verified, &plan, format).expect("a listed unborn HEAD");
            assert_eq!(agreement.manifest_refs, 1);
        }
    }
}

/// An all-Empty capsule has no HEAD, so there is nothing to agree: an empty plan (what B1a would have
/// refused to produce) is the same typed refusal, and no HEAD is synthesized. A refs frame with a missing
/// plan entry, or only a reflog frame, is the same refusal.
#[test]
fn b1b_empty_refs_are_refused_and_no_head_is_synthesized() {
    let capsule = frames_capsule();
    let mut case = CaseV1::with_bytes(&capsule, &[]);
    let error =
        prevalidate(&mut case.verified, far_deadline()).expect_err("B1a refuses Empty refs");
    assert!(
        matches!(error, GitBootstrapErrorV1::EmptyRefsAndHead),
        "{error:?}"
    );
    let empty_plan = ActiveFramePlanV1 { frames: vec![] };
    let error =
        agree(&mut case.verified, &empty_plan, GitObjectFormatV1::Sha1).expect_err("no refs frame");
    assert!(
        matches!(error, GitBootstrapErrorV1::EmptyRefsAndHead),
        "{error:?}"
    );
    assert_eq!(case.dest.entries(), [".restore-work"]);

    // Reflogs alone: still no HEAD.
    let generation = CaseV1::generation(&capsule);
    let logs = encode_frame(LOGS_V1, &generation, &[db("logs")]);
    let mut case = CaseV1::with_bytes(&capsule, &[(LOGS_V1, logs)]);
    let error = prevalidate(&mut case.verified, far_deadline()).expect_err("B1a refuses");
    assert!(
        matches!(error, GitBootstrapErrorV1::EmptyRefsAndHead),
        "{error:?}"
    );
    let error =
        agree(&mut case.verified, &empty_plan, GitObjectFormatV1::Sha1).expect_err("no refs");
    assert!(
        matches!(error, GitBootstrapErrorV1::EmptyRefsAndHead),
        "{error:?}"
    );
}

/// The rich plan-backed capsule's REAL frames, as real Git wrote them (a clone with packed refs, a
/// symbolic `origin/HEAD`, a stash, `ORIG_HEAD` from an interrupted merge, and every reflog), agree. The
/// expected facts come from real Git against the real source repository: its ref list (name, object,
/// symbolic target) becomes the manifest's refs, and its HEAD must be the agreement's HEAD.
#[test]
fn b1b_the_rich_clones_real_frames_agree_with_real_git() {
    let fixture = PlanFixtureV1::rich_clone();
    let (manifest, capability, _receipts) = fixture.planned();
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the rich export seals"),
    );
    // The fixture's source clone, beside its export scratch: `<area>/scratch/capsule`.
    let git_dir = fixture
        .capsule_dir()
        .parent()
        .and_then(Path::parent)
        .expect("the fixture area")
        .join("clone/.git");
    assert!(
        git_dir.join("HEAD").is_file(),
        "the source clone's git directory"
    );
    let (dest, mut verified) = phase_v(&fixture.capsule_dir());
    let plan = prevalidate(&mut verified, far_deadline()).expect("B1a admits the real frames");
    let format = GitObjectFormatV1::Sha1;

    // Without a manifest ref, every active target of the real repository is still checked.
    let bare = agree(&mut verified, &plan, format).expect("the real refs agree");
    assert!(
        verified
            .staged
            .iter()
            .filter(|one| matches!(one.role, CustodyCapsuleArtifactRoleV1::CoveragePayload(_)))
            .all(|one| position(one) == 0),
        "every active frame's descriptor is back at offset 0"
    );
    // Real Git's own view.
    let listing = plumb(
        &git_dir,
        &[
            "for-each-ref",
            "--format=%(refname)\t%(objectname)\t%(symref)",
        ],
        None,
    );
    let head_target = plumb(&git_dir, &["symbolic-ref", "HEAD"], None);
    let head_oid = plumb(&git_dir, &["rev-parse", "HEAD"], None);
    let refs: Vec<(String, String, String)> = listing
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            (
                fields.next().expect("a name").to_owned(),
                fields.next().expect("an object").to_owned(),
                fields.next().unwrap_or("").to_owned(),
            )
        })
        .collect();
    assert!(
        refs.len() >= 5,
        "a clone with branches, remotes, and a stash: {refs:?}"
    );
    assert!(refs
        .iter()
        .any(|(name, _, symref)| name == "refs/stash" && symref.is_empty()));
    assert!(refs
        .iter()
        .any(|(name, _, symref)| name == "refs/remotes/origin/HEAD" && !symref.is_empty()));
    assert_eq!(bare.effective_refs, refs.len() as u64, "the real ref count");
    assert_eq!(
        bare.head,
        HeadStateV1::Symbolic {
            target: head_target.clone(),
            resolved: Some(head_oid.clone()),
        }
    );
    assert!(
        bare.orig_head.is_some(),
        "the interrupted merge left ORIG_HEAD"
    );
    assert!(bare.reflog_files >= 3, "{}", bare.reflog_files);
    assert!(bare.packed_refs > 0, "a clone packs its remote refs");

    // With the manifest listing EVERY real ref and HEAD, each is cross-checked by name and target.
    let objects = verified.control.manifest.original_objects().to_vec();
    let mut listed = vec![symbolic_ref("HEAD", &head_target)];
    for (name, oid, symref) in &refs {
        if symref.is_empty() {
            let object = objects
                .iter()
                .find(|object| object.object_id() == oid)
                .unwrap_or_else(|| panic!("{name} names {oid}, which the manifest lists"));
            listed.push(direct_ref(name, object));
        } else {
            listed.push(symbolic_ref(name, symref));
        }
    }
    set_refs(&mut verified, listed.clone());
    let full = agree(&mut verified, &plan, format).expect("every real ref agrees");
    assert_eq!(full.manifest_refs, listed.len() as u64);
    assert_eq!(full.head, bare.head);

    // A manifest that disagrees with real Git on ONE ref is refused by name.
    let (name, oid, _) = refs
        .iter()
        .find(|(name, _, symref)| name == "refs/heads/main" && symref.is_empty())
        .expect("the real main branch");
    let other = objects
        .iter()
        .find(|object| object.object_id() != oid && object.kind() == KindV1::Commit)
        .expect("another real commit");
    let mut wrong = listed.clone();
    wrong.retain(|record| record.name().as_bytes() != name.as_bytes());
    wrong.push(direct_ref(name, other));
    set_refs(&mut verified, wrong);
    assert_eq!(
        refs_fault(agree(&mut verified, &plan, format).expect_err("a wrong object")),
        RefsFaultV1::OriginalMismatch {
            name: name.clone(),
            expected: format!("direct {}", other.object_id()),
            found: format!("direct {oid}"),
        }
    );
    // And one that lists a ref the real repository never had.
    let mut ghost = listed;
    ghost.push(direct_ref("refs/heads/never-existed", other));
    set_refs(&mut verified, ghost);
    assert_eq!(
        refs_fault(agree(&mut verified, &plan, format).expect_err("a ghost ref")),
        RefsFaultV1::MissingOriginal {
            name: "refs/heads/never-existed".into()
        }
    );
    assert_eq!(dest.entries(), [".restore-work"]);

    // Retained descriptors are the only input: every staged leaf renamed away, then unlinked, still agrees.
    set_refs(&mut verified, Vec::new());
    let plain = dest.work().join("plain");
    for (relative, bytes) in snapshot(&plain) {
        if bytes.is_some() {
            let from = plain.join(&relative);
            let to = from.with_file_name(format!("moved-{}", relative.replace('/', "-")));
            std::fs::rename(&from, &to).expect("a staged leaf is renamed");
        }
    }
    assert_eq!(
        agree(&mut verified, &plan, format).expect("renamed leaves"),
        bare
    );
    for (relative, bytes) in snapshot(&plain) {
        if bytes.is_some() {
            std::fs::remove_file(plain.join(&relative)).expect("a staged leaf is unlinked");
        }
    }
    assert_eq!(
        agree(&mut verified, &plan, format).expect("unlinked leaves"),
        bare
    );
}

/// Prevalidates a case's frames, then returns its plan.
fn planned(case: &mut CaseV1) -> ActiveFramePlanV1 {
    prevalidate(&mut case.verified, far_deadline()).expect("B1a admits the frames")
}

/// A refusal in the retained agreement leaves no trace (the ledger, the whole destination tree, and the
/// directory listing are exactly what they were) and is in B1a's failure shape; a refused run still
/// reports the typed fault. One row per class of fault, each from a real frame beside real objects.
#[test]
fn b1b_retained_refusals_leave_no_trace_and_keep_the_failure_shape() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let format = GitObjectFormatV1::Sha1;
    let commit = real_object(&capsule, KindV1::Commit);
    let blob = real_object(&capsule, KindV1::Blob);
    let missing = "7".repeat(40);
    let generation = CaseV1::generation(&capsule);
    let bad_head = encode_frame(REFS_V1, &generation, &[fb("HEAD", b"not a ref\n")]);
    let blob_branch = format!("{}\n", blob.object_id());
    let kind_frame = encode_frame(
        REFS_V1,
        &generation,
        &[
            fb("HEAD", b"ref: refs/heads/main\n"),
            db("refs"),
            db("refs/heads"),
            fb("refs/heads/main", blob_branch.as_bytes()),
        ],
    );
    let absent = format!("{missing}\n");
    let absent_frame = encode_frame(
        REFS_V1,
        &generation,
        &[
            fb("HEAD", b"ref: refs/heads/main\n"),
            db("refs"),
            db("refs/heads"),
            fb("refs/heads/main", absent.as_bytes()),
        ],
    );
    let good = encode_frame(
        REFS_V1,
        &generation,
        &[
            fb("HEAD", b"ref: refs/heads/main\n"),
            db("refs"),
            db("refs/heads"),
        ],
    );
    let rows: Vec<(&str, Vec<u8>, Vec<CustodyOriginalRefV1>)> = vec![
        ("a malformed HEAD", bad_head, vec![]),
        ("a branch on a blob", kind_frame, vec![]),
        ("a target outside the inventory", absent_frame, vec![]),
        (
            "a manifest ref the frame lacks",
            good.clone(),
            vec![direct_ref("refs/heads/main", &commit)],
        ),
    ];
    for (row, frame, refs) in rows {
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, frame)]);
        set_refs(&mut case.verified, refs);
        let plan = planned(&mut case);
        let ledger = ledger_used(&case.verified);
        let tree = snapshot(&case.dest.path);
        let error = agree(&mut case.verified, &plan, format).expect_err(row);
        assert!(
            matches!(error, GitBootstrapErrorV1::Refs(_)),
            "{row}: {error:?}"
        );
        assert_eq!(ledger_used(&case.verified), ledger, "{row}: the ledger");
        assert_eq!(snapshot(&case.dest.path), tree, "{row}: a byte was written");
        assert_eq!(case.dest.entries(), [".restore-work"], "{row}");
    }
}

fn two_real_frames(capsule: &CustomCapsuleV1) -> CaseV1 {
    real_case(capsule, GitObjectFormatV1::Sha1)
}

/// The agreement reads exactly the frames B1a admitted: a plan that is not this capsule's is refused
/// before a frame is decoded, whatever is wrong with it: an entry digest, a count, a length, a digest, a
/// staged index, a class, the frame order, or a missing or extra frame.
#[test]
fn b1b_the_plan_binds_the_frames_the_agreement_reads() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let format = GitObjectFormatV1::Sha1;
    let mut case = two_real_frames(&capsule);
    let plan = planned(&mut case);
    assert_eq!(plan.frames.len(), 2);
    agree(&mut case.verified, &plan, format).expect("the untampered plan");

    type Edit = fn(&mut ActiveFramePlanV1);
    let edits: [(&str, Edit); 12] = [
        ("an entry digest", |plan| {
            plan.frames[0].entries_sha256[0] ^= 1
        }),
        ("the second frame's entry digest", |plan| {
            plan.frames[1].entries_sha256[31] ^= 1
        }),
        ("an entry count", |plan| plan.frames[0].entries += 1),
        ("a directory count", |plan| plan.frames[1].directories += 1),
        ("a file count", |plan| plan.frames[0].regular_files += 1),
        ("a content byte count", |plan| {
            plan.frames[0].content_bytes += 1
        }),
        ("a length", |plan| plan.frames[0].length += 1),
        ("a staged digest", |plan| plan.frames[1].sha256[0] ^= 1),
        ("a staged index", |plan| {
            plan.frames[0].staged = plan.frames[1].staged
        }),
        ("a swapped frame order", |plan| plan.frames.reverse()),
        ("a missing frame", |plan| {
            plan.frames.pop();
        }),
        ("an extra frame", |plan| {
            let extra = plan.frames[0].clone();
            plan.frames.push(extra);
        }),
    ];
    for (row, edit) in edits {
        let mut tampered = plan.clone();
        edit(&mut tampered);
        let ledger = ledger_used(&case.verified);
        let error = agree(&mut case.verified, &tampered, format).expect_err(row);
        assert!(
            matches!(error, GitBootstrapErrorV1::ActivePlanMismatch(_)),
            "{row}: {error:?}"
        );
        assert_eq!(ledger_used(&case.verified), ledger, "{row}");
    }
    // A plan whose class says something else than the staged role it points at.
    let mut tampered = plan.clone();
    tampered.frames[0].class = LOGS_V1;
    let error = agree(&mut case.verified, &tampered, format).expect_err("a wrong class");
    assert!(
        matches!(error, GitBootstrapErrorV1::ActivePlanMismatch(_)),
        "{error:?}"
    );
}

/// A staged frame that no longer is the verified plaintext is refused as THAT, ahead of whatever it says:
/// bytes changed or length changed after B1a (caught by the rehash), and a frame swapped for a well-formed
/// frame carrying a bad HEAD between the rehash and the decode (caught by binding the decoded bytes to the
/// verified digest: the content fault is held, and integrity outranks it).
#[test]
fn b1b_integrity_outranks_content_and_drift_is_refused() {
    use std::os::unix::fs::FileExt as _;
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let format = GitObjectFormatV1::Sha1;
    for (row, edit) in [
        (
            "a byte changed",
            (|file: &std::fs::File| file.write_all_at(&[0xff], 12).expect("change"))
                as fn(&std::fs::File),
        ),
        ("truncated", |file| {
            let length = file.metadata().expect("metadata").len();
            file.set_len(length - 1).expect("truncate");
        }),
        ("a byte appended", |file| {
            let length = file.metadata().expect("metadata").len();
            file.write_all_at(b"x", length).expect("append");
        }),
    ] {
        let mut case = two_real_frames(&capsule);
        let plan = planned(&mut case);
        let index = staged_index(&case.verified, REFS_V1);
        edit(&case.verified.staged[index].file);
        let name = lossy(case.verified.staged[index].name.as_bytes());
        let error = agree(&mut case.verified, &plan, format).expect_err(row);
        assert!(
            matches!(&error, GitBootstrapErrorV1::StagedInputChanged { name: found } if *found == name),
            "{row}: {error:?}"
        );
    }

    let generation = CaseV1::generation(&capsule);
    let good = encode_frame(
        REFS_V1,
        &generation,
        &[
            fb("HEAD", b"ref: refs/heads/main\n"),
            db("refs"),
            db("refs/heads"),
            db("refs/tags"),
        ],
    );
    let evil = encode_frame(REFS_V1, &generation, &[fb("HEAD", b"not a ref at all\n")]);
    assert!(evil.len() < good.len());
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, good)]);
    let plan = planned(&mut case);
    let handle = case.verified.staged[staged_index(&case.verified, REFS_V1)]
        .file
        .try_clone()
        .expect("a duplicate descriptor");
    let swapped = std::cell::Cell::new(false);
    let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "frame-rewind" && !swapped.replace(true) {
            handle.set_len(0).expect("empty the frame");
            handle.write_all_at(&evil, 0).expect("another frame");
        }
    });
    let error = agree(&mut case.verified, &plan, format).expect_err("a swapped frame");
    assert!(
        matches!(error, GitBootstrapErrorV1::StagedInputChanged { .. }),
        "the swapped frame's bad HEAD must not outrank its integrity: {error:?}"
    );

    // A frame swapped for one whose HEAD content is corrupt (its per-file digest no longer holds) fails
    // inside the content read: that is an invalid frame, never a refs fault and never a success.
    let generation = CaseV1::generation(&capsule);
    let good = encode_frame(
        REFS_V1,
        &generation,
        &[fb("HEAD", b"ref: refs/heads/main\n"), db("refs")],
    );
    let mut corrupt = good.clone();
    let at = corrupt
        .windows(5)
        .position(|window| window == b"ref: ")
        .expect("the HEAD content");
    corrupt[at + 6] ^= 0x01;
    assert_eq!(corrupt.len(), good.len());
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, good)]);
    let plan = planned(&mut case);
    let handle = case.verified.staged[staged_index(&case.verified, REFS_V1)]
        .file
        .try_clone()
        .expect("a duplicate descriptor");
    let swapped = std::cell::Cell::new(false);
    let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
        if event.label == "frame-rewind" && !swapped.replace(true) {
            handle.write_all_at(&corrupt, 0).expect("a corrupt frame");
        }
    });
    let error = agree(&mut case.verified, &plan, format).expect_err("a corrupt frame");
    assert!(
        matches!(
            error,
            GitBootstrapErrorV1::Restore(CustodyRestoreErrorV1::FrameInvalid { class: REFS_V1 })
        ),
        "{error:?}"
    );
}

/// The retained agreement's guards: a clean two-frame run reads the clock at exactly the documented
/// guards in order (the shared rehash guards, then its own); each halts the run alone AT that guard with
/// nothing written; brief delays at every guard, with headroom, still succeed; a deadline already spent
/// refuses at the entry guard; and the deadline is observed before every decoded entry.
#[test]
fn b1b_the_retained_agreement_observes_the_one_deadline() {
    const CLEAN: [&str; 14] = [
        "refs-entry",
        "frame-rehash",
        "frame-rewind",
        "refs-decoded",
        "refs-rewound",
        "frame-rehash",
        "frame-rewind",
        "refs-decoded",
        "refs-rewound",
        "refs-structure",
        "refs-targets",
        "refs-resolved",
        "refs-manifest",
        "refs-final",
    ];
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let format = GitObjectFormatV1::Sha1;
    let fresh = || {
        let mut case = two_real_frames(&capsule);
        let plan = planned(&mut case);
        (case, plan)
    };
    let (mut case, plan) = fresh();
    let (labels, hook) = record_guards();
    agree(&mut case.verified, &plan, format).expect("a clean run");
    drop(hook);
    assert_eq!(*labels.borrow(), CLEAN);

    let mut distinct: Vec<&'static str> = Vec::new();
    for label in CLEAN {
        if !distinct.contains(&label) {
            distinct.push(label);
        }
    }
    for target in distinct {
        let (mut case, plan) = fresh();
        let deadline = Instant::now() + B1A_DEADLINE_V1;
        let labels = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
        let fired = std::rc::Rc::new(std::cell::Cell::new(false));
        let (seen, slow) = (labels.clone(), fired.clone());
        let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, move |event| {
            seen.borrow_mut().push(event.label);
            if event.label == target && !slow.replace(true) {
                pass_the_deadline(deadline);
            }
        });
        let ledger = ledger_used(&case.verified);
        let tree = snapshot(&case.dest.path);
        let error = agree_with(
            &mut case.verified,
            &plan,
            format,
            deadline,
            RefsLimitsV1::v1(),
        )
        .expect_err(target);
        assert!(
            matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
            "{target}: {error:?}"
        );
        assert!(fired.get(), "{target}: the slow step never ran");
        assert_eq!(
            labels.borrow().last().copied(),
            Some(target),
            "{target}: the run halted AT its guard: {:?}",
            labels.borrow()
        );
        assert_eq!(ledger_used(&case.verified), ledger, "{target}");
        assert_eq!(snapshot(&case.dest.path), tree, "{target}");
    }

    let (mut case, plan) = fresh();
    {
        let _hook = install_git_hook_for_test(GitHookPointV1::DeadlineObserved, |_| {
            std::thread::sleep(Duration::from_millis(40));
        });
        agree_with(
            &mut case.verified,
            &plan,
            format,
            Instant::now() + B1A_DEADLINE_V1,
            RefsLimitsV1::v1(),
        )
        .expect("fourteen brief delays leave most of the deadline");
    }

    // Spent: refused at the entry guard, no descriptor moved.
    let (mut case, plan) = fresh();
    for one in &case.verified.staged {
        use std::io::Seek as _;
        (&one.file)
            .seek(SeekFrom::Start(3))
            .expect("park the descriptor");
    }
    let spent = Instant::now();
    std::thread::sleep(Duration::from_millis(5));
    let (labels, hook) = record_guards();
    let error = agree_with(&mut case.verified, &plan, format, spent, RefsLimitsV1::v1())
        .expect_err("a spent deadline");
    drop(hook);
    assert!(
        matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
        "{error:?}"
    );
    assert_eq!(*labels.borrow(), ["refs-entry"]);
    assert!(case.verified.staged.iter().all(|one| position(one) == 3));
}

/// A deadline that passes while one entry is being read stops AT that entry (no later entry is decoded),
/// and a large reflog whose entry spends the deadline is not read past the one buffer the decoder had
/// already taken.
#[test]
fn b1b_the_retained_agreement_decodes_nothing_after_the_deadline() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let format = GitObjectFormatV1::Sha1;
    let generation = CaseV1::generation(&capsule);
    let commit = real_object(&capsule, KindV1::Commit);
    let branch = format!("{}\n", commit.object_id());
    let refs = encode_frame(
        REFS_V1,
        &generation,
        &[
            fb("HEAD", b"ref: refs/heads/a\n"),
            db("refs"),
            db("refs/heads"),
            fb("refs/heads/a", branch.as_bytes()),
            fb("refs/heads/b", branch.as_bytes()),
            fb("refs/heads/c", branch.as_bytes()),
        ],
    );
    // The deadline passes while a DIRECTORY entry is judged, where nothing is read from it: the loop's own
    // guard stops the decode at the next entry. Then while a regular file's entry is judged: the file's
    // admission refuses before its first read. Either way no later entry is decoded.
    for (spender, expected) in [
        ("refs/heads", &["HEAD", "refs", "refs/heads"][..]),
        (
            "refs/heads/a",
            &["HEAD", "refs", "refs/heads", "refs/heads/a"][..],
        ),
    ] {
        let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, refs.clone())]);
        let plan = planned(&mut case);
        let deadline = Instant::now() + B1A_DEADLINE_V1;
        let entries = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let seen = entries.clone();
        let hook = install_git_hook_for_test(GitHookPointV1::FrameEntry, move |event| {
            seen.borrow_mut()
                .push(event.path.to_string_lossy().into_owned());
            if event.path == Path::new(spender) {
                pass_the_deadline(deadline);
            }
        });
        let error = agree_with(
            &mut case.verified,
            &plan,
            format,
            deadline,
            RefsLimitsV1::v1(),
        )
        .expect_err("the deadline passed while judging an entry");
        assert!(
            matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
            "{spender}: {error:?}"
        );
        assert_eq!(
            *entries.borrow(),
            expected,
            "{spender}: no entry after the one that spent the deadline was decoded"
        );
        drop(hook);
    }

    // Reads are paced: a 300 KiB reflog whose entry spends the deadline is not read further.
    let zero = "0".repeat(40);
    let row = format!("{zero} {} {IDENT_V1}\tmessage\n", commit.object_id());
    let log = row.repeat(300 * 1024 / row.len() + 1);
    let logs = encode_frame(
        LOGS_V1,
        &generation,
        &[db("logs"), fb("logs/HEAD", log.as_bytes())],
    );
    let head_frame = encode_frame(
        REFS_V1,
        &generation,
        &[fb("HEAD", b"ref: refs/heads/main\n"), db("refs")],
    );
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, head_frame), (LOGS_V1, logs)]);
    let plan = planned(&mut case);
    let deadline = Instant::now() + B1A_DEADLINE_V1;
    let _hook = install_git_hook_for_test(GitHookPointV1::FrameEntry, move |event| {
        if event.path == Path::new("logs/HEAD") {
            pass_the_deadline(deadline);
        }
    });
    let error = agree_with(
        &mut case.verified,
        &plan,
        format,
        deadline,
        RefsLimitsV1::v1(),
    )
    .expect_err("the deadline passed before the log was read");
    assert!(
        matches!(error, GitBootstrapErrorV1::Git(CustodyGitError::Timeout)),
        "{error:?}"
    );
    let one = &case.verified.staged[staged_index(&case.verified, LOGS_V1)];
    assert!(one.length > 2 * HASH_BUFFER_BYTES_V1 as u64);
    assert!(
        position(one) <= HASH_BUFFER_BYTES_V1 as u64,
        "the decode read {} bytes of {}: past the deadline",
        position(one),
        one.length
    );
}

/// The limits apply through the retained entry point, with the failure shape and no trace: a FETCH_HEAD
/// larger than its cap, and every per-class cap lowered by one from what the real frames need.
#[test]
fn b1b_limits_apply_through_the_retained_entry_point() {
    let capsule = captured(CustodyGitObjectFormatV1::Sha1);
    let format = GitObjectFormatV1::Sha1;
    let generation = CaseV1::generation(&capsule);
    let commit = real_object(&capsule, KindV1::Commit);
    let row = format!(
        "{}\t\tbranch 'main' of https://example.invalid/repository\n",
        commit.object_id()
    );
    let limits = RefsLimitsV1::v1();
    let big = row.repeat((limits.fetch_head_bytes as usize / row.len()) + 2);
    assert!(big.len() as u64 > limits.fetch_head_bytes);
    let frame = encode_frame(
        REFS_V1,
        &generation,
        &[
            fb("HEAD", b"ref: refs/heads/main\n"),
            fb("FETCH_HEAD", big.as_bytes()),
        ],
    );
    let mut case = CaseV1::with_bytes(&capsule, &[(REFS_V1, frame)]);
    let plan = planned(&mut case);
    let ledger = ledger_used(&case.verified);
    let tree = snapshot(&case.dest.path);
    let error = agree(&mut case.verified, &plan, format).expect_err("a FETCH_HEAD past its cap");
    assert_eq!(
        refs_fault(error),
        limit_fault(RefsLimitKindV1::FetchHeadFile, limits.fetch_head_bytes)
    );
    assert_eq!(ledger_used(&case.verified), ledger);
    assert_eq!(snapshot(&case.dest.path), tree);

    // The real two-frame state needs these records; one fewer is refused through the same entry point.
    let mut case = two_real_frames(&capsule);
    let plan = planned(&mut case);
    let needed = 1 + 1 + 1 + 2 + 1 + 2; // HEAD, ORIG_HEAD, loose main, two packed rows, a peeled row, two reflogs
    for (bound, ok) in [(needed, true), (needed - 1, false)] {
        let outcome = agree_with(
            &mut case.verified,
            &plan,
            format,
            far_deadline(),
            limits_with(|limits| limits.entries = bound),
        );
        match (ok, outcome) {
            (true, Ok(_)) => {}
            (false, Err(error)) => assert_eq!(
                refs_fault(error),
                limit_fault(RefsLimitKindV1::Entries, bound)
            ),
            (ok, other) => panic!("{bound}: {ok}: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Structure: the agreement region is pure
// ---------------------------------------------------------------------------------------------

const B1B_MARKER_V1: &str = "// Pass B1b: pure refs agreement and inert config";

/// STRUCTURAL ONLY. B1b's region (the production source from its marker on) is pure: it opens no path,
/// walks nothing, writes nothing, charges and reserves nothing, runs no Git child and no process, routes
/// no object store, and has no unsafe. The marker is unique, follows B1a's region, and its region holds
/// the entry points; B1a's own read-only scan covers it too (it runs to the end of the file). The
/// behavior is proven by the retained rows above, which leave the ledger and the destination unchanged.
#[test]
fn b1b_the_agreement_region_is_pure_and_the_split_is_unique() {
    let source = include_str!("custody_restore_git.rs");
    assert_eq!(source.matches(B1B_MARKER_V1).count(), 1, "one marker");
    assert_eq!(source.matches(B1A_MARKER_V1).count(), 1, "one B1a marker");
    let (before, region) = source.split_once(B1B_MARKER_V1).expect("the marker");
    assert!(before.contains(B1A_MARKER_V1), "B1b follows B1a");
    assert!(before.contains("pub(crate) fn prevalidate_active_frames_v1("));
    assert!(region.contains("pub(crate) fn agree_active_refs_v1("));
    assert!(region.contains("pub(crate) fn encode_inert_config_v1("));
    assert!(
        !before.contains("agree_active_refs_v1(") && !before.contains("encode_inert_config_v1("),
        "the earlier passes do not call B1b"
    );
    let code: String = region
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // The one ledger is not touched here at all: B1b's failure reads its figure through B1a's helper.
    assert_eq!(
        code.matches(".ledger").count(),
        0,
        "no access to the ledger"
    );
    for forbidden in [
        "std::fs",
        "File::open(",
        "OpenOptions",
        "read_dir(",
        "create_new",
        "create_staging_directory",
        "write_all",
        "GitRunnerV1",
        "GitRunRequestV1",
        "GitCommandV1",
        "GitObjectStoreRouteV1",
        "object_store",
        "borrow_mut",
        "ReservedPairV1",
        "OwnedChargeV1",
        "ScratchLedgerV1",
        ".reserve(",
        ".release(",
        "roll_back",
        "unsafe",
        "\"repository\"",
        "remeasure_git_directory",
        "measure_git_directory",
        "walk",
        "Command",
        "spawn",
        "std::process",
        "mkdir",
        "rename",
        "remove_file",
        "set_len",
    ] {
        assert!(
            !code.contains(forbidden),
            "the pure region mentions `{forbidden}`"
        );
    }
}
