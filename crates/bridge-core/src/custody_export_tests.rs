//! In-crate real-Git fixtures and behavioral controls for ADR-0041 slice 2B2.
//!
//! These tests are in-crate because the capture-capability constructor is crate-private and the
//! fixture sealer must implement the sealed envelope traits. An external integration-test crate
//! cannot reach either, and no feature may be added to expose them.
//!
//! Every numbered test is one §7 control. Each asserts the typed refusal its guard produces, and
//! each names the single guard the mutation matrix
//! (`.git/a2a-bridge/mutation/`, recorded in the implementation handoff) removes to turn it red.
//! Where defenses are layered, the fixture bypasses the other layers through an exporter-local
//! `#[cfg(test)]` seam, so the mutated guard is the only thing between the fixture and a wrong
//! success.

use super::*;
use crate::custody_capsule::CustodyCapsuleIndexV1;
use crate::custody_coverage::seam::{self as plan_seam, PlanSeamsV1, WalkEventV1};
use crate::custody_coverage::{
    plan_coverage_v1, CustodyCoverageRequestV1, CustodyPlanningBudgetV1,
    ExternalEvidenceUnresolvedV1, GitDirClassSelectionV1, NoExternalEvidenceRecordedV1, WalkKindV1,
    WorktreeSelectionV1,
};
use crate::custody_envelope_fixture::{
    fixture_envelope_format_v1, FixtureSealerV1, SealerFaultV1, FIXTURE_ENVELOPE_MAGIC_V1,
};
use crate::custody_frame::{
    CustodyFrameBudgetV1, CustodyFrameDecoderV1, CustodyFrameEncoderV1, CustodyFrameEntryV1,
    CustodyFrameHeaderV1,
};
use crate::custody_git::{ExpectedGitDigestV1, GitGuardBypassV1};
use crate::custody_git_tests::retry_on_text_file_busy;
use crate::custody_inventory::CustodyStateClassV1;
use crate::custody_mounts::seam as mount_seam;
use crate::custody_seal::{
    CustodyCoverageEntryV1, CustodyDependencyV1, CustodyExclusionV1, CustodyOriginalObjectV1,
    CustodySealabilityV1,
};
use crate::custody_walk::{walk_tree_v1, WalkSelectionV1};
use crate::fs_custody::PublicationRenameFaultV1;
use std::cell::Cell;
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// The lane Git digest helper (§2). 2B2a's own digest helper is private to its tests, so the
// exporter's tests compute the lane binary's SHA-256 themselves. Production has no
// trust-on-first-use: the digest is always a caller input.
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
        // 2B2a's route admission refuses a final symlink, so the lane helper must resolve to the
        // regular file itself rather than to a launcher symlink.
        if metadata.file_type().is_file() {
            return candidate;
        }
    }
    panic!("no non-symlink `git` regular file on PATH; set A2A_LANE_GIT");
}

fn sha256_of_file(path: &Path) -> [u8; 32] {
    let bytes = std::fs::read(path).expect("the lane Git binary is readable");
    sha256_bytes(&bytes)
}

fn lane_git_route() -> GitRouteRequestV1 {
    let path = lane_git_path();
    let digest = ExpectedGitDigestV1::from_bytes(sha256_of_file(&path));
    GitRouteRequestV1::for_test_system(path, digest).expect("the lane Git route is absolute")
}

// ---------------------------------------------------------------------------------------------
// Fixture Git plumbing. These run the lane binary directly with `std::process::Command`: they
// build fixtures, they are not the exporter's effect seam.
// ---------------------------------------------------------------------------------------------

struct GitOutputV1 {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn fixture_git(git_dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> GitOutputV1 {
    let mut command = Command::new(lane_git_path());
    command
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
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the fixture Git child starts");
    {
        let mut pipe = child.stdin.take().expect("piped stdin");
        if let Some(stdin) = stdin {
            pipe.write_all(stdin).expect("fixture stdin");
        }
    }
    let output = child
        .wait_with_output()
        .expect("the fixture Git child exits");
    GitOutputV1 {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

fn fixture_git_ok(git_dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> String {
    let output = fixture_git(git_dir, args, stdin);
    assert!(
        output.success,
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn fixture_init_bare(git_dir: &Path) {
    fixture_init_bare_in(git_dir, CustodyGitObjectFormatV1::Sha1);
}

fn fixture_format_argument(format: CustodyGitObjectFormatV1) -> &'static str {
    match format {
        CustodyGitObjectFormatV1::Sha1 => "--object-format=sha1",
        CustodyGitObjectFormatV1::Sha256 => "--object-format=sha256",
    }
}

fn fixture_init_bare_in(git_dir: &Path, format: CustodyGitObjectFormatV1) {
    let parent = git_dir.parent().expect("a fixture git dir has a parent");
    std::fs::create_dir_all(parent).expect("the fixture parent exists");
    let init = fixture_git(
        parent,
        &[
            "init",
            "--bare",
            "--template=",
            fixture_format_argument(format),
            git_dir.to_str().expect("utf-8 fixture path"),
        ],
        None,
    );
    assert!(
        init.success,
        "fixture init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("fixture hex"))
        .collect()
}

/// Recursive copy for fixture swaps and templates. Fixture trees hold only directories and
/// regular files.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a fixture copy target");
    for entry in std::fs::read_dir(from).expect("a fixture copy source") {
        let entry = entry.expect("a fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a fixture entry type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("a fixture file copy");
        }
    }
}

/// Swap a directory for a byte-identical copy at the same path: the content is unchanged, but the
/// name now resolves to a different directory object.
fn swap_for_identical_copy(path: &Path) {
    let moved = path.with_extension("pinned-original");
    std::fs::rename(path, &moved).expect("the fixture swap moves the original away");
    copy_tree(&moved, path);
}

/// Swap a non-bare worktree root for a new directory at the same path, moving its `.git` across
/// intact: the git directory and its object store keep their identities, and only the worktree
/// root now resolves to a different directory object.
fn swap_worktree_keeping_its_git_dir(worktree: &Path) {
    let moved = worktree.with_extension("pinned-original");
    std::fs::rename(worktree, &moved).expect("the fixture swap moves the worktree away");
    std::fs::create_dir(worktree).expect("a new worktree root at the same path");
    std::fs::rename(moved.join(".git"), worktree.join(".git"))
        .expect("the git directory moves across intact");
    copy_tree(&moved, worktree);
}

/// Overwrite a file in place with the same number of bytes, every one of them inverted.
fn invert_in_place(path: &Path) {
    let mut bytes = std::fs::read(path).expect("an in-place overwrite target");
    for byte in &mut bytes {
        *byte = !*byte;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("an in-place overwrite target opens for writing");
    file.write_all(&bytes).expect("the in-place overwrite");
    file.sync_all().expect("the in-place overwrite syncs");
}

// ---------------------------------------------------------------------------------------------
// The source fixture
// ---------------------------------------------------------------------------------------------

/// A source repository with NO refs at all, so the manifest inventory — not reachability — is the
/// authority on what the capsule must contain. It holds an unreachable tree with two blob
/// children, a two-commit chain over that tree (the reflog-only commit and its parent), and one
/// orphan blob. Optionally every object lives in a pinned alternate store instead. It is bare
/// unless built by `build_non_bare`.
struct SourceFixtureV1 {
    /// The source repository root: the worktree of a non-bare source, the git directory of a
    /// bare one.
    repository: PathBuf,
    git_dir: PathBuf,
    objects: PathBuf,
    format: CustodyGitObjectFormatV1,
    /// Further blobs the manifest declares, beyond the standard six objects.
    extra_blobs: Vec<String>,
    /// The alternate object store the source's `objects/info/alternates` names, if any.
    alternate: Option<PathBuf>,
    /// Every store the no-mutation observation snapshots.
    watched: Vec<PathBuf>,
    blob_a: String,
    blob_b: String,
    tree: String,
    parent_commit: String,
    child_commit: String,
    orphan_blob: String,
}

struct FixtureObjectsV1 {
    blob_a: String,
    blob_b: String,
    tree: String,
    parent_commit: String,
    child_commit: String,
    orphan_blob: String,
}

fn write_fixture_objects(git_dir: &Path) -> FixtureObjectsV1 {
    let blob_a = fixture_git_ok(git_dir, &["hash-object", "-w", "--stdin"], Some(b"alpha"));
    let blob_b = fixture_git_ok(git_dir, &["hash-object", "-w", "--stdin"], Some(b"beta"));
    let tree = fixture_git_ok(
        git_dir,
        &["mktree"],
        Some(format!("100644 blob {blob_a}\ta\n100644 blob {blob_b}\tb\n").as_bytes()),
    );
    let parent_commit = fixture_git_ok(git_dir, &["commit-tree", &tree, "-m", "parent"], None);
    let child_commit = fixture_git_ok(
        git_dir,
        &["commit-tree", &tree, "-p", &parent_commit, "-m", "child"],
        None,
    );
    let orphan_blob = fixture_git_ok(git_dir, &["hash-object", "-w", "--stdin"], Some(b"orphan"));
    FixtureObjectsV1 {
        blob_a,
        blob_b,
        tree,
        parent_commit,
        child_commit,
        orphan_blob,
    }
}

impl SourceFixtureV1 {
    fn from_objects(
        git_dir: PathBuf,
        alternate: Option<PathBuf>,
        watched: Vec<PathBuf>,
        objects: FixtureObjectsV1,
    ) -> Self {
        Self {
            repository: git_dir.clone(),
            objects: git_dir.join("objects"),
            git_dir,
            format: CustodyGitObjectFormatV1::Sha1,
            extra_blobs: Vec::new(),
            alternate,
            watched,
            blob_a: objects.blob_a,
            blob_b: objects.blob_b,
            tree: objects.tree,
            parent_commit: objects.parent_commit,
            child_commit: objects.child_commit,
            orphan_blob: objects.orphan_blob,
        }
    }

    fn build(root: &Path) -> Self {
        let git_dir = root.join("src.git");
        fixture_init_bare(&git_dir);
        let objects = write_fixture_objects(&git_dir);
        Self::from_objects(git_dir.clone(), None, vec![git_dir], objects)
    }

    /// Every object lives in `<alternate_parent>/alt.git`, which the source names through its own
    /// `objects/info/alternates`. The source's own object store is empty.
    fn build_with_alternate_under(root: &Path, alternate_parent: &Path) -> Self {
        let alt_git = alternate_parent.join("alt.git");
        fixture_init_bare(&alt_git);
        let objects = write_fixture_objects(&alt_git);
        let git_dir = root.join("src.git");
        fixture_init_bare(&git_dir);
        let alternate = alt_git.join("objects");
        std::fs::create_dir_all(git_dir.join("objects/info")).expect("the source info dir");
        std::fs::write(
            git_dir.join("objects/info/alternates"),
            format!("{}\n", alternate.display()),
        )
        .expect("the source alternates file");
        Self::from_objects(
            git_dir.clone(),
            Some(alternate),
            vec![git_dir, alt_git],
            objects,
        )
    }

    fn build_with_alternate(root: &Path) -> Self {
        Self::build_with_alternate_under(root, root)
    }

    /// A non-bare source: `worktree` holds one file of worktree bytes, and `git_dir` holds the
    /// objects. With `git_dir` at `worktree/.git` this is a normal clone; anywhere else it is a
    /// `--separate-git-dir` layout, with a `.git` gitlink file in the worktree.
    fn build_non_bare(worktree: &Path, git_dir: &Path) -> Self {
        std::fs::create_dir_all(worktree).expect("the fixture worktree");
        let gitlink = worktree.join(".git");
        let separate = format!("--separate-git-dir={}", git_dir.display());
        let mut args = vec!["init", "--template=", "--object-format=sha1"];
        if git_dir != gitlink {
            args.push(&separate);
        }
        args.push(worktree.to_str().expect("utf-8 fixture path"));
        let init = fixture_git(&gitlink, &args, None);
        assert!(
            init.success,
            "fixture non-bare init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        let objects = write_fixture_objects(git_dir);
        std::fs::write(
            worktree.join("README"),
            b"worktree bytes the export must not touch\n",
        )
        .expect("the worktree file");
        let watched = if git_dir.starts_with(worktree) {
            vec![worktree.to_path_buf()]
        } else {
            vec![worktree.to_path_buf(), git_dir.to_path_buf()]
        };
        let mut source = Self::from_objects(git_dir.to_path_buf(), None, watched, objects);
        source.repository = worktree.to_path_buf();
        source
    }

    /// The standard source in `format`, plus `count` blobs that share one 1.2 KiB body and differ
    /// only in a final line, written by one `fast-import` stream. `pack-objects` stores nearly
    /// all of them as deltas, so `verify-pack -v` prints a two-object-name row for each.
    fn build_delta_heavy(root: &Path, format: CustodyGitObjectFormatV1, count: usize) -> Self {
        let git_dir = root.join("src.git");
        fixture_init_bare_in(&git_dir, format);
        let objects = write_fixture_objects(&git_dir);
        let body: String = (0..24)
            .map(|line| format!("line {line:04} of the shared custody delta fixture body\n"))
            .collect();
        let mut stream = Vec::new();
        for index in 0..count {
            let data = format!("{body}variant {index}\n");
            stream.extend(format!("blob\nmark :{}\ndata {}\n", index + 1, data.len()).bytes());
            stream.extend(data.bytes());
            stream.push(b'\n');
        }
        let marks = root.join("delta-marks");
        let marks_argument = format!("--export-marks={}", marks.display());
        let _ = fixture_git_ok(
            &git_dir,
            &["fast-import", "--quiet", &marks_argument],
            Some(&stream),
        );
        let extra_blobs: Vec<String> = std::fs::read_to_string(&marks)
            .expect("the fast-import marks")
            .lines()
            .map(|line| {
                line.split_once(' ')
                    .expect("a `:mark object-id` line")
                    .1
                    .to_owned()
            })
            .collect();
        assert_eq!(extra_blobs.len(), count, "every delta blob was imported");
        let mut source = Self::from_objects(git_dir.clone(), None, vec![git_dir], objects);
        source.format = format;
        source.extra_blobs = extra_blobs;
        source
    }

    fn rows(&self) -> Vec<(&str, CustodyGitObjectKindV1)> {
        let mut rows = vec![
            (self.blob_a.as_str(), CustodyGitObjectKindV1::Blob),
            (&self.blob_b, CustodyGitObjectKindV1::Blob),
            (&self.tree, CustodyGitObjectKindV1::Tree),
            (&self.parent_commit, CustodyGitObjectKindV1::Commit),
            (&self.child_commit, CustodyGitObjectKindV1::Commit),
            (&self.orphan_blob, CustodyGitObjectKindV1::Blob),
        ];
        rows.extend(
            self.extra_blobs
                .iter()
                .map(|id| (id.as_str(), CustodyGitObjectKindV1::Blob)),
        );
        rows
    }

    /// The complete `(format, object_id, kind)` inventory this fixture's manifest declares.
    fn full_inventory(&self) -> Vec<CustodyOriginalObjectV1> {
        objects_in(self.format, &self.rows())
    }

    fn inventory_without(&self, omitted: &str) -> Vec<CustodyOriginalObjectV1> {
        let rows: Vec<_> = self
            .rows()
            .into_iter()
            .filter(|(id, _)| *id != omitted)
            .collect();
        objects_in(self.format, &rows)
    }

    fn ids(&self) -> Vec<String> {
        self.rows()
            .into_iter()
            .map(|(id, _)| id.to_owned())
            .collect()
    }

    /// The store that physically holds the fixture objects.
    fn object_home(&self) -> PathBuf {
        self.alternate
            .clone()
            .unwrap_or_else(|| self.objects.clone())
    }

    fn loose_object_path(&self, id: &str) -> PathBuf {
        self.object_home().join(&id[..2]).join(&id[2..])
    }

    /// A historical tree whose one entry records its mode zero-padded (`0100644`). Git writes it
    /// only with `hash-object --literally`; strict object checks reject it as
    /// `zeroPaddedFilemode`.
    fn write_zero_padded_tree(&self) -> String {
        let mut raw = b"0100644 a\0".to_vec();
        raw.extend(hex_to_bytes(&self.blob_a));
        fixture_git_ok(
            &self.git_dir,
            &["hash-object", "-t", "tree", "--literally", "-w", "--stdin"],
            Some(&raw),
        )
    }

    /// Every byte of every watched store, for the end-to-end no-mutation observation.
    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        let mut snapshot = BTreeMap::new();
        for root in &self.watched {
            for (name, bytes) in snapshot_tree(root) {
                snapshot.insert(format!("{}/{name}", root.display()), bytes);
            }
        }
        snapshot
    }
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    let mut frontier = vec![(root.to_path_buf(), String::new())];
    while let Some((path, prefix)) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let metadata = std::fs::symlink_metadata(entry.path()).expect("fixture metadata");
            if metadata.is_dir() {
                frontier.push((entry.path(), relative));
            } else if metadata.file_type().is_symlink() {
                let target = std::fs::read_link(entry.path()).expect("fixture link");
                snapshot.insert(
                    relative,
                    format!("symlink:{}", target.display()).into_bytes(),
                );
            } else {
                snapshot.insert(
                    relative,
                    std::fs::read(entry.path()).expect("fixture file is readable"),
                );
            }
        }
    }
    snapshot
}

/// `snapshot_tree` plus every directory beneath `root`, each keyed with a trailing `/`: the
/// byte-and-entry snapshot, which an effect that creates only empty directories still changes.
fn snapshot_entries(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut snapshot = snapshot_tree(root);
    let mut frontier = vec![root.to_path_buf()];
    while let Some(path) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let metadata = std::fs::symlink_metadata(entry.path()).expect("fixture metadata");
            if metadata.is_dir() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .expect("an entry beneath the snapshot root")
                    .to_string_lossy()
                    .into_owned();
                snapshot.insert(format!("{relative}/"), Vec::new());
                frontier.push(entry.path());
            }
        }
    }
    snapshot
}

fn objects_from(rows: &[(&str, CustodyGitObjectKindV1)]) -> Vec<CustodyOriginalObjectV1> {
    objects_in(CustodyGitObjectFormatV1::Sha1, rows)
}

fn objects_in(
    format: CustodyGitObjectFormatV1,
    rows: &[(&str, CustodyGitObjectKindV1)],
) -> Vec<CustodyOriginalObjectV1> {
    rows.iter()
        .map(|(id, kind)| {
            CustodyOriginalObjectV1::new(format, *id, *kind)
                .expect("a fixture object id is well formed")
        })
        .collect()
}

fn inventory_tuples(
    objects: &[CustodyOriginalObjectV1],
) -> Vec<(CustodyGitObjectFormatV1, String, CustodyGitObjectKindV1)> {
    objects
        .iter()
        .map(|object| {
            (
                object.format(),
                object.object_id().to_owned(),
                object.kind(),
            )
        })
        .collect()
}

/// `pack-objects` stdin for an explicit object list: sorted, one id per line.
fn object_lines(ids: &[String]) -> Vec<u8> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.iter()
        .flat_map(|id| format!("{id}\n").into_bytes())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Manifests and captured streams
// ---------------------------------------------------------------------------------------------

const GENERATION_V1: &str = "generation-2b2";
pub(crate) const IDENTITY_V1: [&str; 4] =
    ["unit-2b2", "run-2b2", "materialization-2b2", GENERATION_V1];

/// Three captured classes — the object database plus two non-Git payloads — so the derived layout
/// has six artifacts across all three reserved capsule directories and control 19 has two
/// equal-length payload roles to exchange.
const CAPTURED_PAYLOAD_CLASSES_V1: [CustodyCoverageClassV1; 2] = [
    CustodyCoverageClassV1::Index,
    CustodyCoverageClassV1::Worktree,
];

fn coverage_rows(captured: &[CustodyCoverageClassV1]) -> Vec<CustodyCoverageEntryV1> {
    CustodyCoverageClassV1::ALL
        .into_iter()
        .map(|class| {
            let state = if captured.contains(&class) {
                CustodyStateClassV1::Captured
            } else {
                CustodyStateClassV1::Empty
            };
            CustodyCoverageEntryV1::new(class, state, vec![], None)
                .expect("a fixture coverage row is valid")
        })
        .collect()
}

pub(crate) fn manifest_for(
    identity: [&str; 4],
    objects: Vec<CustodyOriginalObjectV1>,
) -> CustodyManifestV1 {
    let [unit, run, materialization, generation] = identity;
    CustodyManifestV1::new(
        unit,
        run,
        materialization,
        generation,
        coverage_rows(&captured_classes()),
        vec![],
        objects,
        vec![],
        vec![],
    )
    .expect("a fixture manifest is valid")
}

fn manifest_with(objects: Vec<CustodyOriginalObjectV1>) -> CustodyManifestV1 {
    manifest_for(IDENTITY_V1, objects)
}

fn captured_classes() -> Vec<CustodyCoverageClassV1> {
    let mut classes = vec![CustodyCoverageClassV1::ObjectDatabase];
    classes.extend(CAPTURED_PAYLOAD_CLASSES_V1);
    classes
}

/// Two payload streams of EQUAL length, so control 19's exchange cannot be caught by a length
/// comparison alone.
pub(crate) fn default_streams(generation: &str) -> Vec<CustodyCapturedStreamV1> {
    vec![
        CustodyCapturedStreamV1::for_test_fixture(
            CustodyCoverageClassV1::Index,
            generation,
            b"index-payload-bytes-0123456789ab".to_vec(),
        ),
        CustodyCapturedStreamV1::for_test_fixture(
            CustodyCoverageClassV1::Worktree,
            generation,
            b"worktree-payload-bytes-01234567a".to_vec(),
        ),
    ]
}

const PACK_ARTIFACT_V1: &str = "git/objects.pack.enc";
const MANIFEST_ARTIFACT_V1: &str = "control/manifest.json.enc";
const WORKTREE_ARTIFACT_V1: &str = "payload/worktree.bin.enc";

// ---------------------------------------------------------------------------------------------
// The export harness
// ---------------------------------------------------------------------------------------------

pub(crate) struct HarnessV1 {
    _temp: tempfile::TempDir,
    root: PathBuf,
    source: SourceFixtureV1,
    scratch: PathBuf,
    pub(crate) manifest: CustodyManifestV1,
    budgets: CustodyExportBudgetsV1,
    pub(crate) streams: Vec<CustodyCapturedStreamV1>,
    git_route: GitRouteRequestV1,
}

fn recipients() -> Vec<String> {
    vec!["fixture-recipient".to_owned()]
}

impl HarnessV1 {
    pub(crate) fn new() -> Self {
        Self::with_source(SourceFixtureV1::build)
    }

    fn with_alternate() -> Self {
        Self::with_source(SourceFixtureV1::build_with_alternate)
    }

    fn with_source(build: impl FnOnce(&Path) -> SourceFixtureV1) -> Self {
        let temp = tempfile::TempDir::new().expect("a fixture temp root");
        // Canonical, so the §2 disjointness comparisons see the paths the exporter sees.
        let root = temp
            .path()
            .canonicalize()
            .expect("a canonical fixture root");
        let source = build(&root);
        let scratch = root.join("scratch");
        std::fs::create_dir(&scratch).expect("a fixture scratch root");
        set_owner_private(&scratch);
        let manifest = manifest_with(source.full_inventory());
        Self {
            _temp: temp,
            root,
            source,
            scratch,
            manifest,
            // Small chunks so the §6 chunk-boundary sampling has a first, second, and final chunk
            // to sample on a multi-chunk artifact and on the Git pack.
            budgets: CustodyExportBudgetsV1 {
                max_chunk_bytes: 8,
                ..CustodyExportBudgetsV1::v1_ceilings()
            },
            streams: default_streams(GENERATION_V1),
            git_route: lane_git_route(),
        }
    }

    fn capability_for(&self, manifest: &CustodyManifestV1) -> CustodyCaptureCapabilityV1 {
        CustodyCaptureCapabilityV1::from_fixture_quiescence(
            CustodyQuiescenceDecisionV1::CoherentSnapshot,
            manifest,
            &self.source.repository,
            &self.source.git_dir,
            &self.source.objects,
            self.streams.clone(),
        )
        .expect("the fixture capability mints")
    }

    fn capability(&self) -> CustodyCaptureCapabilityV1 {
        self.capability_for(&self.manifest)
    }

    fn run_with(
        &self,
        sealer: &dyn CustodyEnvelopeSealerV1,
        capability: CustodyCaptureCapabilityV1,
    ) -> Result<CustodyExportOutcomeV1, CustodyExportErrorV1> {
        reset_export_counters_for_test();
        export_capsule_v1(CustodyExportRequestV1 {
            manifest: &self.manifest,
            envelope_format: &fixture_envelope_format_v1(),
            recipients: &recipients(),
            budgets: self.budgets,
            sealer,
            capability,
            git_route: self.git_route.clone(),
            scratch_root: &self.scratch,
            deadline: Instant::now() + Duration::from_secs(120),
        })
    }

    fn run(&self) -> Result<CustodyExportOutcomeV1, CustodyExportErrorV1> {
        self.run_with(&FixtureSealerV1::honest(), self.capability())
    }

    /// Run an export that must seal, with the §7 no-mutation observation: the source's object,
    /// ref, config, and alternate bytes are identical before and after.
    pub(crate) fn run_sealed(&self) -> Box<CustodyExportSealedV1> {
        let before = self.source.snapshot();
        let sealed = expect_sealed(self.run().expect("the fixture export seals"));
        assert_eq!(
            self.source.snapshot(),
            before,
            "the export mutated the source repository"
        );
        assert!(self.seal_present());
        sealed
    }

    /// Run an export that must refuse, and require that no seal exists afterwards.
    fn refuse(&self) -> CustodyExportErrorV1 {
        let error = self.run().expect_err("the export must refuse");
        assert!(
            !self.seal_present(),
            "a refused export left a seal: {error:?}"
        );
        error
    }

    fn capsule(&self) -> PathBuf {
        self.scratch.join(CAPSULE_DIR_NAME)
    }

    /// The published capsule directory, for the 2B3a capsule reader's controls.
    pub(crate) fn capsule_dir(&self) -> PathBuf {
        self.scratch.join(CAPSULE_DIR_NAME)
    }

    fn work(&self) -> PathBuf {
        self.scratch.join(WORK_DIR_NAME)
    }

    fn seal_present(&self) -> bool {
        std::fs::symlink_metadata(self.capsule().join(SEAL_NAME)).is_ok()
    }

    fn scratch_is_empty(&self) -> bool {
        std::fs::read_dir(&self.scratch)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
    }
}

fn set_owner_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .expect("the fixture scratch root is owner-private");
}

/// A fixture Git route sealed the way 2B2a's fixtures are. The script and its anchor are mode
/// `0500`, so the audited components deny effective write access even to a non-root owner (the
/// macOS host lane); root sees no group or other write bit. Drop reopens the anchor so the
/// harness's temporary directory can be removed.
///
/// Sealing also executes the script once as `<script> --version`, which every route here passes
/// to the lane Git. A fork in another test thread while the script's write descriptor was open
/// leaves a writable duplicate in that child until it execs, and exec refuses the script
/// meanwhile (`ETXTBSY`). The export cannot be retried in place, so the warm-up absorbs the race:
/// one exec that is not refused proves no writer remains, and none can appear later because the
/// sealed script is never opened for writing again.
struct SealedRouteAnchorV1 {
    anchor: PathBuf,
}

impl SealedRouteAnchorV1 {
    fn seal(script: &Path, anchor: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o500))
            .expect("seal the fixture route script");
        std::fs::set_permissions(anchor, std::fs::Permissions::from_mode(0o500))
            .expect("seal the fixture route anchor");
        let sealed = Self {
            anchor: anchor.to_path_buf(),
        };
        retry_on_text_file_busy(|| Command::new(script).arg("--version").output())
            .expect("the fixture route warm-up exec");
        sealed
    }
}

impl Drop for SealedRouteAnchorV1 {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&self.anchor, std::fs::Permissions::from_mode(0o700));
    }
}

pub(crate) fn expect_sealed(outcome: CustodyExportOutcomeV1) -> Box<CustodyExportSealedV1> {
    match outcome {
        CustodyExportOutcomeV1::Sealed(sealed) => sealed,
        other => panic!("expected a sealed capsule, observed {other:?}"),
    }
}

/// A one-line account of an export result, so a wrong success names its row without dumping the
/// whole sealed capsule.
fn describe_run(result: &Result<CustodyExportOutcomeV1, CustodyExportErrorV1>) -> String {
    match result {
        Ok(CustodyExportOutcomeV1::Sealed(_)) => "wrong success: a capsule sealed".to_owned(),
        Ok(other) => format!("wrong outcome: {other:?}"),
        Err(error) => format!("refused with {error:?}"),
    }
}

/// A hook that acts on its `nth` invocation only (1-based).
fn on_nth_call(nth: usize, action: impl Fn(&Path) + 'static) -> impl Fn(&Path) + 'static {
    let calls = Cell::new(0_usize);
    move |path| {
        calls.set(calls.get() + 1);
        if calls.get() == nth {
            action(path);
        }
    }
}

/// A hook that acts on the first invocation whose path satisfies `select`, and never again.
fn once_where(
    select: impl Fn(&Path) -> bool + 'static,
    action: impl Fn(&Path) + 'static,
) -> impl Fn(&Path) + 'static {
    let done = Cell::new(false);
    move |path| {
        if !done.get() && select(path) {
            done.set(true);
            action(path);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Control 24 compile-pass arm, the happy path, and the no-mutation observation
// ---------------------------------------------------------------------------------------------

/// Control 24, compile-pass arm: an in-crate caller reaches the crate-private entry point that
/// 24a's compile-fail doctest proves no external crate can name.
#[test]
fn control_24_in_crate_caller_reaches_the_crate_private_entry_point() {
    let harness = HarnessV1::new();
    let sealed = harness.run_sealed();

    assert_eq!(sealed.binding.artifact_names().len(), 6);
    assert_eq!(sealed.seal.artifacts().len(), 6);
    assert_eq!(
        sealed.evidence.object_format,
        CustodyGitObjectFormatV1::Sha1
    );
}

/// §4.2: the evidence records the Git version the runner actually admitted, the exact argv, and
/// the closed environment keys of every child — exit status alone is never evidence.
#[test]
fn the_evidence_records_the_observed_git_version_and_every_child() {
    let harness = HarnessV1::new();
    let sealed = harness.run_sealed();
    let evidence = &sealed.evidence;

    let reported = fixture_git_ok(&harness.root, &["version"], None);
    let observed = crate::custody_git::parse_git_version(reported.as_bytes())
        .expect("the lane Git version parses");
    assert_eq!(evidence.git_version, observed);
    assert!(evidence.git_version >= GitRunnerV1::MINIMUM_VERSION);

    let labels: Vec<&str> = evidence.runs.iter().map(|run| run.label).collect();
    assert_eq!(
        labels,
        [
            "init --bare",
            "init --bare",
            "cat-file --batch-check",
            "pack-objects --stdout",
            "index-pack --strict --stdin",
            "verify-pack",
            "cat-file --batch-all-objects",
            "rev-list --missing=print",
            "fsck --strict",
        ]
    );
    for run in &evidence.runs {
        assert_eq!(run.version, observed);
        assert_eq!(run.exit_status, Some(0), "{}", run.label);
        assert!(run.environment.contains_key("GIT_CONFIG_NOSYSTEM"));
        assert!(!run.argv.is_empty());
    }
    // The pack is streamed, so its stdout evidence is the verified-pack identity.
    let pack_run = &evidence.runs[3];
    assert_eq!(pack_run.stdout.length as u64, evidence.verified_pack_length);
    assert_eq!(pack_run.stdout.sha256, evidence.verified_pack_sha256);
    // `index-pack` was fed exactly the recorded bytes.
    let index_run = &evidence.runs[4];
    assert_eq!(index_run.stdin.length as u64, evidence.verified_pack_length);
    assert_eq!(index_run.stdin.sha256, evidence.verified_pack_sha256);
}

/// §4.2: the recorded Git version is the one the runner admitted, not the 2B2a minimum. The lane
/// Git is exactly the 2.54.0 minimum, so this route reports 2.99.1 for `version` and runs the lane
/// Git for everything else; a hard-coded minimum would record 2.54.0.
#[test]
fn the_evidence_records_the_admitted_git_version_not_the_minimum() {
    let mut harness = HarnessV1::new();
    let anchor = harness.root.join("version-anchor");
    std::fs::create_dir(&anchor).expect("the route anchor");
    set_owner_private(&anchor);
    let script = anchor.join("git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$*\" in *\" version\") echo 'git version 2.99.1'; exit 0;; esac\n\
             exec '{}' \"$@\"\n",
            lane_git_path().display()
        ),
    )
    .expect("the route script");
    let _sealed = SealedRouteAnchorV1::seal(&script, &anchor);
    let digest = ExpectedGitDigestV1::from_bytes(sha256_of_file(&script));
    harness.git_route = GitRouteRequestV1::for_test_fixture(script, digest, anchor)
        .expect("the fixture route is absolute");

    let sealed = harness.run_sealed();
    assert_eq!(sealed.evidence.git_version, (2, 99, 1));
    assert!(sealed
        .evidence
        .runs
        .iter()
        .all(|run| run.version == (2, 99, 1)));
}

/// The exported capsule holds exactly the reserved 2B1 logical names plus the seal, and `work/`
/// contributes no capsule member.
#[test]
fn the_capsule_holds_exactly_the_reserved_names_plus_the_seal() {
    let harness = HarnessV1::new();
    let sealed = harness.run_sealed();

    let published: BTreeSet<String> = snapshot_tree(&harness.capsule()).into_keys().collect();
    let mut expected: BTreeSet<String> = sealed
        .seal
        .artifacts()
        .iter()
        .map(|artifact| String::from_utf8_lossy(artifact.name().as_bytes()).into_owned())
        .collect();
    expected.insert(SEAL_NAME.to_owned());
    assert_eq!(published, expected);

    // The scratch root holds exactly the two exporter-created children.
    let top: BTreeSet<String> = std::fs::read_dir(&harness.scratch)
        .expect("the scratch root")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        top,
        BTreeSet::from([CAPSULE_DIR_NAME.to_owned(), WORK_DIR_NAME.to_owned()])
    );

    // `work/` retains the plaintext pack and the verification database as inert scratch evidence.
    assert!(harness.work().join(PACK_FILE_NAME).exists());
    assert!(harness.work().join(VERIFY_GIT_DIR_NAME).is_dir());
    assert!(harness.work().join(SOURCE_GIT_DIR_NAME).is_dir());
}

/// The published pack artifact's envelope body is byte-for-byte the verified pack, so the bytes
/// §5 verified are the bytes §6 sealed.
#[test]
fn the_sealed_pack_is_the_verified_pack() {
    let harness = HarnessV1::new();
    let sealed = harness.run_sealed();
    let envelope = std::fs::read(harness.capsule().join(PACK_ARTIFACT_V1)).expect("the pack");
    let staged = std::fs::read(harness.work().join(PACK_FILE_NAME)).expect("the staged pack");
    assert_eq!(&envelope[..8], FIXTURE_ENVELOPE_MAGIC_V1);
    assert_eq!(&envelope[8..], staged.as_slice());
    assert_eq!(sha256_bytes(&staged), sealed.evidence.verified_pack_sha256);
}

/// A capsule whose Git objects live entirely in a pinned alternate store seals, and neither store
/// changes.
#[test]
fn an_export_through_a_pinned_alternate_store_seals() {
    let harness = HarnessV1::with_alternate();
    let _ = harness.run_sealed();
}

/// A non-bare source, with the scratch root outside its worktree, seals. The no-mutation
/// observation then covers real worktree bytes, not only the git directory.
#[test]
fn an_export_from_a_non_bare_source_seals_and_leaves_its_worktree_untouched() {
    let harness = HarnessV1::with_source(|root| {
        SourceFixtureV1::build_non_bare(&root.join("repo"), &root.join("repo/.git"))
    });
    assert!(harness
        .source
        .snapshot()
        .keys()
        .any(|key| key.ends_with("repo/README")));
    let _ = harness.run_sealed();
}

// ---------------------------------------------------------------------------------------------
// §5 closure proof: controls 1, 2, 3, 4, 6, 7, 17, 20a, 20b
// ---------------------------------------------------------------------------------------------

/// The seed-then-remove seam of controls 1 and 2: seed `object` as a loose object into
/// `verify.git` before §5 step 1, so `index-pack --strict` can resolve the link, and remove it
/// after step 1 and before step 2.
fn seed_then_remove(harness: &HarnessV1, object: &str) -> (ExportSeamGuardV1, ExportSeamGuardV1) {
    let source = harness.source.loose_object_path(object);
    let relative = PathBuf::from(&object[..2]).join(&object[2..]);
    let seeded = relative.clone();
    let before = install_export_hook_for_test(ExportHookPointV1::BeforeIndexPack, move |work| {
        let target = work.join(VERIFY_GIT_DIR_NAME).join("objects").join(&seeded);
        std::fs::create_dir_all(target.parent().expect("a loose object dir"))
            .expect("the seed directory");
        std::fs::copy(&source, &target).expect("the seed copy");
    });
    let after = install_export_hook_for_test(ExportHookPointV1::AfterIndexPack, move |work| {
        let target = work
            .join(VERIFY_GIT_DIR_NAME)
            .join("objects")
            .join(&relative);
        std::fs::remove_file(target).expect("the seed is removed after step 1");
    });
    (before, after)
}

/// Control 1: the manifest and pack omit a reflog-only commit's parent X. `index-pack --strict`
/// can resolve the link only through the seeded loose X, and X is gone again before step 2, so
/// step 3's all-object closure is the only guard. `fsck --no-dangling` does not report a missing
/// parent of an unreachable commit (probe P1), so deleting only the `rev-list` guard seals.
#[test]
fn control_01_closure_refuses_a_reflog_only_commit_whose_parent_is_absent() {
    let mut harness = HarnessV1::new();
    let parent = harness.source.parent_commit.clone();
    harness.manifest = manifest_with(harness.source.inventory_without(&parent));
    let _seam = seed_then_remove(&harness, &parent);

    let error = harness.refuse();
    assert!(
        matches!(&error, CustodyExportErrorV1::ClosureIncomplete(detail) if detail.contains(&parent)),
        "expected a closure refusal naming {parent}, observed {error:?}"
    );
}

/// Control 2: the manifest and pack omit an unreachable tree's blob child, with the same
/// seed-then-remove seam, so step 3 is again the only guard.
#[test]
fn control_02_closure_refuses_an_unreachable_tree_whose_blob_child_is_absent() {
    let mut harness = HarnessV1::new();
    let child = harness.source.blob_b.clone();
    harness.manifest = manifest_with(harness.source.inventory_without(&child));
    let _seam = seed_then_remove(&harness, &child);

    let error = harness.refuse();
    assert!(
        matches!(&error, CustodyExportErrorV1::ClosureIncomplete(detail) if detail.contains(&child)),
        "expected a closure refusal naming {child}, observed {error:?}"
    );
}

/// Control 3: the orphan blob is in the manifest but dropped from the staged pack. A dropped root
/// is also a `?`-missing root to `rev-list`, so step 3 is bypassed and step 2's equality is the
/// only guard.
#[test]
fn control_03_inventory_equality_refuses_an_orphan_dropped_from_the_pack() {
    let harness = HarnessV1::new();
    let orphan = harness.source.orphan_blob.clone();
    let kept: Vec<String> = harness
        .source
        .ids()
        .into_iter()
        .filter(|id| *id != orphan)
        .collect();
    let _input = override_pack_input_for_test(object_lines(&kept));
    let _bypass = set_export_bypass_for_test(ExportBypassV1 {
        closure_step: true,
        ..ExportBypassV1::default()
    });

    let error = harness.refuse();
    assert!(
        matches!(&error, CustodyExportErrorV1::InventoryMismatch(detail)
            if detail.contains(&orphan) && detail.contains("missing")),
        "expected an inventory refusal for the missing {orphan}, observed {error:?}"
    );
}

/// Control 4 (positive): an orphan blob that is present and valid must NOT refuse. §5 step 4 runs
/// `fsck --no-dangling`, so a legitimately unreachable object is not an ambiguity.
#[test]
fn control_04_a_present_and_valid_orphan_blob_seals() {
    let harness = HarnessV1::new();
    let sealed = harness.run_sealed();
    assert!(sealed
        .seal
        .artifacts()
        .iter()
        .any(|artifact| artifact.name().as_bytes() == PACK_ARTIFACT_V1.as_bytes()));
    let fsck = sealed
        .evidence
        .runs
        .iter()
        .find(|run| run.label == "fsck --strict")
        .expect("fsck ran");
    assert_eq!(fsck.exit_status, Some(0));
}

/// Control 6: the pack carries one extra object the manifest does not declare. The extra object is
/// unreachable from the manifest inventory, so steps 3 and 4 pass and step 2's equality is the
/// only guard.
#[test]
fn control_06_inventory_equality_refuses_an_extra_packed_object() {
    let mut harness = HarnessV1::new();
    let orphan = harness.source.orphan_blob.clone();
    harness.manifest = manifest_with(harness.source.inventory_without(&orphan));
    let _input = override_pack_input_for_test(object_lines(&harness.source.ids()));

    let error = harness.refuse();
    assert!(
        matches!(&error, CustodyExportErrorV1::InventoryMismatch(detail)
            if detail.contains(&orphan) && detail.contains("extra")),
        "expected an inventory refusal for the extra {orphan}, observed {error:?}"
    );
}

/// Control 7: `work/objects.pack` is truncated, or corrupted in place at the same length, after
/// staging. §5 step 1's strict indexing refuses it with a typed `StrictPack`.
#[test]
fn control_07_strict_indexing_refuses_a_truncated_or_corrupt_staged_pack() {
    type DamageV1 = fn(&Path);
    let rows: [(&str, DamageV1); 2] = [
        ("truncated", |path: &Path| {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .expect("the staged pack");
            let length = file.metadata().expect("pack metadata").len();
            file.set_len(length / 2).expect("the truncation");
        }),
        ("corrupt in place", |path: &Path| {
            let mut bytes = std::fs::read(path).expect("the staged pack");
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0xFF;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .expect("the staged pack");
            file.write_all(&bytes).expect("the corruption");
        }),
    ];
    for (row, damage) in rows {
        let harness = HarnessV1::new();
        let _hook = install_export_hook_for_test(ExportHookPointV1::BeforeIndexPack, move |work| {
            damage(&work.join(PACK_FILE_NAME));
        });
        let error = harness.refuse();
        assert!(
            matches!(error, CustodyExportErrorV1::StrictPack(_)),
            "{row}: expected a strict-pack refusal, observed {error:?}"
        );
    }
}

/// §5 step 1 bounds `index-pack`'s stdin by the RECORDED pack length. A staged pack that has grown
/// since it was recorded is refused by the runner before any spawn.
#[test]
fn step_1_refuses_a_staged_pack_that_grew_past_its_recorded_length() {
    let harness = HarnessV1::new();
    let _hook = install_export_hook_for_test(ExportHookPointV1::BeforeIndexPack, |work| {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(work.join(PACK_FILE_NAME))
            .expect("the staged pack");
        file.write_all(b"appended").expect("the growth");
    });
    let error = harness.refuse();
    assert!(
        matches!(
            error,
            CustodyExportErrorV1::Git(CustodyGitError::StdinLimit { .. })
        ),
        "expected a recorded-length stdin refusal, observed {error:?}"
    );
}

/// Control 17: §5 step 1 verifies a byte-distinct pack B with the same inventory and length while
/// the exporter would seal the recorded pack A. B indexes cleanly and passes steps 2–4, so the
/// comparison of `GitRunEvidenceV1.stdin` with the verified-pack identity is the only guard.
#[test]
fn control_17_step_1_refuses_a_verification_input_that_is_not_the_recorded_pack() {
    let harness = HarnessV1::new();
    let source_git_dir = harness.source.git_dir.clone();
    let mut reversed = harness.source.ids();
    reversed.sort();
    reversed.reverse();
    let lines: Vec<u8> = reversed
        .iter()
        .flat_map(|id| format!("{id}\n").into_bytes())
        .collect();
    let _hook = install_export_hook_for_test(ExportHookPointV1::BeforeIndexPack, move |work| {
        let pack = fixture_git(&source_git_dir, &["pack-objects", "--stdout"], Some(&lines));
        assert!(pack.success, "the fixture pack B builds");
        std::fs::write(work.join("objects-b.pack"), pack.stdout).expect("pack B is staged");
    });
    let _substitute = substitute_index_pack_stdin_for_test("objects-b.pack");

    let error = harness.refuse();
    assert!(
        matches!(error, CustodyExportErrorV1::VerificationInputMismatch),
        "expected a verification-input refusal, observed {error:?}"
    );

    // Fixture validity: B is the same length as A, so neither the recorded-length bound nor a
    // length comparison could catch it, and B differs from A.
    let a = std::fs::read(harness.work().join(PACK_FILE_NAME)).expect("pack A");
    let b = std::fs::read(harness.work().join("objects-b.pack")).expect("pack B");
    assert_eq!(a.len(), b.len(), "pack B must have pack A's length");
    assert_ne!(a, b, "pack B must be byte-distinct from pack A");
}

/// The manifest for controls 20a and 20b: the standard inventory plus a historical tree with a
/// zero-padded file mode over the fixture's `alpha` blob.
fn zero_padded_harness() -> (HarnessV1, String) {
    let mut harness = HarnessV1::new();
    let tree = harness.source.write_zero_padded_tree();
    let mut objects = harness.source.full_inventory();
    objects.extend(objects_from(&[(&tree, CustodyGitObjectKindV1::Tree)]));
    harness.manifest = manifest_with(objects);
    (harness, tree)
}

/// Control 20a: the zero-padded tree reaches §5 step 1, and `index-pack --strict`'s rejection is
/// classified as a dedicated `StrictObjectCheck` carrying the Git message id, not a generic
/// strict-pack refusal.
#[test]
fn control_20a_step_1_classifies_a_zero_padded_file_mode_as_a_strict_object_check() {
    let (harness, _tree) = zero_padded_harness();
    let error = harness.refuse();
    assert!(
        matches!(&error, CustodyExportErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        "expected StrictObjectCheck(zeroPaddedFilemode), observed {error:?}"
    );
}

/// Control 20b: a pre-indexed pack plus index is installed in place of §5 step 1, so step 4's
/// `fsck --strict` is the only strict-object guard for the same tree. Deleting only step 4 seals.
#[test]
fn control_20b_step_4_refuses_a_zero_padded_file_mode_when_step_1_is_bypassed() {
    let (harness, _tree) = zero_padded_harness();
    let _bypass = set_export_bypass_for_test(ExportBypassV1 {
        preindexed_pack: true,
        ..ExportBypassV1::default()
    });
    let _hook = install_export_hook_for_test(ExportHookPointV1::BeforeIndexPack, |work| {
        let pack = std::fs::read(work.join(PACK_FILE_NAME)).expect("the staged pack");
        let indexed = fixture_git(
            &work.join(VERIFY_GIT_DIR_NAME),
            &["index-pack", "--stdin"],
            Some(&pack),
        );
        assert!(
            indexed.success,
            "the non-strict pre-index succeeds: {}",
            String::from_utf8_lossy(&indexed.stderr)
        );
    });

    let error = harness.refuse();
    assert!(
        matches!(&error, CustodyExportErrorV1::StrictObjectCheck(id) if id == "zeroPaddedFilemode"),
        "expected StrictObjectCheck(zeroPaddedFilemode) from fsck, observed {error:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// §4.1 callbacks: controls 5a, 5b, 9a, 9b
// ---------------------------------------------------------------------------------------------

/// The persistent source mutations controls 5 and 9 apply at the first pre-spawn callback.
#[derive(Clone, Copy, Debug)]
enum SourceDriftV1 {
    /// 5: rewrite the source's alternates file to name an unbound copy of the pinned alternate,
    /// which holds every needed object.
    RewriteAlternatesToUnboundStore,
    /// 5: retarget the pinned alternate store: its name now resolves to an identical copy.
    RetargetPinnedAlternate,
    /// 9: swap the primary object store for an identical copy.
    SwapPrimaryObjectStore,
    /// 9: swap the source git directory for an identical copy.
    SwapSourceGitDirectory,
    /// 9: swap a non-bare source's worktree root for a new directory, moving its `.git` across
    /// intact, so the worktree root is the only source identity that changed.
    SwapSourceWorktreeRoot,
}

impl SourceDriftV1 {
    fn harness(self) -> HarnessV1 {
        match self {
            Self::RewriteAlternatesToUnboundStore | Self::RetargetPinnedAlternate => {
                HarnessV1::with_alternate()
            }
            Self::SwapPrimaryObjectStore | Self::SwapSourceGitDirectory => HarnessV1::new(),
            Self::SwapSourceWorktreeRoot => HarnessV1::with_source(|root| {
                SourceFixtureV1::build_non_bare(&root.join("repo"), &root.join("repo/.git"))
            }),
        }
    }

    fn apply(self, targets: &DriftTargetsV1) {
        match self {
            Self::RewriteAlternatesToUnboundStore => {
                let alternate = targets.alternate.as_ref().expect("an alternate fixture");
                let unbound = targets.root.join("unbound.git");
                copy_tree(alternate.parent().expect("alt.git"), &unbound);
                std::fs::write(
                    targets.objects.join("info/alternates"),
                    format!("{}\n", unbound.join("objects").display()),
                )
                .expect("the alternates rewrite");
            }
            Self::RetargetPinnedAlternate => {
                swap_for_identical_copy(targets.alternate.as_ref().expect("an alternate fixture"));
            }
            Self::SwapPrimaryObjectStore => swap_for_identical_copy(&targets.objects),
            Self::SwapSourceGitDirectory => swap_for_identical_copy(&targets.git_dir),
            Self::SwapSourceWorktreeRoot => swap_worktree_keeping_its_git_dir(&targets.repository),
        }
    }
}

/// The paths a drift row mutates, owned so the hook closure can outlive the harness borrow.
struct DriftTargetsV1 {
    root: PathBuf,
    repository: PathBuf,
    git_dir: PathBuf,
    objects: PathBuf,
    alternate: Option<PathBuf>,
}

impl DriftTargetsV1 {
    fn of(harness: &HarnessV1) -> Self {
        Self {
            root: harness.root.clone(),
            repository: harness.source.repository.clone(),
            git_dir: harness.source.git_dir.clone(),
            objects: harness.source.objects.clone(),
            alternate: harness.source.alternate.clone(),
        }
    }
}

/// Run one drift row with one callback isolated, and require typed identity drift with no pack
/// produced: the refusal lands before any source-reading child, so no store — bound or unbound —
/// contributed an object.
fn assert_callback_refuses_drift(drift: SourceDriftV1, bypass: ExportBypassV1) {
    let harness = drift.harness();
    let targets = DriftTargetsV1::of(&harness);
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::PreSpawnCallback,
        on_nth_call(1, move |_| drift.apply(&targets)),
    );
    let _bypass = set_export_bypass_for_test(bypass);

    let error = match harness.run() {
        Err(error) => error,
        wrong => panic!("{drift:?}: {}", describe_run(&wrong)),
    };
    assert!(
        !harness.seal_present(),
        "{drift:?}: a refused export left a seal"
    );
    assert!(
        matches!(error, CustodyExportErrorV1::IdentityDrift(_)),
        "{drift:?}: expected identity drift, observed {error:?}"
    );
    assert!(
        !harness.work().join(PACK_FILE_NAME).exists(),
        "{drift:?}: no pack may be produced from a drifted source"
    );
}

const PRE_SPAWN_ONLY_V1: ExportBypassV1 = ExportBypassV1 {
    pre_spawn_callback: false,
    post_exit_callback: true,
    scratch_emptiness: false,
    preindexed_pack: false,
    closure_step: false,
    plaintext_identity_comparison: false,
    seal_barrier: false,
};

const POST_EXIT_ONLY_V1: ExportBypassV1 = ExportBypassV1 {
    pre_spawn_callback: true,
    post_exit_callback: false,
    scratch_emptiness: false,
    preindexed_pack: false,
    closure_step: false,
    plaintext_identity_comparison: false,
    seal_barrier: false,
};

/// Control 5a: a pinned alternate is retargeted, or the alternates file is rewritten to name an
/// unbound store holding every needed object, before spawn. The post-exit callback is bypassed,
/// so the pre-spawn callback is the only guard.
#[test]
fn control_05a_pre_spawn_callback_refuses_alternate_drift() {
    for drift in [
        SourceDriftV1::RewriteAlternatesToUnboundStore,
        SourceDriftV1::RetargetPinnedAlternate,
    ] {
        assert_callback_refuses_drift(drift, PRE_SPAWN_ONLY_V1);
    }
}

/// Control 5b: the same persistent alternate drift lands after the pre-spawn check point, while
/// the child runs. The pre-spawn callback is bypassed, so the post-exit callback is the only
/// guard.
#[test]
fn control_05b_post_exit_callback_refuses_alternate_drift() {
    for drift in [
        SourceDriftV1::RewriteAlternatesToUnboundStore,
        SourceDriftV1::RetargetPinnedAlternate,
    ] {
        assert_callback_refuses_drift(drift, POST_EXIT_ONLY_V1);
    }
}

/// Control 9a: the source identity is swapped before spawn, with the post-exit callback bypassed.
/// The worktree-root row changes only the non-bare source's pinned repository root.
#[test]
fn control_09a_pre_spawn_callback_refuses_a_swapped_source() {
    for drift in [
        SourceDriftV1::SwapPrimaryObjectStore,
        SourceDriftV1::SwapSourceGitDirectory,
        SourceDriftV1::SwapSourceWorktreeRoot,
    ] {
        assert_callback_refuses_drift(drift, PRE_SPAWN_ONLY_V1);
    }
}

/// Control 9b: the source identity is swapped during the child, with the pre-spawn callback
/// bypassed.
#[test]
fn control_09b_post_exit_callback_refuses_a_swapped_source() {
    for drift in [
        SourceDriftV1::SwapPrimaryObjectStore,
        SourceDriftV1::SwapSourceGitDirectory,
        SourceDriftV1::SwapSourceWorktreeRoot,
    ] {
        assert_callback_refuses_drift(drift, POST_EXIT_ONLY_V1);
    }
}

/// Control 9, pre-write rows: every control-5 and control-9 drift lands after the capability is
/// minted and before the export starts, so the capability is already stale when the exporter is
/// entered. The scratch root lies outside the source, so the §2 preflight passes. The capability
/// recheck immediately before the first scratch write must refuse with typed identity drift and
/// leave the scratch root empty. Without it the exporter creates `capsule/` and `work/` on a
/// stale capability and refuses only at the first Git callback. Every row is evaluated, and
/// every failing row is reported.
#[test]
fn control_09_pre_write_recheck_refuses_a_source_drifted_before_the_export() {
    let mut failures = Vec::new();
    for drift in [
        SourceDriftV1::RewriteAlternatesToUnboundStore,
        SourceDriftV1::RetargetPinnedAlternate,
        SourceDriftV1::SwapPrimaryObjectStore,
        SourceDriftV1::SwapSourceGitDirectory,
        SourceDriftV1::SwapSourceWorktreeRoot,
    ] {
        let harness = drift.harness();
        let capability = harness.capability();
        drift.apply(&DriftTargetsV1::of(&harness));

        let result = harness.run_with(&FixtureSealerV1::honest(), capability);
        let written: Vec<String> = snapshot_entries(&harness.scratch).into_keys().collect();
        if !matches!(result, Err(CustodyExportErrorV1::IdentityDrift(_))) || !written.is_empty() {
            failures.push(format!(
                "{drift:?}: {}; scratch entries written: {written:?}",
                describe_run(&result)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "a capability stale at entry was not refused before the first scratch write:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// Control 10: destination components
// ---------------------------------------------------------------------------------------------

/// Control 10, row A: a symlink is planted at a reserved logical name before its rename. The
/// no-replace publication refuses it; the symlink's target is untouched.
#[test]
fn control_10a_no_replace_publication_refuses_a_symlink_at_the_reserved_name() {
    let harness = HarnessV1::new();
    let decoy = harness.root.join("decoy-file");
    std::fs::write(&decoy, b"decoy").expect("the decoy");
    let target = decoy.clone();
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::BeforeArtifactRename,
        once_where(
            |path| path.ends_with(PACK_ARTIFACT_V1),
            move |path| std::os::unix::fs::symlink(&target, path).expect("the planted symlink"),
        ),
    );

    let error = harness.refuse();
    assert!(
        matches!(
            error,
            CustodyExportErrorV1::Fs(FsCustodyError::TargetExists(_))
        ),
        "expected a no-replace publication refusal, observed {error:?}"
    );
    assert_eq!(std::fs::read(&decoy).expect("the decoy"), b"decoy");
}

/// Control 10, row B: after `control/` is populated, its name is retargeted to a symlink naming a
/// decoy directory. The artifacts still verify through their retained descriptors, so the seal
/// barrier's destination-identity recheck is the only guard against sealing a capsule whose
/// `control/` now resolves elsewhere.
#[test]
fn control_10b_seal_barrier_refuses_a_retargeted_capsule_directory() {
    let harness = HarnessV1::new();
    let decoy = harness.root.join("decoy-dir");
    std::fs::create_dir(&decoy).expect("the decoy directory");
    let capsule = harness.capsule();
    let decoy_target = decoy.clone();
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::AfterArtifactPublish,
        once_where(
            |path| path.to_string_lossy().contains("/payload/"),
            move |_| {
                let control = capsule.join("control");
                std::fs::rename(&control, capsule.join("control.moved"))
                    .expect("control/ moves away");
                std::os::unix::fs::symlink(&decoy_target, &control)
                    .expect("control/ now names the decoy");
            },
        ),
    );

    let error = harness.refuse();
    assert!(
        matches!(error, CustodyExportErrorV1::SealBarrier(_)),
        "expected a seal-barrier refusal, observed {error:?}"
    );
    assert!(snapshot_tree(&decoy).is_empty());
}

/// Control 10, row D: a component is pre-planted as a symlink to a decoy directory before the
/// exporter creates it. The create-new, no-follow directory creation refuses it, and nothing is
/// written through the symlink.
#[test]
fn control_10d_create_new_refuses_a_pre_planted_component_symlink() {
    let harness = HarnessV1::new();
    let decoy = harness.root.join("decoy-dir");
    std::fs::create_dir(&decoy).expect("the decoy directory");
    let capsule = harness.capsule();
    let decoy_target = decoy.clone();
    let _hook = install_export_hook_for_test(ExportHookPointV1::BeforeIndexPack, move |_| {
        std::os::unix::fs::symlink(&decoy_target, capsule.join("payload"))
            .expect("payload/ is pre-planted");
    });

    let error = harness.refuse();
    assert!(
        matches!(
            error,
            CustodyExportErrorV1::Fs(FsCustodyError::TargetExists(_))
        ),
        "expected a create-new refusal, observed {error:?}"
    );
    assert!(
        snapshot_tree(&decoy).is_empty(),
        "nothing may be written through the planted symlink"
    );
}

/// Control 10, row C: a case-fold alias of a reserved component. On a case-insensitive
/// filesystem the alias occupies `control/`, and the create-new refusal is the answer. On a
/// case-sensitive filesystem (this Linux lane) the alias is a different name: it must be neither
/// followed nor replaced, and `control/` must be the exporter's own directory. The refusal arm
/// runs on the macOS host lane.
#[test]
fn control_10c_a_case_fold_alias_is_refused_or_left_untouched() {
    let harness = HarnessV1::new();
    let probe = harness.root.join("CaseProbe");
    std::fs::write(&probe, b"").expect("the case probe");
    let case_insensitive = harness.root.join("caseprobe").exists();

    let decoy = harness.root.join("decoy-dir");
    std::fs::create_dir(&decoy).expect("the decoy directory");
    let capsule = harness.capsule();
    let decoy_target = decoy.clone();
    let _hook = install_export_hook_for_test(ExportHookPointV1::BeforeIndexPack, move |_| {
        std::os::unix::fs::symlink(&decoy_target, capsule.join("CONTROL"))
            .expect("the case-fold alias is planted");
    });

    if case_insensitive {
        let error = harness.refuse();
        assert!(
            matches!(
                error,
                CustodyExportErrorV1::Fs(FsCustodyError::TargetExists(_))
            ),
            "expected a create-new refusal for the alias, observed {error:?}"
        );
    } else {
        // A distinct name does not collide; the seal lands and the source is unchanged.
        let _ = harness.run_sealed();
        let control =
            std::fs::symlink_metadata(harness.capsule().join("control")).expect("control/ exists");
        assert!(
            control.is_dir(),
            "control/ must be the exporter's directory"
        );
        let alias = std::fs::symlink_metadata(harness.capsule().join("CONTROL"))
            .expect("the alias is untouched");
        assert!(alias.file_type().is_symlink());
    }
    assert!(snapshot_tree(&decoy).is_empty());
}

// ---------------------------------------------------------------------------------------------
// Control 11: ceilings, the scratch-wide ledger, Git-child reservations, and preflight ordering
// ---------------------------------------------------------------------------------------------

/// Control 11: every caller budget is admitted at its V1 ceiling and refused at ceiling + 1 (and at
/// zero), naming the field.
#[test]
fn control_11_budget_ceilings_admit_max_and_refuse_max_plus_one() {
    let ceilings = CustodyExportBudgetsV1::v1_ceilings();
    assert_eq!(
        ceilings.validate().expect("the ceilings are admitted"),
        ceilings
    );

    type SetterV1 = fn(&mut CustodyExportBudgetsV1, bool);
    let rows: [(&str, SetterV1); 6] = [
        ("artifact count", |budgets, over| {
            budgets.max_artifacts = MAX_ARTIFACTS_V1 + usize::from(over);
        }),
        ("chunk bytes", |budgets, over| {
            budgets.max_chunk_bytes = MAX_CHUNK_BYTES_V1 + u64::from(over);
        }),
        ("chunk count", |budgets, over| {
            budgets.max_chunks = MAX_CHUNKS_V1 + u32::from(over);
        }),
        ("per-artifact ciphertext", |budgets, over| {
            budgets.max_artifact_bytes = MAX_ARTIFACT_BYTES_V1 + u64::from(over);
        }),
        ("scratch-wide ledger", |budgets, over| {
            budgets.max_scratch_bytes = MAX_SCRATCH_BYTES_V1 + u64::from(over);
        }),
        ("canonical JSON metadata", |budgets, over| {
            budgets.max_canonical_json_bytes = MAX_CANONICAL_JSON_BYTES_V1 + usize::from(over);
        }),
    ];
    for (field, set) in rows {
        let mut at_max = ceilings;
        set(&mut at_max, false);
        assert!(at_max.validate().is_ok(), "{field}: max must be admitted");

        let mut over = ceilings;
        set(&mut over, true);
        assert!(
            matches!(over.validate(), Err(CustodyExportErrorV1::BudgetCeiling(named)) if named == field),
            "{field}: max + 1 must be refused naming the field"
        );
    }

    for field in [
        "artifact count",
        "chunk bytes",
        "chunk count",
        "per-artifact ciphertext",
        "scratch-wide ledger",
        "canonical JSON metadata",
    ] {
        let mut zero = ceilings;
        match field {
            "artifact count" => zero.max_artifacts = 0,
            "chunk bytes" => zero.max_chunk_bytes = 0,
            "chunk count" => zero.max_chunks = 0,
            "per-artifact ciphertext" => zero.max_artifact_bytes = 0,
            "scratch-wide ledger" => zero.max_scratch_bytes = 0,
            _ => zero.max_canonical_json_bytes = 0,
        }
        assert!(zero.validate().is_err(), "{field}: zero must be refused");
    }
}

/// Control 11: a planned plaintext is admitted at the per-artifact and chunk-count ceilings and
/// refused one byte over, before any staging file exists.
#[test]
fn control_11_plaintext_budget_admits_max_and_refuses_max_plus_one() {
    let budgets = CustodyExportBudgetsV1 {
        max_chunk_bytes: 8,
        max_chunks: 4,
        max_artifact_bytes: 1_000,
        ..CustodyExportBudgetsV1::v1_ceilings()
    };
    assert!(check_plaintext_budget(0, budgets).is_ok());
    assert!(check_plaintext_budget(32, budgets).is_ok(), "4 chunks of 8");
    assert!(
        matches!(
            check_plaintext_budget(33, budgets),
            Err(CustodyExportErrorV1::BudgetCeiling("chunk count"))
        ),
        "a fifth chunk must be refused"
    );

    let tight = CustodyExportBudgetsV1 {
        max_artifact_bytes: 32,
        max_chunks: 16,
        ..budgets
    };
    assert!(check_plaintext_budget(32, tight).is_ok());
    assert!(matches!(
        check_plaintext_budget(33, tight),
        Err(CustodyExportErrorV1::BudgetCeiling(
            "per-artifact ciphertext"
        ))
    ));
}

/// A destination sink over a scratch file, for the sink-level ceiling rows of control 11.
struct SinkRigV1 {
    _temp: tempfile::TempDir,
    file: File,
    path: PathBuf,
    ledger: RefCell<ScratchLedgerV1>,
    failure: RefCell<Option<CustodyExportErrorV1>>,
}

impl SinkRigV1 {
    fn new(ledger_limit: u64) -> Self {
        let temp = tempfile::TempDir::new().expect("a sink temp root");
        let path = temp.path().join("sink");
        let file = File::create(&path).expect("the sink file");
        Self {
            _temp: temp,
            file,
            path,
            ledger: RefCell::new(ScratchLedgerV1::new(ledger_limit)),
            failure: RefCell::new(None),
        }
    }

    /// Write `sizes` as consecutive chunks (the last one final) and report the first refusal.
    fn write(
        &mut self,
        budgets: CustodyExportBudgetsV1,
        sizes: &[usize],
    ) -> Result<(), CustodyExportErrorV1> {
        let limits = budgets
            .stream_limits()
            .expect("the budget has valid stream limits");
        let mut sink = CapsuleDestinationSinkV1 {
            file: &mut self.file,
            limits,
            validator: Some(CustodyEnvelopeSinkValidatorV1::new(limits)),
            ledger: &self.ledger,
            failure: &self.failure,
            artifact_name: b"control/sink.enc".to_vec(),
            written: 0,
            chunks: 0,
            max_artifact_bytes: budgets.max_artifact_bytes,
        };
        for (index, size) in sizes.iter().enumerate() {
            let ordinal = u32::try_from(index).expect("a small ordinal");
            let chunk =
                CustodyEnvelopeChunkV1::new(ordinal, vec![0x5A; *size], index + 1 == sizes.len())
                    .expect("a fixture chunk");
            if let Err(error) = sink.write_chunk(chunk) {
                return Err(self
                    .failure
                    .borrow_mut()
                    .take()
                    .unwrap_or(CustodyExportErrorV1::Capsule(error)));
            }
        }
        Ok(())
    }

    fn file_length(&self) -> u64 {
        std::fs::metadata(&self.path).expect("the sink file").len()
    }
}

/// Control 11: the destination sink admits a chunk of exactly the budget's chunk bytes and refuses
/// one byte more BEFORE writing it.
#[test]
fn control_11_sink_chunk_bytes_admit_max_and_refuse_max_plus_one_before_writing() {
    let budgets = CustodyExportBudgetsV1 {
        max_chunk_bytes: 8,
        ..CustodyExportBudgetsV1::v1_ceilings()
    };
    let mut at_max = SinkRigV1::new(u64::MAX);
    at_max
        .write(budgets, &[8])
        .expect("an 8-byte chunk is admitted");
    assert_eq!(at_max.file_length(), 8);

    let mut over = SinkRigV1::new(u64::MAX);
    let error = over
        .write(budgets, &[9])
        .expect_err("a 9-byte chunk is refused");
    assert!(
        matches!(error, CustodyExportErrorV1::BudgetCeiling("chunk bytes")),
        "observed {error:?}"
    );
    assert_eq!(over.file_length(), 0, "the refused chunk was never written");
    assert_eq!(
        over.ledger.borrow().used(),
        0,
        "the refused chunk was never charged"
    );
}

/// Control 11: the destination sink admits exactly the budget's chunk count and refuses one more
/// BEFORE writing it.
#[test]
fn control_11_sink_chunk_count_admits_max_and_refuses_max_plus_one_before_writing() {
    let budgets = CustodyExportBudgetsV1 {
        max_chunk_bytes: 8,
        max_chunks: 2,
        ..CustodyExportBudgetsV1::v1_ceilings()
    };
    let mut at_max = SinkRigV1::new(u64::MAX);
    at_max
        .write(budgets, &[8, 8])
        .expect("two chunks are admitted");
    assert_eq!(at_max.file_length(), 16);

    let mut over = SinkRigV1::new(u64::MAX);
    let error = over
        .write(budgets, &[8, 4, 4])
        .expect_err("a third chunk is refused");
    assert!(
        matches!(error, CustodyExportErrorV1::BudgetCeiling("chunk count")),
        "observed {error:?}"
    );
    assert_eq!(over.file_length(), 12, "the third chunk was never written");
}

/// Control 11: the destination sink admits exactly the per-artifact ciphertext ceiling and refuses
/// one byte more BEFORE writing it.
#[test]
fn control_11_sink_per_artifact_ciphertext_admits_max_and_refuses_max_plus_one_before_writing() {
    let budgets = CustodyExportBudgetsV1 {
        max_chunk_bytes: 8,
        max_chunks: 16,
        max_artifact_bytes: 12,
        ..CustodyExportBudgetsV1::v1_ceilings()
    };
    let mut at_max = SinkRigV1::new(u64::MAX);
    at_max
        .write(budgets, &[8, 4])
        .expect("12 bytes are admitted");
    assert_eq!(at_max.file_length(), 12);

    let mut over = SinkRigV1::new(u64::MAX);
    let error = over
        .write(budgets, &[8, 5])
        .expect_err("13 bytes are refused");
    assert!(
        matches!(&error, CustodyExportErrorV1::ScratchLedger(detail) if detail.contains("per-artifact")),
        "observed {error:?}"
    );
    assert_eq!(
        over.file_length(),
        8,
        "the over-ceiling chunk was never written"
    );
}

/// Control 11: an `objects/info/alternates` file is read through a bound one byte past its
/// ceiling. A file of exactly the ceiling is admitted; one byte more is refused, and is never
/// buffered past the bound.
#[test]
fn control_11_alternates_file_admits_max_and_refuses_max_plus_one() {
    let temp = tempfile::TempDir::new().expect("a store root");
    for (length, admitted) in [
        (MAX_ALTERNATES_FILE_BYTES_V1, true),
        (MAX_ALTERNATES_FILE_BYTES_V1 + 1, false),
    ] {
        let store = temp.path().join(format!("store-{length}"));
        std::fs::create_dir_all(store.join("info")).expect("the store info dir");
        std::fs::write(store.join("info/alternates"), vec![b'#'; length]).expect("alternates");
        let pin = PinnedDirectoryV1::open(&store, "alternates bound").expect("the store pin");
        let result = read_alternates_digest(&pin);
        if admitted {
            assert!(
                matches!(&result, Ok(Some((_, bytes))) if bytes.len() == length),
                "max must be admitted: {result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(CustodyExportErrorV1::SourceState(_))),
                "max + 1 must be refused: {result:?}"
            );
        }
    }
}

/// Control 11: the scratch-wide ledger admits exactly its limit, refuses one byte more without
/// charging it, refuses an overflowing reservation, and reconciles a released allowance.
#[test]
fn control_11_scratch_ledger_admits_max_and_refuses_max_plus_one() {
    let mut exact = ScratchLedgerV1::new(100);
    exact.reserve(100).expect("the limit is admitted");
    assert_eq!(exact.used(), 100);

    let mut over = ScratchLedgerV1::new(100);
    assert!(matches!(
        over.reserve(101),
        Err(CustodyExportErrorV1::ScratchLedger(_))
    ));
    over.reserve(60).expect("60 bytes");
    assert!(over.reserve(41).is_err(), "60 + 41 is the limit + 1");
    assert_eq!(over.used(), 60, "a refused reservation charges nothing");
    over.reserve(40).expect("60 + 40 is the limit");

    let mut entries = ScratchLedgerV1::new(2 * ENTRY_ALLOWANCE_BYTES_V1);
    entries.reserve_entries(2).expect("two entries fit");
    let mut entries_over = ScratchLedgerV1::new(2 * ENTRY_ALLOWANCE_BYTES_V1 - 1);
    assert!(entries_over.reserve_entries(2).is_err());

    let mut overflow = ScratchLedgerV1::new(u64::MAX);
    overflow.reserve(1).expect("one byte");
    assert!(overflow.reserve(u64::MAX).is_err(), "overflow is refused");
    assert!(overflow.reserve_entries(u64::MAX).is_err());

    let mut reconciled = ScratchLedgerV1::new(100);
    reconciled.reserve(100).expect("the allowance");
    reconciled.release(70);
    assert_eq!(reconciled.used(), 30);
}

/// Control 11: the enumerated Git-child reservations, each a logical-byte bound plus, apart from
/// it, entries at the allowance. `init --bare` is nine entries plus 4 KiB for `HEAD` and
/// `config`; `index-pack` is three entries plus the pack once and the exact version-2 index and
/// reverse-index bounds. Each is admitted by a ledger with exactly that headroom and refused by one
/// byte less.
#[test]
fn control_11_git_child_reservations_are_exact_and_bounded() {
    assert_eq!(INIT_BARE_ENTRIES_V1, 9);
    assert_eq!(INIT_BARE_LOGICAL_BYTES_V1, 4_096);
    assert_eq!(INDEX_PACK_ENTRIES_V1, 3);
    let init = 9 * 65_536 + 4_096;

    // N = 6 objects, raw SHA-1 width H = 20, pack length L = 315:
    // index 1072 + 6·28 + 8·6 + 40 = 1328; reverse 12 + 4·6 + 40 = 76.
    let logical = index_pack_logical_bound(315, 6, 20).expect("the bound");
    assert_eq!(logical, 315 + 1_328 + 76);
    // SHA-256 width H = 32: index 1072 + 6·40 + 48 + 64 = 1424; reverse 12 + 24 + 64 = 100.
    assert_eq!(
        index_pack_logical_bound(315, 6, 32).expect("the bound"),
        315 + 1_424 + 100
    );
    assert!(index_pack_logical_bound(u64::MAX, 6, 20).is_err());
    assert!(index_pack_logical_bound(0, u64::MAX, 20).is_err());

    for (label, bound) in [("init --bare", init), ("index-pack", logical + 3 * 65_536)] {
        let mut exact = ScratchLedgerV1::new(bound);
        assert!(exact.reserve(bound).is_ok(), "{label}: max");
        let mut short = ScratchLedgerV1::new(bound - 1);
        assert!(short.reserve(bound).is_err(), "{label}: max + 1");
    }
}

/// Control 11: the post-exit re-measure admits a git directory at exactly its reservation and
/// refuses one byte more, or any file outside the enumerated set.
#[test]
fn control_11_post_exit_remeasure_admits_max_and_refuses_max_plus_one() {
    let expected = BTreeSet::from(["HEAD".to_owned(), "config".to_owned()]);
    let at = |bytes: u64, files: &[&str]| GitDirectoryMeasurementV1 {
        files: files.iter().map(|name| (*name).to_owned()).collect(),
        logical_bytes: bytes,
    };
    assert!(verify_git_writes(&at(100, &["HEAD", "config"]), &expected, 100, "g").is_ok());
    assert!(matches!(
        verify_git_writes(&at(101, &["HEAD", "config"]), &expected, 100, "g"),
        Err(CustodyExportErrorV1::ScratchLedger(_))
    ));
    assert!(matches!(
        verify_git_writes(&at(10, &["HEAD", "config", "stray"]), &expected, 100, "g"),
        Err(CustodyExportErrorV1::UnexpectedGitFile(file)) if file == "g/stray"
    ));
}

/// Control 11: a read-only verification child is re-measured too. A file that appears in
/// `verify.git` during `verify-pack` is refused at that child's re-measure — before §5 step 2
/// begins, which the armed step-2 fault proves: reaching it would report the fault instead.
#[test]
fn control_11_a_read_only_child_is_re_measured_before_the_next_step() {
    let harness = HarnessV1::new();
    // Post-exit callbacks run in child order: init, init, cat-file, pack-objects, index-pack,
    // then verify-pack sixth.
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::PostExitCallback,
        on_nth_call(6, |work| {
            let info = work.join(VERIFY_GIT_DIR_NAME).join("objects/info");
            std::fs::create_dir_all(&info).expect("objects/info");
            std::fs::write(info.join("stray"), b"stray").expect("the stray file");
        }),
    );
    let _fault = arm_export_fault_for_test(
        ExportFaultPointV1::InventoryComparison,
        ExportFaultPositionV1::Before,
        1,
    );

    let error = harness.refuse();
    assert!(
        matches!(&error, CustodyExportErrorV1::UnexpectedGitFile(file) if file.ends_with("objects/info/stray")),
        "expected the verify-pack re-measure to refuse, observed {error:?}"
    );
}

/// An independent census of everything below a scratch root, sharing no code with the ledger:
/// `(entries, logical bytes)`, counting every file and directory (the root itself excluded) and
/// the length of every regular file.
fn scratch_census(root: &Path) -> (u64, u64) {
    let (mut entries, mut bytes) = (0_u64, 0_u64);
    let mut frontier = vec![root.to_path_buf()];
    while let Some(directory) = frontier.pop() {
        for entry in std::fs::read_dir(&directory).expect("a census directory") {
            let path = entry.expect("a census entry").path();
            let metadata = std::fs::symlink_metadata(&path).expect("census metadata");
            entries += 1;
            if metadata.is_dir() {
                frontier.push(path);
            } else {
                bytes += metadata.len();
            }
        }
    }
    (entries, bytes)
}

/// §3's scratch use by census: every logical file byte plus the per-entry allowance for every
/// created file and directory.
fn census_use(root: &Path) -> u64 {
    let (entries, bytes) = scratch_census(root);
    bytes + entries * ENTRY_ALLOWANCE_BYTES_V1
}

/// Control 11: the scratch-wide ledger end to end, against an independent filesystem census. The
/// reported ledger use U must equal every created file's logical length plus the per-entry
/// allowance for every created file and directory, including the `HEAD` and `config` that both
/// `git init` runs write. The exact-U arm must seal a capsule whose census fits its U-byte budget,
/// and the U − 1 arm, one byte under the census, must refuse. Every arm is evaluated, and every
/// failing arm is reported.
#[test]
fn control_11_scratch_budget_end_to_end_admits_max_and_refuses_max_plus_one() {
    let probe = HarnessV1::new();
    let used = probe.run_sealed().evidence.scratch_bytes_used;
    let (entries, bytes) = scratch_census(&probe.scratch);
    let census = census_use(&probe.scratch);
    let mut failures = Vec::new();
    if used != census {
        failures.push(format!(
            "census: the ledger reports {used} bytes, but the scratch root holds {bytes} logical \
             bytes in {entries} entries, {census} bytes by §3"
        ));
    }

    let mut exact = HarnessV1::new();
    exact.budgets.max_scratch_bytes = used;
    let result = exact.run();
    let footprint = census_use(&exact.scratch);
    if !matches!(result, Ok(CustodyExportOutcomeV1::Sealed(_))) || footprint > used {
        failures.push(format!(
            "exact-U arm (budget {used}): {}; census footprint {footprint}",
            describe_run(&result)
        ));
    }

    let mut short = HarnessV1::new();
    short.budgets.max_scratch_bytes = census - 1;
    let result = short.run();
    if !matches!(result, Err(CustodyExportErrorV1::ScratchLedger(_))) || short.seal_present() {
        failures.push(format!(
            "U - 1 arm (budget {}, one byte under the census): {}",
            census - 1,
            describe_run(&result)
        ));
    }
    assert!(
        failures.is_empty(),
        "the ledger does not match the scratch census:\n{}",
        failures.join("\n")
    );
}

/// Control 11: a git directory is held to its logical-byte bound, not to its entry allowances.
/// After the source `git init` exits, its `config` grows by 8 KiB of comment lines: past the 4 KiB
/// logical bound for `HEAD` and `config`, but far inside the nine 64 KiB entry allowances. The
/// post-exit re-measure refuses it, so an allowance can never absorb file bytes the ledger did not
/// charge.
#[test]
fn control_11_a_git_directory_is_held_to_its_logical_bound_not_its_entry_allowances() {
    // Fixture validity: 8 KiB lies between the logical bound and the entry allowances.
    const {
        assert!(INIT_BARE_LOGICAL_BYTES_V1 < 8 * 1024);
        assert!(8 * 1024 < INIT_BARE_ENTRIES_V1 * ENTRY_ALLOWANCE_BYTES_V1);
    }
    let harness = HarnessV1::new();
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::PostExitCallback,
        on_nth_call(1, |work| {
            let mut config = std::fs::OpenOptions::new()
                .append(true)
                .open(work.join(SOURCE_GIT_DIR_NAME).join("config"))
                .expect("the synthesized source config");
            let mut line = vec![b'#'; 63];
            line.push(b'\n');
            for _ in 0..128 {
                config.write_all(&line).expect("an 8 KiB config comment");
            }
        }),
    );

    let error = match harness.run() {
        Err(error) => error,
        wrong => panic!("{}", describe_run(&wrong)),
    };
    assert!(
        matches!(&error, CustodyExportErrorV1::ScratchLedger(detail) if detail.contains(SOURCE_GIT_DIR_NAME)),
        "expected a logical-bound refusal for {SOURCE_GIT_DIR_NAME}, observed {error:?}"
    );
    assert!(!harness.seal_present());
}

/// Control 11: the derived layout's artifact count is admitted at the budget and refused one
/// artifact over, before any write.
#[test]
fn control_11_artifact_count_admits_max_and_refuses_max_plus_one() {
    let mut exact = HarnessV1::new();
    exact.budgets.max_artifacts = 6;
    let _ = exact.run_sealed();

    let mut short = HarnessV1::new();
    short.budgets.max_artifacts = 5;
    let error = short.refuse();
    assert!(
        matches!(error, CustodyExportErrorV1::BudgetCeiling("artifact count")),
        "observed {error:?}"
    );
    assert!(short.scratch_is_empty(), "the refusal precedes every write");
}

/// Control 11: the canonical-manifest preflight admits an encoding of exactly the budget and
/// refuses one byte less. Over the limit, `CustodyCapsuleLayoutV1::derive` — which allocates the
/// full encoding — is never entered, and nothing is written.
#[test]
fn control_11_canonical_manifest_preflight_precedes_derive() {
    let harness = HarnessV1::new();
    let length = harness
        .manifest
        .encode_canonical()
        .expect("the canonical encoding")
        .len();
    assert!(preflight_canonical_manifest(&harness.manifest, length).is_ok());
    assert!(matches!(
        preflight_canonical_manifest(&harness.manifest, length - 1),
        Err(CustodyExportErrorV1::CanonicalManifestTooLarge)
    ));

    let mut over = HarnessV1::new();
    over.budgets.max_canonical_json_bytes = length - 1;
    let error = over.refuse();
    assert!(
        matches!(error, CustodyExportErrorV1::CanonicalManifestTooLarge),
        "observed {error:?}"
    );
    assert_eq!(derive_calls_for_test(), 0, "derive must not be entered");
    assert!(over.scratch_is_empty());

    let mut at = HarnessV1::new();
    at.budgets.max_canonical_json_bytes = length;
    let _ = at.run();
    assert_eq!(
        derive_calls_for_test(),
        1,
        "at the limit, derive is entered"
    );
}

// ---------------------------------------------------------------------------------------------
// Controls 12, 13, 14, 14b: the §6 commit point
// ---------------------------------------------------------------------------------------------

/// Control 12: every pre-commit fault point, before and after, returns a typed incomplete outcome
/// with no seal. Chunk boundaries are sampled at the first, second, and final ciphertext chunk of
/// one multi-chunk control artifact and of the Git pack.
#[test]
fn control_12_every_pre_commit_fault_leaves_no_seal() {
    let mut exercised = 0;
    for point in ExportFaultPointV1::ALL {
        for position in [ExportFaultPositionV1::Before, ExportFaultPositionV1::After] {
            if !point.is_pre_commit(position) {
                continue;
            }
            let harness = HarnessV1::new();
            let _fault = arm_export_fault_for_test(point, position, 1);
            let error = harness.run().expect_err("a pre-commit fault must refuse");
            assert!(
                matches!(
                    error,
                    CustodyExportErrorV1::InjectedFault { point: p, position: q } if p == point && q == position
                ),
                "{point:?}/{position:?}: observed {error:?}"
            );
            assert!(
                !harness.seal_present(),
                "{point:?}/{position:?} left a seal"
            );
            exercised += 1;
        }
    }
    assert_eq!(
        exercised, 31,
        "17 boundaries × 2 sides − 3 post-commit sides"
    );

    for artifact in [MANIFEST_ARTIFACT_V1, PACK_ARTIFACT_V1] {
        for selector in [
            ExportChunkSelectorV1::Ordinal(0),
            ExportChunkSelectorV1::Ordinal(1),
            ExportChunkSelectorV1::Final,
        ] {
            let harness = HarnessV1::new();
            let _fault = arm_export_chunk_fault_for_test(artifact.as_bytes(), selector);
            let error = harness.run().expect_err("a chunk fault must refuse");
            assert!(
                matches!(error, CustodyExportErrorV1::InjectedChunkFault { .. }),
                "{artifact} {selector:?}: observed {error:?}"
            );
            assert!(
                !harness.seal_present(),
                "{artifact} {selector:?} left a seal"
            );
        }
    }
}

fn expect_durability_unconfirmed(outcome: CustodyExportOutcomeV1) {
    match outcome {
        CustodyExportOutcomeV1::PublishedDurabilityUnconfirmed { seal_name, .. } => {
            assert_eq!(seal_name, SEAL_NAME);
        }
        other => panic!("expected PublishedDurabilityUnconfirmed, observed {other:?}"),
    }
}

/// Control 13: a fault after the seal rename — on its `After` side, or on either side of the
/// post-seal directory sync — is `PublishedDurabilityUnconfirmed` naming the seal, with the seal
/// present. It is never an incomplete outcome and never success. The `ParentSyncAmbiguous`
/// publication outcome maps to the same arm.
#[test]
fn control_13_a_post_seal_fault_is_published_durability_unconfirmed() {
    for (point, position) in [
        (ExportFaultPointV1::SealRename, ExportFaultPositionV1::After),
        (
            ExportFaultPointV1::PostSealDirectorySync,
            ExportFaultPositionV1::Before,
        ),
        (
            ExportFaultPointV1::PostSealDirectorySync,
            ExportFaultPositionV1::After,
        ),
    ] {
        let harness = HarnessV1::new();
        let _fault = arm_export_fault_for_test(point, position, 1);
        let outcome = harness
            .run()
            .unwrap_or_else(|error| panic!("{point:?}/{position:?} must not be Err: {error:?}"));
        expect_durability_unconfirmed(outcome);
        assert!(
            harness.seal_present(),
            "{point:?}/{position:?}: the seal exists"
        );
    }

    let harness = HarnessV1::new();
    let _override =
        override_seal_publication_for_test(SealPublicationOverrideV1::ParentSyncAmbiguous);
    expect_durability_unconfirmed(harness.run().expect("an outcome, not Err"));
}

/// Control 14: `fs_custody`'s existing `UnlinkSourceOnly` rename fault on the seal rename — the
/// staged seal is gone and the target is not provably the seal — is `SealPublicationUnverified`,
/// claiming neither a published seal nor an absent one.
#[test]
fn control_14_an_unverifiable_seal_rename_is_seal_publication_unverified() {
    let harness = HarnessV1::new();
    // The seal rename is the first publication rename on the `capsule/` pin: every artifact is
    // published through its own `control/`, `git/`, or `payload/` pin.
    let _fault = arm_capsule_rename_fault_for_test(1, PublicationRenameFaultV1::UnlinkSourceOnly);
    match harness.run().expect("an outcome, not Err") {
        CustodyExportOutcomeV1::SealPublicationUnverified { seal_name, .. } => {
            assert_eq!(seal_name, SEAL_NAME);
        }
        other => panic!("expected SealPublicationUnverified, observed {other:?}"),
    }
    assert!(!harness.capsule().join(".a2a-staging-seal").exists());
}

/// Control 14b: the seal rename lands, but the target cannot be re-opened for identity
/// (`TargetIdentityUnverified`). The outcome is typed and names the seal; it is never success and
/// never "no seal".
#[test]
fn control_14b_an_unverified_seal_target_is_seal_publication_target_unverified() {
    let harness = HarnessV1::new();
    let _override =
        override_seal_publication_for_test(SealPublicationOverrideV1::TargetIdentityUnverified);
    match harness.run().expect("an outcome, not Err") {
        CustodyExportOutcomeV1::SealPublicationTargetUnverified { seal_name, .. } => {
            assert_eq!(seal_name, SEAL_NAME);
        }
        other => panic!("expected SealPublicationTargetUnverified, observed {other:?}"),
    }
    assert!(harness.seal_present(), "the rename really landed");
}

// ---------------------------------------------------------------------------------------------
// Controls 15, 16, 19, 32, 33: receipts, plaintext, and remeasurement
// ---------------------------------------------------------------------------------------------

/// Control 15: the fixture sealer mints each receipt from the previous call's context, so two
/// differently hashed artifacts receive each other's receipts. Whole-receipt equality refuses
/// before any seal.
#[test]
fn control_15_whole_receipt_equality_refuses_swapped_receipt_contexts() {
    let harness = HarnessV1::new();
    let sealer = FixtureSealerV1::faulted(
        SealerFaultV1 {
            swap_receipt_contexts: true,
            ..SealerFaultV1::default()
        },
        None,
    );
    let error = harness
        .run_with(&sealer, harness.capability())
        .expect_err("swapped receipts must refuse");
    assert!(
        matches!(error, CustodyExportErrorV1::ReceiptMismatch),
        "observed {error:?}"
    );
    assert!(!harness.seal_present());
}

/// Control 15: each `ReceiptFieldsV1` field perturbed alone makes the view unequal — including a
/// ciphertext length changed with the SHA-256 unchanged, which no genuine receipt can carry. A
/// comparator that ignores any one field turns exactly that field's row red.
#[test]
fn control_15_every_receipt_field_is_compared() {
    let context = CustodyEnvelopeContextV1::new(
        LosslessPathV1::from_bytes(MANIFEST_ARTIFACT_V1.as_bytes().to_vec()),
        Sha256HexV1::digest(b"manifest"),
        fixture_envelope_format_v1(),
        recipients(),
    )
    .expect("the receipt context");
    let limits = CustodyEnvelopeStreamLimitsV1::new(64, 64, 1).expect("limits");
    let mut sink = CustodyEnvelopeSinkValidatorV1::new(limits);
    sink.accept_chunk(
        &CustodyEnvelopeChunkV1::new(0, b"ciphertext".to_vec(), true).expect("chunk"),
    )
    .expect("accepted");
    let receipt = CustodyEnvelopeSealReceiptV1::new(&context, sink.finish().expect("finish"))
        .expect("receipt");
    let base = ReceiptFieldsV1::from_receipt(&receipt);
    assert!(receipt_fields_equal(&base, &base.clone()));

    type PerturbV1 = fn(&mut ReceiptFieldsV1);
    let rows: [(&str, PerturbV1); 8] = [
        ("artifact_name", |fields| fields.artifact_name.push(b'x')),
        ("manifest_digest", |fields| {
            fields.manifest_digest = Sha256HexV1::digest(b"other").as_str().to_owned();
        }),
        ("capsule_format", |fields| fields.capsule_format.push('x')),
        ("sealing_tool", |fields| fields.sealing_tool.push('x')),
        ("sealing_tool_version", |fields| {
            fields.sealing_tool_version.push('x');
        }),
        ("recipients", |fields| {
            fields.recipients.push("another".to_owned())
        }),
        ("ciphertext_length", |fields| fields.ciphertext_length += 1),
        ("ciphertext_sha256", |fields| {
            fields.ciphertext_sha256 = Sha256HexV1::digest(b"other").as_str().to_owned();
        }),
    ];
    for (field, perturb) in rows {
        let mut perturbed = base.clone();
        perturb(&mut perturbed);
        assert_ne!(perturbed, base, "{field}: the fixture perturbs the field");
        assert!(
            !receipt_fields_equal(&base, &perturbed),
            "receipt field row {field}: a perturbed {field} must make the receipts unequal"
        );
    }
}

/// The three plaintext roles control 16 covers: the Git pack, one control artifact, and one
/// non-Git payload.
const PLAINTEXT_ROLES_V1: [&str; 3] = ["objects.pack", "manifest.json", "worktree.bin"];

/// Control 16a: for every plaintext role, a sealer that stops after its first chunk fails the
/// exporter-owned exact-total wrapper's `finish`, before any exterior seal.
#[test]
fn control_16a_a_prefix_only_sealer_fails_the_exact_total_wrapper() {
    for marker in PLAINTEXT_ROLES_V1 {
        let harness = HarnessV1::new();
        let sealer = FixtureSealerV1::faulted(
            SealerFaultV1 {
                stop_after_chunks: Some(1),
                ..SealerFaultV1::default()
            },
            Some(marker.as_bytes()),
        );
        match harness.run_with(&sealer, harness.capability()) {
            Err(CustodyExportErrorV1::PlaintextNotConsumed(name)) if name.contains(marker) => {}
            other => panic!("{marker} stop-after-first-chunk: observed {other:?}"),
        }
        assert!(!harness.seal_present(), "{marker}: no seal");
    }
}

/// Control 16b: for every plaintext role, a sealer that reads substituted bytes of exactly the
/// planned length passes `finish` but fails the plaintext identity comparison, before any
/// exterior seal.
#[test]
fn control_16b_substituted_plaintext_fails_the_identity_comparison() {
    for marker in PLAINTEXT_ROLES_V1 {
        let harness = HarnessV1::new();
        let _substitute = substitute_plaintext_for_test(marker.as_bytes());
        match harness.run() {
            Err(CustodyExportErrorV1::PlaintextIdentity(name)) if name.contains(marker) => {}
            other => panic!("{marker} substituted plaintext: observed {other:?}"),
        }
        assert!(!harness.seal_present(), "{marker}: no seal");
    }
}

/// Control 19: two captured non-Git streams of equal length exchange payloads between roles. The
/// downstream plaintext identity comparison is bypassed, so the capability's stream binding is
/// the only guard.
#[test]
fn control_19_the_capability_refuses_a_stream_replayed_under_another_role() {
    let mut harness = HarnessV1::new();
    let (index, worktree) = harness.streams.split_at_mut(1);
    CustodyCapturedStreamV1::exchange_payloads_for_test(&mut index[0], &mut worktree[0]);
    let _bypass = set_export_bypass_for_test(ExportBypassV1 {
        plaintext_identity_comparison: true,
        ..ExportBypassV1::default()
    });

    let error = harness.refuse();
    assert!(
        matches!(
            error,
            CustodyExportErrorV1::CapabilityBinding(
                "a captured stream does not have its declared SHA-256"
            )
        ),
        "observed {error:?}"
    );
}

/// Control 32: after the sink finishes and the staging file is synced, the staging file is
/// overwritten in place with same-length bytes. The seal barrier is bypassed, so the content
/// remeasurement is the only guard: no rename, no seal.
#[test]
fn control_32_content_remeasurement_refuses_an_in_place_staging_overwrite() {
    let harness = HarnessV1::new();
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::AfterStagingSync,
        once_where(|_| true, invert_in_place),
    );
    let _bypass = set_export_bypass_for_test(ExportBypassV1 {
        seal_barrier: true,
        ..ExportBypassV1::default()
    });

    let error = harness.refuse();
    let CustodyExportErrorV1::ContentRemeasurement(name) = &error else {
        panic!("expected a content-remeasurement refusal, observed {error:?}");
    };
    assert!(
        !harness.capsule().join(name).exists(),
        "the overwritten staging file must not be renamed to {name}"
    );
}

/// Control 33: after an artifact is published and before the seal barrier, it is overwritten in
/// place with same-length bytes. The seal barrier's re-hash refuses: no seal.
#[test]
fn control_33_seal_barrier_refuses_an_in_place_overwrite_of_a_published_artifact() {
    let harness = HarnessV1::new();
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::AfterArtifactPublish,
        once_where(|_| true, invert_in_place),
    );

    let error = harness.refuse();
    assert!(
        matches!(error, CustodyExportErrorV1::SealBarrier(_)),
        "observed {error:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Controls 18 and 21: preflight binding and disjointness
// ---------------------------------------------------------------------------------------------

/// Control 18: a capability whose unit, run, materialization, or generation differs from the
/// manifest's, whose inventory differs, or a mixed-format manifest, is refused before any write.
/// Each identity row's capability is otherwise self-consistent (its streams carry its own
/// generation), so the binding comparison is the only guard.
#[test]
fn control_18_capability_binding_refuses_before_any_write() {
    let identity_rows: [(&str, usize); 4] = [
        ("unit id", 0),
        ("run id", 1),
        ("materialization id", 2),
        ("generation id", 3),
    ];
    for (field, slot) in identity_rows {
        let mut harness = HarnessV1::new();
        let mut identity = IDENTITY_V1;
        let other = format!("{}-other", IDENTITY_V1[slot]);
        identity[slot] = &other;
        let foreign = manifest_for(identity, harness.source.full_inventory());
        harness.streams = default_streams(identity[3]);
        let capability = harness.capability_for(&foreign);

        match harness.run_with(&FixtureSealerV1::honest(), capability) {
            Err(CustodyExportErrorV1::CapabilityBinding(named)) if named == field => {}
            other => panic!("binding row {field}: observed {other:?}"),
        }
        assert!(
            harness.scratch_is_empty(),
            "{field}: the refusal precedes every write"
        );
    }

    let harness = HarnessV1::new();
    let mut capability = harness.capability();
    let orphan = harness.source.orphan_blob.clone();
    capability.set_inventory_for_test(inventory_tuples(&harness.source.inventory_without(&orphan)));
    match harness.run_with(&FixtureSealerV1::honest(), capability) {
        Err(CustodyExportErrorV1::CapabilityBinding("object inventory")) => {}
        other => panic!("binding row object inventory: observed {other:?}"),
    }
    assert!(harness.scratch_is_empty());

    let mut harness = HarnessV1::new();
    let mut objects = harness.source.full_inventory();
    objects.push(
        CustodyOriginalObjectV1::new(
            CustodyGitObjectFormatV1::Sha256,
            "b".repeat(64),
            CustodyGitObjectKindV1::Blob,
        )
        .expect("a SHA-256 object id"),
    );
    harness.manifest = manifest_with(objects);
    match harness.run() {
        Err(CustodyExportErrorV1::MixedObjectFormat) => {}
        other => panic!("binding row mixed formats: observed {other:?}"),
    }
    assert!(harness.scratch_is_empty());
}

/// One control-21 relation: where the source lives, and which directory is the scratch root.
#[derive(Clone, Copy, Debug)]
enum OverlapV1 {
    ScratchIsSourceGitDir,
    ScratchInsideSourceGitDir,
    SourceGitDirInsideScratch,
    ScratchIsAlternateStore,
    ScratchInsideAlternateStore,
    AlternateStoreInsideScratch,
    /// A normal non-bare clone, with the scratch root a sibling of `.git` inside the worktree.
    ScratchInsideSourceWorktree,
    /// A `--separate-git-dir` clone, whose worktree holds no git directory to trip over.
    ScratchIsSourceWorktree,
    /// The same layout, with the scratch root containing the worktree but not the git directory.
    SourceWorktreeInsideScratch,
}

impl OverlapV1 {
    fn harness(self) -> HarnessV1 {
        let mut harness = match self {
            Self::ScratchIsSourceGitDir | Self::ScratchInsideSourceGitDir => HarnessV1::new(),
            Self::SourceGitDirInsideScratch => {
                HarnessV1::with_source(|root| SourceFixtureV1::build(&root.join("outer")))
            }
            Self::ScratchIsAlternateStore | Self::ScratchInsideAlternateStore => {
                HarnessV1::with_alternate()
            }
            Self::AlternateStoreInsideScratch => HarnessV1::with_source(|root| {
                SourceFixtureV1::build_with_alternate_under(root, &root.join("outer"))
            }),
            Self::ScratchInsideSourceWorktree => HarnessV1::with_source(|root| {
                SourceFixtureV1::build_non_bare(&root.join("repo"), &root.join("repo/.git"))
            }),
            Self::ScratchIsSourceWorktree => HarnessV1::with_source(|root| {
                SourceFixtureV1::build_non_bare(&root.join("wt"), &root.join("wt.git"))
            }),
            Self::SourceWorktreeInsideScratch => HarnessV1::with_source(|root| {
                SourceFixtureV1::build_non_bare(&root.join("outer/wt"), &root.join("wt.git"))
            }),
        };
        let alternate = harness.source.alternate.clone();
        harness.scratch = match self {
            Self::ScratchIsSourceGitDir => harness.source.git_dir.clone(),
            Self::ScratchInsideSourceGitDir => harness.source.git_dir.join("inner"),
            Self::SourceGitDirInsideScratch
            | Self::AlternateStoreInsideScratch
            | Self::SourceWorktreeInsideScratch => harness.root.join("outer"),
            Self::ScratchIsAlternateStore => alternate.expect("an alternate"),
            Self::ScratchInsideAlternateStore => alternate.expect("an alternate").join("inner"),
            Self::ScratchInsideSourceWorktree => harness.source.repository.join("custody"),
            Self::ScratchIsSourceWorktree => harness.source.repository.clone(),
        };
        if !harness.scratch.exists() {
            std::fs::create_dir(&harness.scratch).expect("the overlapping scratch root");
            set_owner_private(&harness.scratch);
        }
        harness
    }
}

/// Control 21: the scratch root equals, lies inside, or contains the source git directory, a
/// pinned alternate store, or a non-bare source's worktree. The disjointness preflight is the only
/// guard: each row is refused before any write, and neither the scratch root nor any watched
/// source path changes. The emptiness and owner checks are bypassed only where the overlapping
/// scratch root already holds source bytes; a row whose scratch root is a fresh, empty,
/// owner-private directory, such as `repo/custody` beside `repo/.git`, runs the production
/// preflight unaided. Every row is evaluated, and every failing row is reported.
#[test]
fn control_21_disjointness_preflight_refuses_every_overlap_before_any_write() {
    let mut failures = Vec::new();
    for overlap in [
        OverlapV1::ScratchIsSourceGitDir,
        OverlapV1::ScratchInsideSourceGitDir,
        OverlapV1::SourceGitDirInsideScratch,
        OverlapV1::ScratchIsAlternateStore,
        OverlapV1::ScratchInsideAlternateStore,
        OverlapV1::AlternateStoreInsideScratch,
        OverlapV1::ScratchInsideSourceWorktree,
        OverlapV1::ScratchIsSourceWorktree,
        OverlapV1::SourceWorktreeInsideScratch,
    ] {
        let harness = overlap.harness();
        let _bypass = (!harness.scratch_is_empty()).then(|| {
            set_export_bypass_for_test(ExportBypassV1 {
                scratch_emptiness: true,
                ..ExportBypassV1::default()
            })
        });
        let stores = harness.source.snapshot();
        let scratch = snapshot_tree(&harness.scratch);

        let result = harness.run();
        let after = harness.source.snapshot();
        let created: Vec<&String> = after
            .keys()
            .filter(|key| !stores.contains_key(*key))
            .collect();
        let refused = matches!(result, Err(CustodyExportErrorV1::ScratchPreflight(_)));
        if !refused || after != stores || snapshot_tree(&harness.scratch) != scratch {
            failures.push(format!(
                "{overlap:?}: {}; {} source paths created{}",
                describe_run(&result),
                created.len(),
                created
                    .first()
                    .map_or(String::new(), |first| format!(", first {first}"))
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "overlapping scratch roots were not refused before any write:\n{}",
        failures.join("\n")
    );
}

/// Control 21, stale-pin row: the capability is minted for a `--separate-git-dir` worktree, which
/// is then renamed, and a replacement directory is created at its old path. The scratch root is
/// a fresh, empty, owner-private `custody/` inside the renamed worktree. The pinned path now
/// names the replacement, but the retained descriptor still names the worktree the scratch root
/// lies inside. The §2 preflight must compare against that retained identity and refuse before
/// any write. The renamed worktree and the git directory stay unchanged, byte for byte and entry
/// for entry.
#[test]
fn control_21_a_renamed_worktree_is_refused_by_its_retained_identity_before_any_write() {
    let mut harness = HarnessV1::with_source(|root| {
        SourceFixtureV1::build_non_bare(&root.join("wt"), &root.join("wt.git"))
    });
    let capability = harness.capability();
    let renamed = harness.root.join("wt.renamed");
    std::fs::rename(&harness.source.repository, &renamed)
        .expect("the worktree is renamed after minting");
    std::fs::create_dir(&harness.source.repository).expect("a replacement at the old path");
    harness.scratch = renamed.join("custody");
    std::fs::create_dir(&harness.scratch).expect("a scratch root inside the renamed worktree");
    set_owner_private(&harness.scratch);
    let worktree = snapshot_entries(&renamed);
    let git_dir = snapshot_entries(&harness.source.git_dir);

    let result = harness.run_with(&FixtureSealerV1::honest(), capability);

    let after = snapshot_entries(&renamed);
    let created: Vec<&String> = after
        .keys()
        .filter(|key| !worktree.contains_key(*key))
        .collect();
    assert!(
        matches!(result, Err(CustodyExportErrorV1::ScratchPreflight(_))) && after == worktree,
        "a scratch root inside the renamed, descriptor-pinned worktree was not refused by the \
         preflight before any write: {}; worktree entries created: {created:?}",
        describe_run(&result)
    );
    assert_eq!(
        snapshot_entries(&harness.source.git_dir),
        git_dir,
        "the export changed the source git directory"
    );
}

// ---------------------------------------------------------------------------------------------
// Controls 30 and 31: the synthesized source git dir and the route pin
// ---------------------------------------------------------------------------------------------

/// Control 30's fixture: a promisor remote holds one manifest object the source store lacks, and
/// the source's own config declares the remote as its partial-clone promisor.
fn promisor_harness(template: &Path) -> (HarnessV1, String, PathBuf) {
    let mut harness = HarnessV1::with_source(|root| {
        let arm = root.join("arm");
        copy_tree(template, &arm);
        let git_dir = arm.join("src.git");
        let objects = FixtureObjectsV1 {
            blob_a: std::fs::read_to_string(arm.join("ids/blob_a")).expect("id"),
            blob_b: std::fs::read_to_string(arm.join("ids/blob_b")).expect("id"),
            tree: std::fs::read_to_string(arm.join("ids/tree")).expect("id"),
            parent_commit: std::fs::read_to_string(arm.join("ids/parent_commit")).expect("id"),
            child_commit: std::fs::read_to_string(arm.join("ids/child_commit")).expect("id"),
            orphan_blob: std::fs::read_to_string(arm.join("ids/orphan_blob")).expect("id"),
        };
        let promisor = arm.join("prom.git");
        for (key, value) in [
            ("core.repositoryformatversion", "1".to_owned()),
            ("extensions.partialClone", "p".to_owned()),
            ("remote.p.url", format!("file://{}", promisor.display())),
            ("remote.p.promisor", "true".to_owned()),
        ] {
            let _ = fixture_git_ok(&git_dir, &["config", key, &value], None);
        }
        SourceFixtureV1::from_objects(git_dir.clone(), None, vec![git_dir, promisor], objects)
    });
    let lazy = std::fs::read_to_string(harness.root.join("arm/ids/lazy")).expect("the lazy id");
    let mut objects = harness.source.full_inventory();
    objects.extend(objects_from(&[(&lazy, CustodyGitObjectKindV1::Blob)]));
    harness.manifest = manifest_with(objects);
    let promisor = harness.root.join("arm/prom.git");
    (harness, lazy, promisor)
}

/// The immutable control-30 template: the standard source plus a promisor repository holding the
/// one lazily fetchable object. Each arm copies it.
fn promisor_template(root: &Path) -> PathBuf {
    let template = root.join("promisor-template");
    let git_dir = template.join("src.git");
    fixture_init_bare(&git_dir);
    let objects = write_fixture_objects(&git_dir);
    let promisor = template.join("prom.git");
    fixture_init_bare(&promisor);
    let lazy = fixture_git_ok(
        &promisor,
        &["hash-object", "-w", "--stdin"],
        Some(b"lazy-only"),
    );
    let _ = fixture_git_ok(
        &promisor,
        &["config", "uploadpack.allowAnySHA1InWant", "true"],
        None,
    );
    let ids = template.join("ids");
    std::fs::create_dir(&ids).expect("the id directory");
    for (name, id) in [
        ("blob_a", &objects.blob_a),
        ("blob_b", &objects.blob_b),
        ("tree", &objects.tree),
        ("parent_commit", &objects.parent_commit),
        ("child_commit", &objects.child_commit),
        ("orphan_blob", &objects.orphan_blob),
        ("lazy", &lazy),
    ] {
        std::fs::write(ids.join(name), id).expect("an id");
    }
    template
}

/// Control 30: the source repository carries promisor config naming a `file://` remote that holds
/// one manifest object the source store lacks. With all three lazy-fetch guards disabled through
/// 2B2a's bypass seam, the synthesized `work/source-git/` — which receives no source config — is
/// the only guard: the object is reported missing, nothing is fetched, and the source is
/// unchanged. Every arm uses a fresh source store copied from an immutable template.
#[test]
fn control_30_the_synthesized_git_dir_receives_no_source_config() {
    let template_root = tempfile::TempDir::new().expect("the template root");
    let template = promisor_template(template_root.path());
    let template_before = snapshot_tree(&template);

    for (arm, guards) in [
        (
            "lazy-fetch guards disabled",
            GitGuardBypassV1 {
                no_lazy_fetch_flag: true,
                no_lazy_fetch_environment: true,
                protocol_allow: true,
                ..GitGuardBypassV1::default()
            },
        ),
        ("lazy-fetch guards enabled", GitGuardBypassV1::default()),
    ] {
        let (harness, lazy, promisor) = promisor_harness(&template);
        let _guards = set_git_guard_bypass_for_test(guards);
        let source_before = harness.source.snapshot();
        let promisor_before = snapshot_tree(&promisor);

        let error = harness.refuse();
        assert!(
            matches!(&error, CustodyExportErrorV1::ObjectPresence(detail) if detail.contains(&lazy)),
            "{arm}: expected {lazy} reported missing, observed {error:?}"
        );
        assert_eq!(
            harness.source.snapshot(),
            source_before,
            "{arm}: the source changed"
        );
        assert_eq!(
            snapshot_tree(&promisor),
            promisor_before,
            "{arm}: the promisor changed"
        );
        assert!(
            !harness.work().join(PACK_FILE_NAME).exists(),
            "{arm}: no pack may be produced"
        );
    }
    assert_eq!(
        snapshot_tree(&template),
        template_before,
        "the template is immutable"
    );
}

/// Control 31's marker-writing Git route: a root-anchored fixture script that records every
/// invocation, then execs the lane Git.
fn marker_route(
    root: &Path,
    pin: Option<[u8; 32]>,
) -> (GitRouteRequestV1, PathBuf, SealedRouteAnchorV1) {
    let anchor = root.join("route-anchor");
    std::fs::create_dir(&anchor).expect("the route anchor");
    set_owner_private(&anchor);
    let marker = root.join("route-marker");
    let script = anchor.join("git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho invoked >> '{}'\nexec '{}' \"$@\"\n",
            marker.display(),
            lane_git_path().display()
        ),
    )
    .expect("the route script");
    let sealed = SealedRouteAnchorV1::seal(&script, &anchor);
    // The seal's warm-up exec recorded one invocation; only the export's own may remain.
    std::fs::remove_file(&marker).expect("the warm-up marker");
    let digest = pin.unwrap_or_else(|| sha256_of_file(&script));
    let route = GitRouteRequestV1::for_test_fixture(
        script,
        ExpectedGitDigestV1::from_bytes(digest),
        anchor,
    )
    .expect("the fixture route is absolute");
    (route, marker, sealed)
}

/// Control 31: two otherwise identical exports through a marker-writing Git route, one with a
/// matching pin and one pinned to a different binary's digest. The caller's route request reaches
/// 2B2a admission unchanged: the mismatch refuses with `DigestMismatch` before `version` or any
/// marker, and the match seals through the same route.
#[test]
fn control_31_the_caller_route_pin_reaches_admission_unchanged() {
    let mut matching = HarnessV1::new();
    let (route, marker, _sealed) = marker_route(&matching.root, None);
    matching.git_route = route;
    let _ = matching.run_sealed();
    assert!(marker.exists(), "the matching route was executed");

    let mut mismatching = HarnessV1::new();
    let (route, marker, _sealed) =
        marker_route(&mismatching.root, Some(sha256_of_file(&lane_git_path())));
    mismatching.git_route = route;
    let error = mismatching.refuse();
    assert!(
        matches!(
            error,
            CustodyExportErrorV1::Git(CustodyGitError::DigestMismatch { .. })
        ),
        "observed {error:?}"
    );
    assert!(
        !marker.exists(),
        "a mismatched route must never be executed"
    );
}

// ---------------------------------------------------------------------------------------------
// Controls 34 and 35: the single pack run and its pre-reserved allowance
// ---------------------------------------------------------------------------------------------

/// Control 34: `pack-objects` is spawned exactly once, and the spawn counter proves it
/// independently of control 17's byte identity.
#[test]
fn control_34_pack_objects_is_spawned_exactly_once() {
    let harness = HarnessV1::new();
    let _ = harness.run_sealed();
    assert_eq!(pack_objects_spawns_for_test(), 1);
}

/// Control 35: with remaining pack-output allowance B equal to the pack length, the pack is
/// produced whole; with B one byte short, the runner refuses with `StdoutLimit` before byte B + 1
/// is written. The allowance is exactly the ledger headroom the exporter reserved.
#[test]
fn control_35_the_pack_output_allowance_bounds_the_pack_stream() {
    let probe = HarnessV1::new();
    let sealed = probe.run_sealed();
    let pack_length = sealed.evidence.verified_pack_length;
    let (used_before, _) = last_pack_allowance_for_test();

    // B = L: the pack streams whole, and the export then refuses at the next reservation.
    let mut exact = HarnessV1::new();
    exact.budgets.max_scratch_bytes = used_before + pack_length;
    let error = exact.refuse();
    assert!(
        matches!(error, CustodyExportErrorV1::ScratchLedger(_)),
        "with B = L the pack must stream whole: observed {error:?}"
    );
    assert_eq!(last_pack_allowance_for_test(), (used_before, pack_length));
    assert_eq!(
        std::fs::metadata(exact.work().join(PACK_FILE_NAME))
            .expect("the staged pack")
            .len(),
        pack_length
    );

    // B = L − 1: byte B + 1 is refused before it is written.
    let mut short = HarnessV1::new();
    short.budgets.max_scratch_bytes = used_before + pack_length - 1;
    let error = short.refuse();
    let expected_limit = usize::try_from(pack_length - 1).expect("a small pack");
    assert!(
        matches!(
            error,
            CustodyExportErrorV1::Git(CustodyGitError::StdoutLimit { limit }) if limit == expected_limit
        ),
        "with B = L - 1: observed {error:?}"
    );
    let written = std::fs::metadata(short.work().join(PACK_FILE_NAME))
        .expect("the staged pack")
        .len();
    assert!(written < pack_length, "byte B + 1 was written: {written}");
}

// ---------------------------------------------------------------------------------------------
// The `verify-pack -v` output bound (control 11)
// ---------------------------------------------------------------------------------------------

/// Roughly two thousand related blobs: enough that a SHA-256 pack's verbose `verify-pack` rows
/// outgrow a 128-byte-per-object estimate, while the manifest stays far below 1 MiB.
const DELTA_BLOBS_V1: usize = 2_000;

/// The `verify-pack -v` bound for `objects` objects of hex width `hex_width`, computed here from
/// Git's row formats rather than taken from the exporter: a widest delta row (`2·W + 83` bytes), a
/// widest chain-length histogram line (56 bytes) per object, and the 16 KiB floor.
fn independent_verify_pack_bound(objects: usize, hex_width: usize) -> usize {
    objects * (2 * hex_width + 83 + 56) + 16 * 1024
}

/// Control 11: the `verify-pack -v` bound is format-aware and checked. Its row and histogram
/// widths are the widths of Git's own formats at their widest values; a SHA-256 row is 48 bytes
/// wider than a SHA-1 row; and an unrepresentable bound is refused, never saturated.
#[test]
fn control_11_verify_pack_bound_is_format_aware_and_checked() {
    let name = "f".repeat(64);
    let row = format!(
        "{name} {:<6} {} {} {} {} {name}\n",
        "commit",
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u32::MAX
    );
    assert_eq!(row.len(), 2 * 64 + VERIFY_PACK_ROW_BYTES_V1);
    let histogram = format!("chain length = {}: {} objects\n", i32::MAX, u64::MAX);
    assert_eq!(histogram.len(), VERIFY_PACK_HISTOGRAM_BYTES_V1);

    let sha1 = verify_pack_stdout_limit(DELTA_BLOBS_V1 + 6, GitObjectFormatV1::Sha1);
    let sha256 = verify_pack_stdout_limit(DELTA_BLOBS_V1 + 6, GitObjectFormatV1::Sha256);
    assert_eq!(
        sha1.expect("SHA-1"),
        independent_verify_pack_bound(2_006, 40)
    );
    assert_eq!(
        sha256.expect("SHA-256"),
        independent_verify_pack_bound(2_006, 64)
    );
    assert!(matches!(
        verify_pack_stdout_limit(usize::MAX, GitObjectFormatV1::Sha1),
        Err(CustodyExportErrorV1::Io(_))
    ));
}

/// Export a delta-heavy source in `format`; it must seal. Returns the bytes `verify-pack -v`
/// printed and the object count.
fn seal_a_delta_heavy_pack(format: CustodyGitObjectFormatV1) -> (usize, usize) {
    let mut harness = HarnessV1::with_source(|root| {
        SourceFixtureV1::build_delta_heavy(root, format, DELTA_BLOBS_V1)
    });
    harness.budgets.max_chunk_bytes = MAX_CHUNK_BYTES_V1;
    let sealed = harness.run_sealed();
    let objects = harness.manifest.original_objects().len();
    assert_eq!(sealed.evidence.object_format, format);

    // Fixture validity: the verified pack really is delta-heavy.
    let verify = harness.work().join(VERIFY_GIT_DIR_NAME);
    let index = verify.join(format!(
        "objects/pack/pack-{}.idx",
        sealed.evidence.pack_hash
    ));
    let listing = fixture_git_ok(
        &verify,
        &[
            "verify-pack",
            "-v",
            index.to_str().expect("utf-8 index path"),
        ],
        None,
    );
    let deltas = listing
        .lines()
        .filter(|line| line.split_whitespace().count() == 7)
        .count();
    assert!(
        deltas * 10 >= objects * 9,
        "only {deltas} of {objects} objects are deltas"
    );

    let printed = sealed
        .evidence
        .runs
        .iter()
        .find(|run| run.label == "verify-pack")
        .expect("verify-pack ran")
        .stdout
        .length;
    (printed, objects)
}

/// Positive: a delta-heavy SHA-1 pack seals, and its verbose `verify-pack` output fits the
/// format-aware bound.
#[test]
fn a_delta_heavy_sha1_pack_seals_within_the_verify_pack_bound() {
    let (printed, objects) = seal_a_delta_heavy_pack(CustodyGitObjectFormatV1::Sha1);
    assert!(printed <= independent_verify_pack_bound(objects, 40));
}

/// Positive: a delta-heavy SHA-256 pack seals. Each delta row carries two 64-character object
/// names, so the output outgrows the shared 128-byte-per-object estimate; the format-aware bound
/// admits it.
#[test]
fn a_delta_heavy_sha256_pack_seals_within_the_verify_pack_bound() {
    let (printed, objects) = seal_a_delta_heavy_pack(CustodyGitObjectFormatV1::Sha256);
    assert!(
        printed > objects * 128 + 16 * 1024,
        "fixture validity: {printed} bytes must exceed the 128-byte-per-object estimate"
    );
    assert!(printed <= independent_verify_pack_bound(objects, 64));
}

/// A fixture Git route whose `verify-pack` prints exactly `length` bytes, ending with an
/// integrity line, and which runs the lane Git for every other command.
fn verify_pack_output_route(
    root: &Path,
    length: usize,
) -> (GitRouteRequestV1, SealedRouteAnchorV1) {
    let anchor = root.join("verify-pack-anchor");
    std::fs::create_dir(&anchor).expect("the route anchor");
    set_owner_private(&anchor);
    let ok_line = b"verify.git/objects/pack/pack-edge.pack: ok\n";
    let mut output = Vec::with_capacity(length);
    while output.len() + ok_line.len() < length {
        let width = (length - ok_line.len() - output.len()).min(128);
        output.resize(output.len() + width - 1, b'x');
        output.push(b'\n');
    }
    output.extend_from_slice(ok_line);
    assert_eq!(
        output.len(),
        length,
        "the prepared output has the exact length"
    );
    let printed = anchor.join("verify-pack.out");
    std::fs::write(&printed, &output).expect("the prepared verify-pack output");
    let script = anchor.join("git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$*\" in *\" verify-pack \"*) exec /bin/cat '{}';; esac\n\
             exec '{}' \"$@\"\n",
            printed.display(),
            lane_git_path().display()
        ),
    )
    .expect("the route script");
    let sealed = SealedRouteAnchorV1::seal(&script, &anchor);
    let digest = ExpectedGitDigestV1::from_bytes(sha256_of_file(&script));
    let route = GitRouteRequestV1::for_test_fixture(script, digest, anchor)
        .expect("the fixture route is absolute");
    (route, sealed)
}

/// Control 11: `verify-pack -v` output of exactly the format-aware bound is admitted and seals;
/// one byte more is refused with `StdoutLimit` at exactly that bound. The bound is computed here,
/// not taken from the exporter. Both arms are evaluated, and every failing arm is reported.
#[test]
fn control_11_verify_pack_output_admits_its_exact_bound_and_refuses_one_byte_more() {
    let bound = independent_verify_pack_bound(6, 40);
    let mut failures = Vec::new();
    for (arm, length) in [("exact bound", bound), ("bound + 1", bound + 1)] {
        let mut harness = HarnessV1::new();
        assert_eq!(harness.manifest.original_objects().len(), 6);
        let (route, _sealed) = verify_pack_output_route(&harness.root, length);
        harness.git_route = route;
        let result = harness.run();
        let held = if length == bound {
            matches!(&result, Ok(CustodyExportOutcomeV1::Sealed(sealed)) if sealed
                .evidence
                .runs
                .iter()
                .any(|run| run.label == "verify-pack" && run.stdout.length == bound))
        } else {
            matches!(&result, Err(CustodyExportErrorV1::Git(CustodyGitError::StdoutLimit { limit }))
                if *limit == bound)
                && !harness.seal_present()
        };
        if !held {
            failures.push(format!("{arm} ({length} bytes): {}", describe_run(&result)));
        }
    }
    assert!(
        failures.is_empty(),
        "the verify-pack output bound is not exactly {bound} bytes:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// 2B2b2b2: plan-backed capabilities. `planned_binding_*` are §6.1, `planned_census_*` §6.2,
// `planned_stage_*` and `planned_retention_*` §6.3, and `planned_e2e_*` §6.4. Every source is a
// real clone, planned by 2B2b2b1 with the lane Git and exported through the capability minted
// from that plan.
// ---------------------------------------------------------------------------------------------

/// Bytes only `target/` holds, so a payload that carried any of them is visible.
const TARGET_MARKER_V1: &[u8] = b"cargo build output that no coverage payload may carry";
/// The walk budgets the plan-backed fixtures plan with.
const PLAN_ENTRY_BUDGET_V1: u64 = 10_000;

fn plan_frame_budget() -> CustodyFrameBudgetV1 {
    CustodyFrameBudgetV1::new(1 << 20, 1 << 30).expect("the fixture frame budget")
}

/// A Git command in `directory`, isolated from the host's configuration, for the plan-backed
/// fixtures' worktree operations (clone, commit, stash, merge), which `fixture_git`'s `GIT_DIR`
/// form does not drive.
fn worktree_git_output(directory: &Path, args: &[&str]) -> std::process::Output {
    Command::new(lane_git_path())
        .current_dir(directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=custody",
            "-c",
            "user.email=custody@example.invalid",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("the fixture Git child runs")
}

fn worktree_git(directory: &Path, args: &[&str]) {
    let output = worktree_git_output(directory, args);
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A real source, planned by 2B2b2b1, with an owner-private export scratch beside it.
pub(crate) struct PlanFixtureV1 {
    _temp: tempfile::TempDir,
    area: PathBuf,
    /// The repository root: the worktree, or a bare source's git directory.
    worktree: PathBuf,
    git_dir: PathBuf,
    scratch: PathBuf,
    budgets: CustodyExportBudgetsV1,
    git_route: GitRouteRequestV1,
}

impl PlanFixtureV1 {
    /// `build` makes the source beneath the canonical area and returns `(worktree, git_dir)`.
    fn with(build: impl FnOnce(&Path) -> (PathBuf, PathBuf)) -> Self {
        let temp = tempfile::TempDir::new().expect("a fixture temp root");
        let area = temp
            .path()
            .canonicalize()
            .expect("a canonical fixture root");
        let (worktree, git_dir) = build(&area);
        let scratch = area.join("scratch");
        std::fs::create_dir(&scratch).expect("the export scratch root");
        set_owner_private(&scratch);
        Self {
            _temp: temp,
            area,
            worktree,
            git_dir,
            scratch,
            budgets: CustodyExportBudgetsV1::v1_ceilings(),
            git_route: lane_git_route(),
        }
    }

    /// A clone of a one-commit repository: a `README`, refs, reflogs, a config, and an index.
    fn readme_clone() -> Self {
        Self::with(readme_clone_in)
    }

    /// [`Self::readme_clone`] under the `cargo-target-v1` precondition, with a populated root
    /// `target/`.
    fn cargo_clone() -> Self {
        let fixture = Self::readme_clone();
        fixture.write_cargo();
        fixture
    }

    /// §6.4's rich non-bare clone: a dirty worktree, untracked and ignored files, a stash,
    /// reflogs, an in-progress merge with rerere, a hook, bridge evidence, and a Cargo `target/`.
    pub(crate) fn rich_clone() -> Self {
        let fixture = Self::with(|area| {
            let origin = area.join("origin");
            std::fs::create_dir(&origin).expect("the origin worktree");
            let write = |name: &str, content: &[u8]| {
                std::fs::write(origin.join(name), content).expect("an origin file");
            };
            worktree_git(&origin, &["init", "-q"]);
            write("README", b"coverage fixture\n");
            worktree_git(&origin, &["add", "README"]);
            worktree_git(&origin, &["commit", "-q", "-m", "init"]);
            write("f", b"a\nb\nc\n");
            worktree_git(&origin, &["add", "f"]);
            worktree_git(&origin, &["commit", "-q", "-m", "f"]);
            worktree_git(&origin, &["checkout", "-q", "-b", "side"]);
            write("f", b"a\nSIDE\nc\n");
            worktree_git(&origin, &["commit", "-q", "-am", "side"]);
            worktree_git(&origin, &["checkout", "-q", "main"]);
            write("f", b"a\nMAIN\nc\n");
            worktree_git(&origin, &["commit", "-q", "-am", "main"]);
            worktree_git(area, &["clone", "-q", "origin", "clone"]);
            (area.join("clone"), area.join("clone/.git"))
        });
        let git = |args: &[&str]| worktree_git(&fixture.worktree, args);
        git(&["config", "rerere.enabled", "true"]);
        // A stash, then a local branch for the merge.
        fixture.write("README", b"coverage fixture\nstashed\n");
        git(&["stash", "-q"]);
        git(&["branch", "-q", "side", "origin/side"]);
        // Untracked and ignored files, and a worktree file named `HEAD`.
        fixture.write("untracked.txt", b"untracked\n");
        fixture.write(".gitignore", b"*.log\n");
        fixture.write("build.log", b"ignored\n");
        fixture.write("HEAD", b"a worktree file named HEAD\n");
        // An in-progress merge with rerere, then a dirty worktree.
        let _ = worktree_git_output(&fixture.worktree, &["merge", "side"]);
        assert!(fixture.git_dir.join("MERGE_RR").is_file());
        assert!(fixture.git_dir.join("rr-cache").is_dir());
        fixture.write("README", b"coverage fixture\ndirty\n");
        // A hook and bridge evidence.
        let hook = fixture.git_dir.join("hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().expect("hooks/")).expect("hooks/");
        std::fs::write(&hook, b"#!/bin/sh\nexit 0\n").expect("the hook");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
                .expect("the hook is executable");
        }
        std::fs::create_dir_all(fixture.git_dir.join("a2a-bridge")).expect("a2a-bridge/");
        std::fs::write(fixture.git_dir.join("a2a-bridge/evidence.json"), b"{}\n")
            .expect("bridge evidence");
        std::fs::write(fixture.git_dir.join("A2A_TASK.md"), b"# task\n").expect("a task file");
        fixture.write_cargo();
        fixture
    }

    /// A bare source whose git directory holds only an empty `objects/`: every walked class is
    /// `empty`, so its plan has no receipts.
    fn empty_source() -> Self {
        Self::with(|area| {
            let git_dir = area.join("empty.git");
            std::fs::create_dir_all(git_dir.join("objects")).expect("the empty store");
            (git_dir.clone(), git_dir)
        })
    }

    /// A worktree and a git directory apart from it, so neither root domain's walk observes the
    /// other. With `populated_git_dir`, a bare-initialized git directory beside an empty worktree;
    /// without it, a git directory holding only an empty `objects/` beside a worktree `README`.
    fn separate_git_dir(populated_git_dir: bool) -> Self {
        Self::with(|area| {
            let worktree = area.join("worktree");
            let git_dir = area.join("repo.git");
            std::fs::create_dir(&worktree).expect("the worktree");
            if populated_git_dir {
                fixture_init_bare(&git_dir);
            } else {
                std::fs::create_dir_all(git_dir.join("objects")).expect("the empty store");
                std::fs::write(
                    worktree.join("README"),
                    b"a worktree apart from its git dir\n",
                )
                .expect("the worktree README");
            }
            (worktree, git_dir)
        })
    }

    /// Writes one loose blob into the source's store, copied from a donor bare repository beside
    /// it, and returns it as a manifest object.
    fn add_loose_blob(&self) -> CustodyOriginalObjectV1 {
        let donor = self.area.join("donor.git");
        fixture_init_bare(&donor);
        let object_id = fixture_git_ok(
            &donor,
            &["hash-object", "-w", "--stdin"],
            Some(b"a planned loose blob\n"),
        );
        let (fan_out, rest) = object_id.split_at(2);
        std::fs::create_dir_all(self.objects().join(fan_out)).expect("the fan-out directory");
        std::fs::copy(
            donor.join("objects").join(fan_out).join(rest),
            self.objects().join(fan_out).join(rest),
        )
        .expect("the loose blob");
        CustodyOriginalObjectV1::new(
            CustodyGitObjectFormatV1::Sha1,
            object_id,
            CustodyGitObjectKindV1::Blob,
        )
        .expect("a blob id")
    }

    fn write(&self, relative: &str, content: &[u8]) {
        let path = self.worktree.join(relative);
        std::fs::create_dir_all(path.parent().expect("a worktree parent"))
            .expect("a worktree directory");
        std::fs::write(path, content).expect("a worktree file");
    }

    fn write_cargo(&self) {
        self.write("Cargo.toml", b"[package]\nname = \"fixture\"\n");
        self.write("Cargo.lock", b"version = 4\n");
        self.write("target/debug/build.bin", TARGET_MARKER_V1);
    }

    fn objects(&self) -> PathBuf {
        self.git_dir.join("objects")
    }

    fn work(&self) -> PathBuf {
        self.scratch.join(WORK_DIR_NAME)
    }

    fn capsule(&self) -> PathBuf {
        self.scratch.join(CAPSULE_DIR_NAME)
    }

    /// The published capsule directory, for the 2B3a capsule reader's controls.
    pub(crate) fn capsule_dir(&self) -> PathBuf {
        self.scratch.join(CAPSULE_DIR_NAME)
    }

    /// Every object the source's store holds, as the manifest's object inventory.
    pub(crate) fn inventory(&self) -> Vec<CustodyOriginalObjectV1> {
        fixture_git_ok(
            &self.git_dir,
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
            let kind = match kind {
                "commit" => CustodyGitObjectKindV1::Commit,
                "tree" => CustodyGitObjectKindV1::Tree,
                "blob" => CustodyGitObjectKindV1::Blob,
                "tag" => CustodyGitObjectKindV1::Tag,
                other => panic!("an unexpected object type {other}"),
            };
            CustodyOriginalObjectV1::new(CustodyGitObjectFormatV1::Sha1, object_id, kind)
                .expect("a listed object id")
        })
        .collect()
    }

    /// Plans the source for `generation`, with `inventory` as the object inventory, into a fresh
    /// planning scratch beside it, and keeps the pinned sources for the capability.
    fn plan_with(
        &self,
        generation: &str,
        inventory: &[CustodyOriginalObjectV1],
        adjust: impl FnOnce(&mut CustodyCoverageRequestV1),
    ) -> (CustodyCoveragePlanV1, CustodyCoverageSourcesV1) {
        let planning = tempfile::TempDir::new_in(&self.area).expect("a planning scratch");
        set_owner_private(planning.path());
        let mut request = CustodyCoverageRequestV1 {
            sources: CustodyCoverageSourcesV1::pin(&self.worktree, &self.git_dir, &self.objects())
                .expect("pin the source"),
            generation_id: generation.to_owned(),
            frame_budget: plan_frame_budget(),
            entry_budget: PLAN_ENTRY_BUDGET_V1,
            planning_budget: CustodyPlanningBudgetV1 {
                max_scratch_bytes: 64 * 1024 * 1024,
            },
            scratch_root: planning.path().to_path_buf(),
            external_evidence: NoExternalEvidenceRecordedV1::declare().into(),
            object_format: CustodyGitObjectFormatV1::Sha1,
            object_inventory: inventory.to_vec(),
            git_route: self.git_route.clone(),
            deadline: Instant::now() + Duration::from_secs(120),
        };
        adjust(&mut request);
        let plan = plan_coverage_v1(&request).expect("plan the source");
        (plan, request.sources)
    }

    pub(crate) fn plan(
        &self,
        generation: &str,
        inventory: &[CustodyOriginalObjectV1],
    ) -> (CustodyCoveragePlanV1, CustodyCoverageSourcesV1) {
        self.plan_with(generation, inventory, |_| {})
    }

    /// The ordinary path: every object planned for [`GENERATION_V1`], the manifest built from
    /// the plan's three collections, and the capability minted from both.
    pub(crate) fn planned(
        &self,
    ) -> (
        CustodyManifestV1,
        CustodyCaptureCapabilityV1,
        Vec<CustodyClassReceiptV1>,
    ) {
        let inventory = self.inventory();
        let (plan, sources) = self.plan(GENERATION_V1, &inventory);
        let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
        let receipts = plan.receipts().to_vec();
        let capability = planned_capability(&manifest, plan, sources);
        (manifest, capability, receipts)
    }

    pub(crate) fn export(
        &self,
        manifest: &CustodyManifestV1,
        capability: CustodyCaptureCapabilityV1,
    ) -> Result<CustodyExportOutcomeV1, CustodyExportErrorV1> {
        self.export_to(&self.scratch, self.budgets, manifest, capability)
    }

    fn export_to(
        &self,
        scratch: &Path,
        budgets: CustodyExportBudgetsV1,
        manifest: &CustodyManifestV1,
        capability: CustodyCaptureCapabilityV1,
    ) -> Result<CustodyExportOutcomeV1, CustodyExportErrorV1> {
        reset_export_counters_for_test();
        export_capsule_v1(CustodyExportRequestV1 {
            manifest,
            envelope_format: &fixture_envelope_format_v1(),
            recipients: &recipients(),
            budgets,
            sealer: &FixtureSealerV1::honest(),
            capability,
            git_route: self.git_route.clone(),
            scratch_root: scratch,
            deadline: Instant::now() + Duration::from_secs(120),
        })
    }

    fn scratch_entries(&self) -> Vec<String> {
        snapshot_entries(&self.scratch).into_keys().collect()
    }

    fn seal_present(&self) -> bool {
        std::fs::symlink_metadata(self.capsule().join(SEAL_NAME)).is_ok()
    }
}

/// [`PlanFixtureV1::readme_clone`]'s source, built in `base`: `(worktree, git_dir)`.
fn readme_clone_in(base: &Path) -> (PathBuf, PathBuf) {
    let origin = base.join("origin");
    std::fs::create_dir(&origin).expect("the origin worktree");
    worktree_git(&origin, &["init", "-q"]);
    std::fs::write(origin.join("README"), b"plan-backed export fixture\n")
        .expect("the origin README");
    worktree_git(&origin, &["add", "README"]);
    worktree_git(&origin, &["commit", "-q", "-m", "init"]);
    worktree_git(base, &["clone", "-q", "origin", "clone"]);
    (base.join("clone"), base.join("clone/.git"))
}

/// The manifest §6.4 builds from a plan: its coverage rows, exclusions, and dependencies.
pub(crate) fn manifest_from_plan(
    plan: &CustodyCoveragePlanV1,
    generation: &str,
    objects: Vec<CustodyOriginalObjectV1>,
) -> CustodyManifestV1 {
    manifest_with_rows(plan, generation, objects, plan.coverage().to_vec())
}

fn manifest_with_rows(
    plan: &CustodyCoveragePlanV1,
    generation: &str,
    objects: Vec<CustodyOriginalObjectV1>,
    coverage: Vec<CustodyCoverageEntryV1>,
) -> CustodyManifestV1 {
    let [unit, run, materialization, _] = IDENTITY_V1;
    CustodyManifestV1::new(
        unit,
        run,
        materialization,
        generation,
        coverage,
        vec![],
        objects,
        plan.dependencies().to_vec(),
        plan.exclusions().to_vec(),
    )
    .expect("a manifest from the plan's collections is valid")
}

/// `rows` with `class` set to `state`.
fn with_state(
    rows: &[CustodyCoverageEntryV1],
    class: CustodyCoverageClassV1,
    state: CustodyStateClassV1,
) -> Vec<CustodyCoverageEntryV1> {
    rows.iter()
        .map(|row| {
            if row.class() == class {
                CustodyCoverageEntryV1::new(class, state, vec![], None).expect("a valid row")
            } else {
                row.clone()
            }
        })
        .collect()
}

pub(crate) fn planned_capability(
    manifest: &CustodyManifestV1,
    plan: CustodyCoveragePlanV1,
    sources: CustodyCoverageSourcesV1,
) -> CustodyCaptureCapabilityV1 {
    CustodyCaptureCapabilityV1::from_fixture_quiescence_with_plan(
        CustodyQuiescenceDecisionV1::CoherentSnapshot,
        manifest,
        sources,
        plan,
    )
}

fn path_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

fn receipt_of(
    receipts: &[CustodyClassReceiptV1],
    class: CustodyCoverageClassV1,
) -> CustodyClassReceiptV1 {
    *receipts
        .iter()
        .find(|receipt| receipt.class == class)
        .expect("a captured class has a receipt")
}

/// §6.1: the export refuses `CapabilityBinding(detail)`, and the scratch root gained no entry.
fn assert_binding_refused(
    fixture: &PlanFixtureV1,
    manifest: &CustodyManifestV1,
    capability: CustodyCaptureCapabilityV1,
    detail: &str,
) {
    let result = fixture.export(manifest, capability);
    assert!(
        matches!(&result, Err(CustodyExportErrorV1::CapabilityBinding(observed)) if *observed == detail),
        "expected CapabilityBinding({detail:?}), observed {}",
        describe_run(&result)
    );
    assert_eq!(
        fixture.scratch_entries(),
        Vec::<String>::new(),
        "a binding refusal created a scratch entry"
    );
}

#[test]
fn planned_binding_01_a_plan_with_an_unresolved_row_refuses() {
    let fixture = PlanFixtureV1::readme_clone();
    let inventory = fixture.inventory();
    let (clean, _) = fixture.plan(GENERATION_V1, &inventory);
    let manifest = manifest_from_plan(&clean, GENERATION_V1, inventory.clone());
    assert_eq!(manifest.sealability(), CustodySealabilityV1::Sealable);
    let (plan, sources) = fixture.plan_with(GENERATION_V1, &inventory, |request| {
        request.external_evidence = ExternalEvidenceUnresolvedV1::declare().into();
    });
    assert!(plan
        .coverage()
        .iter()
        .any(|row| row.state() == CustodyStateClassV1::Unresolved));
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        "the coverage plan has an unresolved row",
    );
}

#[test]
fn planned_binding_02_a_generation_mismatch_refuses_with_or_without_receipts() {
    // No receipts: an empty source planned under A, against a B manifest.
    let fixture = PlanFixtureV1::empty_source();
    let (plan, sources) = fixture.plan("generation-a", &[]);
    assert!(plan.receipts().is_empty());
    assert_eq!(plan.generation_id(), "generation-a");
    let manifest = manifest_from_plan(&plan, "generation-b", Vec::new());
    assert_eq!(manifest.sealability(), CustodySealabilityV1::Sealable);
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        "the coverage plan names another generation",
    );

    // Receipts: a clone planned under A, against a B manifest.
    let fixture = PlanFixtureV1::readme_clone();
    let inventory = fixture.inventory();
    let (plan, sources) = fixture.plan("generation-a", &inventory);
    assert!(!plan.receipts().is_empty());
    let manifest = manifest_from_plan(&plan, "generation-b", inventory);
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        "the coverage plan names another generation",
    );
}

#[test]
fn planned_binding_03_a_coverage_row_flipped_either_way_refuses() {
    let fixture = PlanFixtureV1::readme_clone();
    let inventory = fixture.inventory();
    let detail = "the coverage plan's rows are not the manifest's";
    // The object database is outside the receipts, so only the row comparison sees these.
    // Captured in the plan, empty in the manifest.
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let rows = with_state(
        plan.coverage(),
        CustodyCoverageClassV1::ObjectDatabase,
        CustodyStateClassV1::Empty,
    );
    let manifest = manifest_with_rows(&plan, GENERATION_V1, Vec::new(), rows);
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        detail,
    );
    // Empty in the plan, captured in the manifest.
    let (plan, sources) = fixture.plan(GENERATION_V1, &[]);
    let rows = with_state(
        plan.coverage(),
        CustodyCoverageClassV1::ObjectDatabase,
        CustodyStateClassV1::Captured,
    );
    let manifest = manifest_with_rows(&plan, GENERATION_V1, inventory, rows);
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        detail,
    );
}

#[test]
fn planned_binding_04_an_extra_or_a_missing_exclusion_refuses() {
    let fixture = PlanFixtureV1::cargo_clone();
    let inventory = fixture.inventory();
    let detail = "the coverage plan's exclusions are not the manifest's";
    // A manifest cannot hold an exclusion no row references, so the plan side differs.
    let (mut plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    assert_eq!(plan.exclusions().len(), 1);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory.clone());
    plan.exclusions_mut_for_test().push(
        CustodyExclusionV1::new(
            "extra-v1",
            "reproducible_outputs",
            "extra-v1",
            vec!["cargo-lock".to_owned()],
        )
        .expect("a valid exclusion"),
    );
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        detail,
    );
    let (mut plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
    plan.exclusions_mut_for_test().clear();
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        detail,
    );
}

#[test]
fn planned_binding_05_one_changed_dependency_digest_refuses() {
    let fixture = PlanFixtureV1::cargo_clone();
    let inventory = fixture.inventory();
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let mut dependencies = plan.dependencies().to_vec();
    assert_eq!(dependencies.len(), 2);
    dependencies[0] = CustodyDependencyV1::new(
        "cargo-lock",
        "worktree-file",
        Sha256HexV1::digest(b"another lock file"),
        CustodyStateClassV1::Captured,
        vec![],
    )
    .expect("a valid dependency");
    let [unit, run, materialization, _] = IDENTITY_V1;
    let manifest = CustodyManifestV1::new(
        unit,
        run,
        materialization,
        GENERATION_V1,
        plan.coverage().to_vec(),
        vec![],
        inventory,
        dependencies,
        plan.exclusions().to_vec(),
    )
    .expect("a valid manifest");
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        "the coverage plan's dependencies are not the manifest's",
    );
}

#[test]
fn planned_binding_06_a_missing_or_an_extra_receipt_refuses() {
    let fixture = PlanFixtureV1::readme_clone();
    let inventory = fixture.inventory();
    let detail = "the coverage plan's receipts are not the manifest's captured classes";
    let (mut plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory.clone());
    plan.receipts_mut_for_test()
        .retain(|receipt| receipt.class != CustodyCoverageClassV1::Worktree);
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        detail,
    );
    // An extra receipt, in class order, for a class the manifest has empty.
    let (mut plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
    let mut extra = receipt_of(plan.receipts(), CustodyCoverageClassV1::Worktree);
    extra.class = CustodyCoverageClassV1::LinkedWorktrees;
    plan.receipts_mut_for_test().push(extra);
    plan.receipts_mut_for_test()
        .sort_by_key(|receipt| receipt.class);
    assert_binding_refused(
        &fixture,
        &manifest,
        planned_capability(&manifest, plan, sources),
        detail,
    );
}

/// §6.1's alias row, through the plan-backed path. Each row renames one pinned directory after
/// the capability is minted, then exports into a new, empty, owner-private directory beneath the
/// renamed one: no path shows the overlap, and only the retained pin's identity does. Every row
/// is evaluated, and every failing row is reported.
#[test]
fn planned_binding_07_a_scratch_root_aliasing_a_protected_pin_refuses() {
    type PinnedPathV1 = fn(&PlanFixtureV1) -> PathBuf;
    let rows: [(&str, PinnedPathV1); 3] = [
        ("the source repository", |fixture| fixture.worktree.clone()),
        ("the git directory", |fixture| fixture.git_dir.clone()),
        ("the primary store", PlanFixtureV1::objects),
    ];
    let mut failures = Vec::new();
    for (label, pinned) in rows {
        let fixture = PlanFixtureV1::readme_clone();
        let (manifest, capability, _) = fixture.planned();
        let alias = fixture.area.join("alias");
        std::fs::rename(pinned(&fixture), &alias).expect("rename the pinned directory");
        let scratch = alias.join("export-scratch");
        std::fs::create_dir(&scratch).expect("a scratch beneath the alias");
        set_owner_private(&scratch);
        let result = fixture.export_to(&scratch, fixture.budgets, &manifest, capability);
        let written: Vec<String> = snapshot_entries(&scratch).into_keys().collect();
        let refused = matches!(&result, Err(CustodyExportErrorV1::ScratchPreflight(detail))
            if detail.contains("is or lies inside"));
        if !refused || !written.is_empty() {
            failures.push(format!(
                "{label}: {}; scratch entries: {written:?}",
                describe_run(&result)
            ));
        }
    }

    // An alternate store. The plan of a source with one leaves its alternates row unresolved, so
    // the export refuses at binding step 1. Its capability's protected set still meets the
    // exporter's own preflight, which refuses the alias the same way.
    let fixture = PlanFixtureV1::readme_clone();
    let alternate = fixture.area.join("alternate.git");
    worktree_git(
        &fixture.area,
        &["init", "-q", "--bare", alternate.to_str().expect("utf-8")],
    );
    let alternate_store = alternate.join("objects");
    std::fs::create_dir_all(fixture.objects().join("info")).expect("objects/info");
    std::fs::write(
        fixture.objects().join("info/alternates"),
        format!("{}\n", alternate_store.display()),
    )
    .expect("the alternates file");
    let inventory = fixture.inventory();
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
    let capability = planned_capability(&manifest, plan, sources);
    let alias = fixture.area.join("alternate-alias");
    std::fs::rename(&alternate_store, &alias).expect("rename the alternate store");
    let scratch = alias.join("export-scratch");
    std::fs::create_dir(&scratch).expect("a scratch beneath the alias");
    set_owner_private(&scratch);
    let preflight = preflight_scratch_root(&scratch, &capability.protected_directories());
    if !matches!(&preflight, Err(CustodyExportErrorV1::ScratchPreflight(detail))
        if detail.contains("is or lies inside"))
        || !snapshot_entries(&scratch).is_empty()
    {
        failures.push(format!("an alternate store: {:?}", preflight.map(|_| ())));
    }
    let result = fixture.export_to(&scratch, fixture.budgets, &manifest, capability);
    if !matches!(&result, Err(CustodyExportErrorV1::CapabilityBinding(detail))
        if *detail == "the coverage plan has an unresolved row")
        || !snapshot_entries(&scratch).is_empty()
    {
        failures.push(format!(
            "an alternate store's export: {}",
            describe_run(&result)
        ));
    }
    assert!(
        failures.is_empty(),
        "an identity alias of a protected pin was not refused:\n{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------------------------
// §6.2 The mount census
// ---------------------------------------------------------------------------------------------

/// A mount point at the repository's `target`, one inside the git directory, and a non-UTF-8
/// one inside the repository each park the unit before any scratch write. Every row is
/// evaluated, and every failing row is reported.
#[test]
fn planned_census_01_a_mount_inside_the_repository_or_git_directory_parks_the_unit() {
    type MountPointV1 = fn(&PlanFixtureV1) -> Vec<u8>;
    let rows: [(&str, MountPointV1); 3] = [
        ("the repository's target", |fixture| {
            path_bytes(&fixture.worktree.join("target"))
        }),
        ("inside the git directory", |fixture| {
            path_bytes(&fixture.git_dir.join("refs/heads"))
        }),
        ("a 0xff name inside the repository", |fixture| {
            let mut mount_point = path_bytes(&fixture.worktree);
            mount_point.extend_from_slice(b"/\xff\xfe");
            mount_point
        }),
    ];
    let mut failures = Vec::new();
    for (label, inside) in rows {
        let fixture = PlanFixtureV1::cargo_clone();
        let (manifest, capability, _) = fixture.planned();
        let _census = mount_seam::install_list(vec![b"/".to_vec(), inside(&fixture)]);
        let result = fixture.export(&manifest, capability);
        let written = fixture.scratch_entries();
        if !matches!(result, Err(CustodyExportErrorV1::MountBoundary(_))) || !written.is_empty() {
            failures.push(format!(
                "{label}: {}; scratch entries: {written:?}",
                describe_run(&result)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "a mount inside the protected set did not park the unit:\n{}",
        failures.join("\n")
    );
}

/// The mount containing the repository, the repository's own mount, and a prefix sibling
/// (`/repository` beside `/repo`) are none of them strictly inside it: the unit seals. A `.gitx`
/// beside `.git` is a sibling of the git directory but lies inside the repository, and parks.
#[test]
fn planned_census_02_a_prefix_sibling_and_the_containing_mount_do_not_park() {
    let fixture = PlanFixtureV1::cargo_clone();
    let (manifest, capability, _) = fixture.planned();
    let mut sibling = path_bytes(&fixture.worktree);
    sibling.extend_from_slice(b"sibling");
    let census = mount_seam::install_list(vec![
        b"/".to_vec(),
        path_bytes(&fixture.area),
        path_bytes(&fixture.worktree),
        sibling,
    ]);
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the export seals"),
    );
    assert!(census.calls() >= 1, "{} census calls", census.calls());
    drop(census);

    let fixture = PlanFixtureV1::cargo_clone();
    let (manifest, capability, _) = fixture.planned();
    let mut git_sibling = path_bytes(&fixture.git_dir);
    git_sibling.extend_from_slice(b"x");
    let _census = mount_seam::install_list(vec![b"/".to_vec(), git_sibling]);
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(result, Err(CustodyExportErrorV1::MountBoundary(_))),
        "{}",
        describe_run(&result)
    );
}

/// A mount inside an alternate store. A plan-backed export of such a source refuses at binding
/// step 1 (its alternates row is unresolved), so the census the export runs is called on that
/// capability directly.
#[test]
fn planned_census_03_a_mount_inside_an_alternate_store_parks_the_unit() {
    let fixture = PlanFixtureV1::readme_clone();
    let alternate = fixture.area.join("alternate.git");
    worktree_git(
        &fixture.area,
        &["init", "-q", "--bare", alternate.to_str().expect("utf-8")],
    );
    std::fs::create_dir_all(fixture.objects().join("info")).expect("objects/info");
    std::fs::write(
        fixture.objects().join("info/alternates"),
        format!("{}\n", alternate.join("objects").display()),
    )
    .expect("the alternates file");
    let inventory = fixture.inventory();
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
    let capability = planned_capability(&manifest, plan, sources);
    let _census = mount_seam::install_list(vec![
        b"/".to_vec(),
        path_bytes(&alternate.join("objects/pack")),
    ]);
    assert!(matches!(
        capability.recheck_mounts(),
        Err(CustodyExportErrorV1::MountBoundary(detail)) if detail.contains("objects/pack")
    ));
}

/// Every census failure fails closed, before any scratch write.
#[test]
fn planned_census_04_a_census_error_fails_closed() {
    let mut failures = Vec::new();
    for error in [
        CustodyMountErrorV1::Io(std::io::ErrorKind::PermissionDenied),
        CustodyMountErrorV1::MalformedLine { line: 3 },
        CustodyMountErrorV1::MalformedEscape { line: 1 },
        CustodyMountErrorV1::Oversize,
        CustodyMountErrorV1::CensusUnsupported,
        CustodyMountErrorV1::Empty,
        CustodyMountErrorV1::Unstable,
    ] {
        let fixture = PlanFixtureV1::readme_clone();
        let (manifest, capability, _) = fixture.planned();
        let _census = mount_seam::install(Box::new(move |_| Err(error)));
        let result = fixture.export(&manifest, capability);
        let written = fixture.scratch_entries();
        if !matches!(&result, Err(CustodyExportErrorV1::CapabilityBinding(detail))
            if detail.contains("mount census"))
            || !written.is_empty()
        {
            failures.push(format!(
                "{error:?}: {}; scratch entries: {written:?}",
                describe_run(&result)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "a census failure did not fail closed:\n{}",
        failures.join("\n")
    );
}

/// The census runs again immediately before the first scratch write: a mount that appears once
/// binding is over (the layout has been derived) is refused with the scratch root still empty.
/// The census call is chosen by the export's phase, not by its number, so no other call's
/// presence or absence moves it.
#[test]
fn planned_census_05_the_census_runs_again_before_the_first_scratch_write() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, _) = fixture.planned();
    let inside = path_bytes(&fixture.worktree.join("target"));
    let _census = mount_seam::install(Box::new(move |_| {
        let mut mount_points = vec![b"/".to_vec()];
        if derive_calls_for_test() >= 1 {
            mount_points.push(inside.clone());
        }
        Ok(mount_points)
    }));
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(result, Err(CustodyExportErrorV1::MountBoundary(_))),
        "{}",
        describe_run(&result)
    );
    assert_eq!(fixture.scratch_entries(), Vec::<String>::new());
}

/// The census runs again before each restage (§5 step 3): a mount that appears once `work/`
/// exists is refused before the first walked class restages, and no seal exists.
#[test]
fn planned_census_06_the_census_runs_again_before_each_restage() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, _) = fixture.planned();
    let inside = path_bytes(&fixture.git_dir.join("hooks"));
    let work = fixture.work();
    let _census = mount_seam::install(Box::new(move |_| {
        let mut mount_points = vec![b"/".to_vec()];
        if work.exists() {
            mount_points.push(inside.clone());
        }
        Ok(mount_points)
    }));
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(result, Err(CustodyExportErrorV1::MountBoundary(_))),
        "{}",
        describe_run(&result)
    );
    assert!(!fixture.seal_present());
    // The first walked class's frame was created, and nothing was restaged into it.
    let frames: Vec<_> = std::fs::read_dir(fixture.work())
        .expect("work/")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "frame")
        })
        .collect();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(std::fs::metadata(&frames[0]).expect("the frame").len(), 0);
}

/// Unmounts a real bind mount when dropped.
#[cfg(target_os = "linux")]
struct BindMountV1(PathBuf);

#[cfg(target_os = "linux")]
impl Drop for BindMountV1 {
    fn drop(&mut self) {
        let _ = Command::new("umount").arg(&self.0).status();
    }
}

/// The real bind mount of §6.2: a directory containing the git directory, bind-mounted at
/// `repo/target`, is refused. It needs a mount-capable Linux lane. Elsewhere `mount` is refused,
/// the control reports the named exclusion, and the injected-census controls stand in for it.
#[cfg(target_os = "linux")]
#[test]
fn planned_census_07_a_real_bind_mount_at_repo_target_refuses_where_mounting_is_permitted() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, _) = fixture.planned();
    let target = fixture.worktree.join("target");
    std::fs::create_dir(&target).expect("the mount point");
    let mounted = Command::new("mount")
        .arg("--bind")
        .arg(&fixture.worktree)
        .arg(&target)
        .output();
    match mounted {
        Ok(output) if output.status.success() => {}
        other => {
            eprintln!(
                "named exclusion (task §6.2): this lane cannot bind-mount ({other:?}); the \
                 injected-census controls stand in for the real bind mount"
            );
            return;
        }
    }
    let _mount = BindMountV1(target.clone());
    assert!(
        target.join(".git").is_dir(),
        "the bind mount shows the repository"
    );
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(&result, Err(CustodyExportErrorV1::MountBoundary(detail))
            if detail.contains(&*target.to_string_lossy())),
        "{}",
        describe_run(&result)
    );
    assert_eq!(fixture.scratch_entries(), Vec::<String>::new());
}

/// Binding's own census (§3.2 step 5) refuses a mount that only it sees: the mount is gone once
/// binding is over (the layout has been derived), so nothing but step 5 stands between it and a
/// sealed capsule.
#[test]
fn planned_census_08_the_binding_census_refuses_before_any_later_call() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, _) = fixture.planned();
    let inside = path_bytes(&fixture.worktree.join("target"));
    let census = mount_seam::install(Box::new(move |_| {
        let mut mount_points = vec![b"/".to_vec()];
        if derive_calls_for_test() == 0 {
            mount_points.push(inside.clone());
        }
        Ok(mount_points)
    }));
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(result, Err(CustodyExportErrorV1::MountBoundary(_))),
        "{}",
        describe_run(&result)
    );
    assert_eq!(census.calls(), 1);
    assert_eq!(fixture.scratch_entries(), Vec::<String>::new());
}

/// A mount at `target` of a repository whose own path holds the non-UTF-8 bytes `0xff 0xfe`, as
/// the kernel writes it in `mountinfo`, parks the unit. The census parses the bytes verbatim, so
/// the mount point is a byte prefix match of the repository's canonical path; a census that went
/// through a UTF-8 conversion would miss it. Linux only: APFS refuses a non-UTF-8 name.
#[cfg(target_os = "linux")]
#[test]
fn planned_census_09_a_mountinfo_0xff_path_inside_the_repository_parks_the_unit() {
    let fixture = PlanFixtureV1::with(|area| {
        let base = area.join(OsStr::from_bytes(b"\xff\xfe-base"));
        std::fs::create_dir(&base).expect("a non-UTF-8 base directory");
        readme_clone_in(&base)
    });
    let (manifest, capability, _) = fixture.planned();
    let mut text = b"1 0 0:1 / / rw - overlay overlay rw\n2 1 0:1 / ".to_vec();
    text.extend_from_slice(&path_bytes(&fixture.worktree.join("target")));
    text.extend_from_slice(b" rw,relatime - overlay overlay rw\n");
    assert!(text.contains(&0xff));
    let _census = mount_seam::install_mountinfo(text);
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(result, Err(CustodyExportErrorV1::MountBoundary(_))),
        "{}",
        describe_run(&result)
    );
    assert_eq!(fixture.scratch_entries(), Vec::<String>::new());
}

// ---------------------------------------------------------------------------------------------
// §6.3 Bounded staging, and the retention of staged frames
// ---------------------------------------------------------------------------------------------

/// A planned file grown past its receipt, with no ledger headroom beyond the frame's
/// reservation: the export refuses `SourceDrift`, the staged frame is no longer than its
/// reservation, and the bounded writer's record shows it refused byte `frame_length + 1`.
#[test]
fn planned_stage_01_a_file_grown_past_its_receipt_refuses_within_its_reservation() {
    let fixture = PlanFixtureV1::readme_clone();
    let inventory = fixture.inventory();
    let worktree = CustodyCoverageClassV1::Worktree;

    // The exact budget: an unchanged export records the worktree frame's reservation and the
    // ledger's use before it.
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory.clone());
    let receipt = receipt_of(plan.receipts(), worktree);
    let _ = expect_sealed(
        fixture
            .export(&manifest, planned_capability(&manifest, plan, sources))
            .expect("the unchanged export seals"),
    );
    let probe = frame_reservations_for_test()
        .into_iter()
        .find(|reservation| reservation.class == worktree)
        .expect("the worktree frame was reserved");
    assert_eq!(
        probe.reserved,
        receipt.frame_length + ENTRY_ALLOWANCE_BYTES_V1
    );

    // The same source planned again, then grown.
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    assert_eq!(receipt_of(plan.receipts(), worktree), receipt);
    let capability = planned_capability(&manifest, plan, sources);
    let mut readme = std::fs::OpenOptions::new()
        .append(true)
        .open(fixture.worktree.join("README"))
        .expect("the planned README");
    readme
        .write_all(&[b'+'; 4096])
        .expect("the README grows after planning");
    drop(readme);

    let scratch = fixture.area.join("scratch-without-headroom");
    std::fs::create_dir(&scratch).expect("a second scratch");
    set_owner_private(&scratch);
    let budgets = CustodyExportBudgetsV1 {
        max_scratch_bytes: probe.used_before + probe.reserved,
        ..fixture.budgets
    };
    let result = fixture.export_to(&scratch, budgets, &manifest, capability);
    assert!(
        matches!(&result, Err(CustodyExportErrorV1::SourceDrift(detail)) if detail.contains("past its receipt")),
        "{}",
        describe_run(&result)
    );
    let staged = frame_reservations_for_test()
        .into_iter()
        .find(|reservation| reservation.class == worktree)
        .expect("the grown frame was reserved");
    assert_eq!(
        staged.used_before + staged.reserved,
        budgets.max_scratch_bytes,
        "the frame's reservation left the ledger no headroom"
    );
    let frame = scratch
        .join(WORK_DIR_NAME)
        .join(staged_frame_name(worktree));
    let length = std::fs::metadata(&frame).expect("the staged frame").len();
    assert!(
        length <= staged.reserved - ENTRY_ALLOWANCE_BYTES_V1,
        "{length} staged bytes exceed the {} byte reservation",
        staged.reserved - ENTRY_ALLOWANCE_BYTES_V1
    );
    assert_eq!(
        frame_overflows_for_test(),
        vec![(worktree, receipt.frame_length)],
        "the bounded writer refused byte frame_length + 1"
    );
    assert!(std::fs::symlink_metadata(scratch.join(CAPSULE_DIR_NAME).join(SEAL_NAME)).is_err());
}

/// A planned file changed at the same length restages to a frame of the receipt's length, and
/// the digest comparison refuses it.
#[test]
fn planned_stage_02_a_file_changed_at_the_same_length_refuses_by_its_digest() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, receipts) = fixture.planned();
    let readme = fixture.worktree.join("README");
    let before = std::fs::read(&readme).expect("the planned README");
    let after: Vec<u8> = before
        .iter()
        .map(|byte| byte.to_ascii_uppercase())
        .collect();
    assert_eq!((before.len(), before != after), (after.len(), true));
    std::fs::write(&readme, &after).expect("the same-length rewrite");
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(&result, Err(CustodyExportErrorV1::SourceDrift(detail))
            if detail.contains("another frame than its plan receipt")),
        "{}",
        describe_run(&result)
    );
    assert!(frame_overflows_for_test().is_empty());
    let receipt = receipt_of(&receipts, CustodyCoverageClassV1::Worktree);
    let staged = std::fs::read(
        fixture
            .work()
            .join(staged_frame_name(CustodyCoverageClassV1::Worktree)),
    )
    .expect("the staged frame");
    assert_eq!(staged.len() as u64, receipt.frame_length);
    assert_ne!(sha256_bytes(&staged), receipt.frame_sha256);
    assert!(!fixture.seal_present());
}

/// The capability recheck runs before each restage (§5 step 3). The worktree root is swapped for
/// a copy after the first walked frame is created, after every Git child's recheck has passed.
#[test]
fn planned_stage_03_the_recheck_runs_before_each_restage() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, _) = fixture.planned();
    let worktree = fixture.worktree.clone();
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::AfterFrameCreate,
        on_nth_call(1, move |_| swap_worktree_keeping_its_git_dir(&worktree)),
    );
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(result, Err(CustodyExportErrorV1::IdentityDrift(_))),
        "{}",
        describe_run(&result)
    );
    assert!(!fixture.seal_present());
}

/// The published payload body of `class`: its fixture envelope less the magic.
fn payload_body(
    fixture: &PlanFixtureV1,
    manifest: &CustodyManifestV1,
    class: CustodyCoverageClassV1,
) -> Vec<u8> {
    let layout = CustodyCapsuleLayoutV1::derive(manifest).expect("the layout");
    let row = layout
        .index()
        .artifacts()
        .iter()
        .find(|row| *row.role() == CustodyCapsuleArtifactRoleV1::CoveragePayload(class))
        .expect("a captured class has a payload");
    let envelope = std::fs::read(
        fixture
            .capsule()
            .join(OsStr::from_bytes(row.name().as_bytes())),
    )
    .expect("the published payload");
    assert_eq!(&envelope[..8], FIXTURE_ENVELOPE_MAGIC_V1);
    envelope[8..].to_vec()
}

/// Step 6 seals the staged frame's retained descriptor and never reopens it by name. After the
/// first staged frame matched its receipt, its name is given to a same-length file of inverted
/// bytes; the sealed payload is still exactly the receipt's frame.
#[test]
fn planned_stage_04_the_seal_reads_the_retained_descriptor_not_the_name() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, receipts) = fixture.planned();
    let replaced: Rc<RefCell<Option<PathBuf>>> = Rc::default();
    let record = Rc::clone(&replaced);
    let _hook = install_export_hook_for_test(
        ExportHookPointV1::AfterFrameStaged,
        on_nth_call(1, move |path| {
            let staged = std::fs::read(path).expect("the staged frame");
            std::fs::rename(path, path.with_extension("frame-moved")).expect("move it aside");
            let inverted: Vec<u8> = staged.iter().map(|byte| !byte).collect();
            std::fs::write(path, inverted).expect("a same-length decoy at its name");
            *record.borrow_mut() = Some(path.to_path_buf());
        }),
    );
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(result, Ok(CustodyExportOutcomeV1::Sealed(_))),
        "{}",
        describe_run(&result)
    );
    let replaced = replaced
        .borrow()
        .clone()
        .expect("the hook replaced a staged frame");
    let receipt = receipts
        .iter()
        .find(|receipt| replaced.file_name() == Some(OsStr::new(&staged_frame_name(receipt.class))))
        .expect("the replaced frame is a walked class's");
    let payload = payload_body(&fixture, &manifest, receipt.class);
    assert_eq!(payload.len() as u64, receipt.frame_length);
    assert_eq!(sha256_bytes(&payload), receipt.frame_sha256);
}

/// After a successful export every staged frame is still in `work/`, holding exactly its
/// receipt's frame, and no `work/` entry observed during publication was deleted.
#[test]
fn planned_retention_01_every_staged_frame_is_kept_as_evidence() {
    let fixture = PlanFixtureV1::readme_clone();
    let (manifest, capability, receipts) = fixture.planned();
    let observed: Rc<RefCell<BTreeSet<String>>> = Rc::default();
    let seen = Rc::clone(&observed);
    let work = fixture.work();
    let _hook = install_export_hook_for_test(ExportHookPointV1::AfterArtifactPublish, move |_| {
        seen.borrow_mut()
            .extend(snapshot_entries(&work).into_keys());
    });
    let _ = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the export seals"),
    );

    let after: BTreeSet<String> = snapshot_entries(&fixture.work()).into_keys().collect();
    let deleted: Vec<_> = observed.borrow().difference(&after).cloned().collect();
    assert!(
        deleted.is_empty(),
        "work/ entries were deleted: {deleted:?}"
    );
    for receipt in &receipts {
        let name = staged_frame_name(receipt.class);
        assert!(observed.borrow().contains(&name), "{name}");
        let frame = std::fs::read(fixture.work().join(&name)).expect("a retained frame");
        assert_eq!(frame.len() as u64, receipt.frame_length, "{name}");
        assert_eq!(sha256_bytes(&frame), receipt.frame_sha256, "{name}");
    }
    let top: BTreeSet<String> = std::fs::read_dir(fixture.work())
        .expect("work/")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let mut expected: BTreeSet<String> = [
        HOME_DIR_NAME,
        XDG_DIR_NAME,
        SOURCE_GIT_DIR_NAME,
        VERIFY_GIT_DIR_NAME,
        PACK_FILE_NAME,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    expected.extend(
        receipts
            .iter()
            .map(|receipt| staged_frame_name(receipt.class)),
    );
    assert_eq!(top, expected);
}

// ---------------------------------------------------------------------------------------------
// §6.4 End to end
// ---------------------------------------------------------------------------------------------

/// A frame's entries: `(path, kind, content)`, content empty for a directory or symlink.
fn frame_entries(frame: &[u8], header: &CustodyFrameHeaderV1) -> Vec<(Vec<u8>, char, Vec<u8>)> {
    let mut decoder =
        CustodyFrameDecoderV1::new(frame, header, plan_frame_budget()).expect("a frame header");
    let mut entries = Vec::new();
    while let Some(entry) = decoder.next_entry().expect("a well-formed frame") {
        entries.push(match entry {
            CustodyFrameEntryV1::Directory { path, .. } => (path.as_bytes().to_vec(), 'd', vec![]),
            CustodyFrameEntryV1::Symlink { path, target } => {
                (path.as_bytes().to_vec(), 'l', target)
            }
            CustodyFrameEntryV1::Regular {
                path, mut content, ..
            } => {
                let mut bytes = Vec::new();
                content.read_to_end(&mut bytes).expect("verified content");
                (path.as_bytes().to_vec(), 'f', bytes)
            }
        });
    }
    entries
}

#[test]
fn planned_e2e_01_a_rich_clone_exports_a_sealed_capsule_of_its_walked_classes() {
    use CustodyCoverageClassV1 as Class;
    let fixture = PlanFixtureV1::rich_clone();
    let inventory = fixture.inventory();
    assert!(inventory.len() > 10, "{} objects", inventory.len());
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    let receipts = plan.receipts().to_vec();
    assert_eq!(
        receipts
            .iter()
            .map(|receipt| receipt.class)
            .collect::<Vec<_>>(),
        [
            Class::RefsAndHead,
            Class::Index,
            Class::Worktree,
            Class::StashAndReflogs,
            Class::InProgressGitOperations,
            Class::GitConfigurationAndHooks,
            Class::BridgeEvidence,
        ]
    );
    assert_eq!(plan.exclusions().len(), 1);
    assert_eq!(plan.dependencies().len(), 2);
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
    assert_eq!(manifest.sealability(), CustodySealabilityV1::Sealable);
    let capability = planned_capability(&manifest, plan, sources);

    let before = snapshot_tree(&fixture.worktree);
    let installed = plan_seam::install(PlanSeamsV1::default());
    let sealed = expect_sealed(
        fixture
            .export(&manifest, capability)
            .expect("the export seals"),
    );
    let walks = installed.record().walks;
    drop(installed);
    assert_eq!(
        snapshot_tree(&fixture.worktree),
        before,
        "the source changed"
    );

    // 2B1's binding over the published capsule: the seal, the index, the manifest, and every
    // artifact's recorded length and digest.
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).expect("the layout");
    let seal = CustodySealV1::decode_canonical(
        &std::fs::read(fixture.capsule().join(SEAL_NAME)).expect("the published seal"),
    )
    .expect("the published seal decodes");
    assert_eq!(seal, sealed.seal);
    CustodyCapsuleSealProofV1::preflight_generic_seal_for_capsule_v1(&seal)
        .expect("a capsule seal");
    assert_eq!(
        &seal.content_digest().unwrap(),
        sealed.binding.seal_digest()
    );
    assert_eq!(
        &manifest.content_digest().unwrap(),
        sealed.binding.manifest_digest()
    );
    assert_eq!(
        &layout.index().content_digest().unwrap(),
        sealed.binding.index_digest()
    );
    let names: Vec<&[u8]> = layout
        .index()
        .artifacts()
        .iter()
        .map(|row| row.name().as_bytes())
        .collect();
    assert_eq!(
        sealed
            .binding
            .artifact_names()
            .iter()
            .map(LosslessPathV1::as_bytes)
            .collect::<Vec<_>>(),
        names
    );
    for artifact in seal.artifacts() {
        let bytes = std::fs::read(
            fixture
                .capsule()
                .join(OsStr::from_bytes(artifact.name().as_bytes())),
        )
        .expect("a sealed artifact");
        assert_eq!(bytes.len() as u64, artifact.byte_length());
        assert_eq!(&Sha256HexV1::digest(&bytes), artifact.sha256());
    }
    let body = |name: &[u8]| {
        let envelope = std::fs::read(fixture.capsule().join(OsStr::from_bytes(name)))
            .expect("a published artifact");
        assert_eq!(&envelope[..8], FIXTURE_ENVELOPE_MAGIC_V1);
        envelope[8..].to_vec()
    };
    assert_eq!(
        CustodyManifestV1::decode_canonical(&body(MANIFEST_ARTIFACT_V1.as_bytes())).unwrap(),
        manifest
    );
    assert_eq!(
        &CustodyCapsuleIndexV1::decode_canonical(&body(b"control/capsule-index.json.enc")).unwrap(),
        layout.index()
    );

    // The capsule holds the verified Git pack.
    let pack = body(PACK_ARTIFACT_V1.as_bytes());
    assert!(!pack.is_empty());
    assert_eq!(
        pack,
        std::fs::read(fixture.work().join(PACK_FILE_NAME)).unwrap()
    );
    assert_eq!(sha256_bytes(&pack), sealed.evidence.verified_pack_sha256);

    // Every coverage payload is its receipt's frame, byte for byte an independent re-walk with
    // the class's selection, and decodes to exactly that class's walked entries.
    let git_dir_identity = {
        use std::os::unix::fs::MetadataExt as _;
        let metadata = std::fs::metadata(&fixture.git_dir).unwrap();
        (metadata.dev(), metadata.ino())
    };
    let mut payload_classes = Vec::new();
    for row in layout.index().artifacts() {
        let CustodyCapsuleArtifactRoleV1::CoveragePayload(class) = row.role() else {
            continue;
        };
        let class = *class;
        payload_classes.push(class);
        let receipt = receipt_of(&receipts, class);
        let payload = body(row.name().as_bytes());
        assert_eq!(payload.len() as u64, receipt.frame_length, "{class:?}");
        assert_eq!(sha256_bytes(&payload), receipt.frame_sha256, "{class:?}");
        assert_eq!(
            std::fs::read(fixture.work().join(staged_frame_name(class))).unwrap(),
            payload,
            "{class:?}: the retained staged frame"
        );
        let header = CustodyFrameHeaderV1::new(class, GENERATION_V1).unwrap();
        let worktree_selection = WorktreeSelectionV1 {
            git_dir: git_dir_identity,
            cargo_target_excluded: true,
        };
        let git_dir_selection = GitDirClassSelectionV1 { class };
        let (root, selection): (&Path, &dyn WalkSelectionV1) = if class == Class::Worktree {
            (&fixture.worktree, &worktree_selection)
        } else {
            (&fixture.git_dir, &git_dir_selection)
        };
        let pin = PinnedDirectoryV1::open(root, "the e2e re-walk").unwrap();
        let mut rewalked = Vec::new();
        let encoder =
            CustodyFrameEncoderV1::new(&mut rewalked, &header, plan_frame_budget()).unwrap();
        walk_tree_v1(&pin, selection, PLAN_ENTRY_BUDGET_V1, encoder).expect("the re-walk");
        assert_eq!(payload, rewalked, "{class:?}");
        let entries = frame_entries(&payload, &header);
        assert!(!entries.is_empty(), "{class:?}");
        assert!(
            !payload
                .windows(TARGET_MARKER_V1.len())
                .any(|window| window == TARGET_MARKER_V1),
            "{class:?} carries target/ bytes"
        );
        if class == Class::Worktree {
            let paths: BTreeSet<Vec<u8>> = entries.iter().map(|(path, ..)| path.clone()).collect();
            assert!(!paths
                .iter()
                .any(|path| path == b"target" || path.starts_with(b"target/")));
            for name in [
                &b"Cargo.toml"[..],
                b"Cargo.lock",
                b"README",
                b"untracked.txt",
                b"build.log",
                b"HEAD",
                b".git",
            ] {
                assert!(paths.contains(name), "{}", String::from_utf8_lossy(name));
            }
            let readme = entries
                .iter()
                .find(|(path, ..)| path == b"README")
                .map(|(_, _, content)| content.clone());
            assert_eq!(readme.as_deref(), Some(&b"coverage fixture\ndirty\n"[..]));
        }
    }
    let mut sorted = payload_classes.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        receipts
            .iter()
            .map(|receipt| receipt.class)
            .collect::<Vec<_>>()
    );

    // The restages ran one at a time, in layout order, each on its own fresh pin.
    let mut pins = BTreeSet::new();
    let mut restaged = Vec::new();
    for pair in walks.chunks(2) {
        let [WalkEventV1::Begin { walk, pin }, WalkEventV1::End {
            walk: ended,
            pin: ended_pin,
        }] = pair
        else {
            panic!("restages overlap: {walks:?}");
        };
        assert_eq!((walk, pin), (ended, ended_pin), "{walks:?}");
        assert!(pins.insert(*pin), "{walks:?}");
        restaged.push(*walk);
    }
    assert_eq!(
        restaged,
        payload_classes
            .iter()
            .map(|class| WalkKindV1::Class(*class))
            .collect::<Vec<_>>()
    );

    // The ledger charged exactly what the scratch root holds.
    assert_eq!(
        sealed.evidence.scratch_bytes_used,
        census_use(&fixture.scratch)
    );
}

// ---------------------------------------------------------------------------------------------
// Repair round 1: a walked class planned `empty` has no receipt or payload, but it is replayed
// before the seal, so a source that gained its first entry since planning is drift.
// ---------------------------------------------------------------------------------------------

/// An unchanged source whose plan has no receipts seals, with and without a planned loose blob:
/// every empty walk replays to its baseline, and nothing is staged. Every row is evaluated, and
/// every failing row is reported.
#[test]
fn planned_empty_01_an_unchanged_source_with_no_receipts_seals() {
    let mut failures = Vec::new();
    for with_blob in [false, true] {
        let fixture = PlanFixtureV1::empty_source();
        let inventory: Vec<_> = if with_blob {
            vec![fixture.add_loose_blob()]
        } else {
            Vec::new()
        };
        let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
        assert!(plan.receipts().is_empty());
        let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
        assert_eq!(manifest.sealability(), CustodySealabilityV1::Sealable);
        let result = fixture.export(&manifest, planned_capability(&manifest, plan, sources));
        let staged: Vec<String> = fixture
            .scratch_entries()
            .into_iter()
            .filter(|name| name.contains("payload-"))
            .collect();
        if !matches!(result, Ok(CustodyExportOutcomeV1::Sealed(_)))
            || !fixture.seal_present()
            || !staged.is_empty()
        {
            failures.push(format!(
                "with a blob {with_blob}: {}; staged {staged:?}",
                describe_run(&result)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "an unchanged zero-receipt source did not seal:\n{}",
        failures.join("\n")
    );
}

/// The reviewer's state: a zero-receipt plan of a source holding only its store and a planned
/// loose blob, then `HEAD` created after the capability is minted. The manifest claims
/// `refs_and_head` empty, and the pins, binding, and census all still pass; the replay of that
/// empty walk refuses the export as drift, and no seal is published.
#[test]
fn planned_empty_02_head_created_after_planning_is_drift_and_never_sealed() {
    let fixture = PlanFixtureV1::empty_source();
    let inventory = vec![fixture.add_loose_blob()];
    let (plan, sources) = fixture.plan(GENERATION_V1, &inventory);
    assert!(plan.receipts().is_empty());
    let manifest = manifest_from_plan(&plan, GENERATION_V1, inventory);
    assert!(manifest.coverage().iter().any(|row| {
        row.class() == CustodyCoverageClassV1::RefsAndHead
            && row.state() == CustodyStateClassV1::Empty
    }));
    let capability = planned_capability(&manifest, plan, sources);
    std::fs::write(fixture.git_dir.join("HEAD"), b"ref: refs/heads/main\n")
        .expect("HEAD created after planning");
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(&result, Err(CustodyExportErrorV1::SourceDrift(detail))
            if detail.contains("RefsAndHead was planned empty and replayed to another walk")),
        "{}",
        describe_run(&result)
    );
    assert!(!fixture.seal_present());
}

/// Only one root domain lacks captured receipts. The worktree and the git directory are apart,
/// so the other domain's restages never observe the empty one. Each row seals unchanged, is
/// planned again, is changed in its empty domain after minting, and must then refuse as drift
/// with no seal. Every row is evaluated, and every failing row is reported.
#[test]
fn planned_empty_03_a_root_domain_without_captured_receipts_is_still_replayed() {
    type ChangeV1 = fn(&PlanFixtureV1);
    let rows: [(&str, bool, CustodyCoverageClassV1, ChangeV1); 2] = [
        (
            "a git directory of only `objects/` beside a captured worktree, then HEAD",
            false,
            CustodyCoverageClassV1::RefsAndHead,
            |fixture| {
                std::fs::write(fixture.git_dir.join("HEAD"), b"ref: refs/heads/main\n")
                    .expect("HEAD created after planning");
            },
        ),
        (
            "an empty worktree beside a captured git directory, then a worktree file",
            true,
            CustodyCoverageClassV1::Worktree,
            |fixture| fixture.write("new.txt", b"created after planning\n"),
        ),
    ];
    let mut failures = Vec::new();
    for (label, populated_git_dir, empty_class, change) in rows {
        let fixture = PlanFixtureV1::separate_git_dir(populated_git_dir);
        let (plan, sources) = fixture.plan(GENERATION_V1, &[]);
        let captured: Vec<_> = plan
            .receipts()
            .iter()
            .map(|receipt| receipt.class)
            .collect();
        let empty_row = plan
            .coverage()
            .iter()
            .any(|row| row.class() == empty_class && row.state() == CustodyStateClassV1::Empty);
        assert!(
            !captured.is_empty() && !captured.contains(&empty_class) && empty_row,
            "{label}: {captured:?}"
        );
        let manifest = manifest_from_plan(&plan, GENERATION_V1, Vec::new());
        let unchanged = fixture.export(&manifest, planned_capability(&manifest, plan, sources));
        if !matches!(unchanged, Ok(CustodyExportOutcomeV1::Sealed(_))) {
            failures.push(format!("{label}, unchanged: {}", describe_run(&unchanged)));
            continue;
        }

        let (plan, sources) = fixture.plan(GENERATION_V1, &[]);
        let capability = planned_capability(&manifest, plan, sources);
        change(&fixture);
        let scratch = fixture.area.join("scratch-after-change");
        std::fs::create_dir(&scratch).expect("a second scratch");
        set_owner_private(&scratch);
        let result = fixture.export_to(&scratch, fixture.budgets, &manifest, capability);
        let expected = format!("{empty_class:?} was planned empty and replayed to another walk");
        let sealed = std::fs::symlink_metadata(scratch.join(CAPSULE_DIR_NAME).join(SEAL_NAME));
        if !matches!(&result, Err(CustodyExportErrorV1::SourceDrift(detail))
            if detail.contains(&expected))
            || sealed.is_ok()
        {
            failures.push(format!("{label}, changed: {}", describe_run(&result)));
        }
    }
    assert!(
        failures.is_empty(),
        "a change in the root domain without captured receipts was sealed:\n{}",
        failures.join("\n")
    );
}

/// An empty walk whose replay refuses is drift too: `HEAD.lock` created after planning parks the
/// `refs_and_head` replay as an uncontrolled writer, and the export refuses with no seal.
#[test]
fn planned_empty_04_an_empty_walk_that_no_longer_replays_is_drift() {
    let fixture = PlanFixtureV1::empty_source();
    let (plan, sources) = fixture.plan(GENERATION_V1, &[]);
    assert!(plan.receipts().is_empty());
    let manifest = manifest_from_plan(&plan, GENERATION_V1, Vec::new());
    let capability = planned_capability(&manifest, plan, sources);
    std::fs::write(fixture.git_dir.join("HEAD.lock"), b"ref: refs/heads/main\n")
        .expect("HEAD.lock created after planning");
    let result = fixture.export(&manifest, capability);
    assert!(
        matches!(&result, Err(CustodyExportErrorV1::SourceDrift(detail))
            if detail.contains("RefsAndHead was planned empty and did not replay")),
        "{}",
        describe_run(&result)
    );
    assert!(!fixture.seal_present());
}
