//! In-crate controls for ADR-0041 slice 2B2a.

use crate::custody_git::{
    bypass_final_symlink_refusal_for_test, classify_ext4_admission_for_test,
    effective_write_is_permitted_for_test, hold_recheck_facts_constant_for_test,
    install_route_audit_hook_for_test, parse_git_version, route_component_owner_is_trusted_for_test,
    running_as_root_for_test, same_route_facts, CustodyGitError, ExpectedGitDigestV1,
    Ext4AdmissionV1, GitCommandV1, GitGuardBypassV1, GitObjectFormatV1,
    GitObjectStoreRouteV1, GitRootNamesV1, GitRouteRequestV1, GitRunRequestV1, GitRunnerV1,
    RouteFactsV1,
};
use crate::fs_custody::PinnedDirectoryV1;
use ring::digest;
use std::fs;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn git_digest(path: &Path) -> ExpectedGitDigestV1 {
    ExpectedGitDigestV1::from_bytes(
        digest::digest(
            &digest::SHA256,
            &fs::read(path).expect("read test Git route"),
        )
        .as_ref()
        .try_into()
        .expect("SHA-256"),
    )
}

fn root_fixture() -> (TempDir, PinnedDirectoryV1) {
    let fixture = TempDir::new().expect("temporary custody root");
    let pin = PinnedDirectoryV1::open(fixture.path(), "test custody root").expect("pin root");
    (fixture, pin)
}

fn names() -> GitRootNamesV1 {
    GitRootNamesV1::new("home", "xdg", "repo").expect("valid root names")
}

fn system_git_route() -> PathBuf {
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

fn system_runner(root: &PinnedDirectoryV1) -> GitRunnerV1 {
    let route = system_git_route();
    GitRunnerV1::admit(
        GitRouteRequestV1::for_test_system(route.clone(), git_digest(&route)).expect("test route"),
        root,
        &names(),
        Instant::now() + Duration::from_secs(10),
    )
    .expect("admit container Git through test-only root profile")
}

struct FixtureRoute {
    directory: TempDir,
    path: PathBuf,
}

impl FixtureRoute {
    fn new(script: &str) -> Self {
        let directory = TempDir::new().expect("fixture route directory");
        let path = directory.path().join("git-fixture");
        fs::write(&path, script).expect("write fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).expect("chmod fixture");
        // The audited anchor must deny effective write access even on a non-root host.  Drop
        // restores it so tempfile can remove this disposable fixture.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500))
            .expect("seal fixture anchor");
        Self { directory, path }
    }

    fn admit(&self, root: &PinnedDirectoryV1) -> GitRunnerV1 {
        GitRunnerV1::admit(
            GitRouteRequestV1::for_test_fixture(
                self.path.clone(),
                git_digest(&self.path),
                self.directory.path().to_path_buf(),
            )
            .expect("fixture route"),
            root,
            &names(),
            Instant::now() + Duration::from_secs(5),
        )
        .expect("admit fixture")
    }

    fn make_mutable(&self) {
        fs::set_permissions(self.directory.path(), fs::Permissions::from_mode(0o700))
            .expect("unseal fixture anchor");
    }

    fn seal(&self) {
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o500))
            .expect("seal fixture route");
        fs::set_permissions(self.directory.path(), fs::Permissions::from_mode(0o500))
            .expect("reseal fixture anchor");
    }
}

impl Drop for FixtureRoute {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.directory.path(), fs::Permissions::from_mode(0o700));
    }
}

fn fixture_runner(script: &str, root: &PinnedDirectoryV1) -> (FixtureRoute, GitRunnerV1) {
    let fixture = FixtureRoute::new(script);
    let runner = fixture.admit(root);
    (fixture, runner)
}

fn system_git_with_input(arguments: &[String], input: &[u8]) -> std::process::Output {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new(system_git_route())
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fixture Git");
    child
        .stdin
        .take()
        .expect("fixture Git stdin")
        .write_all(input)
        .expect("write fixture Git stdin");
    child.wait_with_output().expect("wait fixture Git")
}

fn fixture_tree_digest(path: &Path) -> [u8; 32] {
    fn update(path: &Path, relative: &Path, context: &mut digest::Context) {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            let child_relative = relative.join(&name);
            context.update(child_relative.as_os_str().as_bytes());
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                context.update(b"d");
                update(&entry.path(), &child_relative, context);
            } else {
                context.update(b"f");
                context.update(&fs::read(entry.path()).unwrap());
            }
        }
    }

    let mut context = digest::Context::new(&digest::SHA256);
    update(path, Path::new(""), &mut context);
    context.finish().as_ref().try_into().unwrap()
}

fn copy_fixture_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_fixture_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn fixture_store_has_object(path: &Path) -> bool {
    fn has_regular_file(path: &Path) -> bool {
        fs::read_dir(path).unwrap().any(|entry| {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            file_type.is_file() || (file_type.is_dir() && has_regular_file(&entry.path()))
        })
    }
    has_regular_file(&path.join("objects"))
}

#[test]
fn a1_a4_descriptor_children_and_rooting_use_the_retained_descriptor() {
    let parent = TempDir::new().expect("parent");
    let original = parent.path().join("root");
    fs::create_dir(&original).expect("original root");
    let pin = PinnedDirectoryV1::open(&original, "root").expect("pin");
    fs::rename(&original, parent.path().join("old")).expect("rename original");
    fs::create_dir(&original).expect("replacement at original name");
    let child = pin
        .create_new_child_directory("nested".as_ref(), "nested")
        .expect("create nested through pin");
    let mut regular = pin
        .create_new_regular_child("record".as_ref(), "record")
        .expect("create regular through pin");
    use std::io::Write as _;
    regular.write_all(b"retained").expect("write regular");
    assert!(parent.path().join("old/nested").is_dir());
    assert!(parent.path().join("old/record").is_file());
    assert!(!original.join("nested").exists());
    assert!(child.identity().dev.is_some());
    let reopened = pin
        .open_existing_child_directory("nested".as_ref(), "reopen")
        .expect("reopen nested");
    let mut command = Command::new("/bin/pwd");
    pin.root_command(&mut command, "root command")
        .expect("root command");
    let output = command.output().expect("run rooted command");
    assert!(output.status.success());
    assert_eq!(
        Path::new(std::str::from_utf8(&output.stdout).unwrap().trim()),
        parent.path().join("old")
    );
    drop(reopened);
}

#[test]
fn a5_a7_a11d_admit_real_git_and_keep_the_command_surface_closed() {
    let (_temp, root) = root_fixture();
    let runner = system_runner(&root);
    let version = runner
        .run(
            &root,
            &names(),
            GitRunRequestV1::new(
                GitCommandV1::Version,
                vec![],
                1024,
                1024,
                Instant::now() + Duration::from_secs(2),
            ),
            || Ok(()),
            || Ok(()),
        )
        .expect("run admitted Git version");
    assert!(version.status.success());
    assert_eq!(
        version.evidence.environment.get("GIT_CONFIG_GLOBAL"),
        Some(&"/dev/null".into())
    );
    assert_eq!(
        version.evidence.environment.get("PATH"),
        Some(&"/usr/bin:/bin".into())
    );
    let init = runner
        .run(
            &root,
            &names(),
            GitRunRequestV1::new(
                GitCommandV1::InitBare {
                    dir: "bare".into(),
                    object_format: GitObjectFormatV1::Sha1,
                },
                vec![],
                4096,
                4096,
                Instant::now() + Duration::from_secs(2),
            ),
            || Ok(()),
            || Ok(()),
        )
        .expect("init bare");
    assert!(init.status.success());
    assert!(!init.evidence.environment.contains_key("GIT_DIR"));
    for name in ["HEAD", "config", "objects", "refs"] {
        assert!(
            root.canonical_path().join("bare").join(name).exists(),
            "{name}"
        );
    }
    assert_eq!(
        GitCommandV1::CatFileBatchCheck.arguments().unwrap().0,
        vec!["cat-file", "--batch-check"]
    );
    assert_eq!(
        GitCommandV1::VerifyPack {
            git_dir: "repo".into(),
            pack_hash: "a".repeat(40),
            object_format: GitObjectFormatV1::Sha1,
        }
        .arguments()
        .unwrap()
        .0
        .last()
        .unwrap(),
        "repo/objects/pack/pack-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.idx"
    );
    assert!(
        GitCommandV1::VerifyPack {
            git_dir: "repo".into(),
            pack_hash: "A".repeat(40),
            object_format: GitObjectFormatV1::Sha1,
        }
        .arguments()
        .is_err()
    );
}

#[test]
fn a5a_a5b_a5e_and_a13_refuse_bad_route_pins_and_versions_before_effects() {
    let (_temp, root) = root_fixture();
    let route = system_git_route();
    let bad = ExpectedGitDigestV1::from_bytes([9; 32]);
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::for_test_system(route.clone(), bad).unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::DigestMismatch { .. })
    ));
    assert!(matches!(
        parse_git_version(b"git version 2.53.9\n"),
        Ok((2, 53, 9))
    ));
    assert!(matches!(
        parse_git_version(b"git version 2.54.0 (Apple Git-157)\n"),
        Ok((2, 54, 0))
    ));
    assert!(parse_git_version(b"nonsense").is_err());
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::production(route.clone(), git_digest(&route)).unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::RouteRefusal(_))
    ));
    let links = TempDir::new().unwrap();
    let link = links.path().join("git-link");
    symlink(&route, &link).unwrap();
    assert!(matches!(
        GitRouteRequestV1::for_test_fixture(link, git_digest(&route), links.path().to_path_buf())
            .and_then(|request| GitRunnerV1::admit(
                request,
                &root,
                &names(),
                Instant::now() + Duration::from_secs(1)
            )),
        Err(CustodyGitError::RouteRefusal(_))
    ));
}

#[test]
fn a8_a8b_a9_a10_bounded_concurrent_io_kills_overflow_and_timeout() {
    let (_root_temp, root) = root_fixture();
    let (_fixture, output_runner) = fixture_runner(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) head -c 64 /dev/zero;; esac\n",
        &root,
    );
    assert!(matches!(
        output_runner.run(
            &root,
            &names(),
            GitRunRequestV1::new(
                GitCommandV1::FsckStrict,
                vec![],
                16,
                1024,
                Instant::now() + Duration::from_secs(1)
            ),
            || Ok(()),
            || Ok(()),
        ),
        Err(CustodyGitError::StdoutLimit { limit: 16 })
    ));
    let (_fixture, sleep_runner) = fixture_runner(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) sleep 2;; esac\n",
        &root,
    );
    assert!(matches!(
        sleep_runner.run(
            &root,
            &names(),
            GitRunRequestV1::new(
                GitCommandV1::FsckStrict,
                vec![],
                1024,
                1024,
                Instant::now() + Duration::from_millis(20)
            ),
            || Ok(()),
            || Ok(()),
        ),
        Err(CustodyGitError::Timeout)
    ));
}

#[test]
fn a12_a15_a18_route_encoding_and_evidence_are_ordered_and_bounded() {
    for prohibited in [":", "\"", "\\", "\n"] {
        assert!(
            GitObjectStoreRouteV1::new(PathBuf::from(format!("/tmp/a{prohibited}")), vec![])
                .is_err()
        );
    }
    let nul = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\0b".to_vec()));
    assert!(GitObjectStoreRouteV1::new(nul, vec![]).is_err());
    for invalid in [
        PathBuf::new(),
        PathBuf::from("relative"),
        PathBuf::from("../outside"),
    ] {
        assert!(GitObjectStoreRouteV1::new(invalid, vec![]).is_err());
    }
    for invalid in [
        PathBuf::new(),
        PathBuf::from("relative"),
        PathBuf::from("../outside"),
    ] {
        assert!(GitObjectStoreRouteV1::new(PathBuf::from("/objects"), vec![invalid]).is_err());
    }
    let first = GitObjectStoreRouteV1::new(
        PathBuf::from("/objects"),
        vec![PathBuf::from("/one"), PathBuf::from("/two")],
    )
    .unwrap();
    let second = GitObjectStoreRouteV1::new(
        PathBuf::from("/objects"),
        vec![PathBuf::from("/two"), PathBuf::from("/one")],
    )
    .unwrap();
    assert_ne!(first.evidence(), second.evidence());
    let role_swap = GitObjectStoreRouteV1::new(
        PathBuf::from("/one"),
        vec![PathBuf::from("/objects"), PathBuf::from("/two")],
    )
    .unwrap();
    assert_ne!(first.evidence(), role_swap.evidence());
    assert!(GitRootNamesV1::new(".", "xdg", "repo").is_err());

    let matching_ext4 = "36 25 8:1 / /fixture rw - ext4 /dev/sda1 rw";
    assert_eq!(
        classify_ext4_admission_for_test(0xEF53, (8, 1), matching_ext4),
        Ext4AdmissionV1::Admitted
    );
    for filesystem in ["ext2", "ext3"] {
        assert_eq!(
            classify_ext4_admission_for_test(
                0xEF53,
                (8, 1),
                &format!("36 25 8:1 / /fixture rw - {filesystem} /dev/sda1 rw"),
            ),
            Ext4AdmissionV1::WrongFilesystemType
        );
    }
    assert_eq!(
        classify_ext4_admission_for_test(0xEF53, (8, 2), matching_ext4),
        Ext4AdmissionV1::MissingMount
    );
    assert_eq!(
        classify_ext4_admission_for_test(
            0xEF53,
            (8, 1),
            &format!("{matching_ext4}\n{matching_ext4}")
        ),
        Ext4AdmissionV1::AmbiguousMount
    );
    assert_eq!(
        classify_ext4_admission_for_test(0xEF53, (8, 1), "not mountinfo"),
        Ext4AdmissionV1::MalformedMountInfo
    );
    assert_eq!(
        classify_ext4_admission_for_test(0x1234, (8, 1), matching_ext4),
        Ext4AdmissionV1::NotExt4Superblock
    );
}

fn request(command: GitCommandV1, stdin: Vec<u8>) -> GitRunRequestV1 {
    GitRunRequestV1::new(
        command,
        stdin,
        2 * 1024 * 1024,
        2 * 1024 * 1024,
        Instant::now() + Duration::from_secs(10),
    )
}

#[test]
fn a5_route_binding_rechecks_and_digest_refusals_carry_descriptor_facts() {
    let (_root_temp, root) = root_fixture();
    let fixture = FixtureRoute::new("#!/bin/sh\necho 'git version 2.54.0'\n");
    let expected = ExpectedGitDigestV1::from_bytes([7; 32]);
    let canonical = fixture.path.canonicalize().expect("canonical fixture");
    let expected_facts = RouteFactsV1::from_metadata(&fs::metadata(&canonical).unwrap());
    match GitRunnerV1::admit(
        GitRouteRequestV1::for_test_fixture(
            fixture.path.clone(),
            expected,
            fixture.directory.path().to_path_buf(),
        )
        .unwrap(),
        &root,
        &names(),
        Instant::now() + Duration::from_secs(1),
    ) {
        Err(CustodyGitError::DigestMismatch { details }) => {
            assert_eq!(details.canonical_path, canonical);
            assert!(same_route_facts(&details.facts, &expected_facts));
        }
        Ok(_) => panic!("digest mismatch unexpectedly admitted the route"),
        Err(other) => panic!("expected descriptor-bearing digest mismatch, got {other}"),
    }

    let fixture = FixtureRoute::new(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) echo original > marker;; esac\n",
    );
    let runner = fixture.admit(&root);
    fixture.make_mutable();
    fs::write(
        &fixture.path,
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) echo changed > marker;; esac\n",
    )
    .unwrap();
    fixture.seal();
    assert!(matches!(
        runner.run(
            &root,
            &names(),
            request(GitCommandV1::FsckStrict, vec![]),
            || Ok(()),
            || Ok(())
        ),
        Err(CustodyGitError::RouteIdentityChanged | CustodyGitError::DigestMismatch { .. })
    ));
    assert!(!root.canonical_path().join("marker").exists());
}

#[test]
fn a5c_a5e_and_a5e_r_recheck_after_spawn_bind_and_audit_ancestors() {
    let (_root_temp, root) = root_fixture();
    let fixture = FixtureRoute::new(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) exit 0;; esac\n",
    );
    let runner = fixture.admit(&root);
    let route = fixture.path.clone();
    let anchor = fixture.directory.path().to_path_buf();
    assert!(matches!(
        runner.run(
            &root,
            &names(),
            request(GitCommandV1::FsckStrict, vec![]),
            move || {
                fs::set_permissions(&anchor, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(&route, "#!/bin/sh\nexit 0\n").unwrap();
                fs::set_permissions(&route, fs::Permissions::from_mode(0o500)).unwrap();
                fs::set_permissions(&anchor, fs::Permissions::from_mode(0o500)).unwrap();
                Ok(())
            },
            || Ok(())
        ),
        Err(CustodyGitError::BinaryDrift(_))
    ));

    let fixture = FixtureRoute::new("#!/bin/sh\necho 'git version 2.54.0'\n");
    let expected = git_digest(&fixture.path);
    let route = fixture.path.clone();
    let anchor = fixture.directory.path().to_path_buf();
    let replacement = anchor.join("replacement");
    let bytes = fs::read(&route).unwrap();
    let _hook = install_route_audit_hook_for_test(move |audited| {
        assert_eq!(audited, route);
        fs::set_permissions(&anchor, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&replacement, &bytes).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o500)).unwrap();
        fs::rename(&replacement, &route).unwrap();
        fs::set_permissions(&anchor, fs::Permissions::from_mode(0o500)).unwrap();
    });
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::for_test_fixture(
                fixture.path.clone(),
                expected,
                fixture.directory.path().to_path_buf(),
            )
            .unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::RouteIdentityChanged)
    ));
    drop(_hook);

    let fixture = FixtureRoute::new("#!/bin/sh\necho 'git version 2.54.0'\n");
    let runner = fixture.admit(&root);
    // Root's conservative profile rejects group/other write bits. An ordinary user gets an
    // owner-write flip, which reaches `faccessat(AT_EACCESS)` rather than relying on root mode
    // semantics. Both arms prove the predicate becomes writable before the spawn is refused.
    let writable_mode = if running_as_root_for_test() {
        0o520
    } else {
        0o700
    };
    fs::set_permissions(
        fixture.directory.path(),
        fs::Permissions::from_mode(writable_mode),
    )
    .unwrap();
    assert!(effective_write_is_permitted_for_test(fixture.directory.path()).unwrap());
    assert!(matches!(
        runner.run(
            &root,
            &names(),
            request(GitCommandV1::FsckStrict, vec![]),
            || Ok(()),
            || Ok(())
        ),
        Err(CustodyGitError::RouteRefusal(_))
    ));
    fixture.seal();
}

#[test]
fn a5_route_rule_table_and_final_symlink_guard_are_discriminating() {
    let (_root_temp, root) = root_fixture();
    assert!(route_component_owner_is_trusted_for_test(42, 42));
    assert!(!route_component_owner_is_trusted_for_test(41, 42));

    let fixture = FixtureRoute::new("#!/bin/sh\necho 'git version 2.54.0'\n");
    fixture.make_mutable();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(fixture.directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::for_test_fixture(
                fixture.path.clone(),
                git_digest(&fixture.path),
                fixture.directory.path().to_path_buf(),
            )
            .unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::RouteRefusal(_))
    ));
    fixture.seal();

    let fixture = FixtureRoute::new("#!/bin/sh\necho 'git version 2.54.0'\n");
    fixture.make_mutable();
    let target = fixture.directory.path().join("admissible-target");
    fs::rename(&fixture.path, &target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o500)).unwrap();
    symlink(&target, &fixture.path).unwrap();
    fs::set_permissions(fixture.directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let request = || {
        GitRouteRequestV1::for_test_fixture(
            fixture.path.clone(),
            git_digest(&target),
            fixture.directory.path().to_path_buf(),
        )
        .unwrap()
    };
    assert!(matches!(
        GitRunnerV1::admit(
            request(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1)
        ),
        Err(CustodyGitError::RouteRefusal(_))
    ));
    let _mutation = bypass_final_symlink_refusal_for_test();
    assert!(
        GitRunnerV1::admit(
            request(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1)
        )
        .is_ok()
    );
}

#[test]
fn a5e_fact_table_and_a5f_in_place_rehash_are_individual_guards() {
    let facts = RouteFactsV1::from_metadata(&fs::metadata(system_git_route()).unwrap());
    let mut rows = Vec::new();
    for field in 0..11 {
        let mut changed = facts;
        match field {
            0 => changed.dev = changed.dev.wrapping_add(1),
            1 => changed.ino = changed.ino.wrapping_add(1),
            2 => changed.file_type ^= 1,
            3 => changed.uid = changed.uid.wrapping_add(1),
            4 => changed.gid = changed.gid.wrapping_add(1),
            5 => changed.mode ^= 0o100,
            6 => changed.size = changed.size.wrapping_add(1),
            7 => changed.mtime_secs = changed.mtime_secs.wrapping_add(1),
            8 => changed.mtime_nanos = changed.mtime_nanos.wrapping_add(1),
            9 => changed.ctime_secs = changed.ctime_secs.wrapping_add(1),
            10 => changed.ctime_nanos = changed.ctime_nanos.wrapping_add(1),
            _ => unreachable!(),
        }
        rows.push(changed);
    }
    assert!(
        rows.iter()
            .all(|changed| !same_route_facts(&facts, changed))
    );

    let (_root_temp, root) = root_fixture();
    let fixture = FixtureRoute::new("#!/bin/sh\necho 'git version 2.54.0'\n# stable\n");
    let runner = fixture.admit(&root);
    let modified = fs::metadata(&fixture.path).unwrap().modified().unwrap();
    fixture.make_mutable();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        &fixture.path,
        "#!/bin/sh\necho 'git version 2.54.0'\n# drift!\n",
    )
    .unwrap();
    fs::File::open(&fixture.path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fixture.seal();
    let _facts = hold_recheck_facts_constant_for_test();
    assert!(matches!(
        runner.run(
            &root,
            &names(),
            request(GitCommandV1::FsckStrict, vec![]),
            || Ok(()),
            || Ok(())
        ),
        Err(CustodyGitError::DigestMismatch { .. })
    ));
}

#[test]
fn a6_a7_and_a11d_use_exact_environment_argv_and_init_shape() {
    let (_root_temp, root) = root_fixture();
    let (_fixture, runner) = fixture_runner(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) env | sort; echo __CWD__; pwd;; esac\n",
        &root,
    );
    let result = runner
        .run(
            &root,
            &names(),
            request(GitCommandV1::FsckStrict, vec![]),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    let output = String::from_utf8(result.captured_stdout().unwrap().to_vec()).unwrap();
    let (environment, cwd) = output.split_once("__CWD__\n").expect("environment marker");
    let expected = [
        "GIT_CONFIG_GLOBAL=/dev/null",
        "GIT_CONFIG_NOSYSTEM=1",
        "GIT_DIR=repo",
        "GIT_NO_LAZY_FETCH=1",
        "GIT_NO_REPLACE_OBJECTS=1",
        "GIT_OPTIONAL_LOCKS=0",
        "GIT_TERMINAL_PROMPT=0",
        "HOME=home",
        "LANG=C",
        "LC_ALL=C",
        "PATH=/usr/bin:/bin",
        &format!("PWD={}", root.canonical_path().display()),
        "XDG_CONFIG_HOME=xdg",
    ]
    .join("\n");
    assert_eq!(environment.trim(), expected);
    assert_eq!(Path::new(cwd.trim()), root.canonical_path());
    let mut inherited = request(GitCommandV1::FsckStrict, vec![]);
    inherited.guard_bypass.add_environment_variable = true;
    let inherited = runner
        .run(&root, &names(), inherited, || Ok(()), || Ok(()))
        .unwrap();
    assert!(
        String::from_utf8(inherited.captured_stdout().unwrap().to_vec())
            .unwrap()
            .contains("A2A_GIT_TEST_INHERITED=1")
    );

    let expected_common = vec![
        "--no-optional-locks",
        "--no-replace-objects",
        "--no-lazy-fetch",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "protocol.allow=never",
    ];
    let variants = vec![
        (GitCommandV1::Version, vec!["version"]),
        (
            GitCommandV1::InitBare {
                dir: "bare".into(),
                object_format: GitObjectFormatV1::Sha1,
            },
            vec![
                "init",
                "--bare",
                "--template=",
                "--object-format=sha1",
                "bare",
            ],
        ),
        (
            GitCommandV1::CatFileBatchCheck,
            vec!["cat-file", "--batch-check"],
        ),
        (
            GitCommandV1::PackObjectsStdout,
            vec!["pack-objects", "--stdout"],
        ),
        (
            GitCommandV1::IndexPackStrictStdin,
            vec!["index-pack", "--strict", "--stdin"],
        ),
        (
            GitCommandV1::VerifyPack {
                git_dir: "repo".into(),
                pack_hash: "f".repeat(40),
                object_format: GitObjectFormatV1::Sha1,
            },
            vec![
                "verify-pack",
                "-v",
                "repo/objects/pack/pack-ffffffffffffffffffffffffffffffffffffffff.idx",
            ],
        ),
        (
            GitCommandV1::CatFileAllObjects,
            vec![
                "cat-file",
                "--batch-all-objects",
                "--batch-check=%(objectname) %(objecttype)",
            ],
        ),
        (
            GitCommandV1::RevListMissingPrint,
            vec![
                "rev-list",
                "--objects",
                "--no-object-names",
                "--missing=print",
                "--stdin",
            ],
        ),
        (
            GitCommandV1::FsckStrict,
            vec![
                "fsck",
                "--strict",
                "--full",
                "--no-reflogs",
                "--no-dangling",
                "--no-progress",
            ],
        ),
    ];
    for (command, tail) in variants {
        let result = runner
            .run(
                &root,
                &names(),
                request(command, vec![]),
                || Ok(()),
                || Ok(()),
            )
            .unwrap();
        let actual = result
            .evidence
            .argv
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            expected_common
                .iter()
                .chain(tail.iter())
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
        );
    }
    assert!(
        GitCommandV1::VerifyPack {
            git_dir: "repo".into(),
            pack_hash: "g".repeat(40),
            object_format: GitObjectFormatV1::Sha1,
        }
        .arguments()
        .is_err()
    );
    assert!(
        GitCommandV1::VerifyPack {
            git_dir: "repo".into(),
            pack_hash: "z".repeat(64),
            object_format: GitObjectFormatV1::Sha256,
        }
        .arguments()
        .is_err()
    );

    let (_temp, root) = root_fixture();
    let runner = system_runner(&root);
    let init = runner
        .run(
            &root,
            &names(),
            request(
                GitCommandV1::InitBare {
                    dir: "bare".into(),
                    object_format: GitObjectFormatV1::Sha1,
                },
                vec![],
            ),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert!(init.status.success());
    assert!(!init.evidence.environment.contains_key("GIT_DIR"));
    let mut entries = fs::read_dir(root.canonical_path().join("bare"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(entries, ["HEAD", "config", "objects", "refs"]);
    let config = fs::read_to_string(root.canonical_path().join("bare/config")).unwrap();
    assert!(!config.contains("remote."));
    assert!(!config.contains("extensions.partialClone"));

    let planted = root.canonical_path().join("planted");
    fs::create_dir(&planted).unwrap();
    fs::write(planted.join("template-leak"), b"leak").unwrap();
    let mut git_dir_mutation = request(
        GitCommandV1::InitBare {
            dir: "git-dir-mutation".into(),
            object_format: GitObjectFormatV1::Sha1,
        },
        vec![],
    );
    git_dir_mutation.guard_bypass.init_sets_git_dir = true;
    let git_dir_mutation = runner
        .run(&root, &names(), git_dir_mutation, || Ok(()), || Ok(()))
        .unwrap();
    assert!(
        git_dir_mutation
            .evidence
            .environment
            .contains_key("GIT_DIR")
    );
    let mut template_mutation = request(
        GitCommandV1::InitBare {
            dir: "template-mutation".into(),
            object_format: GitObjectFormatV1::Sha1,
        },
        vec![],
    );
    template_mutation.guard_bypass.init_uses_template = true;
    let template_mutation = runner
        .run(&root, &names(), template_mutation, || Ok(()), || Ok(()))
        .unwrap();
    assert!(template_mutation.status.success());
    assert!(
        root.canonical_path()
            .join("template-mutation/template-leak")
            .exists()
    );
}

#[test]
fn a8_a8b_a9_a10_bound_streams_deadlines_and_descendant_pipe_holders() {
    let (_root_temp, root) = root_fixture();
    let (_fixture, runner) = fixture_runner(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *stdout*) head -c 17 /dev/zero;; *stderr-exact*) head -c 16 /dev/zero >&2;; *stderr*) head -c 17 /dev/zero >&2;; *exact*) head -c 16 /dev/zero;; *) head -c 1048576 /dev/zero; head -c 1048576 /dev/zero >&2; cat >/dev/null;; esac\n",
        &root,
    );
    let bounded = |suffix: &str, out: usize, err: usize, input: Vec<u8>| {
        let mut value = request(
            GitCommandV1::InitBare {
                dir: suffix.into(),
                object_format: GitObjectFormatV1::Sha1,
            },
            input,
        );
        value.stdout_limit = out;
        value.stderr_limit = err;
        value
    };
    assert!(matches!(
        runner.run(
            &root,
            &names(),
            bounded("stdout", 16, 64, vec![]),
            || Ok(()),
            || Ok(())
        ),
        Err(CustodyGitError::StdoutLimit { limit: 16 })
    ));
    assert!(matches!(
        runner.run(
            &root,
            &names(),
            bounded("stderr", 64, 16, vec![]),
            || Ok(()),
            || Ok(())
        ),
        Err(CustodyGitError::StderrLimit { limit: 16 })
    ));
    let mut stdout_mutation = bounded("stdout", 16, 64, vec![]);
    stdout_mutation.guard_bypass.skip_stdout_limit = true;
    assert_eq!(
        runner
            .run(&root, &names(), stdout_mutation, || Ok(()), || Ok(()))
            .unwrap()
            .captured_stdout()
            .unwrap()
            .len(),
        17
    );
    let mut stderr_mutation = bounded("stderr", 64, 16, vec![]);
    stderr_mutation.guard_bypass.skip_stderr_limit = true;
    assert_eq!(
        runner
            .run(&root, &names(), stderr_mutation, || Ok(()), || Ok(()))
            .unwrap()
            .stderr
            .len(),
        17
    );
    let exact = runner
        .run(
            &root,
            &names(),
            bounded("exact", 16, 64, vec![]),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(exact.captured_stdout().unwrap().len(), 16);
    let stderr_exact = runner
        .run(
            &root,
            &names(),
            bounded("stderr-exact", 64, 16, vec![]),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(stderr_exact.stderr.len(), 16);
    let stdin_path = root.canonical_path().join("duplex-input");
    let stdout_path = root.canonical_path().join("duplex-output");
    fs::write(&stdin_path, vec![0; 1024 * 1024]).unwrap();
    let mut duplex = GitRunRequestV1::from_file(
        GitCommandV1::InitBare {
            dir: "duplex".into(),
            object_format: GitObjectFormatV1::Sha1,
        },
        fs::File::open(&stdin_path).unwrap(),
        2 * 1024 * 1024,
        2 * 1024 * 1024,
        Instant::now() + Duration::from_secs(3),
    );
    duplex.stream_stdout_to_file(fs::File::create(&stdout_path).unwrap());
    let duplex = runner
        .run(&root, &names(), duplex, || Ok(()), || Ok(()))
        .unwrap();
    assert!(matches!(
        duplex.stdout,
        crate::custody_git::GitStdoutV1::Streamed(_)
    ));
    assert_eq!(duplex.evidence.stdout.length, 1024 * 1024);
    assert_eq!(fs::metadata(stdout_path).unwrap().len(), 1024 * 1024);
    assert_eq!(duplex.stderr.len(), 1024 * 1024);

    for bypass in [
        GitGuardBypassV1 {
            stdin_before_readers: true,
            ..GitGuardBypassV1::default()
        },
        GitGuardBypassV1 {
            stdout_after_stdin: true,
            ..GitGuardBypassV1::default()
        },
        GitGuardBypassV1 {
            stderr_after_exit: true,
            ..GitGuardBypassV1::default()
        },
    ] {
        let mut serial = bounded(
            "duplex",
            2 * 1024 * 1024,
            2 * 1024 * 1024,
            vec![0; 1024 * 1024],
        );
        serial.deadline = Instant::now() + Duration::from_millis(80);
        serial.guard_bypass = bypass;
        assert!(matches!(
            runner.run(&root, &names(), serial, || Ok(()), || Ok(())),
            Err(CustodyGitError::Timeout)
        ));
    }

    let pid_file = root.canonical_path().join("grandchild-pid");
    let (_fixture, runner) = fixture_runner(
        "#!/bin/sh\ntrap 'wait; exit 0' TERM\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) sleep 5 & echo $! > grandchild-pid; wait;; esac\n",
        &root,
    );
    let mut deadline = request(GitCommandV1::FsckStrict, vec![]);
    deadline.deadline = Instant::now() + Duration::from_millis(80);
    let started = Instant::now();
    assert!(matches!(
        runner.run(&root, &names(), deadline, || Ok(()), || Ok(())),
        Err(CustodyGitError::Timeout)
    ));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "timeout must include inherited-pipe descendants"
    );
    let pid = fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let gone_by = Instant::now() + Duration::from_secs(1);
    let mut alive = true;
    while alive && Instant::now() < gone_by {
        alive = Command::new("/bin/sh")
            .args(["-c", r#"kill -0 "$1" 2>/dev/null"#, "sh", &pid.to_string()])
            .status()
            .expect("portable process probe")
            .success();
        if alive {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert!(!alive, "process-group termination must leave no grandchild");

    let (_fixture, orphan_runner) = fixture_runner(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) sleep 5 & echo $! > orphan-pid; exit 0;; esac\n",
        &root,
    );
    let mut orphan = request(GitCommandV1::FsckStrict, vec![]);
    orphan.deadline = Instant::now() + Duration::from_millis(80);
    let started = Instant::now();
    assert!(matches!(
        orphan_runner.run(&root, &names(), orphan, || Ok(()), || Ok(())),
        Err(CustodyGitError::Timeout)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a16_real_index_pack_then_verify_pack_uses_the_fixed_pack_namespace() {
    let source = TempDir::new().unwrap();
    let source_path = source.path().join("source.git");
    let initialized = Command::new(system_git_route())
        .args(["init", "--bare", source_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(initialized.status.success(), "{:?}", initialized.stderr);
    let hash = system_git_with_input(
        &[
            format!("--git-dir={}", source_path.display()),
            "hash-object".into(),
            "-w".into(),
            "--stdin".into(),
        ],
        b"real pack fixture\n",
    );
    assert!(hash.status.success(), "{:?}", hash.stderr);
    let object = String::from_utf8(hash.stdout).unwrap().trim().to_owned();
    let pack = system_git_with_input(
        &[
            format!("--git-dir={}", source_path.display()),
            "pack-objects".into(),
            "--stdout".into(),
            "--revs".into(),
        ],
        format!("{object}\n").as_bytes(),
    );
    assert!(pack.status.success(), "{:?}", pack.stderr);

    let (_root_temp, root) = root_fixture();
    let runner = system_runner(&root);
    runner
        .run(
            &root,
            &names(),
            request(
                GitCommandV1::InitBare {
                    dir: "repo".into(),
                    object_format: GitObjectFormatV1::Sha1,
                },
                vec![],
            ),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    let pack_file = root.canonical_path().join("incoming.pack");
    fs::write(&pack_file, pack.stdout).unwrap();
    let index = runner
        .run(
            &root,
            &names(),
            GitRunRequestV1::from_file(
                GitCommandV1::IndexPackStrictStdin,
                fs::File::open(&pack_file).unwrap(),
                4096,
                4096,
                Instant::now() + Duration::from_secs(10),
            ),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert!(index.status.success());
    let index_stdout = std::str::from_utf8(index.captured_stdout().unwrap()).unwrap();
    let pack_hash = index_stdout
        .trim()
        .strip_prefix("pack\t")
        .expect("index-pack prints the parsed pack marker")
        .to_owned();
    assert_eq!(pack_hash.len(), 40);
    let verify = runner
        .run(
            &root,
            &names(),
            request(
                GitCommandV1::VerifyPack {
                    git_dir: "repo".into(),
                    pack_hash: pack_hash.clone(),
                    object_format: GitObjectFormatV1::Sha1,
                },
                vec![],
            ),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert!(verify.status.success());
    assert!(
        verify
            .evidence
            .argv
            .iter()
            .any(|arg| arg.to_string_lossy() == format!("repo/objects/pack/pack-{pack_hash}.idx"))
    );
}

#[test]
fn a5g_post_exit_rehash_a13_version_table_and_a17_profiles_are_enforced() {
    let (_root_temp, root) = root_fixture();
    let fixture = FixtureRoute::new(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) exec /bin/sh -c 'echo ready > ready; sleep .2';; esac\n",
    );
    let runner = fixture.admit(&root);
    let route = fixture.path.clone();
    let anchor = fixture.directory.path().to_path_buf();
    let ready = root.canonical_path().join("ready");
    let mutator = std::thread::spawn(move || {
        let limit = Instant::now() + Duration::from_secs(1);
        while !ready.exists() && Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ready.exists(), "helper must report readiness after exec");
        fs::set_permissions(&anchor, fs::Permissions::from_mode(0o700)).unwrap();
        let original = fs::read_to_string(&route).unwrap();
        let rewritten = original.replace("sleep .2", "sleep .3");
        assert_eq!(rewritten.len(), original.len());
        fs::set_permissions(&route, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&route, rewritten).unwrap();
        fs::set_permissions(&route, fs::Permissions::from_mode(0o500)).unwrap();
        fs::set_permissions(&anchor, fs::Permissions::from_mode(0o500)).unwrap();
    });
    let _facts = hold_recheck_facts_constant_for_test();
    assert!(matches!(
        runner.run(
            &root,
            &names(),
            request(GitCommandV1::FsckStrict, vec![]),
            || Ok(()),
            || Ok(())
        ),
        Err(CustodyGitError::BinaryDrift(_))
    ));
    mutator.join().unwrap();

    for (output, accepted) in [
        ("git version 2.53.9", false),
        ("git version 2.54.0", true),
        ("git version 2.54.0 (Apple Git-157)", true),
        ("not a Git version", false),
    ] {
        let fixture = FixtureRoute::new(&format!("#!/bin/sh\necho '{output}'\n"));
        let result = GitRunnerV1::admit(
            GitRouteRequestV1::for_test_fixture(
                fixture.path.clone(),
                git_digest(&fixture.path),
                fixture.directory.path().to_path_buf(),
            )
            .unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        );
        assert_eq!(result.is_ok(), accepted, "{output}");
    }
    let nonzero = FixtureRoute::new("#!/bin/sh\nexit 7\n");
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::for_test_fixture(
                nonzero.path.clone(),
                git_digest(&nonzero.path),
                nonzero.directory.path().to_path_buf(),
            )
            .unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::UnsupportedVersion(_))
    ));

    let fixture = FixtureRoute::new("#!/bin/sh\necho 'git version 2.54.0'\n");
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::production(fixture.path.clone(), git_digest(&fixture.path)).unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::RouteRefusal(_))
    ));
    let unrelated_anchor = TempDir::new().unwrap();
    fs::set_permissions(unrelated_anchor.path(), fs::Permissions::from_mode(0o500)).unwrap();
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::for_test_fixture(
                fixture.path.clone(),
                git_digest(&fixture.path),
                unrelated_anchor.path().to_path_buf(),
            )
            .unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::RouteRefusal(_))
    ));
    fs::set_permissions(unrelated_anchor.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a11a_a11b_a11c_each_runner_owned_lazy_fetch_guard_is_independent() {
    let fixture = TempDir::new().unwrap();
    let source = fixture.path().join("promisor-source.git");
    assert!(
        Command::new(system_git_route())
            .args(["init", "--bare", source.to_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    let object = system_git_with_input(
        &[
            format!("--git-dir={}", source.display()),
            "hash-object".into(),
            "-w".into(),
            "--stdin".into(),
        ],
        b"missing promisor object\n",
    );
    assert!(object.status.success(), "{:?}", object.stderr);
    let object = String::from_utf8(object.stdout).unwrap().trim().to_owned();

    let template = fixture.path().join("immutable-template.git");
    assert!(
        Command::new(system_git_route())
            .args(["init", "--bare", template.to_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    for (key, value) in [
        ("extensions.partialClone", "p"),
        ("remote.p.promisor", "true"),
        ("remote.p.url", &format!("file://{}", source.display())),
    ] {
        assert!(
            Command::new(system_git_route())
                .args([
                    format!("--git-dir={}", template.display()),
                    "config".into(),
                    key.into(),
                    value.into(),
                ])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let template_digest = fixture_tree_digest(&template);

    let mut rows = vec![
        GitGuardBypassV1 {
            no_lazy_fetch_environment: true,
            protocol_allow: true,
            ..GitGuardBypassV1::default()
        },
        GitGuardBypassV1 {
            no_lazy_fetch_flag: true,
            protocol_allow: true,
            ..GitGuardBypassV1::default()
        },
        GitGuardBypassV1 {
            no_lazy_fetch_flag: true,
            no_lazy_fetch_environment: true,
            ..GitGuardBypassV1::default()
        },
    ];
    let rotation = std::process::id() as usize % rows.len();
    rows.rotate_left(rotation);
    for bypass in rows {
        let (_root_temp, root) = root_fixture();
        let store = root.canonical_path().join("repo");
        copy_fixture_tree(&template, &store);
        assert!(!fixture_store_has_object(&store));
        let runner = system_runner(&root);
        let mut guarded = request(
            GitCommandV1::CatFileBatchCheck,
            format!("{object}\n").into_bytes(),
        );
        guarded.guard_bypass = bypass;
        let result = runner
            .run(&root, &names(), guarded, || Ok(()), || Ok(()))
            .unwrap();
        assert_eq!(
            result.captured_stdout().unwrap(),
            format!("{object} missing\n").as_bytes()
        );
        assert!(!fixture_store_has_object(&store));
        assert_eq!(fixture_tree_digest(&template), template_digest);
    }

    let (_root_temp, root) = root_fixture();
    let store = root.canonical_path().join("repo");
    copy_fixture_tree(&template, &store);
    let runner = system_runner(&root);
    let mut unguarded = request(
        GitCommandV1::CatFileBatchCheck,
        format!("{object}\n").into_bytes(),
    );
    unguarded.guard_bypass = GitGuardBypassV1 {
        no_lazy_fetch_flag: true,
        no_lazy_fetch_environment: true,
        protocol_allow: true,
        ..GitGuardBypassV1::default()
    };
    let result = runner
        .run(&root, &names(), unguarded, || Ok(()), || Ok(()))
        .unwrap();
    assert!(result.captured_stdout().unwrap().ends_with(b" blob 24\n"));
    assert!(fixture_store_has_object(&store));
    assert_eq!(fixture_tree_digest(&template), template_digest);
}

#[test]
fn a14_ast_inventory_keeps_new_unsafe_boundaries_exact() {
    use std::collections::BTreeMap;
    use syn::visit::Visit as _;

    #[derive(Default)]
    struct UnsafeInventory {
        functions: Vec<String>,
        counts: BTreeMap<String, usize>,
    }

    impl UnsafeInventory {
        fn enter(&mut self, name: String) {
            self.functions.push(name);
        }

        fn leave(&mut self) {
            self.functions
                .pop()
                .expect("function traversal is balanced");
        }
    }

    impl<'ast> syn::visit::Visit<'ast> for UnsafeInventory {
        fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
            let name = self
                .functions
                .last()
                .expect("unsafe expression belongs to a function");
            *self.counts.entry(name.clone()).or_default() += 1;
            syn::visit::visit_expr_unsafe(self, node);
        }

        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            // `syn` leaves macro arguments tokenized; the two base test sites use
            // `assert_eq!(unsafe { ... })`, so retain their source-level unsafe boundary too.
            let count = node.tokens.to_string().matches("unsafe {").count();
            if count != 0 {
                let name = self.functions.last().expect("macro belongs to a function");
                *self.counts.entry(name.clone()).or_default() += count;
            }
            syn::visit::visit_macro(self, node);
        }

        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            self.enter(node.sig.ident.to_string());
            syn::visit::visit_block(self, &node.block);
            self.leave();
        }

        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            self.enter(node.sig.ident.to_string());
            syn::visit::visit_block(self, &node.block);
            self.leave();
        }
    }

    fn inventory(source: &str) -> BTreeMap<String, usize> {
        let parsed = syn::parse_file(source).expect("parse Rust source");
        let mut visitor = UnsafeInventory::default();
        visitor.visit_file(&parsed);
        visitor.counts
    }

    let expected_base = BTreeMap::from([
        (
            "child_open_options_nonblocking_refuses_a_writerless_fifo_instead_of_blocking".into(),
            1,
        ),
        ("create_new_regular_child_at".into(), 2),
        ("drop".into(), 1),
        ("enumerate_directory_names".into(), 8),
        ("errno_location".into(), 2),
        ("inject_publication_rename_fault".into(), 1),
        ("next_name".into(), 4),
        ("open".into(), 4),
        ("open_child_no_follow".into(), 2),
        ("open_regular_child_for_update".into(), 2),
        ("path_identity_refuses_an_unreadable_ancestor".into(), 1),
        ("rename_child_no_replace".into(), 2),
        ("rename_child_replacing".into(), 1),
        (
            "retained_enumeration_refuses_a_non_directory_without_blocking".into(),
            1,
        ),
        ("retire_captured_regular_child_v2_with".into(), 1),
        ("stat_child_no_follow".into(), 2),
    ]);
    assert_eq!(expected_base.values().sum::<usize>(), 35);

    let mut fs_inventory = inventory(include_str!("fs_custody.rs"));
    let owned: BTreeMap<String, usize> = BTreeMap::from([
        ("create_new_child_directory".into(), 2),
        ("root_command".into(), 1),
    ]);
    for (function, count) in &owned {
        assert_eq!(fs_inventory.remove(function), Some(*count), "{function}");
    }
    assert_eq!(fs_inventory, expected_base);

    let git_inventory = inventory(include_str!("custody_git.rs"));
    assert_eq!(
        git_inventory,
        BTreeMap::from([
            ("deny_effective_write".into(), 1),
            ("effective_uid".into(), 1),
        ])
    );

    // A compiling, owned-function mutation contributes an unexpected site to the AST inventory.
    let mutation = "fn create_new_child_directory() { unsafe { libc::getpid(); } }";
    assert_eq!(
        inventory(mutation).get("create_new_child_directory"),
        Some(&1)
    );
}

#[test]
fn a5a_wrong_digest_prevents_fixture_effect_and_own_digest_allows_it() {
    let (_root_temp, root) = root_fixture();
    let marker = root.canonical_path().join("digest-marker");
    let fixture = FixtureRoute::new(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 'git version 2.54.0';; *) touch digest-marker;; esac\n",
    );
    assert!(matches!(
        GitRunnerV1::admit(
            GitRouteRequestV1::for_test_fixture(
                fixture.path.clone(),
                ExpectedGitDigestV1::from_bytes([0xA5; 32]),
                fixture.directory.path().to_path_buf(),
            )
            .unwrap(),
            &root,
            &names(),
            Instant::now() + Duration::from_secs(1),
        ),
        Err(CustodyGitError::DigestMismatch { .. })
    ));
    assert!(!marker.exists());
    let runner = fixture.admit(&root);
    runner
        .run(
            &root,
            &names(),
            request(GitCommandV1::FsckStrict, vec![]),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
    assert!(marker.exists());
}
