//! Behavioral controls for the ADR-0041 slice 2B2b2b1 coverage planner.
//!
//! Each control names the task §5 criterion it belongs to: `probe_*` (1), `ledger_*` and
//! `overlap_*` and `barrier_*` (2), `table_*` (3), `completeness_*` (4), `separate_*` (5),
//! `detector_*` (6), `skip_*` (7), `cargo_*` (8), `objects_*` (9), `error_*` and `runner_*` and
//! `class_local_*` (10), and `walks_*` (11). Every control plans a real Git repository built with
//! the operator-inventoried Git, and every probe child runs through 2B2a's runner under a test
//! route profile, as 2B2's controls do. The mutation matrix recorded in the implementation
//! handoff removes one guard at a time and names the control each removal turns red.

use super::seam::{self, PlanPointV1, PlanSeamsV1, ProbeRunV1, WalkEventV1};
use super::*;
use crate::custody_frame::{CustodyFrameDecoderV1, CustodyFrameEntryV1};
use crate::custody_git::{ExpectedGitDigestV1, GitDigestMismatchV1, RouteFactsV1};
use crate::custody_seal::{CustodyGitObjectKindV1, CustodyManifestV1};
use crate::custody_walk::seam as walk_seam;
use crate::custody_walk::{ObservationV1, PassV1};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

use CustodyCoverageClassV1 as Class;
use CustodyReasonCodeV1 as Reason;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// Every walk's entry budget when a control is not about the budget.
const ENTRY_BUDGET: u64 = 10_000;
/// The planning scratch budget when a control is not about the ledger.
const PLANNING_BUDGET: u64 = 64 * 1024 * 1024;
/// 2B2's per-entry allocation allowance, restated independently for the census.
const ENTRY_ALLOWANCE: u64 = 64 * 1024;
const GENERATION: &str = "generation-1";
const SUCCESS_DEADLINE: Duration = Duration::from_secs(60);

fn system_git() -> PathBuf {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Library/Developer/CommandLineTools/usr/bin/git",
            "/opt/git/bin/git",
            "/usr/bin/git",
        ]
    } else {
        &["/opt/git/bin/git", "/usr/bin/git"]
    };
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
        .expect("operator-inventoried Git route is available")
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    out
}

fn system_route() -> GitRouteRequestV1 {
    let route = system_git();
    let digest = ExpectedGitDigestV1::from_bytes(sha256(&fs::read(&route).unwrap()));
    GitRouteRequestV1::for_test_system(route, digest).expect("test system route")
}

/// A fixture Git: `version` answers 2.54.0, `init` and `ls-files` run the given shell, and any
/// other subcommand exits 97. The default `init` is the real Git, so the probe repository is
/// the real one and 2B2's re-measure applies unchanged.
struct FixtureRouteV1 {
    directory: TempDir,
    path: PathBuf,
}

impl FixtureRouteV1 {
    fn new(ls_files: &str) -> Self {
        Self::with_init(&format!("exec {} \"$@\"", system_git().display()), ls_files)
    }

    fn with_init(init: &str, ls_files: &str) -> Self {
        let directory = TempDir::new().expect("fixture route directory");
        let path = directory.path().join("git-fixture");
        let script = format!(
            "#!/bin/sh\ncase \"$*\" in\n*\" version\") echo 'git version 2.54.0';;\n\
             *\" init \"*) {init};;\n*\" ls-files \"*) {ls_files};;\n*) exit 97;;\nesac\n"
        );
        fs::write(&path, script).expect("write fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).expect("chmod fixture");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500))
            .expect("seal fixture anchor");
        // A fork in another test thread while the script's write descriptor was open leaves a
        // writable duplicate in that child until it execs, and exec refuses the script meanwhile
        // (`ETXTBSY`, 2B2a's known fixture race). One exec that is not refused proves no writer
        // remains, and none can appear later: the script is never opened for writing again.
        for _ in 0..400 {
            match Command::new(&path).arg("warm-up").output() {
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => break,
            }
        }
        Self { directory, path }
    }

    fn route(&self) -> GitRouteRequestV1 {
        let digest = ExpectedGitDigestV1::from_bytes(sha256(&fs::read(&self.path).unwrap()));
        GitRouteRequestV1::for_test_fixture(
            self.path.clone(),
            digest,
            self.directory.path().to_path_buf(),
        )
        .expect("fixture route")
    }
}

impl Drop for FixtureRouteV1 {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.directory.path(), fs::Permissions::from_mode(0o700));
    }
}

/// A Git command isolated from the host's configuration.
fn git_command(directory: &Path) -> Command {
    let mut command = Command::new(system_git());
    command
        .current_dir(directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory)
        .env("LANG", "C")
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
        ]);
    command
}

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = git_command(directory)
        .args(arguments)
        .output()
        .expect("run fixture Git");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 Git output")
}

/// Runs a Git command that is expected to stop in a conflicted state.
fn git_allow_failure(directory: &Path, arguments: &[&str]) {
    git_command(directory)
        .args(arguments)
        .output()
        .expect("run fixture Git");
}

/// One planned source: its repository root, its git directory, and its primary object store.
struct SourceV1 {
    _area: TempDir,
    area: PathBuf,
    worktree: PathBuf,
    git_dir: PathBuf,
}

impl SourceV1 {
    fn objects(&self) -> PathBuf {
        self.git_dir.join("objects")
    }

    fn at(&self, relative: &str) -> PathBuf {
        self.worktree.join(relative)
    }

    fn git_at(&self, relative: &str) -> PathBuf {
        self.git_dir.join(relative)
    }

    fn write(&self, relative: &str, content: &[u8]) {
        let path = self.at(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn write_git(&self, relative: &str, content: &[u8]) {
        let path = self.git_at(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn git(&self, arguments: &[&str]) -> String {
        git(&self.worktree, arguments)
    }
}

/// A non-bare repository with one committed `README`, in `format`.
fn repository(format: &str) -> SourceV1 {
    let area = TempDir::new().expect("fixture area");
    let path = area.path().canonicalize().unwrap();
    let worktree = path.join("repo");
    fs::create_dir(&worktree).unwrap();
    git(
        &worktree,
        &["init", "-q", &format!("--object-format={format}")],
    );
    fs::write(worktree.join("README"), b"coverage fixture\n").unwrap();
    git(&worktree, &["add", "README"]);
    git(&worktree, &["commit", "-q", "-m", "init"]);
    SourceV1 {
        git_dir: worktree.join(".git"),
        worktree,
        area: path,
        _area: area,
    }
}

/// An ordinary clone of a one-commit repository with a `README` and no attributes.
fn readme_clone() -> SourceV1 {
    let origin = repository("sha1");
    let worktree = origin.area.join("clone");
    git(
        &origin.area,
        &["clone", "-q", origin.worktree.to_str().unwrap(), "clone"],
    );
    SourceV1 {
        git_dir: worktree.join(".git"),
        worktree,
        area: origin.area.clone(),
        _area: origin._area,
    }
}

/// A fresh owner-private, empty planning scratch root.
fn new_scratch() -> TempDir {
    let scratch = TempDir::new().expect("scratch root");
    fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).unwrap();
    scratch
}

fn request_with(
    source: &SourceV1,
    scratch_root: &Path,
    route: GitRouteRequestV1,
) -> CustodyCoverageRequestV1 {
    CustodyCoverageRequestV1 {
        sources: CustodyCoverageSourcesV1::pin(
            &source.worktree,
            &source.git_dir,
            &source.objects(),
        )
        .expect("pin the source"),
        generation_id: GENERATION.to_owned(),
        frame_budget: CustodyFrameBudgetV1::new(1 << 20, 1 << 30).unwrap(),
        entry_budget: ENTRY_BUDGET,
        planning_budget: CustodyPlanningBudgetV1 {
            max_scratch_bytes: PLANNING_BUDGET,
        },
        scratch_root: scratch_root.to_path_buf(),
        external_evidence: NoExternalEvidenceRecordedV1::declare().into(),
        object_format: CustodyGitObjectFormatV1::Sha1,
        object_inventory: Vec::new(),
        git_route: route,
        deadline: Instant::now() + SUCCESS_DEADLINE,
    }
}

fn default_request(source: &SourceV1, scratch_root: &Path) -> CustodyCoverageRequestV1 {
    request_with(source, scratch_root, system_route())
}

/// Plans `source` with the default request into a fresh scratch.
fn plan_source(source: &SourceV1) -> CustodyCoveragePlanV1 {
    let scratch = new_scratch();
    plan_coverage_v1(&default_request(source, scratch.path())).expect("plan the source")
}

fn entry(
    class: CustodyCoverageClassV1,
    state: CustodyStateClassV1,
    reasons: &[CustodyReasonCodeV1],
) -> CustodyCoverageEntryV1 {
    CustodyCoverageEntryV1::new(class, state, reasons.to_vec(), None).expect("a valid row")
}

fn captured(class: CustodyCoverageClassV1) -> CustodyCoverageEntryV1 {
    entry(class, CustodyStateClassV1::Captured, &[])
}

fn empty(class: CustodyCoverageClassV1) -> CustodyCoverageEntryV1 {
    entry(class, CustodyStateClassV1::Empty, &[])
}

fn unresolved(
    class: CustodyCoverageClassV1,
    reasons: &[CustodyReasonCodeV1],
) -> CustodyCoverageEntryV1 {
    entry(class, CustodyStateClassV1::Unresolved, reasons)
}

fn row(plan: &CustodyCoveragePlanV1, class: CustodyCoverageClassV1) -> CustodyCoverageEntryV1 {
    assert_eq!(plan.coverage().len(), CustodyCoverageClassV1::ALL.len());
    plan.coverage()
        .iter()
        .find(|row| row.class() == class)
        .cloned()
        .expect("every class has a row")
}

fn receipt(plan: &CustodyCoveragePlanV1, class: CustodyCoverageClassV1) -> CustodyClassReceiptV1 {
    *plan
        .receipts()
        .iter()
        .find(|receipt| receipt.class == class)
        .expect("a captured class has a receipt")
}

/// The rows of an ordinary non-bare clone with nothing special: every walked class but
/// `in_progress_git_operations` and `bridge_evidence` captured, everything else empty. A
/// repository that committed locally also holds `COMMIT_EDITMSG`: see [`source_rows`].
fn ordinary_rows() -> BTreeMap<CustodyCoverageClassV1, CustodyCoverageEntryV1> {
    CustodyCoverageClassV1::ALL
        .into_iter()
        .map(|class| {
            let row = match class {
                Class::RefsAndHead
                | Class::Index
                | Class::StashAndReflogs
                | Class::GitConfigurationAndHooks
                | Class::Worktree => captured(class),
                _ => empty(class),
            };
            (class, row)
        })
        .collect()
}

fn assert_rows(
    plan: &CustodyCoveragePlanV1,
    expected: &BTreeMap<CustodyCoverageClassV1, CustodyCoverageEntryV1>,
) {
    let actual: BTreeMap<_, _> = plan
        .coverage()
        .iter()
        .map(|row| (row.class(), row.clone()))
        .collect();
    assert_eq!(&actual, expected);
    assert!(plan
        .coverage()
        .iter()
        .map(CustodyCoverageEntryV1::class)
        .eq(CustodyCoverageClassV1::ALL));
}

fn with_rows(
    changes: &[CustodyCoverageEntryV1],
) -> BTreeMap<CustodyCoverageClassV1, CustodyCoverageEntryV1> {
    let mut rows = ordinary_rows();
    for change in changes {
        rows.insert(change.class(), change.clone());
    }
    rows
}

/// [`with_rows`] for a source that committed locally, whose `COMMIT_EDITMSG` is captured in
/// `in_progress_git_operations`.
fn source_rows(
    source: &SourceV1,
    changes: &[CustodyCoverageEntryV1],
) -> BTreeMap<CustodyCoverageClassV1, CustodyCoverageEntryV1> {
    assert!(source.git_at("COMMIT_EDITMSG").is_file());
    let mut rows = with_rows(&[captured(Class::InProgressGitOperations)]);
    for change in changes {
        rows.insert(change.class(), change.clone());
    }
    rows
}

fn identity(path: &Path) -> (u64, u64) {
    let metadata = fs::symlink_metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

fn frame_budget() -> CustodyFrameBudgetV1 {
    CustodyFrameBudgetV1::new(1 << 20, 1 << 30).unwrap()
}

/// Re-walks one class with the planner's own selection into a decodable frame, and proves the
/// frame is the one the receipt describes.
fn rewalk(
    root: &Path,
    class: CustodyCoverageClassV1,
    selection: &dyn WalkSelectionV1,
    expected: Option<&CustodyClassReceiptV1>,
) -> Vec<(Vec<u8>, char)> {
    let pin = PinnedDirectoryV1::open(root, "coverage control re-walk").unwrap();
    let header = CustodyFrameHeaderV1::new(class, GENERATION).unwrap();
    let mut frame = Vec::new();
    let encoder = CustodyFrameEncoderV1::new(&mut frame, &header, frame_budget()).unwrap();
    let walked = walk_tree_v1(&pin, selection, ENTRY_BUDGET, encoder).expect("re-walk the class");
    if let Some(expected) = expected {
        assert_eq!(frame.len() as u64, expected.frame_length, "{class:?}");
        assert_eq!(sha256(&frame), expected.frame_sha256, "{class:?}");
        assert_eq!(
            walked.inventory_sha256(),
            expected.inventory_digest,
            "{class:?}"
        );
    }
    decode(&frame, &header)
}

fn decode(frame: &[u8], header: &CustodyFrameHeaderV1) -> Vec<(Vec<u8>, char)> {
    let mut decoder = CustodyFrameDecoderV1::new(frame, header, frame_budget()).unwrap();
    let mut entries = Vec::new();
    while let Some(entry) = decoder.next_entry().unwrap() {
        entries.push(match entry {
            CustodyFrameEntryV1::Directory { path, .. } => (path.as_bytes().to_vec(), 'd'),
            CustodyFrameEntryV1::Regular { path, .. } => (path.as_bytes().to_vec(), 'f'),
            CustodyFrameEntryV1::Symlink { path, .. } => (path.as_bytes().to_vec(), 'l'),
        });
    }
    entries
}

fn rewalk_worktree(
    source: &SourceV1,
    cargo_target_excluded: bool,
    expected: Option<&CustodyClassReceiptV1>,
) -> Vec<(Vec<u8>, char)> {
    let selection = WorktreeSelectionV1 {
        git_dir: identity(&source.git_dir),
        cargo_target_excluded,
    };
    rewalk(&source.worktree, Class::Worktree, &selection, expected)
}

fn rewalk_git_dir(
    source: &SourceV1,
    class: CustodyCoverageClassV1,
    expected: Option<&CustodyClassReceiptV1>,
) -> Vec<(Vec<u8>, char)> {
    rewalk(
        &source.git_dir,
        class,
        &GitDirClassSelectionV1 { class },
        expected,
    )
}

fn paths(entries: &[(Vec<u8>, char)]) -> Vec<String> {
    entries
        .iter()
        .map(|(path, _)| String::from_utf8_lossy(path).into_owned())
        .collect()
}

/// An independent recursive listing, sharing no code with the planner: every entry below `root`
/// by relative path, never following a symlink and never descending the directory `prune`.
fn listing(root: &Path, prune: Option<(u64, u64)>) -> BTreeSet<Vec<u8>> {
    fn visit(
        directory: &Path,
        prefix: &[u8],
        prune: Option<(u64, u64)>,
        out: &mut BTreeSet<Vec<u8>>,
    ) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let mut relative = prefix.to_vec();
            if !relative.is_empty() {
                relative.push(b'/');
            }
            relative.extend_from_slice(entry.file_name().as_bytes());
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            if metadata.is_dir() && prune != Some((metadata.dev(), metadata.ino())) {
                visit(&entry.path(), &relative, prune, out);
            }
            out.insert(relative);
        }
    }
    let mut out = BTreeSet::new();
    visit(root, b"", prune, &mut out);
    out
}

/// An independent census of everything below a planning scratch root: every file and directory
/// (the root excluded) at the per-entry allowance, plus every regular file's length.
fn scratch_census(root: &Path) -> u64 {
    let (mut entries, mut bytes) = (0_u64, 0_u64);
    let mut frontier = vec![root.to_path_buf()];
    while let Some(directory) = frontier.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            entries += 1;
            if metadata.is_dir() {
                frontier.push(path);
            } else {
                bytes += metadata.len();
            }
        }
    }
    bytes + entries * ENTRY_ALLOWANCE
}

fn is_empty_directory(path: &Path) -> bool {
    fs::read_dir(path).unwrap().next().is_none()
}

/// The rich non-bare clone of §5.4.
fn rich_clone() -> SourceV1 {
    let origin = repository("sha1");
    let origin_tree = &origin.worktree;
    fs::write(origin_tree.join("f"), b"a\nb\nc\n").unwrap();
    git(origin_tree, &["add", "f"]);
    git(origin_tree, &["commit", "-q", "-m", "f"]);
    git(origin_tree, &["checkout", "-q", "-b", "side"]);
    fs::write(origin_tree.join("f"), b"a\nSIDE\nc\n").unwrap();
    git(origin_tree, &["commit", "-q", "-am", "side"]);
    git(origin_tree, &["checkout", "-q", "main"]);
    fs::write(origin_tree.join("f"), b"a\nMAIN\nc\n").unwrap();
    git(origin_tree, &["commit", "-q", "-am", "main"]);

    git(
        &origin.area,
        &["clone", "-q", origin_tree.to_str().unwrap(), "clone"],
    );
    let source = SourceV1 {
        worktree: origin.area.join("clone"),
        git_dir: origin.area.join("clone/.git"),
        area: origin.area.clone(),
        _area: origin._area,
    };
    source.git(&["config", "rerere.enabled", "true"]);
    // A stash, then an ordinary side branch.
    fs::write(source.at("README"), b"coverage fixture\nstashed\n").unwrap();
    source.git(&["stash", "-q"]);
    source.git(&["branch", "-q", "side", "origin/side"]);
    // Untracked and ignored files, and a worktree file named `HEAD`.
    source.write("untracked.txt", b"untracked\n");
    source.write(".gitignore", b"*.log\n");
    source.write("build.log", b"ignored\n");
    source.write("HEAD", b"a worktree file named HEAD\n");
    // An in-progress merge with rerere, then a dirty worktree.
    git_allow_failure(&source.worktree, &["merge", "side"]);
    assert!(source.git_at("MERGE_RR").is_file());
    assert!(source.git_at("rr-cache").is_dir());
    fs::write(source.at("README"), b"coverage fixture\ndirty\n").unwrap();
    // A hook, a sparse-checkout file, and bridge evidence.
    source.write_git("hooks/pre-commit", b"#!/bin/sh\nexit 0\n");
    fs::set_permissions(
        source.git_at("hooks/pre-commit"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    source.write_git("info/sparse-checkout", b"/*\n!/unused/\n");
    source.write_git("a2a-bridge/evidence.json", b"{}\n");
    source.write_git("A2A_TASK.md", b"# task\n");
    source
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RootDomainV1 {
    RepositoryRoot,
    GitDirectoryRoot,
}

/// §5.4's multiset: the union of every class frame, keyed by `(RootDomain, path)`, plus
/// `objects/**` and the excluded `target/**`, equals the full source walk, each key once.
fn assert_complete_multiset(
    source: &SourceV1,
    plan: &CustodyCoveragePlanV1,
    cargo_target_excluded: bool,
) {
    let git_dir_identity = identity(&source.git_dir);
    let mut full = BTreeSet::new();
    for path in listing(&source.worktree, Some(git_dir_identity)) {
        full.insert((RootDomainV1::RepositoryRoot, path));
    }
    for path in listing(&source.git_dir, None) {
        full.insert((RootDomainV1::GitDirectoryRoot, path));
    }

    let mut counts: BTreeMap<(RootDomainV1, Vec<u8>), u64> = BTreeMap::new();
    for receipt in plan.receipts() {
        let (domain, entries) = if receipt.class == Class::Worktree {
            (
                RootDomainV1::RepositoryRoot,
                rewalk_worktree(source, cargo_target_excluded, Some(receipt)),
            )
        } else {
            (
                RootDomainV1::GitDirectoryRoot,
                rewalk_git_dir(source, receipt.class, Some(receipt)),
            )
        };
        for (path, _) in entries {
            *counts.entry((domain, path)).or_default() += 1;
        }
    }
    let excluded = |key: &(RootDomainV1, Vec<u8>)| match key.0 {
        RootDomainV1::GitDirectoryRoot => key.1 == b"objects" || key.1.starts_with(b"objects/"),
        RootDomainV1::RepositoryRoot => {
            cargo_target_excluded && (key.1 == b"target" || key.1.starts_with(b"target/"))
        }
    };
    for key in full.iter().filter(|key| excluded(key)) {
        *counts.entry(key.clone()).or_default() += 1;
    }
    let repeated: Vec<_> = counts.iter().filter(|(_, count)| **count != 1).collect();
    assert!(
        repeated.is_empty(),
        "keys owned more than once: {repeated:?}"
    );
    let counted: BTreeSet<_> = counts.into_keys().collect();
    let missing: Vec<_> = full.difference(&counted).collect();
    let extra: Vec<_> = counted.difference(&full).collect();
    assert!(missing.is_empty(), "entries no class owns: {missing:?}");
    assert!(
        extra.is_empty(),
        "frame entries outside the source: {extra:?}"
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn dependency(id: &str, content: &[u8]) -> CustodyDependencyV1 {
    CustodyDependencyV1::new(
        id,
        "worktree-file",
        Sha256HexV1::parse(hex(&sha256(content))).unwrap(),
        CustodyStateClassV1::Captured,
        vec![],
    )
    .unwrap()
}

/// A worktree under the `cargo-target-v1` precondition, with a populated root `target/`.
fn cargo_source() -> SourceV1 {
    let source = readme_clone();
    source.write("Cargo.toml", b"[package]\nname = \"fixture\"\n");
    source.write("Cargo.lock", b"version = 4\n");
    source.write("target/debug/build.bin", b"reproducible");
    source
}

// ---------------------------------------------------------------------------------------------
// §5.1 Probe
// ---------------------------------------------------------------------------------------------

fn tail(argv: &[std::ffi::OsString], length: usize) -> Vec<String> {
    argv[argv.len() - length..]
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn probe_01_sha1_and_sha256_indexes_reach_one_exact_argv_probe() {
    for (format, object_format, hex_length) in [
        ("sha1", CustodyGitObjectFormatV1::Sha1, 40),
        ("sha256", CustodyGitObjectFormatV1::Sha256, 64),
    ] {
        let source = repository(format);
        let scratch = new_scratch();
        let mut request = default_request(&source, scratch.path());
        request.object_format = object_format;
        let installed = seam::install(PlanSeamsV1::default());
        let plan = plan_coverage_v1(&request).expect("plan");
        let runs = installed.record().runs;

        assert_eq!(runs.len(), 2, "{format}: one init, then one probe");
        assert_eq!(runs[0].stage, ProbeStageV1::InitBare);
        assert_eq!(
            tail(&runs[0].argv, 5),
            [
                "init",
                "--bare",
                "--template=",
                &format!("--object-format={format}"),
                "index-probe.git"
            ]
        );
        assert_eq!(runs[0].git_dir, None);
        assert_eq!(
            runs[1],
            ProbeRunV1 {
                stage: ProbeStageV1::LsFilesStageZ,
                argv: runs[1].argv.clone(),
                git_dir: Some("index-probe.git".into()),
                exit_status: Some(0),
            }
        );
        assert_eq!(tail(&runs[1].argv, 3), ["ls-files", "--stage", "-z"]);
        assert_eq!(
            plan.gitlinks(),
            CustodyGitlinkEvidenceV1::Listed {
                records: 1,
                gitlinks: 0
            }
        );
        assert_eq!(row(&plan, Class::Index), captured(Class::Index));
        assert_eq!(hex_length, super::hex_length(object_format));
    }
    // The probe repository is one component; a nested operand refuses before any spawn.
    assert!(matches!(
        GitCommandV1::InitBare {
            dir: "work/index-probe.git".into(),
            object_format: GitObjectFormatV1::Sha1,
        }
        .arguments(),
        Err(CustodyGitError::InvalidCommand(_))
    ));
}

#[test]
fn probe_02_a_gitlink_without_gitmodules_parks_nested() {
    let source = repository("sha1");
    let blob = source.git(&["rev-parse", "HEAD"]).trim().to_owned();
    source.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{blob},vendored"),
    ]);
    assert!(!source.at(".gitmodules").exists());
    let plan = plan_source(&source);
    assert_eq!(
        plan.gitlinks(),
        CustodyGitlinkEvidenceV1::Listed {
            records: 2,
            gitlinks: 1
        }
    );
    assert_rows(
        &plan,
        &source_rows(
            &source,
            &[unresolved(
                Class::NestedRepositoriesAndSubmodules,
                &[Reason::DependencyUnresolved],
            )],
        ),
    );
}

#[test]
fn probe_03_a_split_index_is_probed_and_a_missing_sharedindex_makes_index_unresolved() {
    let source = repository("sha1");
    source.git(&["update-index", "--split-index"]);
    let shared: Vec<_> = fs::read_dir(&source.git_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .as_bytes()
                .starts_with(b"sharedindex.")
        })
        .collect();
    assert_eq!(shared.len(), 1);

    // With its shared index, the split index lists normally.
    let plan_with_shared = plan_source(&source);
    assert_eq!(
        plan_with_shared.gitlinks(),
        CustodyGitlinkEvidenceV1::Listed {
            records: 1,
            gitlinks: 0
        }
    );
    assert_rows(&plan_with_shared, &source_rows(&source, &[]));

    fs::remove_file(&shared[0]).unwrap();
    let plan = plan_source(&source);
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::Unresolved);
    assert_rows(
        &plan,
        &source_rows(
            &source,
            &[unresolved(Class::Index, &[Reason::ContentUnresolved])],
        ),
    );
}

#[test]
fn probe_04_an_exit_1_listing_with_empty_stdout_makes_index_unresolved() {
    let source = repository("sha1");
    let fixture = FixtureRouteV1::new("exit 1");
    let scratch = new_scratch();
    let plan = plan_coverage_v1(&request_with(&source, scratch.path(), fixture.route()))
        .expect("an exit is class-local");
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::Unresolved);
    assert_rows(
        &plan,
        &source_rows(
            &source,
            &[unresolved(Class::Index, &[Reason::ContentUnresolved])],
        ),
    );
}

/// Valid records of exactly `length` bytes: paths of up to 200 bytes, each record 51 bytes plus
/// its path.
fn listing_of_length(length: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        let left = length - bytes.len();
        let path_length = if left >= 251 + 52 { 200 } else { left - 51 };
        assert!((1..=255).contains(&path_length));
        let path = "p".repeat(path_length);
        bytes.extend_from_slice(format!("100644 {} 0\t{path}\0", "a".repeat(40)).as_bytes());
    }
    assert_eq!(bytes.len(), length);
    bytes
}

#[test]
fn probe_05_stdout_at_its_bound_succeeds_and_one_byte_more_is_unresolved() {
    let source = repository("sha1");
    let index_bytes = fs::metadata(source.git_at("index")).unwrap().len();
    let limit = usize::try_from(128 * index_bytes + 64 * 1024).unwrap();
    assert_eq!(probe_stdout_limit(index_bytes), limit);
    assert_eq!(probe_stdout_limit(u64::MAX), 256 * 1024 * 1024);

    let outputs = TempDir::new().unwrap();
    for (length, succeeds) in [(limit, true), (limit + 1, false)] {
        let output = outputs.path().join(format!("listing-{length}"));
        let listing = listing_of_length(length);
        let records = listing.iter().filter(|byte| **byte == 0).count() as u64;
        fs::write(&output, &listing).unwrap();
        let fixture = FixtureRouteV1::new(&format!("cat {}", output.display()));
        let scratch = new_scratch();
        let plan = plan_coverage_v1(&request_with(&source, scratch.path(), fixture.route()))
            .expect("a bound is class-local");
        if succeeds {
            assert_eq!(
                plan.gitlinks(),
                CustodyGitlinkEvidenceV1::Listed {
                    records,
                    gitlinks: 0
                }
            );
            assert_eq!(row(&plan, Class::Index), captured(Class::Index));
        } else {
            assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::Unresolved);
            assert_eq!(
                row(&plan, Class::Index),
                unresolved(Class::Index, &[Reason::ContentUnresolved])
            );
        }
    }
}

#[test]
fn probe_06_every_malformed_listing_shape_is_refused() {
    use IndexListingFaultV1 as Fault;
    let oid = "0123456789abcdef0123456789abcdef01234567";
    let good = format!("100644 {oid} 0\tdir/file\0160000 {oid} 3\tsub\0");
    assert_eq!(
        parse_index_listing(good.as_bytes(), 40),
        Ok(IndexListingV1 {
            records: 2,
            gitlinks: 1
        })
    );
    assert_eq!(
        parse_index_listing(b"", 40),
        Ok(IndexListingV1 {
            records: 0,
            gitlinks: 0
        })
    );
    let long_path = format!("100644 {oid} 0\t{}\0", vec!["c".repeat(255); 17].join("/"));
    let max_path = format!("100644 {oid} 0\t{}\0", {
        let mut path = vec!["c".repeat(254); 16].join("/");
        path.push('/');
        path.push_str(&"d".repeat(4096 - path.len()));
        assert_eq!(path.len(), 4096);
        path
    });
    assert_eq!(
        parse_index_listing(max_path.as_bytes(), 40).map(|listing| listing.records),
        Ok(1)
    );
    let cases: Vec<(String, Fault)> = vec![
        (format!("100644 {oid} 0\tfile"), Fault::TrailingBytes),
        (format!("100644 {oid} 0\tfile\0tail"), Fault::TrailingBytes),
        (format!("10064 {oid} 0\tfile\0"), Fault::Mode),
        (format!("1006440 {oid} 0\tfile\0"), Fault::Separator),
        (format!("100648 {oid} 0\tfile\0"), Fault::Mode),
        (format!("10064a {oid} 0\tfile\0"), Fault::Mode),
        (format!("100644\t{oid} 0\tfile\0"), Fault::Separator),
        (format!("100644 {} 0\tfile\0", &oid[..39]), Fault::ObjectId),
        (format!("100644 {oid}0 0\tfile\0"), Fault::Separator),
        (
            format!("100644 {} 0\tfile\0", oid.to_uppercase()),
            Fault::ObjectId,
        ),
        (format!("100644 {}g 0\tfile\0", &oid[..39]), Fault::ObjectId),
        (format!("100644 {oid}_0\tfile\0"), Fault::Separator),
        (format!("100644 {oid} 4\tfile\0"), Fault::Stage),
        (format!("100644 {oid} 0 file\0"), Fault::Separator),
        (format!("100644 {oid} 0\t\0"), Fault::Path),
        (format!("100644 {oid} 0\t/abs\0"), Fault::Path),
        (format!("100644 {oid} 0\ta//b\0"), Fault::Path),
        (format!("100644 {oid} 0\ta/../b\0"), Fault::Path),
        (format!("100644 {oid} 0\ta/./b\0"), Fault::Path),
        (format!("100644 {oid} 0\tdir/\0"), Fault::Path),
        (long_path, Fault::Path),
        (format!("100644 {oid} 0"), Fault::TrailingBytes),
        (format!("100644 {oid}\0"), Fault::Short),
        ("\0".to_owned(), Fault::Short),
    ];
    for (listing, fault) in cases {
        assert_eq!(
            parse_index_listing(listing.as_bytes(), 40),
            Err(fault),
            "{listing:?}"
        );
    }
    // The object id length is the source format's: a SHA-1 id under SHA-256 is refused.
    assert!(parse_index_listing(format!("100644 {oid} 0\tfile\0").as_bytes(), 64).is_err());
    let sha256_oid = "a".repeat(64);
    assert_eq!(
        parse_index_listing(format!("160000 {sha256_oid} 0\tsub\0").as_bytes(), 64),
        Ok(IndexListingV1 {
            records: 1,
            gitlinks: 1
        })
    );
}

/// A malformed listing from the probe child is refused as a whole: `index` is unresolved.
#[test]
fn probe_06_a_malformed_probe_listing_makes_index_unresolved() {
    let source = repository("sha1");
    let fixture = FixtureRouteV1::new("printf '100644 abc 0\\tfile\\000'");
    let scratch = new_scratch();
    let plan = plan_coverage_v1(&request_with(&source, scratch.path(), fixture.route()))
        .expect("a malformed listing is class-local");
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::Unresolved);
    assert_eq!(
        row(&plan, Class::Index),
        unresolved(Class::Index, &[Reason::ContentUnresolved])
    );
}

#[test]
fn probe_07_the_source_index_bytes_and_mtime_are_unchanged() {
    let source = rich_clone();
    let index = source.git_at("index");
    let before = (fs::read(&index).unwrap(), fs::metadata(&index).unwrap());
    let plan = plan_source(&source);
    assert!(matches!(
        plan.gitlinks(),
        CustodyGitlinkEvidenceV1::Listed { gitlinks: 0, .. }
    ));
    let after = (fs::read(&index).unwrap(), fs::metadata(&index).unwrap());
    assert_eq!(before.0, after.0);
    assert_eq!(before.1.mtime(), after.1.mtime());
    assert_eq!(before.1.mtime_nsec(), after.1.mtime_nsec());
    assert_eq!(before.1.ino(), after.1.ino());
}

// ---------------------------------------------------------------------------------------------
// §5.2 The planning ledger, the overlap preflight, and the pre-write barrier
// ---------------------------------------------------------------------------------------------

#[test]
fn ledger_01_the_ledger_equals_an_independent_census_of_the_planning_scratch() {
    let source = rich_clone();
    let scratch = new_scratch();
    let plan = plan_coverage_v1(&default_request(&source, scratch.path())).unwrap();
    assert_eq!(plan.scratch_bytes_used(), scratch_census(scratch.path()));
    // `work/`, `home/`, `xdg/`, the nine init entries, and the one copied index.
    let index_bytes = fs::metadata(source.git_at("index")).unwrap().len();
    let probe = scratch.path().join("work/index-probe.git");
    let init_bytes = fs::metadata(probe.join("HEAD")).unwrap().len()
        + fs::metadata(probe.join("config")).unwrap().len();
    assert_eq!(
        plan.scratch_bytes_used(),
        13 * ENTRY_ALLOWANCE + init_bytes + index_bytes
    );
    assert_eq!(
        fs::read(probe.join("index")).unwrap(),
        fs::read(source.git_at("index")).unwrap()
    );
}

#[test]
fn ledger_02_the_exact_budget_succeeds_and_one_byte_less_refuses() {
    let source = readme_clone();
    let used = plan_source(&source).scratch_bytes_used();
    let index_bytes = fs::metadata(source.git_at("index")).unwrap().len();
    // The init reservation is the peak before the copy, and the copy's charge exceeds it.
    let init_peak = 12 * ENTRY_ALLOWANCE + 4 * 1024;
    assert!(init_peak < used);

    let run = |budget: u64| {
        let scratch = new_scratch();
        let mut request = default_request(&source, scratch.path());
        request.planning_budget.max_scratch_bytes = budget;
        (plan_coverage_v1(&request), scratch)
    };

    let (exact, exact_scratch) = run(used);
    assert_eq!(exact.expect("the exact budget").scratch_bytes_used(), used);
    assert_eq!(scratch_census(exact_scratch.path()), used);

    // One byte less: the index copy's reservation refuses, after the probe was initialized.
    let (short, short_scratch) = run(used - 1);
    assert!(matches!(
        short,
        Err(CustodyCoverageErrorV1::PlanningScratchBudget(_))
    ));
    let probe = short_scratch.path().join("work/index-probe.git");
    assert!(probe.join("HEAD").is_file());
    assert!(
        !probe.join("index").exists(),
        "no byte is copied past the budget"
    );
    assert!(
        index_bytes < 4 * 1024,
        "the copy fits the reconciled init reservation's slack"
    );

    // The init reservation itself: exactly its peak admits the spawn; one byte less refuses
    // before the probe repository exists.
    let (at_init, at_init_scratch) = run(init_peak);
    assert!(matches!(
        at_init,
        Err(CustodyCoverageErrorV1::PlanningScratchBudget(_))
    ));
    assert!(at_init_scratch
        .path()
        .join("work/index-probe.git/HEAD")
        .is_file());
    let (below_init, below_init_scratch) = run(init_peak - 1);
    assert!(matches!(
        below_init,
        Err(CustodyCoverageErrorV1::PlanningScratchBudget(_))
    ));
    assert!(!below_init_scratch
        .path()
        .join("work/index-probe.git")
        .exists());
    assert!(below_init_scratch.path().join("work/home").is_dir());

    // The layout's three entries.
    let (layout, layout_scratch) = run(3 * ENTRY_ALLOWANCE - 1);
    assert!(matches!(
        layout,
        Err(CustodyCoverageErrorV1::PlanningScratchBudget(_))
    ));
    assert!(layout_scratch.path().join("work").is_dir());
    assert!(!layout_scratch.path().join("work/home").exists());
}

/// §2.2: every probe child is followed by 2B2's re-measure, so a child that writes into the
/// probe repository is refused even when it exits 0.
#[test]
fn ledger_03_a_probe_child_that_writes_is_refused_by_the_post_exit_remeasure() {
    let source = readme_clone();
    let fixture = FixtureRouteV1::new(&format!(
        ": > index-probe.git/stray; exec {} \"$@\"",
        system_git().display()
    ));
    let scratch = new_scratch();
    let outcome = plan_coverage_v1(&request_with(&source, scratch.path(), fixture.route()));
    assert!(
        matches!(
            outcome,
            Err(CustodyCoverageErrorV1::PlanningScratchBudget(_))
        ),
        "{outcome:?}"
    );
}

/// A source whose primary store names one alternate store `A` (a non-primary member of the
/// chain), plus a store `C` that contains a scratch root and is named by nothing yet.
struct ChainV1 {
    source: SourceV1,
    alternate: PathBuf,
    containing_store: PathBuf,
    contained_scratch: PathBuf,
}

fn chain_fixture() -> ChainV1 {
    let source = repository("sha1");
    let alternate_repository = source.area.join("alternate.git");
    git(
        &source.area,
        &[
            "init",
            "-q",
            "--bare",
            alternate_repository.to_str().unwrap(),
        ],
    );
    let alternate = alternate_repository.join("objects");
    source.write_git(
        "objects/info/alternates",
        format!("{}\n", alternate.display()).as_bytes(),
    );
    let containing_store = source.area.join("containing-store");
    let contained_scratch = containing_store.join("scratch");
    fs::create_dir_all(&contained_scratch).unwrap();
    fs::set_permissions(&contained_scratch, fs::Permissions::from_mode(0o700)).unwrap();
    ChainV1 {
        source,
        alternate,
        containing_store,
        contained_scratch,
    }
}

#[test]
fn overlap_01_a_scratch_inside_or_aliasing_an_alternate_store_refuses_invalid_scratch() {
    // An empty, owner-private descendant of the alternate store.
    let chain = chain_fixture();
    let descendant = chain.alternate.join("empty-scratch");
    fs::create_dir(&descendant).unwrap();
    fs::set_permissions(&descendant, fs::Permissions::from_mode(0o700)).unwrap();
    let outcome = plan_coverage_v1(&default_request(&chain.source, &descendant));
    assert!(
        matches!(outcome, Err(CustodyCoverageErrorV1::InvalidScratch(_))),
        "{outcome:?}"
    );
    assert!(is_empty_directory(&descendant), "no entry is created");

    // An identity alias: the pinned store renamed after pinning, named by its new path.
    let chain = chain_fixture();
    let request = default_request(
        &chain.source,
        &chain.alternate.with_file_name("objects-alias"),
    );
    let alias = chain.alternate.with_file_name("objects-alias");
    fs::rename(&chain.alternate, &alias).unwrap();
    let before = listing(&alias, None);
    let outcome = plan_coverage_v1(&request);
    assert!(
        matches!(outcome, Err(CustodyCoverageErrorV1::InvalidScratch(_))),
        "{outcome:?}"
    );
    assert_eq!(listing(&alias, None), before, "no entry is created");

    // The other protected members refuse the same way.
    let chain = chain_fixture();
    let inside_worktree = chain.source.at("scratch");
    fs::create_dir(&inside_worktree).unwrap();
    fs::set_permissions(&inside_worktree, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        plan_coverage_v1(&default_request(&chain.source, &inside_worktree)),
        Err(CustodyCoverageErrorV1::InvalidScratch(_))
    ));
    assert!(is_empty_directory(&inside_worktree));
}

/// §5.2's table-driven barrier regression: each row makes one change after request construction
/// and must refuse `SourceRootDrift` before any scratch entry appears.
fn barrier_row(change: impl FnOnce(&ChainV1) -> Option<PathBuf>) {
    let chain = chain_fixture();
    let own_scratch = new_scratch();
    let mut request = default_request(&chain.source, own_scratch.path());
    if let Some(contained) = change(&chain) {
        // This row plans into the scratch its rewrite made reachable.
        request.scratch_root = contained;
    }
    let outcome = plan_coverage_v1(&request);
    assert!(
        matches!(outcome, Err(CustodyCoverageErrorV1::SourceRootDrift(_))),
        "{outcome:?}"
    );
    assert!(
        is_empty_directory(&request.scratch_root),
        "no scratch entry may appear before the barrier refuses"
    );
}

#[test]
fn barrier_01_replacing_the_source_repository_refuses_before_any_write() {
    barrier_row(|chain| {
        let worktree = &chain.source.worktree;
        let old = worktree.with_file_name("repo-old");
        fs::rename(worktree, &old).unwrap();
        fs::create_dir(worktree).unwrap();
        // The git directory, and so the primary store, keep their identity and path.
        fs::rename(old.join(".git"), worktree.join(".git")).unwrap();
        None
    });
}

#[test]
fn barrier_02_replacing_the_git_directory_refuses_before_any_write() {
    barrier_row(|chain| {
        let git_dir = &chain.source.git_dir;
        let old = git_dir.with_file_name(".git-old");
        fs::rename(git_dir, &old).unwrap();
        fs::create_dir(git_dir).unwrap();
        // The primary store keeps its identity and path.
        fs::rename(old.join("objects"), git_dir.join("objects")).unwrap();
        None
    });
}

#[test]
fn barrier_03_replacing_the_primary_object_store_refuses_before_any_write() {
    barrier_row(|chain| {
        let objects = chain.source.objects();
        fs::rename(&objects, objects.with_file_name("objects-old")).unwrap();
        fs::create_dir_all(objects.join("info")).unwrap();
        // The same alternates content, so only the store's identity changed.
        fs::write(
            objects.join("info/alternates"),
            format!("{}\n", chain.alternate.display()),
        )
        .unwrap();
        None
    });
}

#[test]
fn barrier_04_replacing_a_pinned_alternate_store_refuses_before_any_write() {
    barrier_row(|chain| {
        fs::rename(
            &chain.alternate,
            chain.alternate.with_file_name("objects-old"),
        )
        .unwrap();
        fs::create_dir(&chain.alternate).unwrap();
        None
    });
}

#[test]
fn barrier_05_rewriting_the_primary_alternates_file_refuses_before_any_write() {
    barrier_row(|chain| {
        let alternates = chain.source.git_at("objects/info/alternates");
        let mut content = fs::read(&alternates).unwrap();
        content.extend_from_slice(b"# rewritten\n");
        fs::write(&alternates, content).unwrap();
        None
    });
}

#[test]
fn barrier_06_a_non_primary_alternates_rewrite_naming_the_scratch_store_refuses_before_any_write() {
    barrier_row(|chain| {
        // The overlap preflight uses the chain pinned at request construction, which does not
        // hold the containing store; only the barrier sees the rewrite.
        fs::create_dir_all(chain.alternate.join("info")).unwrap();
        fs::write(
            chain.alternate.join("info/alternates"),
            format!("{}\n", chain.containing_store.display()),
        )
        .unwrap();
        Some(chain.contained_scratch.clone())
    });
}

#[test]
fn barrier_07_an_unchanged_chain_succeeds() {
    let chain = chain_fixture();
    let scratch = new_scratch();
    let plan = plan_coverage_v1(&default_request(&chain.source, scratch.path())).expect("plan");
    assert_eq!(
        row(&plan, Class::AlternatesAndSharedStores),
        unresolved(
            Class::AlternatesAndSharedStores,
            &[Reason::DependencyUnresolved]
        )
    );
    assert!(!is_empty_directory(scratch.path()));
    // The contained scratch is usable while nothing names its store.
    let plan = plan_coverage_v1(&default_request(&chain.source, &chain.contained_scratch));
    assert!(plan.is_ok(), "{plan:?}");
}

/// Alternates evidence is read two ways, each its own guard: the git directory's
/// `objects/info/alternates` through the pin, and the alternate chain pinned from the primary
/// store. A request whose primary store is not the git directory's own separates them.
#[test]
fn alternates_01_the_alternates_file_or_the_pinned_chain_each_parks_alternates() {
    let alternates = unresolved(
        Class::AlternatesAndSharedStores,
        &[Reason::DependencyUnresolved],
    );
    let plan_with_store = |source: &SourceV1, primary: &Path| {
        let scratch = new_scratch();
        let request = CustodyCoverageRequestV1 {
            sources: CustodyCoverageSourcesV1::pin(&source.worktree, &source.git_dir, primary)
                .unwrap(),
            ..default_request(source, scratch.path())
        };
        plan_coverage_v1(&request).unwrap()
    };
    let bare = |source: &SourceV1, name: &str| {
        let path = source.area.join(name);
        git(
            &source.area,
            &["init", "-q", "--bare", path.to_str().unwrap()],
        );
        path.join("objects")
    };

    // Only the pinned chain: the primary store names an alternate; the git directory's own
    // `objects/` names none.
    let source = readme_clone();
    let primary = bare(&source, "primary.git");
    let alternate = bare(&source, "alternate.git");
    fs::write(
        primary.join("info/alternates"),
        format!("{}\n", alternate.display()),
    )
    .unwrap();
    let plan = plan_with_store(&source, &primary);
    assert_eq!(row(&plan, Class::AlternatesAndSharedStores), alternates);

    // Only the file: the git directory's `objects/` names an alternate; the primary store given
    // names none.
    let source = readme_clone();
    let primary = bare(&source, "primary.git");
    let alternate = bare(&source, "alternate.git");
    source.write_git(
        "objects/info/alternates",
        format!("# a comment\n{}\n", alternate.display()).as_bytes(),
    );
    let plan = plan_with_store(&source, &primary);
    assert_eq!(row(&plan, Class::AlternatesAndSharedStores), alternates);

    // Comments alone are no evidence.
    let source = readme_clone();
    source.write_git("objects/info/alternates", b"# only a comment\n\n");
    assert_rows(&plan_source(&source), &ordinary_rows());
}

// ---------------------------------------------------------------------------------------------
// §5.3 The class table
// ---------------------------------------------------------------------------------------------

#[test]
fn table_01_every_git_dir_row_routes_as_specified() {
    let route = |class, unresolved| GitDirRouteV1 { class, unresolved };
    let plain = |class| route(class, None);
    let mut cases: Vec<(&[u8], GitDirRouteV1)> = vec![];
    for name in [
        &b"HEAD"[..],
        b"ORIG_HEAD",
        b"FETCH_HEAD",
        b"packed-refs",
        b"refs",
    ] {
        cases.push((name, plain(Class::RefsAndHead)));
    }
    cases.push((b"logs", plain(Class::StashAndReflogs)));
    for name in [&b"index"[..], b"sharedindex.", b"sharedindex.0123abcd"] {
        cases.push((name, plain(Class::Index)));
    }
    for name in [
        &b"config"[..],
        b"config.worktree",
        b"description",
        b"hooks",
        b"info",
        b"branches",
        b"remotes",
        b"rr-cache",
        b"gc.log",
    ] {
        cases.push((name, plain(Class::GitConfigurationAndHooks)));
    }
    for name in [
        &b"MERGE_HEAD"[..],
        b"MERGE_MSG",
        b"MERGE_MODE",
        b"MERGE_RR",
        b"MERGE_AUTOSTASH",
        b"AUTO_MERGE",
        b"CHERRY_PICK_HEAD",
        b"REVERT_HEAD",
        b"REBASE_HEAD",
        b"SQUASH_MSG",
        b"COMMIT_EDITMSG",
        b"TAG_EDITMSG",
        b"BISECT_LOG",
        b"BISECT_START",
        b"BISECT_",
        b"NOTES_MERGE_REF",
        b"NOTES_MERGE_PARTIAL",
        b"NOTES_MERGE_WORKTREE",
        b"rebase-merge",
        b"rebase-apply",
        b"sequencer",
    ] {
        cases.push((name, plain(Class::InProgressGitOperations)));
    }
    for name in [&b"a2a-bridge"[..], b"A2A_TASK.md", b"A2A_"] {
        cases.push((name, plain(Class::BridgeEvidence)));
    }
    cases.push((b"objects", plain(Class::ObjectDatabase)));
    cases.push((b"worktrees", plain(Class::LinkedWorktrees)));
    cases.push((b"modules", plain(Class::NestedRepositoriesAndSubmodules)));
    cases.push((b"lfs", plain(Class::LfsAndExternalPayloads)));
    // The special rows.
    cases.push((
        b"commondir",
        route(Class::LinkedWorktrees, Some(Reason::DependencyUnresolved)),
    ));
    cases.push((
        b"shallow",
        route(Class::RefsAndHead, Some(Reason::ContentUnresolved)),
    ));
    let writer = Some(Reason::WriterUncontrolled);
    cases.push((b"index.lock", route(Class::Index, writer)));
    cases.push((b"sharedindex.ab.lock", route(Class::Index, writer)));
    cases.push((b"HEAD.lock", route(Class::RefsAndHead, writer)));
    cases.push((b"packed-refs.lock", route(Class::RefsAndHead, writer)));
    cases.push((b"shallow.lock", route(Class::RefsAndHead, writer)));
    cases.push((
        b"config.lock",
        route(Class::GitConfigurationAndHooks, writer),
    ));
    cases.push((
        b"BISECT_LOG.lock",
        route(Class::InProgressGitOperations, writer),
    ));
    cases.push((b"A2A_X.lock", route(Class::BridgeEvidence, writer)));
    cases.push((b"objects.lock", route(Class::ObjectDatabase, writer)));
    cases.push((b"commondir.lock", route(Class::LinkedWorktrees, writer)));
    cases.push((b"gc.pid", route(Class::GitConfigurationAndHooks, writer)));
    cases.push((
        b"unknown.lock",
        route(Class::GitConfigurationAndHooks, writer),
    ));
    cases.push((b".lock", route(Class::GitConfigurationAndHooks, writer)));
    let unknown = Some(Reason::ContentUnresolved);
    for name in [
        &b"mystery"[..],
        b"gitdir",
        b"reftable",
        b"head",
        b"Index",
        b"bisect_log",
        b"a2a_task",
        b"gc.pid.bak",
        b"objects2",
    ] {
        cases.push((name, route(Class::GitConfigurationAndHooks, unknown)));
    }
    for (name, expected) in cases {
        assert_eq!(
            route_git_dir_entry(name),
            expected,
            "{}",
            String::from_utf8_lossy(name)
        );
    }
}

/// Plans an ordinary clone after one change and expects the ordinary rows with `changes`.
fn special_row(change: impl FnOnce(&SourceV1), changes: &[CustodyCoverageEntryV1]) {
    let source = readme_clone();
    change(&source);
    let plan = plan_source(&source);
    assert_rows(&plan, &with_rows(changes));
}

#[test]
fn table_02_commondir_parks_linked_worktrees() {
    special_row(
        |source| source.write_git("commondir", b"..\n"),
        &[unresolved(
            Class::LinkedWorktrees,
            &[Reason::DependencyUnresolved],
        )],
    );
}

#[test]
fn table_02_shallow_makes_refs_unresolved() {
    special_row(
        |source| {
            let head = source.git(&["rev-parse", "HEAD"]);
            source.write_git("shallow", head.as_bytes());
        },
        &[unresolved(Class::RefsAndHead, &[Reason::ContentUnresolved])],
    );
}

#[test]
fn table_02_info_grafts_makes_configuration_unresolved() {
    special_row(
        |source| source.write_git("info/grafts", b"\n"),
        &[unresolved(
            Class::GitConfigurationAndHooks,
            &[Reason::ContentUnresolved],
        )],
    );
}

#[test]
fn table_02_a_top_level_lock_parks_its_stem_class() {
    special_row(
        |source| source.write_git("index.lock", b""),
        &[unresolved(Class::Index, &[Reason::WriterUncontrolled])],
    );
}

#[test]
fn table_02_gc_pid_parks_configuration() {
    special_row(
        |source| source.write_git("gc.pid", b"1 host\n"),
        &[unresolved(
            Class::GitConfigurationAndHooks,
            &[Reason::WriterUncontrolled],
        )],
    );
}

#[test]
fn table_02_a_lock_of_a_non_walked_class_parks_it() {
    special_row(
        |source| source.write_git("objects.lock", b""),
        &[unresolved(
            Class::ObjectDatabase,
            &[Reason::WriterUncontrolled],
        )],
    );
}

#[test]
fn table_02_a_nested_lock_parks_its_enclosing_class() {
    special_row(
        |source| source.write_git("refs/heads/main.lock", b""),
        &[unresolved(
            Class::RefsAndHead,
            &[Reason::WriterUncontrolled],
        )],
    );
}

#[test]
fn table_02_an_unknown_name_makes_configuration_unresolved() {
    special_row(
        |source| source.write_git("mystery", b"?"),
        &[unresolved(
            Class::GitConfigurationAndHooks,
            &[Reason::ContentUnresolved],
        )],
    );
}

#[test]
fn table_02_non_empty_worktrees_modules_and_lfs_park_their_classes() {
    special_row(
        |source| {
            source.write_git("worktrees/wt/HEAD", b"ref: refs/heads/main\n");
            source.write_git("modules/sub/HEAD", b"ref: refs/heads/main\n");
            source.write_git("lfs/objects/aa/blob", b"payload");
        },
        &[
            unresolved(Class::LinkedWorktrees, &[Reason::DependencyUnresolved]),
            unresolved(
                Class::NestedRepositoriesAndSubmodules,
                &[Reason::DependencyUnresolved],
            ),
            unresolved(
                Class::LfsAndExternalPayloads,
                &[Reason::DependencyUnresolved],
            ),
        ],
    );
    // Empty, they are owned and empty.
    special_row(
        |source| {
            for name in ["worktrees", "modules", "lfs"] {
                fs::create_dir(source.git_at(name)).unwrap();
            }
        },
        &[],
    );
}

#[test]
fn table_03_rerere_notes_merge_and_bisect_states_are_captured_in_progress() {
    // Rerere: the rich clone's conflicted merge.
    let source = rich_clone();
    let plan = plan_source(&source);
    let frame = rewalk_git_dir(
        &source,
        Class::InProgressGitOperations,
        Some(&receipt(&plan, Class::InProgressGitOperations)),
    );
    for name in [
        "MERGE_RR",
        "MERGE_HEAD",
        "MERGE_MSG",
        "MERGE_MODE",
        "AUTO_MERGE",
    ] {
        assert!(paths(&frame).contains(&name.to_owned()), "{name}");
    }

    // A conflicted notes merge.
    let notes = repository("sha1");
    notes.git(&["notes", "add", "-m", "base", "HEAD"]);
    notes.git(&["update-ref", "refs/notes/other", "refs/notes/commits"]);
    notes.git(&["notes", "add", "-f", "-m", "in commits", "HEAD"]);
    notes.git(&[
        "notes",
        "--ref=other",
        "add",
        "-f",
        "-m",
        "in other",
        "HEAD",
    ]);
    git_allow_failure(&notes.worktree, &["notes", "merge", "other"]);
    assert!(notes.git_at("NOTES_MERGE_WORKTREE").is_dir());
    let plan = plan_source(&notes);
    assert_eq!(
        row(&plan, Class::InProgressGitOperations),
        captured(Class::InProgressGitOperations)
    );
    let frame = paths(&rewalk_git_dir(
        &notes,
        Class::InProgressGitOperations,
        Some(&receipt(&plan, Class::InProgressGitOperations)),
    ));
    for name in [
        "NOTES_MERGE_PARTIAL",
        "NOTES_MERGE_REF",
        "NOTES_MERGE_WORKTREE",
    ] {
        assert!(frame.contains(&name.to_owned()), "{name}: {frame:?}");
    }

    // A bisect.
    let bisect = repository("sha1");
    bisect.write("README", b"second\n");
    bisect.git(&["commit", "-q", "-am", "second"]);
    bisect.git(&["bisect", "start"]);
    bisect.git(&["bisect", "bad"]);
    git_allow_failure(&bisect.worktree, &["bisect", "good", "HEAD~1"]);
    let plan = plan_source(&bisect);
    let frame = paths(&rewalk_git_dir(
        &bisect,
        Class::InProgressGitOperations,
        Some(&receipt(&plan, Class::InProgressGitOperations)),
    ));
    assert!(
        frame.iter().any(|name| name.starts_with("BISECT_")),
        "{frame:?}"
    );
    assert!(frame.contains(&"BISECT_LOG".to_owned()));
}

#[test]
fn table_03_linked_worktree_states_park_linked_worktrees() {
    let source = repository("sha1");
    let linked = source.area.join("linked");
    source.git(&["worktree", "add", "-q", linked.to_str().unwrap()]);

    // The main repository holds `worktrees/`.
    let main = plan_source(&source);
    assert_eq!(
        row(&main, Class::LinkedWorktrees),
        unresolved(Class::LinkedWorktrees, &[Reason::DependencyUnresolved])
    );

    // The linked worktree's own git directory holds `commondir`, and its gitfile names it.
    let linked_git_dir = fs::read_dir(source.git_at("worktrees"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let scratch = new_scratch();
    let request = CustodyCoverageRequestV1 {
        sources: CustodyCoverageSourcesV1::pin(&linked, &linked_git_dir, &source.objects())
            .unwrap(),
        ..default_request(&source, scratch.path())
    };
    let plan = plan_coverage_v1(&request).expect("plan the linked worktree");
    assert_eq!(
        row(&plan, Class::LinkedWorktrees),
        unresolved(Class::LinkedWorktrees, &[Reason::DependencyUnresolved])
    );
}

// ---------------------------------------------------------------------------------------------
// §5.4 Completeness, and §5.5 the in-worktree separate git directory
// ---------------------------------------------------------------------------------------------

#[test]
fn completeness_01_every_entry_of_a_rich_clone_is_owned_exactly_once() {
    let source = rich_clone();
    assert!(source.at("HEAD").is_file() && source.git_at("HEAD").is_file());
    assert!(source.git_at("refs/stash").is_file());
    assert!(source.git_at("logs/refs/stash").is_file());
    let plan = plan_source(&source);
    assert_rows(
        &plan,
        &with_rows(&[
            captured(Class::InProgressGitOperations),
            captured(Class::BridgeEvidence),
        ]),
    );
    assert_eq!(plan.receipts().len(), 7);
    assert_complete_multiset(&source, &plan, false);

    // Whole-subtree ownership: the stash ref with refs, its reflog with the reflogs, and the
    // sparse-checkout file with the configuration.
    let refs = paths(&rewalk_git_dir(&source, Class::RefsAndHead, None));
    assert!(refs.contains(&"refs/stash".to_owned()));
    let logs = paths(&rewalk_git_dir(&source, Class::StashAndReflogs, None));
    assert!(logs.contains(&"logs/refs/stash".to_owned()));
    let config = paths(&rewalk_git_dir(
        &source,
        Class::GitConfigurationAndHooks,
        None,
    ));
    assert!(config.contains(&"info/sparse-checkout".to_owned()));
    assert!(config.contains(&"hooks/pre-commit".to_owned()));
    let worktree = paths(&rewalk_worktree(&source, false, None));
    for name in ["HEAD", "untracked.txt", "build.log", ".gitignore", ".git"] {
        assert!(worktree.contains(&name.to_owned()), "{name}");
    }
    assert!(!worktree.iter().any(|path| path.starts_with(".git/")));
}

#[test]
fn separate_01_an_in_worktree_separate_git_dir_is_a_childless_connector() {
    let area = TempDir::new().unwrap();
    let area_path = area.path().canonicalize().unwrap();
    let worktree = area_path.join("repo");
    let separate = worktree.join(".gd");
    fs::create_dir(&worktree).unwrap();
    git(
        &area_path,
        &[
            "init",
            "-q",
            &format!("--separate-git-dir={}", separate.display()),
            worktree.to_str().unwrap(),
        ],
    );
    fs::write(worktree.join("README"), b"separate\n").unwrap();
    git(&worktree, &["add", "README"]);
    git(&worktree, &["commit", "-q", "-m", "init"]);
    assert!(worktree.join(".git").is_file());
    let source = SourceV1 {
        worktree,
        git_dir: separate,
        area: area_path,
        _area: area,
    };

    let plan = plan_source(&source);
    assert_rows(&plan, &source_rows(&source, &[]));
    let frame = rewalk_worktree(&source, false, Some(&receipt(&plan, Class::Worktree)));
    assert!(frame.contains(&(b".gd".to_vec(), 'd')), "{frame:?}");
    assert!(frame.contains(&(b".git".to_vec(), 'f')));
    assert!(!frame.iter().any(|(path, _)| path.starts_with(b".gd/")));
    assert_complete_multiset(&source, &plan, false);
}

#[test]
fn separate_02_a_gitfile_naming_another_git_dir_parks_linked_worktrees() {
    let source = readme_clone();
    let elsewhere = source.area.join("elsewhere.git");
    fs::create_dir(&elsewhere).unwrap();
    // A worktree whose `.git` gitfile names another directory than the pinned git directory.
    let worktree = source.area.join("gitfile-worktree");
    fs::create_dir(&worktree).unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", elsewhere.display()),
    )
    .unwrap();
    fs::write(worktree.join("README"), b"gitfile\n").unwrap();
    let scratch = new_scratch();
    let request = CustodyCoverageRequestV1 {
        sources: CustodyCoverageSourcesV1::pin(&worktree, &source.git_dir, &source.objects())
            .unwrap(),
        ..default_request(&source, scratch.path())
    };
    let plan = plan_coverage_v1(&request).unwrap();
    assert_eq!(
        row(&plan, Class::LinkedWorktrees),
        unresolved(Class::LinkedWorktrees, &[Reason::DependencyUnresolved])
    );
    assert_eq!(row(&plan, Class::Worktree), captured(Class::Worktree));

    // The same worktree naming the pinned git directory is ordinary.
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", source.git_dir.display()),
    )
    .unwrap();
    let scratch = new_scratch();
    let request = CustodyCoverageRequestV1 {
        sources: CustodyCoverageSourcesV1::pin(&worktree, &source.git_dir, &source.objects())
            .unwrap(),
        ..default_request(&source, scratch.path())
    };
    let plan = plan_coverage_v1(&request).unwrap();
    assert_eq!(
        row(&plan, Class::LinkedWorktrees),
        empty(Class::LinkedWorktrees)
    );
}

#[test]
fn separate_03_a_root_git_directory_other_than_the_pinned_one_parks() {
    let source = readme_clone();
    let worktree = source.area.join("foreign-worktree");
    fs::create_dir(&worktree).unwrap();
    git(&worktree, &["init", "-q"]);
    fs::write(worktree.join("README"), b"foreign\n").unwrap();
    let scratch = new_scratch();
    let request = CustodyCoverageRequestV1 {
        sources: CustodyCoverageSourcesV1::pin(&worktree, &source.git_dir, &source.objects())
            .unwrap(),
        ..default_request(&source, scratch.path())
    };
    let plan = plan_coverage_v1(&request).unwrap();
    assert_rows(
        &plan,
        &with_rows(&[
            unresolved(Class::Worktree, &[Reason::DependencyUnresolved]),
            unresolved(Class::LinkedWorktrees, &[Reason::DependencyUnresolved]),
        ]),
    );
}

/// Review round 1: a gitfile target relative to the worktree, as Git writes for
/// `--relative-paths` worktrees, resolves by its leading `..` components. A `..` behind a
/// component the target names could cross a symlink, so it is refused: Git resolves
/// `../decoy/../pinned.git` through the symlink `decoy` to another repository.
#[test]
fn separate_04_a_relative_gitfile_resolves_by_its_leading_parent_components() {
    let area = TempDir::new().unwrap();
    let area_path = area.path().canonicalize().unwrap();
    let worktree = area_path.join("wt");
    let pinned = area_path.join("pinned.git");
    fs::create_dir(&worktree).unwrap();
    git(
        &area_path,
        &[
            "init",
            "-q",
            &format!("--separate-git-dir={}", pinned.display()),
            worktree.to_str().unwrap(),
        ],
    );
    fs::write(worktree.join("README"), b"relative\n").unwrap();
    git(&worktree, &["add", "README"]);
    git(&worktree, &["commit", "-q", "-m", "init"]);
    let source = SourceV1 {
        worktree: worktree.clone(),
        git_dir: pinned.clone(),
        area: area_path.clone(),
        _area: area,
    };
    let absolute_git_dir = || {
        git(&worktree, &["rev-parse", "--absolute-git-dir"])
            .trim()
            .to_owned()
    };

    // Git itself resolves the relative gitfile to the pinned directory, and so does the plan.
    fs::write(worktree.join(".git"), b"gitdir: ../pinned.git\n").unwrap();
    assert_eq!(absolute_git_dir(), pinned.to_str().unwrap());
    assert_rows(&plan_source(&source), &source_rows(&source, &[]));

    // A relative gitfile naming another directory parks linked worktrees.
    let elsewhere = area_path.join("elsewhere.git");
    git(
        &area_path,
        &["init", "-q", "--bare", elsewhere.to_str().unwrap()],
    );
    fs::write(worktree.join(".git"), b"gitdir: ../elsewhere.git\n").unwrap();
    let linked = unresolved(Class::LinkedWorktrees, &[Reason::DependencyUnresolved]);
    assert_eq!(row(&plan_source(&source), Class::LinkedWorktrees), linked);

    // The decoy: lexically `../pinned.git`, but Git follows `decoy` to another repository.
    let other = area_path.join("other");
    fs::create_dir_all(other.join("sub")).unwrap();
    git(
        &area_path,
        &[
            "init",
            "-q",
            "--bare",
            other.join("pinned.git").to_str().unwrap(),
        ],
    );
    symlink(other.join("sub"), area_path.join("decoy")).unwrap();
    fs::write(worktree.join(".git"), b"gitdir: ../decoy/../pinned.git\n").unwrap();
    assert_eq!(
        absolute_git_dir(),
        other.join("pinned.git").to_str().unwrap()
    );
    assert_eq!(row(&plan_source(&source), Class::LinkedWorktrees), linked);

    // Git's own `--relative-paths` linked worktree gitfile resolves to its git directory.
    let main = repository("sha1");
    let linked_worktree = main.area.join("linked");
    main.git(&[
        "worktree",
        "add",
        "-q",
        "--relative-paths",
        linked_worktree.to_str().unwrap(),
    ]);
    assert_eq!(
        fs::read(linked_worktree.join(".git")).unwrap(),
        b"gitdir: ../repo/.git/worktrees/linked\n"
    );
    let sources = CustodyCoverageSourcesV1::pin(
        &linked_worktree,
        &main.git_at("worktrees/linked"),
        &main.objects(),
    )
    .unwrap();
    assert!(gitfile_names_pinned_git_dir(&sources));
}

/// The gitfile resolver, exactly: leading `..` removes components of the canonical root, and every
/// other `..` refuses.
#[test]
fn separate_05_gitfile_targets_resolve_lexically_and_exactly() {
    let root = Path::new("/w/t");
    let resolve = |target: &str| resolve_gitfile_target(root, Path::new(target));
    let some = |path: &str| Some(PathBuf::from(path));
    assert_eq!(resolve("../p.git"), some("/w/p.git"));
    assert_eq!(
        resolve("../../x/.git/worktrees/l"),
        some("/x/.git/worktrees/l")
    );
    assert_eq!(resolve("../../g"), some("/g"));
    assert_eq!(resolve(".gd"), some("/w/t/.gd"));
    assert_eq!(resolve("./.gd"), some("/w/t/.gd"));
    assert_eq!(resolve("/abs/g"), some("/abs/g"));
    assert_eq!(resolve("a/../g"), None);
    assert_eq!(resolve("../a/../g"), None);
    assert_eq!(resolve("/abs/../g"), None);
    assert_eq!(resolve("../../../g"), None, "climbs past `/`");
}

// ---------------------------------------------------------------------------------------------
// §5.6 Detectors
// ---------------------------------------------------------------------------------------------

#[test]
fn detector_01_an_excluded_target_with_lfs_attributes_neither_consumes_budget_nor_parks_lfs() {
    let source = cargo_source();
    source.write(
        "target/.gitattributes",
        b"*.bin filter=lfs diff=lfs merge=lfs -text\n",
    );
    for index in 0..2_000 {
        source.write(&format!("target/debug/deps/unit-{index}.o"), b"o");
    }
    let scratch = new_scratch();
    let mut request = default_request(&source, scratch.path());
    request.entry_budget = 600;
    let plan = plan_coverage_v1(&request).expect("plan");
    assert_eq!(
        row(&plan, Class::LfsAndExternalPayloads),
        empty(Class::LfsAndExternalPayloads)
    );
    assert_eq!(row(&plan, Class::Worktree), captured(Class::Worktree));
}

#[test]
fn detector_02_a_nested_gitattributes_with_lfs_parks_lfs() {
    special_row(
        |source| source.write("assets/.gitattributes", b"*.psd filter=lfs diff=lfs\n"),
        &[unresolved(
            Class::LfsAndExternalPayloads,
            &[Reason::DependencyUnresolved],
        )],
    );
    // Without the filter, attributes are ordinary worktree content.
    special_row(
        |source| source.write("assets/.gitattributes", b"*.psd binary\n"),
        &[],
    );
}

#[test]
fn detector_02_info_attributes_with_lfs_parks_lfs() {
    special_row(
        |source| source.write_git("info/attributes", b"*.iso filter=lfs\n"),
        &[unresolved(
            Class::LfsAndExternalPayloads,
            &[Reason::DependencyUnresolved],
        )],
    );
}

#[test]
fn detector_03_a_commit_graph_lock_parks_the_object_database() {
    special_row(
        |source| source.write_git("objects/info/commit-graph.lock", b""),
        &[unresolved(
            Class::ObjectDatabase,
            &[Reason::WriterUncontrolled],
        )],
    );
}

#[test]
fn detector_04_the_lfs_detector_prunes_every_nested_git_directory() {
    let source = readme_clone();
    source.write("vendor/.git/info/.gitattributes", b"* filter=lfs\n");
    let plan = plan_source(&source);
    assert_eq!(
        row(&plan, Class::LfsAndExternalPayloads),
        empty(Class::LfsAndExternalPayloads)
    );
    assert_eq!(
        row(&plan, Class::NestedRepositoriesAndSubmodules),
        unresolved(
            Class::NestedRepositoriesAndSubmodules,
            &[Reason::DependencyUnresolved]
        )
    );
    assert_eq!(
        row(&plan, Class::Worktree),
        unresolved(Class::Worktree, &[Reason::DependencyUnresolved])
    );
}

#[test]
fn detector_05_the_lfs_detector_prunes_the_pinned_git_directory_by_identity() {
    let area = TempDir::new().unwrap();
    let area_path = area.path().canonicalize().unwrap();
    let worktree = area_path.join("repo");
    let separate = worktree.join(".gd");
    fs::create_dir(&worktree).unwrap();
    git(
        &area_path,
        &[
            "init",
            "-q",
            &format!("--separate-git-dir={}", separate.display()),
            worktree.to_str().unwrap(),
        ],
    );
    fs::write(worktree.join("README"), b"separate\n").unwrap();
    git(&worktree, &["add", "README"]);
    git(&worktree, &["commit", "-q", "-m", "init"]);
    // Bridge evidence inside the git directory that the detector must never read.
    fs::create_dir(separate.join("a2a-bridge")).unwrap();
    fs::write(
        separate.join("a2a-bridge/.gitattributes"),
        b"* filter=lfs\n",
    )
    .unwrap();
    let source = SourceV1 {
        worktree,
        git_dir: separate,
        area: area_path,
        _area: area,
    };
    let plan = plan_source(&source);
    assert_rows(
        &plan,
        &source_rows(&source, &[captured(Class::BridgeEvidence)]),
    );
}

/// §3.3: the LFS detector's in-memory sink is bounded at 1 MiB; a detector frame past it leaves
/// the worktree's evidence incomplete, so the worktree is unresolved.
#[test]
fn detector_06_the_lfs_detector_sink_is_bounded_at_one_mebibyte() {
    special_row(
        |source| source.write(".gitattributes", &vec![b'#'; 1024 * 1024 + 1]),
        &[unresolved(Class::Worktree, &[Reason::ContentUnresolved])],
    );
    // Well within the bound, the same attributes are ordinary worktree content.
    special_row(
        |source| source.write(".gitattributes", &vec![b'#'; 512 * 1024]),
        &[],
    );
}

/// Review round 1: the object lock sentinel walks under the plan's entry budget. A loose object
/// store wider than the census and every other walk plans at exactly the sentinel's own entry
/// count, and one entry less leaves only `object_database` unresolved.
#[test]
fn detector_07_the_object_sentinel_walks_under_the_entry_budget() {
    let source = readme_clone();
    // Sixty-four loose objects, hashed from files outside the worktree.
    let blobs = source.area.join("blobs");
    fs::create_dir(&blobs).unwrap();
    let files: Vec<String> = (0..64)
        .map(|index| {
            let path = blobs.join(format!("blob-{index}"));
            fs::write(&path, format!("loose object {index}\n")).unwrap();
            path.to_str().unwrap().to_owned()
        })
        .collect();
    let mut arguments = vec!["hash-object", "-w"];
    arguments.extend(files.iter().map(String::as_str));
    source.git(&arguments);
    // The sentinel descends every directory below `objects/`, so it lists every entry there.
    let sentinel_entries = listing(&source.objects(), None).len() as u64;
    assert!(sentinel_entries > 64, "{sentinel_entries}");
    let plan_within = |entry_budget: u64| {
        let scratch = new_scratch();
        let mut request = default_request(&source, scratch.path());
        request.entry_budget = entry_budget;
        plan_coverage_v1(&request).expect("an entry budget is class-local")
    };
    assert_rows(&plan_within(sentinel_entries), &ordinary_rows());
    // The census and every other walk still fit; only the sentinel is refused.
    assert_rows(
        &plan_within(sentinel_entries - 1),
        &with_rows(&[unresolved(
            Class::ObjectDatabase,
            &[Reason::ContentUnresolved],
        )]),
    );
}

// ---------------------------------------------------------------------------------------------
// §5.7 Class-skip accounting
// ---------------------------------------------------------------------------------------------

#[test]
fn skip_01_an_ordinary_readme_clone_plans_and_detectors_are_exempt_from_the_equation() {
    let source = readme_clone();
    let plan = plan_source(&source);
    assert_rows(&plan, &ordinary_rows());
    assert_complete_multiset(&source, &plan, false);

    // Negative control: the LFS detector skips every file that is not `.gitattributes`, so the
    // class equation (no skip at all without an excluded target) would refuse its receipt.
    let pin = PinnedDirectoryV1::open(&source.worktree, "detector control").unwrap();
    let header = CustodyFrameHeaderV1::new(Class::LfsAndExternalPayloads, GENERATION).unwrap();
    let mut frame = Vec::new();
    let encoder = CustodyFrameEncoderV1::new(&mut frame, &header, frame_budget()).unwrap();
    let detector = walk_tree_v1(
        &pin,
        &LfsDetectorSelectionV1 {
            git_dir: identity(&source.git_dir),
            cargo_target_excluded: false,
        },
        ENTRY_BUDGET,
        encoder,
    )
    .unwrap();
    assert!(detector.skipped_entries() >= 2, "README and the connector");
    assert!(matches!(
        require_class_skips(Class::Worktree, 0, &detector),
        Err(CustodyCoverageErrorV1::AccountingMismatch { .. })
    ));
}

#[test]
fn skip_02_a_top_level_entry_appearing_after_the_census_is_an_accounting_mismatch() {
    let source = readme_clone();
    let orig_head = source.git_at("ORIG_HEAD");
    let head = source.git(&["rev-parse", "HEAD"]);
    let _installed = seam::install(PlanSeamsV1 {
        point: Some(Box::new(move |point: &PlanPointV1| {
            if *point == PlanPointV1::CensusTaken {
                fs::write(&orig_head, &head).unwrap();
            }
        })),
        ..PlanSeamsV1::default()
    });
    let scratch = new_scratch();
    let outcome = plan_coverage_v1(&default_request(&source, scratch.path()));
    assert!(
        matches!(
            outcome,
            Err(CustodyCoverageErrorV1::AccountingMismatch {
                class: Class::Index,
                ..
            })
        ),
        "{outcome:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// §5.8 cargo-target-v1
// ---------------------------------------------------------------------------------------------

#[test]
fn cargo_01_the_records_validate_in_a_manifest_and_bind_every_dependency() {
    let source = cargo_source();
    source.write("rust-toolchain.toml", b"[toolchain]\nchannel = \"1.94\"\n");
    let plan = plan_source(&source);
    assert_eq!(
        row(&plan, Class::ReproducibleOutputs),
        CustodyCoverageEntryV1::new(
            Class::ReproducibleOutputs,
            CustodyStateClassV1::ExcludedReproducible,
            vec![],
            Some("cargo-target-v1".into())
        )
        .unwrap()
    );
    // The exact emitted record; `content_class` is the snake-case wire name of the class.
    assert_eq!(
        serde_json::to_string(&Class::ReproducibleOutputs).unwrap(),
        "\"reproducible_outputs\""
    );
    assert_eq!(
        plan.exclusions(),
        [CustodyExclusionV1::new(
            "cargo-target-v1",
            "reproducible_outputs",
            "cargo-target-v1",
            vec![
                "cargo-toml".into(),
                "cargo-lock".into(),
                "rust-toolchain".into()
            ],
        )
        .unwrap()]
    );
    assert_eq!(
        plan.dependencies(),
        [
            dependency("cargo-lock", b"version = 4\n"),
            dependency("cargo-toml", b"[package]\nname = \"fixture\"\n"),
            dependency("rust-toolchain", b"[toolchain]\nchannel = \"1.94\"\n"),
        ]
    );
    let manifest = CustodyManifestV1::new(
        "unit",
        "run",
        "materialization",
        GENERATION,
        plan.coverage().to_vec(),
        vec![],
        vec![],
        plan.dependencies().to_vec(),
        plan.exclusions().to_vec(),
    )
    .expect("the plan's records assemble into a valid manifest");
    manifest.validate().unwrap();
    // The dependency files are captured in the worktree frame; the target is not.
    let frame = paths(&rewalk_worktree(
        &source,
        true,
        Some(&receipt(&plan, Class::Worktree)),
    ));
    for name in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"] {
        assert!(frame.contains(&name.to_owned()));
    }
    assert!(!frame.iter().any(|path| path.starts_with("target")));
    assert_complete_multiset(&source, &plan, true);

    // Changing `Cargo.lock` changes only its digest.
    source.write("Cargo.lock", b"version = 4\n# changed\n");
    let changed = plan_source(&source);
    assert_eq!(
        changed.dependencies()[0],
        dependency("cargo-lock", b"version = 4\n# changed\n")
    );
    assert_ne!(changed.dependencies()[0], plan.dependencies()[0]);
    assert_eq!(changed.dependencies()[1..], plan.dependencies()[1..]);
}

#[test]
fn cargo_02_a_regular_file_or_symlink_root_target_is_captured() {
    for make_target in [
        (|source: &SourceV1| source.write("target", b"not a directory")) as fn(&SourceV1),
        |source: &SourceV1| symlink("elsewhere", source.at("target")).unwrap(),
    ] {
        let source = readme_clone();
        source.write("Cargo.toml", b"[package]\n");
        source.write("Cargo.lock", b"version = 4\n");
        make_target(&source);
        let plan = plan_source(&source);
        assert_rows(&plan, &ordinary_rows());
        assert!(plan.exclusions().is_empty() && plan.dependencies().is_empty());
        let frame = paths(&rewalk_worktree(
            &source,
            false,
            Some(&receipt(&plan, Class::Worktree)),
        ));
        assert!(frame.contains(&"target".to_owned()));
        assert_complete_multiset(&source, &plan, false);
    }
}

#[test]
fn cargo_03_a_nested_target_is_captured() {
    let source = cargo_source();
    source.write("crates/member/target/debug/nested.bin", b"nested");
    let plan = plan_source(&source);
    assert_eq!(row(&plan, Class::Worktree), captured(Class::Worktree));
    let frame = paths(&rewalk_worktree(
        &source,
        true,
        Some(&receipt(&plan, Class::Worktree)),
    ));
    assert!(frame.contains(&"crates/member/target/debug/nested.bin".to_owned()));
    assert_complete_multiset(&source, &plan, true);
}

#[test]
fn cargo_04_without_both_cargo_files_a_root_target_is_ordinary_content() {
    let source = readme_clone();
    source.write("Cargo.toml", b"[package]\n");
    source.write("target/debug/out", b"out");
    let plan = plan_source(&source);
    assert_rows(&plan, &ordinary_rows());
    assert!(plan.exclusions().is_empty());
    let frame = paths(&rewalk_worktree(&source, false, None));
    assert!(frame.contains(&"target/debug/out".to_owned()));
}

/// Review round 1: a Cargo worktree whose git directory Git's `--separate-git-dir` placed at
/// `repo/<git_dir>`, the root `target` itself or a directory beneath it.
fn cargo_separate_git_dir_source(git_dir: &str) -> SourceV1 {
    let area = TempDir::new().unwrap();
    let area_path = area.path().canonicalize().unwrap();
    let worktree = area_path.join("repo");
    let separate = worktree.join(git_dir);
    // Git creates the separate directory, but not its parent.
    fs::create_dir_all(separate.parent().unwrap()).unwrap();
    git(
        &area_path,
        &[
            "init",
            "-q",
            &format!("--separate-git-dir={}", separate.display()),
            worktree.to_str().unwrap(),
        ],
    );
    let source = SourceV1 {
        worktree,
        git_dir: separate,
        area: area_path,
        _area: area,
    };
    source.write("README", b"separate\n");
    source.write("Cargo.toml", b"[package]\nname = \"fixture\"\n");
    source.write("Cargo.lock", b"version = 4\n");
    source.git(&["add", "README", "Cargo.toml", "Cargo.lock"]);
    source.git(&["commit", "-q", "-m", "init"]);
    assert!(source.at(".git").is_file() && source.at("target").is_dir());
    source
}

/// The plan succeeds with no `cargo-target-v1` record, the pinned git directory at `git_dir` is
/// the worktree frame's one childless connector, and every git-directory entry is owned by a
/// git-directory class alone. Returns the worktree frame's paths.
fn assert_target_git_dir_is_the_connector(source: &SourceV1, git_dir: &str) -> Vec<String> {
    let plan = plan_source(source);
    assert_rows(&plan, &source_rows(source, &[]));
    assert!(plan.exclusions().is_empty() && plan.dependencies().is_empty());
    let frame = rewalk_worktree(source, false, Some(&receipt(&plan, Class::Worktree)));
    let connector: Vec<_> = frame
        .iter()
        .filter(|(path, _)| path.as_slice() == git_dir.as_bytes())
        .collect();
    assert_eq!(
        connector,
        [&(git_dir.as_bytes().to_vec(), 'd')],
        "{frame:?}"
    );
    let beneath = format!("{git_dir}/");
    assert!(
        !frame
            .iter()
            .any(|(path, _)| path.starts_with(beneath.as_bytes())),
        "{frame:?}"
    );
    assert_complete_multiset(source, &plan, false);
    paths(&frame)
}

/// Review round 1: `--separate-git-dir=repo/target` makes the root `target` the pinned git
/// directory. It is the worktree's connector, so `cargo-target-v1` does not apply, and the valid
/// clone plans instead of failing the class-skip equation.
#[test]
fn cargo_05_a_separate_git_dir_that_is_the_root_target_is_its_connector() {
    let source = cargo_separate_git_dir_source("target");
    assert_eq!(identity(&source.at("target")), identity(&source.git_dir));
    let frame = assert_target_git_dir_is_the_connector(&source, "target");
    for name in ["Cargo.toml", "Cargo.lock", ".git"] {
        assert!(frame.contains(&name.to_owned()), "{name}");
    }
}

/// Review round 1: `--separate-git-dir=repo/target/.gd` puts the pinned git directory beneath the
/// root `target`. Excluding `target` would drop the connector and call git metadata reproducible
/// output, so `target` and its build output are captured and `.gd` is the connector.
#[test]
fn cargo_06_a_separate_git_dir_beneath_the_root_target_is_its_connector() {
    let source = cargo_separate_git_dir_source("target/.gd");
    source.write("target/debug/build.bin", b"reproducible");
    let frame = assert_target_git_dir_is_the_connector(&source, "target/.gd");
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "target",
        "target/debug/build.bin",
    ] {
        assert!(frame.contains(&name.to_owned()), "{name}");
    }
}

/// A bare source's repository root is its git directory: there is no worktree walk, no LFS
/// detector walk, and no `cargo-target-v1` policy.
#[test]
fn bare_01_a_bare_source_plans_its_git_directory_only() {
    let origin = repository("sha1");
    let bare = origin.area.join("bare.git");
    git(
        &origin.area,
        &[
            "clone",
            "-q",
            "--bare",
            origin.worktree.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let source = SourceV1 {
        worktree: bare.clone(),
        git_dir: bare,
        area: origin.area.clone(),
        _area: origin._area,
    };
    let installed = seam::install(PlanSeamsV1::default());
    let plan = plan_source(&source);
    let mut expected: BTreeMap<_, _> = CustodyCoverageClassV1::ALL
        .into_iter()
        .map(|class| (class, empty(class)))
        .collect();
    for class in [Class::RefsAndHead, Class::GitConfigurationAndHooks] {
        expected.insert(class, captured(class));
    }
    assert_rows(&plan, &expected);
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::NoIndex);
    let walks: Vec<_> = installed
        .record()
        .walks
        .into_iter()
        .filter_map(|event| match event {
            WalkEventV1::Begin { walk, .. } => Some(walk),
            WalkEventV1::End { .. } => None,
        })
        .collect();
    assert!(!walks.contains(&WalkKindV1::Class(Class::Worktree)));
    assert!(!walks.contains(&WalkKindV1::LfsDetector));
    assert!(walks.contains(&WalkKindV1::ObjectLockSentinel));
}

// ---------------------------------------------------------------------------------------------
// §5.9 The object database
// ---------------------------------------------------------------------------------------------

#[test]
fn objects_01_an_empty_repository_is_empty_and_a_one_object_repository_is_captured() {
    let area = TempDir::new().unwrap();
    let area_path = area.path().canonicalize().unwrap();
    let worktree = area_path.join("repo");
    fs::create_dir(&worktree).unwrap();
    git(&worktree, &["init", "-q"]);
    let source = SourceV1 {
        git_dir: worktree.join(".git"),
        worktree,
        area: area_path,
        _area: area,
    };
    let installed = seam::install(PlanSeamsV1::default());
    let plan = plan_source(&source);
    assert_eq!(
        row(&plan, Class::ObjectDatabase),
        empty(Class::ObjectDatabase)
    );
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::NoIndex);

    let object = git_command(&source.worktree)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(b"one object\n")?;
            child.wait_with_output()
        })
        .unwrap();
    let object = String::from_utf8(object.stdout).unwrap().trim().to_owned();
    let scratch = new_scratch();
    let mut request = default_request(&source, scratch.path());
    request.object_inventory = vec![CustodyOriginalObjectV1::new(
        CustodyGitObjectFormatV1::Sha1,
        object,
        CustodyGitObjectKindV1::Blob,
    )
    .unwrap()];
    let plan = plan_coverage_v1(&request).unwrap();
    assert_eq!(
        row(&plan, Class::ObjectDatabase),
        captured(Class::ObjectDatabase)
    );
    // No frame is requested for the object database either way.
    assert!(plan
        .receipts()
        .iter()
        .all(|receipt| receipt.class != Class::ObjectDatabase));
    assert!(installed.record().walks.iter().all(|event| !matches!(
        event,
        WalkEventV1::Begin {
            walk: WalkKindV1::Class(Class::ObjectDatabase),
            ..
        }
    )));
}

// ---------------------------------------------------------------------------------------------
// §5.10 Errors, the runner-failure table, and one class-local case per reason
// ---------------------------------------------------------------------------------------------

#[test]
fn error_01_invalid_scratch_refuses_a_non_empty_or_shared_scratch_before_any_write() {
    let source = readme_clone();
    let non_empty = new_scratch();
    fs::write(non_empty.path().join("present"), b"x").unwrap();
    assert!(matches!(
        plan_coverage_v1(&default_request(&source, non_empty.path())),
        Err(CustodyCoverageErrorV1::InvalidScratch(_))
    ));
    assert_eq!(listing(non_empty.path(), None).len(), 1);

    let shared = new_scratch();
    fs::set_permissions(shared.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        plan_coverage_v1(&default_request(&source, shared.path())),
        Err(CustodyCoverageErrorV1::InvalidScratch(_))
    ));
    assert!(is_empty_directory(shared.path()));

    let missing = source.area.join("missing-scratch");
    assert!(matches!(
        plan_coverage_v1(&default_request(&source, &missing)),
        Err(CustodyCoverageErrorV1::InvalidScratch(_))
    ));
}

#[test]
fn error_02_a_root_replaced_after_the_barrier_refuses_source_root_drift() {
    let source = readme_clone();
    let worktree = source.worktree.clone();
    let _installed = seam::install(PlanSeamsV1 {
        point: Some(Box::new(move |point: &PlanPointV1| {
            if *point == PlanPointV1::CensusTaken {
                let old = worktree.with_file_name("clone-old");
                fs::rename(&worktree, &old).unwrap();
                fs::create_dir(&worktree).unwrap();
                fs::rename(old.join(".git"), worktree.join(".git")).unwrap();
            }
        })),
        ..PlanSeamsV1::default()
    });
    let scratch = new_scratch();
    let outcome = plan_coverage_v1(&default_request(&source, scratch.path()));
    assert!(
        matches!(outcome, Err(CustodyCoverageErrorV1::SourceRootDrift(_))),
        "{outcome:?}"
    );
}

#[test]
fn error_03_an_io_failure_listing_the_pinned_git_directory_refuses_io() {
    let source = readme_clone();
    let _installed = seam::install(PlanSeamsV1 {
        census_listing_fault: true,
        ..PlanSeamsV1::default()
    });
    let scratch = new_scratch();
    assert!(matches!(
        plan_coverage_v1(&default_request(&source, scratch.path())),
        Err(CustodyCoverageErrorV1::Io(_))
    ));
    assert!(is_empty_directory(scratch.path()));
}

#[test]
fn error_04_a_census_over_the_entry_budget_leaves_every_census_class_unresolved() {
    let source = readme_clone();
    let scratch = new_scratch();
    let mut request = default_request(&source, scratch.path());
    request.entry_budget = 3;
    let plan = plan_coverage_v1(&request).expect("a budget is class-local");
    for class in CENSUS_CLASSES_V1 {
        assert_eq!(
            row(&plan, class),
            unresolved(class, &[Reason::ContentUnresolved]),
            "{class:?}"
        );
    }
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::Unresolved);
    assert!(is_empty_directory(scratch.path()));
}

type RunnerErrorV1 = (&'static str, fn() -> CustodyGitError);

fn io_error() -> std::io::Error {
    std::io::Error::other("injected")
}

fn route_facts() -> RouteFactsV1 {
    RouteFactsV1 {
        dev: 1,
        ino: 2,
        file_type: 0o100000,
        uid: 0,
        gid: 0,
        mode: 0o755,
        size: 3,
        mtime_secs: 4,
        mtime_nanos: 5,
        ctime_secs: 6,
        ctime_nanos: 7,
    }
}

/// Every `CustodyGitError` variant, by name.
fn every_runner_error() -> Vec<RunnerErrorV1> {
    let errors: [RunnerErrorV1; 18] = [
        ("InvalidRoute", || CustodyGitError::InvalidRoute("x".into())),
        ("InvalidCommand", || {
            CustodyGitError::InvalidCommand("x".into())
        }),
        ("InvalidObjectStoreRoute", || {
            CustodyGitError::InvalidObjectStoreRoute("x".into())
        }),
        ("ObjectStoreRouteRefused", || {
            CustodyGitError::ObjectStoreRouteRefused { command: "x" }
        }),
        ("RouteRefusal", || CustodyGitError::RouteRefusal("x".into())),
        ("RouteIdentityChanged", || {
            CustodyGitError::RouteIdentityChanged
        }),
        ("DigestMismatch", || CustodyGitError::DigestMismatch {
            details: Box::new(GitDigestMismatchV1 {
                expected: ExpectedGitDigestV1::from_bytes([1; 32]),
                observed: [2; 32],
                canonical_path: PathBuf::from("/x"),
                facts: route_facts(),
            }),
        }),
        ("BinaryDrift", || CustodyGitError::BinaryDrift("x".into())),
        ("UnsupportedVersion", || {
            CustodyGitError::UnsupportedVersion("x".into())
        }),
        ("Spawn", || CustodyGitError::Spawn(io_error())),
        ("Fs", || {
            CustodyGitError::Fs(FsCustodyError::Unsupported("x".into()))
        }),
        ("StdinNotRegular", || CustodyGitError::StdinNotRegular {
            observed: "fifo",
        }),
        ("StdinLimit", || CustodyGitError::StdinLimit { limit: 1 }),
        ("Stdin", || CustodyGitError::Stdin(io_error())),
        ("Timeout", || CustodyGitError::Timeout),
        ("Stream", || CustodyGitError::Stream(io_error())),
        ("StdoutLimit", || CustodyGitError::StdoutLimit { limit: 1 }),
        ("StderrLimit", || CustodyGitError::StderrLimit { limit: 1 }),
    ];
    errors.to_vec()
}

const CHILD_OUTCOMES: [&str; 4] = ["Timeout", "Stream", "StdoutLimit", "StderrLimit"];

/// The runner-failure table as a pure mapping: every stage and every variant, one outcome.
#[test]
fn runner_00_the_failure_table_maps_every_stage_and_variant_exactly_once() {
    let errors = every_runner_error();
    assert_eq!(errors.len(), 18);
    for stage in [
        ProbeStageV1::Admission,
        ProbeStageV1::InitBare,
        ProbeStageV1::LsFilesStageZ,
    ] {
        for (name, error) in &errors {
            let expected = if stage == ProbeStageV1::LsFilesStageZ && CHILD_OUTCOMES.contains(name)
            {
                RunnerOutcomeV1::IndexUnresolved
            } else {
                RunnerOutcomeV1::Infrastructure
            };
            assert_eq!(
                classify_runner_error(stage, &error()),
                expected,
                "{stage:?} {name}"
            );
        }
    }
}

/// Plans a probed clone with one runner stage refused by `error`.
fn plan_with_runner_fault(
    stage: ProbeStageV1,
    error: fn() -> CustodyGitError,
) -> PlanResult<CustodyCoveragePlanV1> {
    let source = readme_clone();
    let _installed = seam::install(PlanSeamsV1 {
        runner_fault: Some((stage, Box::new(error))),
        ..PlanSeamsV1::default()
    });
    let scratch = new_scratch();
    plan_coverage_v1(&default_request(&source, scratch.path()))
}

fn assert_infrastructure(stage: ProbeStageV1, names: &[&str]) {
    for (name, error) in every_runner_error()
        .into_iter()
        .filter(|(name, _)| names.contains(name))
    {
        let outcome = plan_with_runner_fault(stage, error);
        assert!(
            matches!(outcome, Err(CustodyCoverageErrorV1::ProbeInfrastructure(_))),
            "{stage:?} {name}: {outcome:?}"
        );
    }
}

const INFRASTRUCTURE_ERRORS: [&str; 14] = [
    "InvalidRoute",
    "InvalidCommand",
    "InvalidObjectStoreRoute",
    "ObjectStoreRouteRefused",
    "RouteRefusal",
    "RouteIdentityChanged",
    "DigestMismatch",
    "BinaryDrift",
    "UnsupportedVersion",
    "Spawn",
    "Fs",
    "StdinNotRegular",
    "StdinLimit",
    "Stdin",
];

/// Cell 1: any runner call refused by the route, command, or effect seam.
#[test]
fn runner_01_an_infrastructure_refusal_at_any_runner_call_refuses_the_plan() {
    for stage in [
        ProbeStageV1::Admission,
        ProbeStageV1::InitBare,
        ProbeStageV1::LsFilesStageZ,
    ] {
        assert_infrastructure(stage, &INFRASTRUCTURE_ERRORS);
    }
}

/// Cell 2: runner admission's own child outcomes.
#[test]
fn runner_02_an_admission_child_outcome_refuses_the_plan() {
    assert_infrastructure(ProbeStageV1::Admission, &CHILD_OUTCOMES);
}

/// Cell 3: `init --bare`'s child outcomes and its nonzero exit.
#[test]
fn runner_03_an_init_child_outcome_or_nonzero_exit_refuses_the_plan() {
    assert_infrastructure(ProbeStageV1::InitBare, &CHILD_OUTCOMES);
    let source = readme_clone();
    let fixture = FixtureRouteV1::with_init("exit 1", "exit 0");
    let scratch = new_scratch();
    let outcome = plan_coverage_v1(&request_with(&source, scratch.path(), fixture.route()));
    assert!(
        matches!(outcome, Err(CustodyCoverageErrorV1::ProbeInfrastructure(_))),
        "{outcome:?}"
    );
}

/// Cell 4: an index that cannot be opened through the pin is class-local.
#[test]
fn runner_04_an_index_copy_open_failure_makes_index_unresolved() {
    let source = readme_clone();
    let index = source.git_at("index");
    fs::rename(&index, source.git_at("index-real")).unwrap();
    symlink("index-real", &index).unwrap();
    let plan = plan_source(&source);
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::Unresolved);
    assert_rows(
        &plan,
        &with_rows(&[
            unresolved(Class::Index, &[Reason::ContentUnresolved]),
            unresolved(
                Class::GitConfigurationAndHooks,
                &[Reason::ContentUnresolved],
            ),
        ]),
    );
}

/// Cell 5: the probe listing's child outcomes, and its nonzero exit (`probe_04`), are
/// class-local.
#[test]
fn runner_05_a_listing_child_outcome_makes_index_unresolved() {
    for (name, error) in every_runner_error()
        .into_iter()
        .filter(|(name, _)| CHILD_OUTCOMES.contains(name))
    {
        let plan = plan_with_runner_fault(ProbeStageV1::LsFilesStageZ, error)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            plan.gitlinks(),
            CustodyGitlinkEvidenceV1::Unresolved,
            "{name}"
        );
        assert_eq!(
            row(&plan, Class::Index),
            unresolved(Class::Index, &[Reason::ContentUnresolved]),
            "{name}"
        );
    }
    // A real timeout, through the runner's own deadline.
    let source = readme_clone();
    let fixture = FixtureRouteV1::new("sleep 30");
    let scratch = new_scratch();
    let mut request = request_with(&source, scratch.path(), fixture.route());
    request.deadline = Instant::now() + Duration::from_secs(8);
    let plan = plan_coverage_v1(&request).expect("a timeout is class-local");
    assert_eq!(
        row(&plan, Class::Index),
        unresolved(Class::Index, &[Reason::ContentUnresolved])
    );
}

/// Cell 7: with no source index, no probe runs and nothing is written.
#[test]
fn runner_07_no_index_runs_no_probe_and_is_empty_gitlink_evidence() {
    let source = readme_clone();
    fs::remove_file(source.git_at("index")).unwrap();
    let installed = seam::install(PlanSeamsV1::default());
    let scratch = new_scratch();
    let plan = plan_coverage_v1(&default_request(&source, scratch.path())).unwrap();
    assert_eq!(plan.gitlinks(), CustodyGitlinkEvidenceV1::NoIndex);
    assert!(installed.record().runs.is_empty());
    assert!(is_empty_directory(scratch.path()));
    assert_eq!(plan.scratch_bytes_used(), 0);
    assert_eq!(row(&plan, Class::Index), empty(Class::Index));
    assert_eq!(
        row(&plan, Class::NestedRepositoriesAndSubmodules),
        empty(Class::NestedRepositoriesAndSubmodules)
    );
}

#[test]
fn class_local_01_a_special_file_is_content_unresolved() {
    special_row(
        |source| {
            UnixListener::bind(source.git_at("hooks/listener.sock")).unwrap();
        },
        &[unresolved(
            Class::GitConfigurationAndHooks,
            &[Reason::ContentUnresolved],
        )],
    );
}

#[test]
fn class_local_02_a_gitmodules_file_is_dependency_unresolved() {
    special_row(
        |source| source.write(".gitmodules", b"[submodule \"x\"]\n"),
        &[
            unresolved(Class::Worktree, &[Reason::DependencyUnresolved]),
            unresolved(
                Class::NestedRepositoriesAndSubmodules,
                &[Reason::DependencyUnresolved],
            ),
        ],
    );
}

#[test]
fn class_local_03_a_device_boundary_is_mount_boundary() {
    let source = readme_clone();
    let _walk = walk_seam::install(walk_seam::WalkSeamsV1 {
        stat: Some(Box::new(
            |_, _, path: Option<&CustodyFramePathV1>, stat: &mut ChildStatV1| {
                if path.is_some_and(|path| path.as_bytes() == b"README") {
                    stat.dev ^= 1;
                }
            },
        )),
        ..walk_seam::WalkSeamsV1::default()
    });
    let plan = plan_source(&source);
    assert_rows(
        &plan,
        &with_rows(&[unresolved(Class::Worktree, &[Reason::MountBoundary])]),
    );
}

#[test]
fn class_local_04_drift_during_a_walk_is_identity_changed() {
    let source = readme_clone();
    let readme = source.at("README");
    let mut fired = false;
    let _walk = walk_seam::install(walk_seam::WalkSeamsV1 {
        stat: Some(Box::new(
            move |pass: PassV1,
                  observation: ObservationV1,
                  path: Option<&CustodyFramePathV1>,
                  _: &mut ChildStatV1| {
                // Once, during the worktree walk's final pass, `README` is rewritten.
                if !fired
                    && pass == PassV1::Verify
                    && observation == ObservationV1::Listing
                    && path.is_some_and(|path| path.as_bytes() == b".git")
                {
                    fired = true;
                    fs::write(&readme, b"changed during the walk\n").unwrap();
                }
            },
        )),
        ..walk_seam::WalkSeamsV1::default()
    });
    let plan = plan_source(&source);
    assert_eq!(
        row(&plan, Class::Worktree),
        unresolved(Class::Worktree, &[Reason::IdentityChanged])
    );
}

#[test]
fn class_local_05_external_evidence_is_a_required_declaration() {
    let source = readme_clone();
    let scratch = new_scratch();
    let mut request = default_request(&source, scratch.path());
    request.external_evidence = ExternalEvidenceUnresolvedV1::declare().into();
    let plan = plan_coverage_v1(&request).unwrap();
    assert_rows(
        &plan,
        &with_rows(&[unresolved(
            Class::ExternalEvidence,
            &[Reason::DependencyUnresolved],
        )]),
    );
}

#[test]
fn class_local_06_a_dependency_file_changed_after_its_capture_is_identity_changed() {
    let source = cargo_source();
    let lock = source.at("Cargo.lock");
    let _installed = seam::install(PlanSeamsV1 {
        point: Some(Box::new(move |point: &PlanPointV1| {
            // The worktree walk captured `Cargo.lock`; now it no longer has the bound bytes.
            if *point == PlanPointV1::WorktreeWalked {
                fs::write(&lock, b"version = 4\n# rewritten\n").unwrap();
            }
        })),
        ..PlanSeamsV1::default()
    });
    let plan = plan_source(&source);
    assert_eq!(
        row(&plan, Class::ReproducibleOutputs),
        unresolved(Class::ReproducibleOutputs, &[Reason::IdentityChanged])
    );
    assert!(plan.exclusions().is_empty() && plan.dependencies().is_empty());
    assert_eq!(row(&plan, Class::Worktree), captured(Class::Worktree));
}

// ---------------------------------------------------------------------------------------------
// §5.11 Sequential walks
// ---------------------------------------------------------------------------------------------

#[test]
fn walks_01_run_one_at_a_time_each_with_its_own_fresh_pin() {
    let source = rich_clone();
    let installed = seam::install(PlanSeamsV1::default());
    let _plan = plan_source(&source);
    let walks = installed.record().walks;
    let mut kinds = Vec::new();
    let mut pins = BTreeSet::new();
    for pair in walks.chunks(2) {
        let [WalkEventV1::Begin { walk, pin }, WalkEventV1::End {
            walk: ended,
            pin: ended_pin,
        }] = pair
        else {
            panic!("walks overlap: {walks:?}");
        };
        assert_eq!((walk, pin), (ended, ended_pin), "{walks:?}");
        assert!(pins.insert(*pin), "two walks shared pin {pin}: {walks:?}");
        kinds.push(*walk);
    }
    assert_eq!(
        kinds,
        [
            WalkKindV1::Class(Class::RefsAndHead),
            WalkKindV1::Class(Class::Index),
            WalkKindV1::Class(Class::StashAndReflogs),
            WalkKindV1::Class(Class::InProgressGitOperations),
            WalkKindV1::Class(Class::GitConfigurationAndHooks),
            WalkKindV1::Class(Class::BridgeEvidence),
            WalkKindV1::Class(Class::Worktree),
            WalkKindV1::LfsDetector,
            WalkKindV1::ObjectLockSentinel,
        ]
    );
}
