//! Behavioral controls for the ADR-0041 slice 2B2b2a descriptor-relative walker.
//!
//! Every `control_NN_*` test belongs to task §5 criterion `NN`. The `primitive_*` tests cover the
//! four §2 `PinnedDirectoryV1` primitives, `inventory_*` pins the §4.6 digest encoding, and
//! `unsafe_*` is the new-unsafe-site inventory. Every control walks a real temporary tree.
//!
//! Controls that race the walker change the real tree from a seam hook between two walker steps.
//! Where a later layer would also notice the change, the control uses the stat seam to stand in
//! for a coarse-timestamp filesystem (mtime and ctime read as zero), and restores the tree before
//! the later layer looks. Then removing the guard under test is a wrong success, not a later
//! refusal. The mutation matrix recorded in the implementation handoff removes one guard at a time
//! and names the control each removal turns red.

use super::seam::{self, PointHookV1, PointV1, StatHookV1, WalkRecordV1, WalkSeamsV1};
use super::*;
use crate::custody_frame::{
    CustodyFrameBudgetV1, CustodyFrameDecoderV1, CustodyFrameEntryV1, CustodyFrameHeaderV1,
};
use crate::custody_seal::CustodyCoverageClassV1;
use crate::fs_custody::listed_child_names_for_test;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

use super::CustodyWalkErrorV1 as E;
use super::WalkDriftV1 as Drift;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// The emitting pass's entry budget when a control is not about the budget.
const BUDGET: u64 = 1_000;

/// A temporary directory holding the walked `root` and a sibling `aside` on the same filesystem.
/// Hooks park the objects they swap out in `aside`, outside the walked tree.
struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    aside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("root");
        let aside = temp.path().join("aside");
        fs::create_dir(&root).expect("create root");
        fs::create_dir(&aside).expect("create aside");
        Self {
            _temp: temp,
            root,
            aside,
        }
    }

    fn at(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    fn dir(&self, relative: impl AsRef<Path>, mode: u32) -> &Self {
        let path = self.at(relative);
        fs::create_dir(&path).expect("create directory");
        chmod(&path, mode);
        self
    }

    fn file(&self, relative: impl AsRef<Path>, mode: u32, content: &[u8]) -> &Self {
        let path = self.at(relative);
        fs::write(&path, content).expect("write file");
        chmod(&path, mode);
        self
    }

    fn link(&self, relative: impl AsRef<Path>, target: impl AsRef<Path>) -> &Self {
        symlink(target, self.at(relative)).expect("create symlink");
        self
    }

    /// Ages every directory and regular file, then pins the root. An aged mtime is far in the
    /// past, so any later change moves it however coarse the kernel's clock is.
    fn pin(&self) -> PinnedDirectoryV1 {
        age_tree(&self.root);
        PinnedDirectoryV1::open(&self.root, "walker control root").expect("pin root")
    }
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

fn age(path: &Path) {
    let file = fs::File::open(path).expect("open to age");
    file.set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .expect("set mtime");
}

fn age_tree(directory: &Path) {
    for entry in fs::read_dir(directory).expect("read fixture directory") {
        let path = entry.expect("fixture entry").path();
        let kind = fs::symlink_metadata(&path)
            .expect("fixture stat")
            .file_type();
        if kind.is_dir() {
            age_tree(&path);
        } else if kind.is_file() {
            age(&path);
        }
    }
    age(directory);
}

fn frame_path(bytes: &[u8]) -> CustodyFramePathV1 {
    CustodyFramePathV1::from_bytes(bytes).expect("valid frame path")
}

fn some_path(bytes: &[u8]) -> Option<CustodyFramePathV1> {
    Some(frame_path(bytes))
}

fn header() -> CustodyFrameHeaderV1 {
    CustodyFrameHeaderV1::new(CustodyCoverageClassV1::Worktree, "g1").expect("header")
}

fn frame_budget() -> CustodyFrameBudgetV1 {
    CustodyFrameBudgetV1::new(1 << 20, 1 << 30).expect("frame budget")
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A selection by exact path. Everything unnamed is `Include`.
#[derive(Clone, Default)]
struct Select {
    skip: Vec<&'static [u8]>,
    entry_only: Vec<&'static [u8]>,
    /// Parks every entry whose path starts with this prefix.
    park_prefix: Option<&'static [u8]>,
}

impl WalkSelectionV1 for Select {
    fn decide(&self, path: &CustodyFramePathV1, _stat: &ChildStatV1) -> WalkDecisionV1 {
        let bytes = path.as_bytes();
        if self
            .park_prefix
            .is_some_and(|prefix| bytes.starts_with(prefix))
        {
            return WalkDecisionV1::Park(WalkParkV1::new(
                CustodyReasonCodeV1::WriterUncontrolled,
                path.clone(),
            ));
        }
        if self.skip.contains(&bytes) {
            return WalkDecisionV1::Skip;
        }
        if self.entry_only.contains(&bytes) {
            return WalkDecisionV1::IncludeEntryOnly;
        }
        WalkDecisionV1::Include
    }
}

struct Outcome {
    result: Result<WalkReceiptV1, CustodyWalkErrorV1>,
    frame: Vec<u8>,
    record: WalkRecordV1,
}

impl Outcome {
    fn receipt(&self) -> WalkReceiptV1 {
        match &self.result {
            Ok(receipt) => *receipt,
            Err(error) => panic!("walk refused: {error:?}"),
        }
    }

    fn error(&self) -> CustodyWalkErrorV1 {
        match &self.result {
            Ok(receipt) => panic!("wrong success: {receipt:?}"),
            Err(error) => error.clone(),
        }
    }

    fn entries(&self) -> Vec<Entry> {
        decode(&self.frame).entries
    }

    fn opened(&self, pass: PassV1) -> Vec<Vec<u8>> {
        self.record
            .opened
            .iter()
            .filter(|(opened_pass, _)| *opened_pass == pass)
            .map(|(_, path)| path.as_bytes().to_vec())
            .collect()
    }
}

fn walk_full(
    root: &PinnedDirectoryV1,
    selection: &Select,
    budget: u64,
    frame_budget: CustodyFrameBudgetV1,
    seams: WalkSeamsV1,
) -> Outcome {
    let installed = seam::install(seams);
    let mut frame = Vec::new();
    let encoder = CustodyFrameEncoderV1::new(&mut frame, &header(), frame_budget).expect("encoder");
    let result = walk_tree_v1(root, selection, budget, encoder);
    let record = installed.record();
    drop(installed);
    Outcome {
        result,
        frame,
        record,
    }
}

fn walk_seamed(fixture: &Fixture, selection: &Select, seams: WalkSeamsV1) -> Outcome {
    walk_full(&fixture.pin(), selection, BUDGET, frame_budget(), seams)
}

fn walk(fixture: &Fixture, selection: &Select) -> Outcome {
    walk_seamed(fixture, selection, WalkSeamsV1::default())
}

fn with_hook(hook: impl FnMut(PassV1, &PointV1) + 'static) -> WalkSeamsV1 {
    WalkSeamsV1 {
        point: Some(Box::new(hook) as PointHookV1),
        ..WalkSeamsV1::default()
    }
}

/// Stands in for a filesystem whose timestamp granularity cannot separate the change a control
/// makes: every observation reads mtime and ctime as zero.
fn coarse_timestamps() -> Option<StatHookV1> {
    Some(Box::new(|_, _, _, stat: &mut ChildStatV1| {
        stat.mtime_ns = 0;
        stat.ctime_ns = 0;
    }))
}

fn coarse_with_hook(hook: impl FnMut(PassV1, &PointV1) + 'static) -> WalkSeamsV1 {
    WalkSeamsV1 {
        stat: coarse_timestamps(),
        ..with_hook(hook)
    }
}

/// Whether `point` is the named emitting-pass point.
fn emit_point(pass: PassV1, point: &PointV1, expected: &PointV1) -> bool {
    pass == PassV1::Emit && point == expected
}

// ---------------------------------------------------------------------------------------------
// Decoding with the 2B2b1 decoder
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Directory(Vec<u8>, u32),
    Regular(Vec<u8>, u32, Vec<u8>),
    Symlink(Vec<u8>, Vec<u8>),
}

fn dir_entry(path: &str, mode: u32) -> Entry {
    Entry::Directory(path.as_bytes().to_vec(), mode)
}

fn file_entry(path: &str, mode: u32, content: &[u8]) -> Entry {
    Entry::Regular(path.as_bytes().to_vec(), mode, content.to_vec())
}

fn link_entry(path: &str, target: &str) -> Entry {
    Entry::Symlink(path.as_bytes().to_vec(), target.as_bytes().to_vec())
}

impl Entry {
    fn path(&self) -> &[u8] {
        match self {
            Self::Directory(path, _) | Self::Regular(path, _, _) | Self::Symlink(path, _) => path,
        }
    }
}

struct Decoded {
    entries: Vec<Entry>,
    content_bytes: u64,
}

fn decode(frame: &[u8]) -> Decoded {
    let mut decoder =
        CustodyFrameDecoderV1::new(frame, &header(), frame_budget()).expect("frame header");
    let mut entries = Vec::new();
    let mut content_bytes = 0;
    while let Some(entry) = decoder.next_entry().expect("frame entry") {
        entries.push(match entry {
            CustodyFrameEntryV1::Directory { path, mode } => {
                Entry::Directory(path.as_bytes().to_vec(), mode)
            }
            CustodyFrameEntryV1::Regular {
                path,
                mode,
                length,
                mut content,
            } => {
                let mut bytes = Vec::new();
                content.read_to_end(&mut bytes).expect("frame content");
                assert_eq!(bytes.len() as u64, length);
                content_bytes += length;
                Entry::Regular(path.as_bytes().to_vec(), mode, bytes)
            }
            CustodyFrameEntryV1::Symlink { path, target } => {
                Entry::Symlink(path.as_bytes().to_vec(), target)
            }
        });
    }
    Decoded {
        entries,
        content_bytes,
    }
}

/// The receipt's frame summary against the finished sink, decoded by the 2B2b1 decoder.
fn assert_receipt_matches_frame(receipt: &WalkReceiptV1, frame: &[u8]) {
    let decoded = decode(frame);
    let summary = receipt.summary();
    assert_eq!(summary.entries(), decoded.entries.len() as u64);
    assert_eq!(summary.content_bytes(), decoded.content_bytes);
    assert_eq!(summary.frame_bytes(), frame.len() as u64);
    assert_eq!(summary.frame_sha256(), sha256(frame));
}

/// An independent statement of the canonical order: component by component, each by bytes.
fn component_order(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    left.split(|byte| *byte == b'/')
        .cmp(right.split(|byte| *byte == b'/'))
}

// ---------------------------------------------------------------------------------------------
// §2 primitives
// ---------------------------------------------------------------------------------------------

#[test]
fn primitive_list_child_names_returns_exact_bytes_and_charges_one_shared_budget() {
    let fixture = Fixture::new();
    fixture
        .file("b", 0o644, b"")
        .file("a", 0o644, b"")
        .dir("é", 0o755)
        .file("é/x", 0o644, b"")
        .dir("d", 0o755);
    let root = fixture.pin();
    let child = root
        .open_existing_child_directory(OsStr::new("é"), "control")
        .expect("open child");

    let mut remaining = 5;
    let mut names = root.list_child_names(&mut remaining, "control").unwrap();
    names.sort();
    let names: Vec<&[u8]> = names.iter().map(ChildBytesV1::as_bytes).collect();
    assert_eq!(names, [&b"a"[..], b"b", b"d", "é".as_bytes()]);
    assert_eq!(remaining, 1);

    // The same budget: the child's one name takes the last unit.
    let child_names = child.list_child_names(&mut remaining, "control").unwrap();
    assert_eq!(child_names.len(), 1);
    assert_eq!(remaining, 0);

    // At zero, the next name refuses before it is stored, and an empty directory still lists.
    let before = listed_child_names_for_test();
    assert!(matches!(
        child.list_child_names(&mut remaining, "control"),
        Err(FsCustodyError::EnumerationLimitExceeded { limit: 0, .. })
    ));
    assert_eq!(listed_child_names_for_test(), before);
    let empty = root
        .open_existing_child_directory(OsStr::new("d"), "control")
        .unwrap();
    assert!(empty
        .list_child_names(&mut remaining, "control")
        .unwrap()
        .is_empty());
}

#[test]
fn primitive_list_child_names_rewinds_the_shared_directory_offset() {
    let fixture = Fixture::new();
    fixture.file("a", 0o644, b"").file("b", 0o644, b"");
    let root = fixture.pin();
    let mut remaining = BUDGET;
    let mut first = root.list_child_names(&mut remaining, "control").unwrap();
    let mut second = root.list_child_names(&mut remaining, "control").unwrap();
    first.sort();
    second.sort();
    assert_eq!(first.len(), 2);
    assert_eq!(first, second);
}

#[test]
fn primitive_child_metadata_is_no_follow_with_the_full_mode() {
    let fixture = Fixture::new();
    fixture
        .file("setuid", 0o4755, b"abc")
        .dir("sticky", 0o1777)
        .link("link", "setuid");
    let root = fixture.pin();
    let stat = |name: &str| {
        root.child_metadata_no_follow(OsStr::new(name), "control")
            .unwrap()
    };

    let setuid = stat("setuid").expect("present");
    assert_eq!(setuid.kind, ChildKindV1::Regular);
    assert_eq!(setuid.mode, 0o4755);
    assert_eq!(setuid.size, 3);
    let sticky = stat("sticky").expect("present");
    assert_eq!(sticky.kind, ChildKindV1::Directory);
    assert_eq!(sticky.mode, 0o1777);
    let link = stat("link").expect("present");
    assert_eq!(link.kind, ChildKindV1::Symlink, "the entry, not its target");
    assert_eq!(link.size, b"setuid".len() as u64);
    assert_ne!(link.ino, setuid.ino);
    assert_eq!(stat("absent"), None);

    // The descriptor observation agrees with the listing observation of the same object.
    let opened = root
        .open_existing_child_directory(OsStr::new("sticky"), "control")
        .unwrap();
    assert_eq!(opened.current_metadata("control").unwrap(), sticky);
    let file = root
        .open_regular_file(OsStr::new("setuid"), "control")
        .unwrap();
    assert_eq!(
        ChildStatV1::from_metadata(&file.metadata().unwrap()),
        setuid
    );
}

#[test]
fn primitive_read_child_symlink_never_truncates_or_follows() {
    let fixture = Fixture::new();
    fixture
        .link("dangling", "no/such/target")
        .file("regular", 0o644, b"x");
    let root = fixture.pin();
    let read = |name: &str, max: usize| root.read_child_symlink(OsStr::new(name), max, "control");
    let target = b"no/such/target";
    assert_eq!(read("dangling", 4095).unwrap(), target);
    assert_eq!(read("dangling", target.len()).unwrap(), target);
    assert!(matches!(
        read("dangling", target.len() - 1),
        Err(FsCustodyError::Unsupported(_))
    ));
    assert!(matches!(
        read("regular", 4095),
        Err(FsCustodyError::Io(_, _))
    ));
}

#[test]
fn unsafe_inventory_pins_the_new_2b2b2a_sites_and_their_files() {
    use syn::visit::Visit as _;

    #[derive(Default)]
    struct UnsafeInventory {
        functions: Vec<String>,
        counts: BTreeMap<String, usize>,
    }

    impl<'ast> syn::visit::Visit<'ast> for UnsafeInventory {
        fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
            let name = self.functions.last().expect("unsafe belongs to a function");
            *self.counts.entry(name.clone()).or_default() += 1;
            syn::visit::visit_expr_unsafe(self, node);
        }

        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            // Macro arguments stay tokenized, so an `unsafe` block inside one is counted here.
            let count = node.tokens.to_string().matches("unsafe {").count();
            if count != 0 {
                let name = self.functions.last().expect("macro belongs to a function");
                *self.counts.entry(name.clone()).or_default() += count;
            }
            syn::visit::visit_macro(self, node);
        }

        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            self.functions.push(node.sig.ident.to_string());
            syn::visit::visit_block(self, &node.block);
            self.functions.pop();
        }

        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            self.functions.push(node.sig.ident.to_string());
            syn::visit::visit_block(self, &node.block);
            self.functions.pop();
        }
    }

    fn inventory(source: &str) -> BTreeMap<String, usize> {
        let parsed = syn::parse_file(source).expect("parse Rust source");
        let mut visitor = UnsafeInventory::default();
        visitor.visit_file(&parsed);
        visitor.counts
    }

    let mut fs_inventory = inventory(include_str!("fs_custody.rs"));
    // The new sites: the listing's `fdopendir`, `rewinddir`, errno-cleared `readdir`, and name
    // borrow, and the symlink's `readlinkat`. `child_metadata_no_follow` reuses the existing
    // `stat_child_no_follow` and `current_metadata` uses `File::metadata`, so neither adds one.
    let added = BTreeMap::from([
        ("list_child_names".to_owned(), 4),
        ("read_child_symlink".to_owned(), 1),
    ]);
    for (function, count) in &added {
        assert_eq!(fs_inventory.remove(function), Some(*count), "{function}");
    }
    // Every other `fs_custody.rs` site is the one 2B2a's A14 control pins, unchanged.
    let predecessor = BTreeMap::from([
        (
            "child_open_options_nonblocking_refuses_a_writerless_fifo_instead_of_blocking"
                .to_owned(),
            1,
        ),
        ("create_new_child_directory".to_owned(), 2),
        ("create_new_regular_child_at".to_owned(), 2),
        ("drop".to_owned(), 1),
        ("enumerate_directory_names".to_owned(), 8),
        ("errno_location".to_owned(), 2),
        ("inject_publication_rename_fault".to_owned(), 1),
        ("next_name".to_owned(), 4),
        ("open".to_owned(), 4),
        ("open_child_no_follow".to_owned(), 2),
        ("open_regular_child_for_update".to_owned(), 2),
        ("path_identity_refuses_an_unreadable_ancestor".to_owned(), 1),
        ("rename_child_no_replace".to_owned(), 2),
        ("rename_child_replacing".to_owned(), 1),
        (
            "retained_enumeration_refuses_a_non_directory_without_blocking".to_owned(),
            1,
        ),
        ("retire_captured_regular_child_v2_with".to_owned(), 1),
        ("root_command".to_owned(), 1),
        ("stat_child_no_follow".to_owned(), 2),
    ]);
    assert_eq!(fs_inventory, predecessor);

    assert_eq!(
        inventory(include_str!("custody_walk.rs")),
        BTreeMap::new(),
        "the walker itself holds no unsafe"
    );
    // The one control site builds a FIFO, which has no safe std constructor.
    assert_eq!(
        inventory(include_str!("custody_walk_tests.rs")),
        BTreeMap::from([(
            "control_03_a_fifo_and_a_socket_refuse_as_unsupported_entries".to_owned(),
            1
        )])
    );
}

// ---------------------------------------------------------------------------------------------
// §5.1 Order
// ---------------------------------------------------------------------------------------------

fn mixed_tree(fixture: &Fixture) {
    fixture
        .file("B", 0o644, b"upper")
        .dir("a", 0o755)
        .dir("a/b", 0o700)
        .file("a/b/c", 0o600, b"c")
        .link("a/b.x", "b")
        .file("a.b", 0o644, b"dot")
        .link("z", "a/b/c")
        .dir("é", 0o755)
        .file("é/ü", 0o644, b"umlaut")
        .file("日本", 0o444, b"nihon");
}

fn mixed_tree_entries() -> Vec<Entry> {
    vec![
        file_entry("B", 0o644, b"upper"),
        dir_entry("a", 0o755),
        dir_entry("a/b", 0o700),
        file_entry("a/b/c", 0o600, b"c"),
        link_entry("a/b.x", "b"),
        file_entry("a.b", 0o644, b"dot"),
        link_entry("z", "a/b/c"),
        dir_entry("é", 0o755),
        file_entry("é/ü", 0o644, b"umlaut"),
        file_entry("日本", 0o444, b"nihon"),
    ]
}

#[test]
fn control_01_the_frame_holds_exactly_the_tree_in_canonical_order() {
    let expected = mixed_tree_entries();
    assert!(
        expected
            .windows(2)
            .all(|pair| component_order(pair[0].path(), pair[1].path()).is_lt()),
        "the expected list is itself in component-wise order"
    );
    for reverse_listings in [false, true] {
        let fixture = Fixture::new();
        mixed_tree(&fixture);
        let outcome = walk_seamed(
            &fixture,
            &Select::default(),
            WalkSeamsV1 {
                reverse_listings,
                ..WalkSeamsV1::default()
            },
        );
        let receipt = outcome.receipt();
        assert_eq!(outcome.entries(), expected, "reversed: {reverse_listings}");
        assert_receipt_matches_frame(&receipt, &outcome.frame);
        assert_eq!(receipt.skipped_entries(), 0);
    }
}

// ---------------------------------------------------------------------------------------------
// §5.2 Entry kinds
// ---------------------------------------------------------------------------------------------

#[test]
fn control_02_every_entry_kind_is_captured_and_nothing_outside_the_root_is_opened() {
    let fixture = Fixture::new();
    let outside = fixture.aside.join("outside");
    fs::write(&outside, b"OUTSIDE THE ROOT").unwrap();
    let big: Vec<u8> = (0..200_003_u32).map(|index| (index % 251) as u8).collect();
    fixture
        .dir("empty", 0o755)
        .file("zero", 0o644, b"")
        .file("big", 0o644, &big)
        .file("exec", 0o755, b"#!/bin/sh\n")
        .link("absolute", &outside)
        .link("up", "..")
        .link("dangling", "missing/target");

    let outcome = walk(&fixture, &Select::default());
    let receipt = outcome.receipt();
    assert_eq!(
        outcome.entries(),
        vec![
            Entry::Symlink(
                b"absolute".to_vec(),
                outside.as_os_str().as_bytes().to_vec()
            ),
            file_entry("big", 0o644, &big),
            link_entry("dangling", "missing/target"),
            dir_entry("empty", 0o755),
            file_entry("exec", 0o755, b"#!/bin/sh\n"),
            link_entry("up", ".."),
            file_entry("zero", 0o644, b""),
        ]
    );
    assert_receipt_matches_frame(&receipt, &outcome.frame);
    assert_eq!(receipt.summary().content_bytes(), 200_003 + 10);

    // Every `openat` is one of these descriptor-relative opens of a tree entry. No symlink is
    // opened, so neither the outside file nor `..` is ever reached.
    assert_eq!(
        outcome.opened(PassV1::Emit),
        [&b"big"[..], b"empty", b"exec", b"zero"]
    );
    assert_eq!(outcome.opened(PassV1::Verify), [&b"empty"[..]]);
}

// ---------------------------------------------------------------------------------------------
// §5.3 Special files and modes
// ---------------------------------------------------------------------------------------------

#[test]
fn control_03_a_fifo_and_a_socket_refuse_as_unsupported_entries() {
    let fixture = Fixture::new();
    fixture.file("a", 0o644, b"a");
    let fifo = std::ffi::CString::new(fixture.at("fifo").as_os_str().as_bytes()).unwrap();
    let made = unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) };
    assert_eq!(made, 0);
    let outcome = walk(&fixture, &Select::default());
    assert_eq!(
        outcome.error(),
        E::UnsupportedEntry {
            path: frame_path(b"fifo"),
            kind: ChildKindV1::Special(0o010_000),
        }
    );

    let fixture = Fixture::new();
    let _listener = std::os::unix::net::UnixListener::bind(fixture.at("sock")).unwrap();
    let outcome = walk(&fixture, &Select::default());
    assert_eq!(
        outcome.error(),
        E::UnsupportedEntry {
            path: frame_path(b"sock"),
            kind: ChildKindV1::Special(0o140_000),
        }
    );
}

#[test]
fn control_03_a_setuid_file_and_a_sticky_directory_refuse_through_the_encoder() {
    let fixture = Fixture::new();
    fixture.file("setuid", 0o4755, b"x");
    assert_eq!(
        walk(&fixture, &Select::default()).error(),
        E::Frame(CustodyFrameErrorV1::UnsupportedMode)
    );

    let fixture = Fixture::new();
    fixture.dir("sticky", 0o1777);
    assert_eq!(
        walk(&fixture, &Select::default()).error(),
        E::Frame(CustodyFrameErrorV1::UnsupportedMode)
    );
}

// ---------------------------------------------------------------------------------------------
// §5.4 Selection
// ---------------------------------------------------------------------------------------------

#[test]
fn control_04_skip_omits_and_counts_and_never_descends() {
    let fixture = Fixture::new();
    fixture
        .file("kept", 0o644, b"k")
        .dir("skipped", 0o755)
        .file("skipped/inner", 0o644, b"i")
        .file("skipped-file", 0o644, b"s");
    let selection = Select {
        skip: vec![b"skipped", b"skipped-file"],
        ..Select::default()
    };
    let outcome = walk(&fixture, &selection);
    let receipt = outcome.receipt();
    assert_eq!(outcome.entries(), vec![file_entry("kept", 0o644, b"k")]);
    assert_eq!(receipt.skipped_entries(), 2);
    assert_eq!(outcome.opened(PassV1::Emit), [&b"kept"[..]]);
    assert!(outcome.opened(PassV1::Verify).is_empty());

    // A skipped special file is the caller's to account for, so it does not refuse.
    let fixture = Fixture::new();
    let _listener = std::os::unix::net::UnixListener::bind(fixture.at("sock")).unwrap();
    let selection = Select {
        skip: vec![b"sock"],
        ..Select::default()
    };
    assert_eq!(walk(&fixture, &selection).receipt().skipped_entries(), 1);
}

#[test]
fn control_04_include_entry_only_emits_a_childless_directory_it_never_opens() {
    let fixture = Fixture::new();
    fixture
        .dir(".git", 0o750)
        .file(".git/HEAD", 0o644, b"ref: refs/heads/main\n")
        .dir(".git/objects", 0o755)
        .file("tracked", 0o644, b"t");
    let selection = Select {
        entry_only: vec![b".git"],
        ..Select::default()
    };
    let outcome = walk(&fixture, &selection);
    outcome.receipt();
    assert_eq!(
        outcome.entries(),
        vec![dir_entry(".git", 0o750), file_entry("tracked", 0o644, b"t")]
    );
    assert_eq!(outcome.opened(PassV1::Emit), [&b"tracked"[..]]);
    assert!(outcome.opened(PassV1::Verify).is_empty());
}

#[test]
fn control_04_park_refuses_with_the_callers_reason() {
    let fixture = Fixture::new();
    fixture.file("a", 0o644, b"a").dir("p", 0o755);
    let selection = Select {
        park_prefix: Some(b"p"),
        ..Select::default()
    };
    assert_eq!(
        walk(&fixture, &selection).error(),
        E::Park(WalkParkV1::new(
            CustodyReasonCodeV1::WriterUncontrolled,
            frame_path(b"p")
        ))
    );
}

// ---------------------------------------------------------------------------------------------
// §5.5 Device boundary
// ---------------------------------------------------------------------------------------------

/// Stands in for a mount at `mounted`: every observation of that one entry reports another
/// device, as a real mount point's would. Its descendants keep the root's device.
fn mounted_at(mounted: &'static [u8]) -> Option<StatHookV1> {
    Some(Box::new(move |_, _, path, stat: &mut ChildStatV1| {
        if path.is_some_and(|path| path.as_bytes() == mounted) {
            stat.dev = stat.dev.wrapping_add(1);
        }
    }))
}

fn device_tree(fixture: &Fixture) {
    fixture
        .dir("sub", 0o755)
        .file("sub/inner", 0o644, b"i")
        .file("file", 0o644, b"f")
        .link("link", "file");
}

#[test]
fn control_05_a_directory_on_another_device_refuses_mount_boundary() {
    let fixture = Fixture::new();
    device_tree(&fixture);
    let seams = WalkSeamsV1 {
        stat: mounted_at(b"sub"),
        ..WalkSeamsV1::default()
    };
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::MountBoundary {
            path: frame_path(b"sub")
        }
    );
}

#[test]
fn control_05_a_regular_file_on_another_device_refuses_mount_boundary() {
    let fixture = Fixture::new();
    device_tree(&fixture);
    let seams = WalkSeamsV1 {
        stat: mounted_at(b"file"),
        ..WalkSeamsV1::default()
    };
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::MountBoundary {
            path: frame_path(b"file")
        }
    );
}

#[test]
fn control_05_a_symlink_on_another_device_refuses_mount_boundary() {
    let fixture = Fixture::new();
    device_tree(&fixture);
    let seams = WalkSeamsV1 {
        stat: mounted_at(b"link"),
        ..WalkSeamsV1::default()
    };
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::MountBoundary {
            path: frame_path(b"link")
        }
    );
}

#[test]
fn control_05_every_entry_is_checked_even_a_skipped_one() {
    let fixture = Fixture::new();
    device_tree(&fixture);
    let seams = WalkSeamsV1 {
        stat: mounted_at(b"sub"),
        ..WalkSeamsV1::default()
    };
    let selection = Select {
        skip: vec![b"sub"],
        ..Select::default()
    };
    assert_eq!(
        walk_seamed(&fixture, &selection, seams).error(),
        E::MountBoundary {
            path: frame_path(b"sub")
        }
    );
}

#[test]
fn control_05_a_file_whose_opened_device_differs_from_its_listing_refuses() {
    let fixture = Fixture::new();
    device_tree(&fixture);
    let seams = WalkSeamsV1 {
        stat: Some(Box::new(|_, observation, path, stat: &mut ChildStatV1| {
            if observation == ObservationV1::Opened
                && path.is_some_and(|path| path.as_bytes() == b"file")
            {
                stat.dev = stat.dev.wrapping_add(1);
            }
        })),
        ..WalkSeamsV1::default()
    };
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"file"),
            detail: Drift::OpenedIdentity
        }
    );
}

// ---------------------------------------------------------------------------------------------
// §5.6 Drift
// ---------------------------------------------------------------------------------------------

/// The lineage round 2 #1 regression: an earlier sibling is rewritten in place, after its own
/// post-read check, while a later sibling is being read. Its name, kind, device, inode, and its
/// directory's metadata are all unchanged, so only the final pass can see it.
#[test]
fn control_06_an_earlier_sibling_edited_during_a_later_read_is_caught_by_the_final_pass() {
    let fixture = Fixture::new();
    fixture
        .file("a", 0o644, b"alpha")
        .file("b", 0o644, b"bravo");
    let earlier = fixture.at("a");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Reading(frame_path(b"b"))) {
            let mut file = OpenOptions::new().write(true).open(&earlier).unwrap();
            file.write_all(b"ALPHA").unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: None,
            detail: Drift::FinalVerification
        }
    );
}

fn chmod_tree(fixture: &Fixture) {
    fixture
        .dir("d", 0o755)
        .file("d/x", 0o644, b"x")
        .file("e", 0o644, b"e");
}

#[test]
fn control_06_a_directory_chmodded_between_listing_and_descent_refuses() {
    let fixture = Fixture::new();
    chmod_tree(&fixture);
    let directory = fixture.at("d");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"d"))) {
            chmod(&directory, 0o700);
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"d"),
            detail: Drift::DescentMode
        }
    );
}

/// The chmod is undone as soon as the opened directory has been checked, and timestamps are too
/// coarse to show either change, so only the descent mode comparison sees the transient mode.
#[test]
fn control_06_a_transient_chmod_at_descent_refuses_on_a_coarse_timestamp_filesystem() {
    let fixture = Fixture::new();
    chmod_tree(&fixture);
    let directory = fixture.at("d");
    let seams = coarse_with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"d"))) {
            chmod(&directory, 0o700);
        }
        if emit_point(pass, point, &PointV1::Listed(some_path(b"d"))) {
            chmod(&directory, 0o755);
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"d"),
            detail: Drift::DescentMode
        }
    );
}

/// The review's #5 regression: a name added to a directory whose inode is unchanged.
#[test]
fn control_06_a_file_created_after_its_directory_was_listed_refuses() {
    let fixture = Fixture::new();
    fixture.dir("d", 0o755).file("d/x", 0o644, b"x");
    let created = fixture.at("d/new");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Listed(some_path(b"d"))) {
            fs::write(&created, b"new").unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"d"),
            detail: Drift::ChildNames
        }
    );
}

/// The added name is gone again before the final pass, and timestamps are too coarse to show it,
/// so only the post-subtree name-set comparison sees it.
#[test]
fn control_06_a_transient_file_created_after_listing_refuses_on_a_coarse_timestamp_filesystem() {
    let fixture = Fixture::new();
    fixture.file("a", 0o644, b"a");
    let created = fixture.at("transient");
    let seams = coarse_with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Listed(None)) {
            fs::write(&created, b"t").unwrap();
        }
        if emit_point(pass, point, &PointV1::Verifying) {
            fs::remove_file(&created).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: None,
            detail: Drift::ChildNames
        }
    );
}

fn deletion_tree(fixture: &Fixture) {
    fixture
        .file("a", 0o644, b"a")
        .file("b", 0o644, b"b")
        .file("c", 0o644, b"c");
}

#[test]
fn control_06_a_file_deleted_after_listing_refuses() {
    let fixture = Fixture::new();
    deletion_tree(&fixture);
    let deleted = fixture.at("b");
    let seams = coarse_with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Listed(None)) {
            fs::remove_file(&deleted).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"b"),
            detail: Drift::Vanished
        }
    );
}

#[test]
fn control_06_a_file_deleted_between_stat_and_open_refuses() {
    let fixture = Fixture::new();
    deletion_tree(&fixture);
    let deleted = fixture.at("b");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"b"))) {
            fs::remove_file(&deleted).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"b"),
            detail: Drift::Vanished
        }
    );
}

#[test]
fn control_06_a_file_deleted_after_it_was_emitted_refuses() {
    let fixture = Fixture::new();
    deletion_tree(&fixture);
    let deleted = fixture.at("b");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Subtree(None)) {
            fs::remove_file(&deleted).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: None,
            detail: Drift::ChildNames
        }
    );
}

#[test]
fn control_06_a_directory_replaced_between_listing_and_descent_refuses() {
    let fixture = Fixture::new();
    chmod_tree(&fixture);
    let directory = fixture.at("d");
    let saved = fixture.aside.join("d");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"d"))) {
            fs::rename(&directory, &saved).unwrap();
            fs::create_dir(&directory).unwrap();
            chmod(&directory, 0o755);
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"d"),
            detail: Drift::DescentIdentity
        }
    );
}

#[test]
fn control_06_a_directory_replaced_by_a_file_before_descent_refuses() {
    let fixture = Fixture::new();
    chmod_tree(&fixture);
    let directory = fixture.at("d");
    let saved = fixture.aside.join("d");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"d"))) {
            fs::rename(&directory, &saved).unwrap();
            fs::write(&directory, b"now a file").unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"d"),
            detail: Drift::Replaced
        }
    );
}

#[test]
fn control_06_a_file_whose_size_changes_while_it_is_read_refuses() {
    let fixture = Fixture::new();
    fixture.file("f", 0o644, b"content").file("g", 0o644, b"g");
    let growing = fixture.at("f");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Reading(frame_path(b"f"))) {
            let mut file = OpenOptions::new().append(true).open(&growing).unwrap();
            file.write_all(b" and more").unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"f"),
            detail: Drift::DuringRead
        }
    );
}

fn replace_file(path: &Path, staging: &Path, content: &[u8]) {
    fs::write(staging, content).unwrap();
    chmod(staging, 0o644);
    fs::rename(staging, path).unwrap();
}

#[test]
fn control_06_a_file_replaced_between_stat_and_open_refuses() {
    let fixture = Fixture::new();
    fixture.file("f", 0o644, b"original").file("g", 0o644, b"g");
    let target = fixture.at("f");
    let staging = fixture.aside.join("staging");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"f"))) {
            replace_file(&target, &staging, b"replaced");
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"f"),
            detail: Drift::OpenedIdentity
        }
    );
}

/// A same-size impostor is opened, and the original is back before any later check, with
/// timestamps too coarse to show either swap. Only the opened-descriptor identity check sees
/// that the bytes being read are not the listed file's.
#[test]
fn control_06_a_file_swapped_only_while_it_is_opened_refuses_on_a_coarse_timestamp_filesystem() {
    let fixture = Fixture::new();
    fixture.file("f", 0o644, b"original").file("g", 0o644, b"g");
    let target = fixture.at("f");
    let saved = fixture.aside.join("f");
    let impostor = fixture.aside.join("impostor");
    let seams = coarse_with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"f"))) {
            fs::rename(&target, &saved).unwrap();
            fs::write(&target, b"IMPOSTOR").unwrap();
            chmod(&target, 0o644);
        }
        if emit_point(pass, point, &PointV1::Opened(frame_path(b"f"))) {
            fs::rename(&target, &impostor).unwrap();
            fs::rename(&saved, &target).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"f"),
            detail: Drift::OpenedIdentity
        }
    );
}

#[test]
fn control_06_a_file_replaced_by_a_symlink_before_open_refuses() {
    let fixture = Fixture::new();
    fixture.file("f", 0o644, b"original").file("g", 0o644, b"g");
    let target = fixture.at("f");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"f"))) {
            fs::remove_file(&target).unwrap();
            symlink("g", &target).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"f"),
            detail: Drift::Replaced
        }
    );
}

/// A symlink is replaced by one with a same-length target before its target is read. The read
/// target has the listed length, so only the re-stat after the read sees another object.
#[test]
fn control_06_a_symlink_replaced_before_its_target_is_read_refuses() {
    let fixture = Fixture::new();
    fixture.link("l", "aaaa").file("m", 0o644, b"m");
    let link = fixture.at("l");
    let staging = fixture.aside.join("staging");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"l"))) {
            symlink("bbbb", &staging).unwrap();
            fs::rename(&staging, &link).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"l"),
            detail: Drift::SymlinkTarget
        }
    );
}

/// An impostor symlink is in place only while the target is read, and the original is back
/// before the re-stat, with timestamps too coarse to show either swap. Only the target's length
/// against the listed size shows that the target read is not the listed symlink's.
#[test]
fn control_06_a_symlink_swapped_only_while_its_target_is_read_refuses_on_a_coarse_timestamp_filesystem(
) {
    let fixture = Fixture::new();
    fixture.link("l", "aaaa").file("m", 0o644, b"m");
    let link = fixture.at("l");
    let saved = fixture.aside.join("l");
    let seams = coarse_with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::BeforeOpen(frame_path(b"l"))) {
            fs::rename(&link, &saved).unwrap();
            symlink("bb", &link).unwrap();
        }
        if emit_point(pass, point, &PointV1::TargetRead(frame_path(b"l"))) {
            fs::remove_file(&link).unwrap();
            fs::rename(&saved, &link).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"l"),
            detail: Drift::SymlinkTarget
        }
    );
}

/// A kept child is replaced after it was emitted, and the original is back before the final pass.
#[test]
fn control_06_a_kept_child_replaced_during_its_directorys_subtree_refuses() {
    let fixture = Fixture::new();
    fixture.file("a", 0o644, b"a").file("b", 0o644, b"b");
    let target = fixture.at("a");
    let saved = fixture.aside.join("a");
    let staging = fixture.aside.join("staging");
    let restore = (target.clone(), saved.clone());
    let seams = coarse_with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Subtree(None)) {
            fs::rename(&target, &saved).unwrap();
            replace_file(&target, &staging, b"a");
        }
        if emit_point(pass, point, &PointV1::Verifying) {
            fs::rename(&restore.1, &restore.0).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: some_path(b"a"),
            detail: Drift::ChildIdentity
        }
    );
}

/// A name is added and removed again during the directory's subtree: the name set and every child
/// are unchanged at the re-listing and at the final pass, but the directory's own mtime moved.
#[test]
fn control_06_a_transient_entry_is_caught_by_the_directorys_own_metadata() {
    let fixture = Fixture::new();
    fixture.file("a", 0o644, b"a");
    let transient = fixture.at("transient");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Listed(None)) {
            fs::write(&transient, b"t").unwrap();
            fs::remove_file(&transient).unwrap();
        }
    });
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: None,
            detail: Drift::DirectoryMetadata
        }
    );
}

#[test]
fn control_06_the_root_identity_is_checked_at_the_end_of_the_walk() {
    let fixture = Fixture::new();
    fixture.file("a", 0o644, b"a");
    let seams = WalkSeamsV1 {
        stat: Some(Box::new(|pass, observation, _, stat: &mut ChildStatV1| {
            if pass == PassV1::Verify && observation == ObservationV1::Root {
                stat.ino = stat.ino.wrapping_add(1);
            }
        })),
        ..WalkSeamsV1::default()
    };
    assert_eq!(
        walk_seamed(&fixture, &Select::default(), seams).error(),
        E::SourceDrift {
            path: None,
            detail: Drift::RootIdentity
        }
    );
}

/// With the budget exactly spent, a directory that grew makes its re-listing run out. That is the
/// directory's drift, not a budget refusal.
#[test]
fn control_06_a_relisting_that_outgrows_the_budget_is_drift() {
    let fixture = Fixture::new();
    fixture
        .file("a", 0o644, b"a")
        .file("b", 0o644, b"b")
        .file("c", 0o644, b"c");
    let created = fixture.at("d");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Listed(None)) {
            fs::write(&created, b"d").unwrap();
        }
    });
    let outcome = walk_full(&fixture.pin(), &Select::default(), 3, frame_budget(), seams);
    assert_eq!(
        outcome.error(),
        E::SourceDrift {
            path: None,
            detail: Drift::ChildNames
        }
    );
}

#[test]
fn control_06_a_final_pass_that_outgrows_the_budget_is_drift() {
    let fixture = Fixture::new();
    fixture
        .file("a", 0o644, b"a")
        .file("b", 0o644, b"b")
        .file("c", 0o644, b"c");
    let created = fixture.at("d");
    let seams = with_hook(move |pass, point| {
        if emit_point(pass, point, &PointV1::Verifying) {
            fs::write(&created, b"d").unwrap();
        }
    });
    let outcome = walk_full(&fixture.pin(), &Select::default(), 3, frame_budget(), seams);
    assert_eq!(
        outcome.error(),
        E::SourceDrift {
            path: None,
            detail: Drift::FinalVerification
        }
    );
}

// ---------------------------------------------------------------------------------------------
// §5.7 Budget
// ---------------------------------------------------------------------------------------------

/// Five listed names in three lists of at most three: only a budget shared by every list can
/// refuse the fifth.
fn budget_tree(fixture: &Fixture) {
    fixture
        .dir("d1", 0o755)
        .file("d1/x", 0o644, b"x")
        .dir("d2", 0o755)
        .file("d2/y", 0o644, b"y")
        .file("f", 0o644, b"f");
}

#[test]
fn control_07_the_global_entry_budget_admits_max_and_refuses_max_plus_one() {
    let fixture = Fixture::new();
    budget_tree(&fixture);
    let root = fixture.pin();
    let at = |budget| {
        walk_full(
            &root,
            &Select::default(),
            budget,
            frame_budget(),
            WalkSeamsV1::default(),
        )
    };
    let admitted = at(5);
    admitted.receipt();
    assert_eq!(admitted.entries().len(), 5);
    assert_eq!(at(4).error(), E::EntryLimit);
    assert_eq!(at(0).error(), E::EntryLimit);
}

#[test]
fn control_07_the_peak_live_name_count_is_within_two_budgets() {
    const BUDGET_NAMES: u64 = 6;
    let fixture = Fixture::new();
    fixture.dir("a", 0o755);
    for index in 0..BUDGET_NAMES - 1 {
        fixture.file(format!("a/f{index}"), 0o644, b"");
    }
    let outcome = walk_full(
        &fixture.pin(),
        &Select::default(),
        BUDGET_NAMES,
        frame_budget(),
        WalkSeamsV1::default(),
    );
    outcome.receipt();
    // The root's one name and `a`'s five are live when `a` is re-listed: 1 + 5 + 5.
    assert_eq!(outcome.record.peak_live_names, 2 * BUDGET_NAMES - 1);
    assert!(outcome.record.peak_live_names <= 2 * BUDGET_NAMES);

    let fixture = Fixture::new();
    mixed_tree(&fixture);
    let outcome = walk_full(
        &fixture.pin(),
        &Select::default(),
        10,
        frame_budget(),
        WalkSeamsV1::default(),
    );
    outcome.receipt();
    assert!(outcome.record.peak_live_names <= 2 * 10);
}

#[test]
fn control_07_a_wide_child_refuses_before_its_list_reaches_full_size() {
    const WIDTH: u64 = 20;
    const SMALL_BUDGET: u64 = WIDTH + 1 + 4;
    let fixture = Fixture::new();
    fixture.dir("d", 0o755);
    for index in 0..WIDTH {
        fixture.file(format!("f{index:02}"), 0o644, b"");
        fixture.file(format!("d/g{index:02}"), 0o644, b"");
    }
    let root = fixture.pin();
    let before = listed_child_names_for_test();
    let outcome = walk_full(
        &root,
        &Select::default(),
        SMALL_BUDGET,
        frame_budget(),
        WalkSeamsV1::default(),
    );
    let allocated = listed_child_names_for_test() - before;
    assert_eq!(outcome.error(), E::EntryLimit);
    // The parent's 21 names and exactly the 4 the budget had left: never the child's 20.
    assert_eq!(allocated, SMALL_BUDGET);
    assert!(allocated < 2 * WIDTH + 1);
}

#[test]
fn control_07_frame_budget_refusals_propagate() {
    let fixture = Fixture::new();
    fixture.file("big", 0o644, &[7_u8; 1_000]);
    let small = CustodyFrameBudgetV1::new(16, 200).unwrap();
    let outcome = walk_full(
        &fixture.pin(),
        &Select::default(),
        BUDGET,
        small,
        WalkSeamsV1::default(),
    );
    assert_eq!(outcome.error(), E::Frame(CustodyFrameErrorV1::ByteBudget));
}

// ---------------------------------------------------------------------------------------------
// §5.8 Non-UTF-8 names (Linux; macOS APFS refuses them)
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn control_08_a_non_utf8_name_round_trips() {
    let fixture = Fixture::new();
    let directory = fixture.at(OsStr::from_bytes(b"d\xff"));
    fs::create_dir(&directory).unwrap();
    chmod(&directory, 0o755);
    fs::write(directory.join(OsStr::from_bytes(b"\xfe\x80")), b"bytes").unwrap();
    chmod(&directory.join(OsStr::from_bytes(b"\xfe\x80")), 0o644);
    let outcome = walk(&fixture, &Select::default());
    outcome.receipt();
    assert_eq!(
        outcome.entries(),
        vec![
            Entry::Directory(b"d\xff".to_vec(), 0o755),
            Entry::Regular(b"d\xff/\xfe\x80".to_vec(), 0o644, b"bytes".to_vec()),
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// §5.9 Case (the case-insensitive half runs in the controller's macOS lane)
// ---------------------------------------------------------------------------------------------

#[test]
fn control_09_the_on_disk_spelling_is_emitted_and_never_folded() {
    let fixture = Fixture::new();
    fixture
        .dir("CaSe", 0o755)
        .file("CaSe/InNeR", 0o644, b"i")
        .file("MiXeD", 0o644, b"m");
    // On a case-insensitive filesystem another spelling resolves to the same entry.
    let case_insensitive = fs::symlink_metadata(fixture.at("mixed")).is_ok();
    #[cfg(target_os = "macos")]
    eprintln!("case-insensitive filesystem: {case_insensitive}");
    let outcome = walk(&fixture, &Select::default());
    outcome.receipt();
    assert_eq!(
        outcome.entries(),
        vec![
            dir_entry("CaSe", 0o755),
            file_entry("CaSe/InNeR", 0o644, b"i"),
            file_entry("MiXeD", 0o644, b"m"),
        ],
        "case-insensitive: {case_insensitive}"
    );
}

// ---------------------------------------------------------------------------------------------
// §5.10 Determinism
// ---------------------------------------------------------------------------------------------

#[test]
fn control_10_walks_are_deterministic_and_independent_of_readdir_order() {
    let fixture = Fixture::new();
    mixed_tree(&fixture);
    let root = fixture.pin();
    let at = |reverse_listings| {
        let outcome = walk_full(
            &root,
            &Select {
                skip: vec![b"a.b"],
                ..Select::default()
            },
            BUDGET,
            frame_budget(),
            WalkSeamsV1 {
                reverse_listings,
                ..WalkSeamsV1::default()
            },
        );
        (outcome.receipt(), outcome.frame)
    };
    let first = at(false);
    assert_eq!(at(false), first);
    assert_eq!(at(true), first);
    assert_eq!(first.0.skipped_entries(), 1);
}

#[test]
fn control_10_the_first_park_is_independent_of_readdir_order() {
    let fixture = Fixture::new();
    fixture
        .file("a", 0o644, b"a")
        .file("p1", 0o644, b"1")
        .dir("p2", 0o755)
        .file("p3", 0o644, b"3");
    let root = fixture.pin();
    let selection = Select {
        park_prefix: Some(b"p"),
        ..Select::default()
    };
    for reverse_listings in [false, true] {
        let outcome = walk_full(
            &root,
            &selection,
            BUDGET,
            frame_budget(),
            WalkSeamsV1 {
                reverse_listings,
                ..WalkSeamsV1::default()
            },
        );
        assert_eq!(
            outcome.error(),
            E::Park(WalkParkV1::new(
                CustodyReasonCodeV1::WriterUncontrolled,
                frame_path(b"p1")
            )),
            "reversed: {reverse_listings}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// §5.11 Receipt
// ---------------------------------------------------------------------------------------------

#[test]
fn control_11_the_receipt_summary_equals_the_decoded_frame() {
    let fixture = Fixture::new();
    let outcome = walk(&fixture, &Select::default());
    let receipt = outcome.receipt();
    assert_receipt_matches_frame(&receipt, &outcome.frame);
    assert_eq!(receipt.summary().entries(), 0);
    assert_eq!(receipt.skipped_entries(), 0);
    assert_eq!(
        receipt.inventory_sha256(),
        sha256(b"a2a-walk-inventory-v1\0"),
        "an empty tree folds only the domain prefix"
    );

    let fixture = Fixture::new();
    mixed_tree(&fixture);
    let outcome = walk(&fixture, &Select::default());
    let receipt = outcome.receipt();
    assert_receipt_matches_frame(&receipt, &outcome.frame);
    assert_eq!(receipt.summary().entries(), 10);
    assert_eq!(receipt.summary().content_bytes(), 5 + 1 + 3 + 6 + 5);
}

// ---------------------------------------------------------------------------------------------
// §4.6 The inventory digest encoding
// ---------------------------------------------------------------------------------------------

/// The digest of a small tree whose every observation is normalized by the stat seam: device 7,
/// fixed inodes, fixed timestamps (one before the epoch), and directory sizes of zero. The hex
/// literal is computed by an independent Python script recorded in the handoff.
#[test]
fn inventory_digest_of_a_small_tree_is_pinned() {
    const GOLDEN_INVENTORY_HEX: &str =
        "abfc2a54da1adedc2870fe53cc72a09f5ee54a3b87742fb3e0362deec56e1a67";
    let fixture = Fixture::new();
    fixture
        .dir("d", 0o755)
        .file("d/f", 0o644, b"hi")
        .link("d/l", "f")
        .file("s", 0o600, b"xyz");
    let seams = WalkSeamsV1 {
        stat: Some(Box::new(|_, _, path, stat: &mut ChildStatV1| {
            stat.dev = 7;
            stat.ino = match path.map(CustodyFramePathV1::as_bytes) {
                None => 1,
                Some(b"d") => 11,
                Some(b"d/f") => 12,
                Some(b"d/l") => 13,
                Some(b"s") => 14,
                Some(other) => panic!("unexpected path {other:?}"),
            };
            stat.mtime_ns = 1_700_000_000_123_456_789;
            stat.ctime_ns = -1_500_000_000;
            match stat.kind {
                ChildKindV1::Directory => stat.size = 0,
                ChildKindV1::Symlink => stat.mode = 0o777,
                _ => {}
            }
        })),
        ..WalkSeamsV1::default()
    };
    let selection = Select {
        skip: vec![b"s"],
        ..Select::default()
    };
    let outcome = walk_seamed(&fixture, &selection, seams);
    let receipt = outcome.receipt();
    assert_eq!(receipt.skipped_entries(), 1);
    assert_eq!(hex(&receipt.inventory_sha256()), GOLDEN_INVENTORY_HEX);
}
