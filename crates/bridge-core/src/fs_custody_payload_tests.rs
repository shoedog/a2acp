//! Controls for the ADR-0041 slice 2B3c increment C1 primitives (task spec "Primitives (increment
//! C1)" and §C1).
//!
//! `symlink_*` cover [`PinnedDirectoryV1::create_new_symlink_child`], `mode_*` cover
//! [`set_exact_file_mode`] and [`PinnedDirectoryV1::set_exact_mode`], and `unsafe_*` is the
//! 2B3c new-unsafe-site inventory. Every control works on a real temporary tree and holds as a
//! non-root user on a case-insensitive file system. Controls that race a primitive change the tree
//! from the test-only payload primitive hook between two of its steps. The mutation matrix in the
//! implementation handoff removes one guard at a time and names the control each removal turns red.

use super::*;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions, Permissions};
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const LABEL: &str = "2B3c C1 control";

/// The modes the restore must set exactly: no access at all, write-only, read-only, and the
/// common directory and executable modes up to every permission bit.
const EXACT_MODES: [u32; 6] = [0o000, 0o200, 0o400, 0o555, 0o755, 0o777];

/// Set only in the isolated umask child (`mode_01`), to the octal umask its shell installed.
const UMASK_CHILD_ENV: &str = "BRIDGE_CORE_2B3C_UMASK_CHILD";

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// A pinned, empty temporary directory.
fn pinned() -> (TempDir, PinnedDirectoryV1) {
    let temp = tempfile::tempdir().expect("temporary directory");
    let pin = PinnedDirectoryV1::open(temp.path(), LABEL).expect("pin the temporary directory");
    (temp, pin)
}

fn link_at(
    pin: &PinnedDirectoryV1,
    name: &str,
    target: &[u8],
) -> Result<ChildStatV1, SymlinkChildRefusalV1> {
    pin.create_new_symlink_child(OsStr::new(name), target, LABEL)
}

fn entry(pin: &PinnedDirectoryV1, name: &str) -> Option<ChildStatV1> {
    pin.child_metadata_no_follow(OsStr::new(name), LABEL)
        .expect("no-follow stat")
}

/// The no-follow observation of `path`: kind, mode, size, device, inode, mtime, and ctime.
fn observe(path: &Path) -> ChildStatV1 {
    ChildStatV1::from_metadata(&fs::symlink_metadata(path).expect("no-follow stat"))
}

fn mode_of(file: &File) -> u32 {
    ChildStatV1::from_metadata(&file.metadata().expect("fstat")).mode
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, Permissions::from_mode(mode)).expect("fixture chmod");
}

fn names(path: &Path) -> Vec<OsString> {
    let mut names = fs::read_dir(path)
        .expect("list directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// A `length`-byte relative target made of short components, so no component exceeds `NAME_MAX`
/// and only the total length is under test.
fn long_target(length: usize) -> Vec<u8> {
    let mut target = b"d/".repeat(length / 2);
    target.resize(length, b'x');
    target
}

fn assert_identity_changed(result: Result<ChildStatV1, SymlinkChildRefusalV1>, case: &str) {
    assert!(
        matches!(
            result,
            Err(SymlinkChildRefusalV1::Custody(
                FsCustodyError::IdentityChanged(_)
            ))
        ),
        "{case}: {result:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// create_new_symlink_child
// ---------------------------------------------------------------------------------------------

/// Absolute, `..`, and non-UTF-8 targets are written verbatim and read back byte for byte, both
/// through the retained descriptor and by an independent path-based read. The returned value is
/// the observation of the entry now at the name.
#[test]
fn symlink_01_absolute_dotdot_and_non_utf8_targets_round_trip_byte_exact() {
    let (_temp, pin) = pinned();
    for (name, target) in [
        ("absolute", &b"/nonexistent-2b3c/absolute/target"[..]),
        ("dotdot", b"../../outside/escape"),
        ("non-utf8", b"caf\xc3\xa9/\xff\xfe/\x80-raw"),
    ] {
        let created = link_at(&pin, name, target).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(created.kind, ChildKindV1::Symlink, "{name}");
        assert_eq!(created.size, target.len() as u64, "{name}");
        assert_eq!(entry(&pin, name), Some(created), "{name}");
        assert_eq!(
            pin.read_child_symlink(OsStr::new(name), 4095, LABEL)
                .unwrap(),
            target,
            "{name}"
        );
        assert_eq!(
            fs::read_link(pin.canonical_path().join(name))
                .unwrap()
                .as_os_str()
                .as_bytes(),
            target,
            "{name}"
        );
    }
}

/// The frame format admits a 4095-byte target; whether the destination holds one is measured at
/// run time by std's path-based `symlink` on the same file system, never assumed from the OS. Where
/// the probe holds the target the primitive must too, byte for byte; where it refuses
/// `ENAMETOOLONG` the primitive refuses `DestinationUnsupported` and leaves no entry. A 4096-byte
/// target exceeds the kernel path limit on every supported lane (Linux and macOS), so the refusal
/// arm runs in every lane, not only where 4095 bytes are refused.
#[test]
fn symlink_02_a_long_target_follows_the_destinations_runtime_capability() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let probe = temp.path().join("probe");
    let destination = temp.path().join("destination");
    fs::create_dir(&probe).unwrap();
    fs::create_dir(&destination).unwrap();
    let pin = PinnedDirectoryV1::open(&destination, LABEL).unwrap();

    let mut refused = 0;
    for length in [4095, 4096] {
        let target = long_target(length);
        assert_eq!(target.len(), length);
        let name = format!("long-{length}");
        let holds = match symlink(OsStr::from_bytes(&target), probe.join(&name)) {
            Ok(()) => true,
            Err(error) if error.raw_os_error() == Some(libc::ENAMETOOLONG) => false,
            Err(error) => panic!("capability probe for {length} bytes: {error}"),
        };
        println!("PAYLOAD-SYMLINK-CAPABILITY target_len={length} holds={holds}");
        let created = link_at(&pin, &name, &target);
        if holds {
            let created = created.unwrap_or_else(|error| panic!("{length}: {error}"));
            assert_eq!(created.size, length as u64);
            assert_eq!(
                pin.read_child_symlink(OsStr::new(&name), 4096, LABEL)
                    .unwrap(),
                target
            );
        } else {
            refused += 1;
            match created {
                Err(SymlinkChildRefusalV1::DestinationUnsupported {
                    target_len,
                    ref source,
                    ..
                }) => {
                    assert_eq!(target_len, length);
                    assert_eq!(source.raw_os_error(), Some(libc::ENAMETOOLONG));
                }
                other => panic!("{length}: expected DestinationUnsupported, got {other:?}"),
            }
            assert_eq!(entry(&pin, &name), None, "{length}: no entry");
        }
    }
    assert!(refused >= 1, "the DestinationUnsupported arm must run");
    // Exactly the held targets exist; nothing was truncated into place.
    let held = names(&probe);
    assert_eq!(names(&destination), held);
}

/// An occupied name refuses `TargetExists` whatever occupies it, and the occupant is left exactly
/// as it was: a regular file, a directory with a child, a dangling symlink (whose target is not
/// created through it), and a live symlink (whose target is not touched).
#[test]
fn symlink_03_an_existing_name_refuses_typed_and_is_left_untouched() {
    let (_temp, pin) = pinned();
    let root = pin.canonical_path().to_path_buf();
    fs::write(root.join("file"), b"kept bytes").unwrap();
    fs::create_dir(root.join("directory")).unwrap();
    fs::write(root.join("directory").join("inner"), b"inner").unwrap();
    symlink("would-be-created", root.join("dangling")).unwrap();
    symlink("file", root.join("live")).unwrap();
    let occupants = ["file", "directory", "dangling", "live"];
    let snapshot = || {
        (
            occupants.map(|name| entry(&pin, name)),
            fs::read(root.join("file")).unwrap(),
            fs::read_link(root.join("dangling")).unwrap(),
            fs::read_link(root.join("live")).unwrap(),
            names(&root.join("directory")),
        )
    };
    let before = snapshot();

    for name in occupants {
        let refused = link_at(&pin, name, b"new-target");
        assert!(
            matches!(
                refused,
                Err(SymlinkChildRefusalV1::Custody(
                    FsCustodyError::TargetExists(_)
                ))
            ),
            "{name}: {refused:?}"
        );
    }
    assert_eq!(snapshot(), before);
    assert!(fs::symlink_metadata(root.join("would-be-created")).is_err());
    assert_eq!(
        names(&root),
        ["dangling", "directory", "file", "live"].map(OsString::from)
    );
}

/// A parent pin whose directory was removed and replaced at its name refuses: the retained
/// directory no longer exists, so the descriptor-relative create cannot happen and the replacement
/// receives nothing. A pin whose directory was only renamed away keeps working descriptor-relative
/// (as every create-new primitive does, see `a1_a2_a3_a4_descriptor_creation_and_rooting_controls`):
/// the link lands in the retained directory, never in the decoy, and the re-pointed name is what
/// the caller's chain gate refuses.
#[test]
fn symlink_04_a_replaced_parent_refuses_and_a_renamed_one_never_reaches_the_decoy() {
    let temp = tempfile::tempdir().expect("temporary directory");

    let replaced = temp.path().join("replaced");
    fs::create_dir(&replaced).unwrap();
    let pin = PinnedDirectoryV1::open(&replaced, LABEL).unwrap();
    fs::remove_dir(&replaced).unwrap();
    fs::create_dir(&replaced).unwrap();
    let refused = link_at(&pin, "link", b"target");
    assert!(
        matches!(
            refused,
            Err(SymlinkChildRefusalV1::Custody(FsCustodyError::Io(_, _)))
        ),
        "{refused:?}"
    );
    assert_eq!(names(&replaced), Vec::<OsString>::new());

    let renamed = temp.path().join("renamed");
    fs::create_dir(&renamed).unwrap();
    let pin = PinnedDirectoryV1::open(&renamed, LABEL).unwrap();
    let retained = temp.path().join("retained");
    fs::rename(&renamed, &retained).unwrap();
    fs::create_dir(&renamed).unwrap();
    link_at(&pin, "link", b"target").unwrap();
    assert_eq!(
        fs::read_link(retained.join("link")).unwrap(),
        Path::new("target")
    );
    assert_eq!(names(&renamed), Vec::<OsString>::new());
    assert!(pinned_root_unchanged(&pin).is_err());
}

/// Creating and verifying a link never follows it: its target is an existing regular file whose
/// observation (mode, size, inode, mtime, ctime) and bytes are unchanged afterwards, through a
/// relative and through an absolute target.
#[test]
fn symlink_05_the_link_is_never_followed() {
    let (_temp, pin) = pinned();
    let target = pin.canonical_path().join("target-file");
    fs::write(&target, b"target bytes").unwrap();
    chmod(&target, 0o640);
    let before = (observe(&target), fs::read(&target).unwrap());
    assert_eq!(before.0.mode, 0o640);

    let relative = link_at(&pin, "relative", b"target-file").unwrap();
    let absolute = link_at(&pin, "absolute", target.as_os_str().as_bytes()).unwrap();
    for link in [relative, absolute] {
        assert_eq!(link.kind, ChildKindV1::Symlink);
        assert_ne!(link.ino, before.0.ino);
    }
    assert_eq!((observe(&target), fs::read(&target).unwrap()), before);
}

/// The entry at the name is replaced after `symlinkat` and before the verifying `fstatat`. Each
/// substitute isolates one check: a regular file of the target's length passes owner, device, and
/// size, so only the kind check refuses it; a symlink to another target of the same length passes
/// every stat check, so only the byte comparison refuses it; and an absent entry refuses at the
/// stat.
#[test]
fn symlink_06_an_entry_substituted_after_symlinkat_refuses() {
    type Substitute = fn(&Path);
    const TARGET: &[u8] = b"expected-target";
    let substitutes: [(&str, Substitute); 3] = [
        ("a regular file of the target's length", |path| {
            fs::remove_file(path).unwrap();
            fs::write(path, TARGET).unwrap();
        }),
        ("a symlink to another same-length target", |path| {
            fs::remove_file(path).unwrap();
            symlink("unexpected-tgt!", path).unwrap();
        }),
        ("nothing", |path| fs::remove_file(path).unwrap()),
    ];
    assert_eq!(b"unexpected-tgt!".len(), TARGET.len());
    for (case, substitute) in substitutes {
        let (_temp, pin) = pinned();
        let _hook = install_payload_primitive_hook_for_test(Box::new(move |point| {
            if let PayloadPrimitivePointV1::SymlinkCreated(path) = point {
                substitute(path);
            }
        }));
        assert_identity_changed(link_at(&pin, "link", TARGET), case);
    }
}

/// A symlink with the same target bytes is renamed over the created entry after `readlinkat` and
/// before the closing `fstatat`. Every byte the read saw still matches, so only the closing
/// observation can tell the objects apart. The substitute exists before the original is released,
/// so its inode number cannot be a reuse of the original's.
#[test]
fn symlink_07_an_entry_substituted_after_readlinkat_refuses() {
    let (_temp, pin) = pinned();
    let _hook = install_payload_primitive_hook_for_test(Box::new(|point| {
        if let PayloadPrimitivePointV1::TargetRead(path) = point {
            let staging = path.with_file_name("staging");
            symlink("same-target", &staging).unwrap();
            fs::rename(&staging, path).unwrap();
        }
    }));
    assert_identity_changed(link_at(&pin, "link", b"same-target"), "same-target swap");
}

/// The owner and device arms cannot be provoked through a real directory by an unprivileged test
/// (nobody else can create an entry there, and a child entry is on its parent's device), so each
/// arm of the verifying predicate is discriminated with injected values, as 2B2a's A2b owner arm
/// is. The kind and size arms are covered here too.
#[test]
fn symlink_08_each_arm_of_the_verifying_predicate_refuses_an_injected_observation() {
    let link = ChildStatV1 {
        kind: ChildKindV1::Symlink,
        mode: 0o777,
        size: 6,
        dev: 7,
        ino: 8,
        mtime_ns: 9,
        ctime_ns: 10,
    };
    assert!(created_symlink_is_valid(&link, 42, 42, Some(7), 6));
    // Owner: another user, including root seen by an unprivileged process.
    assert!(!created_symlink_is_valid(&link, 43, 42, Some(7), 6));
    assert!(!created_symlink_is_valid(&link, 0, 42, Some(7), 6));
    // Device: another device than the parent's, or no parent device to compare.
    assert!(!created_symlink_is_valid(&link, 42, 42, Some(70), 6));
    assert!(!created_symlink_is_valid(&link, 42, 42, None, 6));
    // Size: a symlink's size is its target's length.
    assert!(!created_symlink_is_valid(&link, 42, 42, Some(7), 5));
    assert!(!created_symlink_is_valid(&link, 42, 42, Some(7), 7));
    // Kind: anything but a symlink.
    for kind in [
        ChildKindV1::Regular,
        ChildKindV1::Directory,
        ChildKindV1::Special(0o010_000),
    ] {
        assert!(!created_symlink_is_valid(
            &ChildStatV1 { kind, ..link },
            42,
            42,
            Some(7),
            6
        ));
    }
}

/// A target `symlinkat` cannot express the same way on every platform (empty, or holding a NUL)
/// and a name that is not one safe component both refuse before anything is created.
#[test]
fn symlink_09_an_unexpressible_target_or_an_unsafe_name_refuses_before_any_entry() {
    let (_temp, pin) = pinned();
    for target in [&b""[..], b"\0", b"a\0b", b"target\0"] {
        let refused = link_at(&pin, "link", target);
        assert!(
            matches!(
                refused,
                Err(SymlinkChildRefusalV1::Custody(FsCustodyError::Unsupported(
                    _
                )))
            ),
            "{target:?}: {refused:?}"
        );
    }
    for name in [&b""[..], b".", b"..", b"a/b", b"a\0b"] {
        let refused = pin.create_new_symlink_child(OsStr::from_bytes(name), b"target", LABEL);
        assert!(
            matches!(
                refused,
                Err(SymlinkChildRefusalV1::Custody(
                    FsCustodyError::InvalidChildName(_)
                ))
            ),
            "{name:?}: {refused:?}"
        );
    }
    assert_eq!(names(pin.canonical_path()), Vec::<OsString>::new());
}

// ---------------------------------------------------------------------------------------------
// set_exact_file_mode and PinnedDirectoryV1::set_exact_mode
// ---------------------------------------------------------------------------------------------

/// Every exact mode, on files and on directories, holds under a restrictive (`077`) and an open
/// (`000`) umask. Each umask runs in its own child process: a shell installs it and then becomes
/// this test binary, filtered to this one test. The umask is process-global, so this process's
/// own umask is never changed (it would race every concurrently running test).
#[test]
fn mode_01_exact_modes_hold_under_any_umask_in_an_isolated_child() {
    if let Some(umask) = std::env::var_os(UMASK_CHILD_ENV) {
        let umask = umask.to_str().expect("an octal umask");
        exact_modes_under_this_process_umask(
            u32::from_str_radix(umask, 8).expect("an octal umask"),
        );
        println!("PAYLOAD-UMASK-CHILD-OK umask={umask}");
        return;
    }
    let test = format!(
        "{}::mode_01_exact_modes_hold_under_any_umask_in_an_isolated_child",
        module_path!()
            .strip_prefix("bridge_core::")
            .expect("the crate's module path")
    );
    for umask in ["077", "000"] {
        let output = Command::new("/bin/sh")
            .args([
                "-c",
                r#"umask "$1" && exec "$0" "$2" --exact --test-threads=1 --nocapture"#,
            ])
            .arg(std::env::current_exe().expect("the test binary"))
            .args([umask, test.as_str()])
            .env(UMASK_CHILD_ENV, umask)
            .output()
            .expect("the umask child runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let marker = format!("PAYLOAD-UMASK-CHILD-OK umask={umask}");
        assert!(
            output.status.success() && stdout.contains(&marker),
            "the umask {umask} child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{marker}");
    }
}

/// The body of `mode_01`, run only inside a umask child.
fn exact_modes_under_this_process_umask(umask: u32) {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};

    let (_temp, pin) = pinned();
    let root = pin.canonical_path().to_path_buf();
    // The child really runs under `umask`: a creation mode is masked by it.
    let probe = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o666)
        .open(root.join("umask-probe-file"))
        .unwrap();
    assert_eq!(mode_of(&probe), 0o666 & !umask);
    fs::DirBuilder::new()
        .mode(0o777)
        .create(root.join("umask-probe-directory"))
        .unwrap();
    assert_eq!(
        observe(&root.join("umask-probe-directory")).mode,
        0o777 & !umask
    );

    for mode in EXACT_MODES {
        let file_name = format!("file-{mode:03o}");
        let file = pin
            .create_new_regular_child(OsStr::new(&file_name), LABEL)
            .unwrap();
        assert_eq!(mode_of(&file), 0o600 & !umask, "created under the umask");
        set_exact_file_mode(&file, mode, LABEL).unwrap();
        assert_eq!(mode_of(&file), mode, "file {mode:03o}");
        assert_eq!(entry(&pin, &file_name).unwrap().mode, mode);

        let directory_name = format!("directory-{mode:03o}");
        let directory = pin
            .create_new_child_directory(OsStr::new(&directory_name), LABEL)
            .unwrap();
        directory.set_exact_mode(mode, LABEL).unwrap();
        assert_eq!(
            directory.current_metadata(LABEL).unwrap().mode,
            mode,
            "directory {mode:03o}"
        );
        assert_eq!(entry(&pin, &directory_name).unwrap().mode, mode);
        // Searchable again, so a non-root user can remove the temporary tree.
        directory.set_exact_mode(0o700, LABEL).unwrap();
    }
}

/// The mode is set through the retained descriptor, never by path: with the pinned directory (or
/// the open file) renamed away and a decoy put at its old name, the retained inode receives the
/// mode and the decoy's whole observation is unchanged.
#[test]
fn mode_02_a_renamed_away_object_gets_the_mode_and_the_decoy_is_untouched() {
    let temp = tempfile::tempdir().expect("temporary directory");

    let named = temp.path().join("directory");
    fs::create_dir(&named).unwrap();
    chmod(&named, 0o700);
    let pin = PinnedDirectoryV1::open(&named, LABEL).unwrap();
    let retained = temp.path().join("retained-directory");
    fs::rename(&named, &retained).unwrap();
    fs::create_dir(&named).unwrap();
    chmod(&named, 0o755);
    let decoy = observe(&named);
    pin.set_exact_mode(0o500, LABEL).unwrap();
    assert_eq!(observe(&retained).mode, 0o500);
    assert_eq!(pin.current_metadata(LABEL).unwrap().mode, 0o500);
    assert_eq!(observe(&named), decoy);
    pin.set_exact_mode(0o700, LABEL).unwrap();

    let parent = PinnedDirectoryV1::open(temp.path(), LABEL).unwrap();
    let named = temp.path().join("file");
    fs::write(&named, b"retained").unwrap();
    chmod(&named, 0o600);
    let file = parent.open_regular_file(OsStr::new("file"), LABEL).unwrap();
    let retained = temp.path().join("retained-file");
    fs::rename(&named, &retained).unwrap();
    fs::write(&named, b"decoy").unwrap();
    chmod(&named, 0o644);
    let decoy = observe(&named);
    set_exact_file_mode(&file, 0o400, LABEL).unwrap();
    assert_eq!(observe(&retained).mode, 0o400);
    assert_eq!(observe(&named), decoy);
    assert_eq!(fs::read(&retained).unwrap(), b"retained");
}

/// The confirming `fstat` is a real check: a mode changed through the same descriptor after the
/// `fchmod` and before the `fstat` refuses `IdentityChanged`, for a file and a directory pin.
#[test]
fn mode_03_a_mode_changed_before_the_confirming_fstat_refuses() {
    let (_temp, pin) = pinned();
    let file = pin
        .create_new_regular_child(OsStr::new("file"), LABEL)
        .unwrap();
    let directory = pin
        .create_new_child_directory(OsStr::new("directory"), LABEL)
        .unwrap();
    let hook = install_payload_primitive_hook_for_test(Box::new(|point| {
        if let PayloadPrimitivePointV1::ModeSet(file) = point {
            file.set_permissions(Permissions::from_mode(0o750)).unwrap();
        }
    }));
    assert!(matches!(
        set_exact_file_mode(&file, 0o600, LABEL),
        Err(FsCustodyError::IdentityChanged(_))
    ));
    assert!(matches!(
        directory.set_exact_mode(0o700, LABEL),
        Err(FsCustodyError::IdentityChanged(_))
    ));
    drop(hook);
    assert_eq!(mode_of(&file), 0o750);
    assert_eq!(directory.current_metadata(LABEL).unwrap().mode, 0o750);
}

/// Only permission bits are exact modes: setuid, setgid, sticky, or any bit above them refuses
/// before anything changes.
#[test]
fn mode_04_a_mode_beyond_the_permission_bits_refuses_before_any_change() {
    let (_temp, pin) = pinned();
    let file = pin
        .create_new_regular_child(OsStr::new("file"), LABEL)
        .unwrap();
    set_exact_file_mode(&file, 0o640, LABEL).unwrap();
    let directory = pin
        .create_new_child_directory(OsStr::new("directory"), LABEL)
        .unwrap();
    for mode in [0o1777, 0o2755, 0o4755, 0o7777, 0o10_644] {
        assert!(
            matches!(
                set_exact_file_mode(&file, mode, LABEL),
                Err(FsCustodyError::Unsupported(_))
            ),
            "file {mode:o}"
        );
        assert!(
            matches!(
                directory.set_exact_mode(mode, LABEL),
                Err(FsCustodyError::Unsupported(_))
            ),
            "directory {mode:o}"
        );
    }
    assert_eq!(mode_of(&file), 0o640);
    assert_eq!(directory.current_metadata(LABEL).unwrap().mode, 0o700);
}

// ---------------------------------------------------------------------------------------------
// The 2B3c new-unsafe inventory
// ---------------------------------------------------------------------------------------------

/// The new-unsafe inventory, in the style of 2B2a's A14 control: C1 adds exactly one site, the
/// symlink primitive's `symlinkat` block (which also reads the effective uid), the mode primitives
/// and the test seam hold none, and this control file holds none. The `fs_custody.rs` total is
/// pinned too, so a second new site anywhere in that file turns this control red as well as the
/// A14 and 2B2b2a controls.
#[test]
fn unsafe_inventory_pins_the_2b3c_symlink_site() {
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

    let fs_inventory = inventory(include_str!("fs_custody.rs"));
    assert_eq!(fs_inventory.get("create_new_symlink_child"), Some(&1));
    for function in [
        "set_exact_file_mode",
        "set_exact_mode",
        "created_symlink_is_valid",
        "install_payload_primitive_hook_for_test",
        "run_payload_primitive_hook",
    ] {
        assert_eq!(fs_inventory.get(function), None, "{function}");
    }
    // 2B2a's 35 sites, 2B2a's owned 3, 2B2b2a's 5, and this one.
    assert_eq!(fs_inventory.values().sum::<usize>(), 35 + 3 + 5 + 1);
    assert_eq!(
        inventory(include_str!("fs_custody_payload_tests.rs")),
        BTreeMap::new()
    );
}
