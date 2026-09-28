//! In-crate controls for ADR-0041 slice 2B3a, the capsule reader (phase V: verify and stage).
//!
//! These tests are in-crate because the reader, the 2B2 export harness that publishes their
//! capsules, and the fixture opener are all crate-private. Every capsule here is a real 2B2 export:
//! the fixture-stream harness (`HarnessV1`) or the plan-backed rich clone (`PlanFixtureV1`).
//!
//! The mutation matrix (`.git/a2a-bridge/mutation/`, recorded in the implementation handoff) maps
//! each guard to the controls that turn red when it alone is removed.

use super::*;
use crate::custody_envelope_fixture::{FixtureOpenerV1, OpenerFaultV1, FIXTURE_ENVELOPE_MAGIC_V1};
use crate::custody_export::tests::{
    default_streams, expect_sealed, manifest_for, manifest_from_plan, planned_capability,
    HarnessV1, PlanFixtureV1, IDENTITY_V1,
};
use crate::custody_seal::CustodySealedArtifactV1;
use std::collections::BTreeMap;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::PathBuf;

// ---------------------------------------------------------------------------------------------
// Capsule fixtures
// ---------------------------------------------------------------------------------------------

/// A sealed 2B2 fixture-stream export. The harness owns the temporary tree, so it is returned to
/// keep the capsule alive.
fn exported() -> HarnessV1 {
    let harness = HarnessV1::new();
    let _sealed = harness.run_sealed();
    harness
}

fn read_seal(capsule: &Path) -> CustodySealV1 {
    CustodySealV1::decode_canonical(
        &std::fs::read(capsule.join(RESTORE_SEAL_NAME_V1)).expect("the published seal"),
    )
    .expect("the published seal decodes")
}

/// Overwrites the published seal with `seal`'s canonical encoding.
fn write_seal(capsule: &Path, seal: &CustodySealV1) {
    std::fs::write(
        capsule.join(RESTORE_SEAL_NAME_V1),
        seal.encode_canonical().expect("a canonical seal"),
    )
    .expect("the seal is rewritten");
}

/// `seal` rebuilt canonically with `manifest_digest` and `rows`, keeping every other field.
fn rebuilt_seal(
    seal: &CustodySealV1,
    manifest_digest: Sha256HexV1,
    rows: Vec<CustodySealedArtifactV1>,
) -> CustodySealV1 {
    CustodySealV1::new(
        manifest_digest,
        rows,
        seal.encryption_recipients().to_vec(),
        seal.capsule_format(),
        seal.sealing_tool(),
        seal.sealing_tool_version(),
    )
    .expect("a rebuilt seal")
}

/// One lower-hex character of `digest` changed.
fn edited_digest(digest: &Sha256HexV1) -> Sha256HexV1 {
    let mut hex = digest.as_str().to_owned();
    let first = if hex.starts_with('0') { "1" } else { "0" };
    hex.replace_range(..1, first);
    Sha256HexV1::parse(hex).expect("an edited digest is still a digest")
}

fn artifact_path(capsule: &Path, name: &str) -> PathBuf {
    capsule.join(name)
}

fn create_private_dir_all(path: &Path) {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .expect("an owner-private fixture directory");
}

/// Copies `capsule` to `out` (owner-private, new), replacing the plaintext of each named artifact.
/// The fixture envelope is `A2AFIX1\n` followed by the plaintext, so plaintext = ciphertext[8..].
/// Every ciphertext is re-wrapped, and the seal is rebuilt with `CustodySealV1::new` from the
/// original seal's recipients, format, tool, and version, with fresh
/// `CustodySealedArtifactV1::new(name, len, sha256)` rows. The seal's manifest digest is
/// `manifest_digest.unwrap_or(original)`, and the seal is written via `encode_canonical`.
fn reseal_for_test(
    capsule: &Path,
    out: &Path,
    replace: &[(&str, Vec<u8>)],
    manifest_digest: Option<Sha256HexV1>,
) {
    let seal = read_seal(capsule);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(out)
        .expect("the reseal target is new");
    let mut rows = Vec::new();
    let mut replaced = 0_usize;
    for artifact in seal.artifacts() {
        let name = artifact.name().as_bytes();
        let ciphertext =
            std::fs::read(capsule.join(OsStr::from_bytes(name))).expect("a published ciphertext");
        assert_eq!(
            &ciphertext[..8],
            FIXTURE_ENVELOPE_MAGIC_V1,
            "a fixture envelope"
        );
        let plaintext = match replace
            .iter()
            .find(|(replaced_name, _)| replaced_name.as_bytes() == name)
        {
            Some((_, plaintext)) => {
                replaced += 1;
                plaintext.clone()
            }
            None => ciphertext[8..].to_vec(),
        };
        let rewrapped = [&FIXTURE_ENVELOPE_MAGIC_V1[..], &plaintext].concat();
        let target = out.join(OsStr::from_bytes(name));
        create_private_dir_all(target.parent().expect("an artifact parent"));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)
            .and_then(|mut file| file.write_all(&rewrapped))
            .expect("a resealed ciphertext");
        rows.push(
            CustodySealedArtifactV1::new(
                artifact.name().clone(),
                rewrapped.len() as u64,
                Sha256HexV1::digest(&rewrapped),
            )
            .expect("a resealed row"),
        );
    }
    assert_eq!(
        replaced,
        replace.len(),
        "every replaced name is a sealed artifact"
    );
    let manifest_digest = manifest_digest.unwrap_or_else(|| seal.manifest_digest().clone());
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(out.join(RESTORE_SEAL_NAME_V1))
        .and_then(|mut file| {
            file.write_all(
                &rebuilt_seal(&seal, manifest_digest, rows)
                    .encode_canonical()
                    .expect("a canonical resealed seal"),
            )
        })
        .expect("the resealed seal");
}

/// Every regular file beneath `root`, by relative path, with its bytes.
fn file_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut frontier = vec![root.to_path_buf()];
    while let Some(directory) = frontier.pop() {
        for entry in std::fs::read_dir(&directory).expect("a fixture directory") {
            let path = entry.expect("a fixture entry").path();
            let metadata = std::fs::symlink_metadata(&path).expect("fixture metadata");
            if metadata.is_dir() {
                frontier.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("beneath the root")
                    .to_owned();
                files.insert(relative, std::fs::read(&path).expect("a fixture file"));
            }
        }
    }
    files
}

fn pinned(capsule: &Path) -> PinnedCapsuleV1 {
    pin_and_verify_capsule_v1(capsule).expect("the capsule pins and verifies")
}

fn pin_refusal(capsule: &Path) -> CustodyRestoreErrorV1 {
    pin_and_verify_capsule_v1(capsule).expect_err("the capsule must refuse")
}

// ---------------------------------------------------------------------------------------------
// Task 3: pin the capsule, read the seal, and verify every ciphertext (nothing opened)
// ---------------------------------------------------------------------------------------------

/// The reseal helper's own control: with no replacement, it reproduces every file byte for byte,
/// the seal included.
#[test]
fn reseal_with_no_replacement_is_byte_identical() {
    let harness = exported();
    let area = tempfile::TempDir::new().expect("a fixture area");
    let out = area.path().join("resealed");
    reseal_for_test(&harness.capsule_dir(), &out, &[], None);

    let original = file_bytes(&harness.capsule_dir());
    assert_eq!(original.len(), 7, "six artifacts and the seal");
    assert_eq!(file_bytes(&out), original);
}

#[test]
fn pins_and_verifies_an_exported_capsule() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let seal = capsule.seal.seal();

    assert_eq!(capsule.ciphertexts.len(), seal.artifacts().len());
    assert_eq!(capsule.ciphertexts.len(), 6);
    for (ciphertext, artifact) in capsule.ciphertexts.iter().zip(seal.artifacts()) {
        assert_eq!(&ciphertext.name, artifact.name());
        assert_eq!(ciphertext.length, artifact.byte_length());
        let mut file = &ciphertext.file;
        assert_eq!(file.stream_position().expect("a position"), 0, "rewound");
    }
    assert_eq!(
        capsule.root.canonical_path(),
        harness
            .capsule_dir()
            .canonicalize()
            .expect("a canonical capsule")
    );
}

/// Review focus 4: the seal is read through the bounded reader. Exactly the bound is read (and
/// then refused as JSON); one byte more is refused unread.
#[test]
fn oversize_seal_refuses() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let bound = usize::try_from(RESTORE_CONTROL_READ_BOUND_V1).expect("the bound fits");

    std::fs::write(capsule.join(RESTORE_SEAL_NAME_V1), vec![b' '; bound + 1]).unwrap();
    let error = pin_refusal(&capsule);
    assert!(
        matches!(error, CustodyRestoreErrorV1::SealUnreadable(_)),
        "observed {error:?}"
    );

    std::fs::write(capsule.join(RESTORE_SEAL_NAME_V1), vec![b' '; bound]).unwrap();
    let error = pin_refusal(&capsule);
    assert!(
        matches!(error, CustodyRestoreErrorV1::SealInvalid(_)),
        "observed {error:?}"
    );
}

/// T1: a malformed or out-of-bounds seal refuses in task 3.
#[test]
fn malformed_seal_refuses() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let original = std::fs::read(capsule.join(RESTORE_SEAL_NAME_V1)).unwrap();
    let seal = read_seal(&capsule);

    let not_json = b"{\"schema\":".to_vec();
    let mut not_canonical = original.clone();
    not_canonical.insert(1, b' ');
    assert!(serde_json::from_slice::<serde_json::Value>(&not_canonical).is_ok());
    let mut rows = seal.artifacts().to_vec();
    rows[0] =
        CustodySealedArtifactV1::new(rows[0].name().clone(), 0, rows[0].sha256().clone()).unwrap();
    let zero_length = rebuilt_seal(&seal, seal.manifest_digest().clone(), rows)
        .encode_canonical()
        .unwrap();

    for (row, bytes) in [
        ("not JSON", not_json),
        ("not canonical", not_canonical),
        ("a zero-length artifact", zero_length),
    ] {
        std::fs::write(capsule.join(RESTORE_SEAL_NAME_V1), &bytes).unwrap();
        let error = pin_refusal(&capsule);
        assert!(
            matches!(error, CustodyRestoreErrorV1::SealInvalid(_)),
            "{row}: observed {error:?}"
        );
        let error = refused_before_the_work_tree(&capsule);
        assert!(
            matches!(error, CustodyRestoreErrorV1::SealInvalid(_)),
            "{row}, through the pipeline: observed {error:?}"
        );
    }
}

/// T2: a canonical seal whose artifact digest disagrees with the ciphertext refuses in task 3.
#[test]
fn seal_artifact_digest_edit_refuses_before_staging() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let seal = read_seal(&capsule);
    let mut rows = seal.artifacts().to_vec();
    let edited = rows.len() - 1;
    rows[edited] = CustodySealedArtifactV1::new(
        rows[edited].name().clone(),
        rows[edited].byte_length(),
        edited_digest(rows[edited].sha256()),
    )
    .unwrap();
    write_seal(
        &capsule,
        &rebuilt_seal(&seal, seal.manifest_digest().clone(), rows),
    );

    let error = pin_refusal(&capsule);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::CiphertextDigest { name }
            if name.as_bytes() == seal.artifacts()[edited].name().as_bytes()),
        "observed {error:?}"
    );
    let error = refused_before_the_work_tree(&capsule);
    assert!(
        matches!(error, CustodyRestoreErrorV1::CiphertextDigest { .. }),
        "through the pipeline: observed {error:?}"
    );
}

/// The T3 boundary: a canonical edit of the seal's manifest digest agrees with every ciphertext,
/// so task 3 admits it. Task 5's binding owns its refusal.
#[test]
fn seal_manifest_digest_edit_passes_task_3() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let seal = read_seal(&capsule);
    write_seal(
        &capsule,
        &rebuilt_seal(
            &seal,
            edited_digest(seal.manifest_digest()),
            seal.artifacts().to_vec(),
        ),
    );

    let pinned = pinned(&capsule);
    assert_eq!(
        pinned.seal.seal().manifest_digest(),
        &edited_digest(seal.manifest_digest())
    );
}

#[test]
fn ciphertext_length_mismatch_refuses() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    std::fs::OpenOptions::new()
        .append(true)
        .open(artifact_path(&capsule, "payload/worktree.bin.enc"))
        .and_then(|mut file| file.write_all(b"x"))
        .unwrap();

    let error = pin_refusal(&capsule);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::CiphertextLength { name }
            if name == "payload/worktree.bin.enc"),
        "observed {error:?}"
    );
}

#[test]
fn ciphertext_digest_mismatch_refuses() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let path = artifact_path(&capsule, "control/manifest.json.enc");
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();

    let error = pin_refusal(&capsule);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::CiphertextDigest { name }
            if name == "control/manifest.json.enc"),
        "observed {error:?}"
    );
}

#[test]
fn missing_artifact_refuses() {
    for name in ["payload/worktree.bin.enc", "git/objects.pack.enc"] {
        let harness = exported();
        let capsule = harness.capsule_dir();
        std::fs::remove_file(artifact_path(&capsule, name)).unwrap();

        let error = pin_refusal(&capsule);
        assert!(
            matches!(&error, CustodyRestoreErrorV1::CapsuleEntryMissing { name: missing }
                if missing == name),
            "{name}: observed {error:?}"
        );
    }
}

/// Review focus 2: any entry the seal does not name refuses, at the root or in a component
/// directory, a leftover exporter staging name included.
#[test]
fn capsule_extra_entry_refuses() {
    for extra in ["stray", ".a2a-staging-seal", "control/.a2a-staging-x"] {
        let harness = exported();
        let capsule = harness.capsule_dir();
        std::fs::write(capsule.join(extra), b"not sealed").unwrap();

        let error = pin_refusal(&capsule);
        assert!(
            matches!(&error, CustodyRestoreErrorV1::CapsuleEntryUnexpected { name }
                if name == extra),
            "{extra}: observed {error:?}"
        );
    }
}

/// Review focus 1: an artifact replaced by a symlink to a byte-identical copy refuses, and is
/// never followed.
#[test]
fn capsule_symlinked_artifact_refuses() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let elsewhere = tempfile::TempDir::new().unwrap();
    let copy = elsewhere.path().join("manifest.json.enc");
    let path = artifact_path(&capsule, "control/manifest.json.enc");
    std::fs::rename(&path, &copy).unwrap();
    std::os::unix::fs::symlink(&copy, &path).unwrap();

    let error = pin_refusal(&capsule);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::SymlinkRefused { name }
            if name == "control/manifest.json.enc"),
        "observed {error:?}"
    );
}

/// Review focus 1: a component directory replaced by a symlink to an identical directory
/// refuses, and is never followed.
#[test]
fn capsule_symlinked_component_refuses() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let elsewhere = tempfile::TempDir::new().unwrap();
    let copy = elsewhere.path().join("payload");
    std::fs::rename(capsule.join("payload"), &copy).unwrap();
    std::os::unix::fs::symlink(&copy, capsule.join("payload")).unwrap();

    let error = pin_refusal(&capsule);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::SymlinkRefused { name } if name == "payload"),
        "observed {error:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Task 4: destination preflight, the work tree, and the ledger
// ---------------------------------------------------------------------------------------------

/// The bytes task 4 charges: `.restore-work/` and `.restore-work/plain/`, one 64 KiB entry
/// allowance each (2B2's `ScratchLedgerV1` allowance).
const TASK_4_CHARGE_V1: u64 = 2 * 64 * 1024;

/// A new, empty, owner-private destination beneath its own temporary area.
struct DestinationV1 {
    _area: tempfile::TempDir,
    area: PathBuf,
    path: PathBuf,
}

impl DestinationV1 {
    fn new() -> Self {
        let area = tempfile::TempDir::new().expect("a destination area");
        let canonical = area.path().canonicalize().expect("a canonical area");
        let path = canonical.join("dest");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("an owner-private destination");
        Self {
            _area: area,
            area: canonical,
            path,
        }
    }

    fn work(&self) -> PathBuf {
        self.path.join(".restore-work")
    }

    fn entries(&self) -> Vec<String> {
        entries_beneath(&self.path)
    }
}

/// Every entry beneath `root`, directories with a trailing `/`, sorted.
fn entries_beneath(root: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let mut frontier = vec![root.to_path_buf()];
    while let Some(directory) = frontier.pop() {
        for entry in std::fs::read_dir(&directory).expect("a fixture directory") {
            let path = entry.expect("a fixture entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("beneath the root")
                .to_string_lossy()
                .into_owned();
            if std::fs::symlink_metadata(&path).expect("metadata").is_dir() {
                entries.push(format!("{relative}/"));
                frontier.push(path);
            } else {
                entries.push(relative);
            }
        }
    }
    entries.sort();
    entries
}

fn prepared(capsule: &PinnedCapsuleV1, destination: &Path) -> RestoreDestinationV1 {
    prepare_destination_v1(destination, capsule, CustodyRestoreBudgetV1::ceiling())
        .expect("the destination prepares")
}

fn prepare_refusal(
    capsule: &PinnedCapsuleV1,
    destination: &Path,
    budget: CustodyRestoreBudgetV1,
) -> CustodyRestoreErrorV1 {
    prepare_destination_v1(destination, capsule, budget).expect_err("the destination must refuse")
}

#[test]
fn prepares_a_new_empty_destination() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let destination = DestinationV1::new();
    let prepared = prepared(&capsule, &destination.path);

    assert_eq!(
        destination.entries(),
        [".restore-work/", ".restore-work/plain/"]
    );
    assert_eq!(prepared.ledger.borrow().used(), TASK_4_CHARGE_V1);
    assert_eq!(prepared.root.canonical_path(), destination.path);
}

#[test]
fn destination_must_be_empty() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let destination = DestinationV1::new();
    std::fs::write(destination.path.join("occupant"), b"already here").unwrap();

    let error = prepare_refusal(
        &capsule,
        &destination.path,
        CustodyRestoreBudgetV1::ceiling(),
    );
    assert!(
        matches!(&error, CustodyRestoreErrorV1::DestinationInvalid(detail)
            if detail.contains("not empty")),
        "observed {error:?}"
    );
    assert_eq!(destination.entries(), ["occupant"]);
}

/// The capsule is pinned first, so its exterior check has already passed when the destination is
/// made inside it: only the overlap predicate refuses.
#[test]
fn destination_inside_capsule_refuses() {
    let harness = exported();
    let capsule_dir = harness.capsule_dir();
    let capsule = pinned(&capsule_dir);
    let inside = capsule_dir.join("payload/restore-here");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&inside)
        .unwrap();

    let error = prepare_refusal(&capsule, &inside, CustodyRestoreBudgetV1::ceiling());
    assert!(
        matches!(&error, CustodyRestoreErrorV1::DestinationInvalid(detail)
            if detail.contains("lies inside")),
        "observed {error:?}"
    );
    assert_eq!(entries_beneath(&inside), Vec::<String>::new());
}

/// The destination holds the capsule. The 2B2 emptiness check would also refuse it, so the
/// fixture bypasses that one outer layer: the overlap predicate is the only guard.
#[test]
fn capsule_inside_destination_refuses() {
    let harness = exported();
    let destination = DestinationV1::new();
    let capsule_dir = destination.path.join("capsule");
    reseal_for_test(&harness.capsule_dir(), &capsule_dir, &[], None);
    let capsule = pinned(&capsule_dir);
    let _bypass =
        crate::custody_export::set_export_bypass_for_test(crate::custody_export::ExportBypassV1 {
            scratch_emptiness: true,
            ..crate::custody_export::ExportBypassV1::default()
        });

    let error = prepare_refusal(
        &capsule,
        &destination.path,
        CustodyRestoreBudgetV1::ceiling(),
    );
    assert!(
        matches!(&error, CustodyRestoreErrorV1::DestinationInvalid(detail)
            if detail.contains("contains")),
        "observed {error:?}"
    );
    assert!(!destination.work().exists());
}

/// Review focus 5: the destination is renamed away and replaced by an empty directory between
/// its preflight and the first create. The restore refuses, and writes into neither directory.
#[test]
fn destination_swapped_before_first_write_refuses() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let destination = DestinationV1::new();
    let moved = destination.area.join("dest-moved");
    let swap_to = moved.clone();
    let _hook = install_restore_hook_for_test(RestoreHookPointV1::BeforeFirstCreate, move |path| {
        std::fs::rename(path, &swap_to).expect("the destination moves away");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .expect("an empty replacement");
    });

    let error = prepare_refusal(
        &capsule,
        &destination.path,
        CustodyRestoreBudgetV1::ceiling(),
    );
    assert!(
        matches!(error, CustodyRestoreErrorV1::IdentityChanged(_)),
        "observed {error:?}"
    );
    assert!(moved.is_dir(), "the hook ran");
    assert_eq!(entries_beneath(&moved), Vec::<String>::new());
    assert_eq!(destination.entries(), Vec::<String>::new());
}

#[test]
fn ledger_max_and_max_plus_one() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());

    let at_max = DestinationV1::new();
    let budget = CustodyRestoreBudgetV1::new(TASK_4_CHARGE_V1).unwrap();
    let prepared = prepare_destination_v1(&at_max.path, &capsule, budget)
        .expect("exactly the charged bytes admit");
    assert_eq!(prepared.ledger.borrow().used(), TASK_4_CHARGE_V1);

    let short = DestinationV1::new();
    let budget = CustodyRestoreBudgetV1::new(TASK_4_CHARGE_V1 - 1).unwrap();
    let error = prepare_refusal(&capsule, &short.path, budget);
    assert!(
        matches!(error, CustodyRestoreErrorV1::Budget(_)),
        "observed {error:?}"
    );
    assert!(
        !short.work().join("plain").exists(),
        "created past the budget"
    );
}

#[test]
fn budget_ceiling_and_ceiling_plus_one() {
    let ceiling = CustodyRestoreBudgetV1::new(RESTORE_BYTES_CEILING_V1).unwrap();
    assert_eq!(ceiling.max_restore_bytes(), RESTORE_BYTES_CEILING_V1);
    assert_eq!(ceiling, CustodyRestoreBudgetV1::ceiling());
    assert_eq!(RESTORE_BYTES_CEILING_V1, 10 * 1024 * 1024 * 1024);
    for refused in [RESTORE_BYTES_CEILING_V1 + 1, 0] {
        assert!(
            matches!(
                CustodyRestoreBudgetV1::new(refused),
                Err(CustodyRestoreErrorV1::Budget(_))
            ),
            "{refused}"
        );
    }
}

#[test]
fn mount_inside_destination_refuses() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());

    let destination = DestinationV1::new();
    let inside = [destination.path.as_os_str().as_bytes(), b"/inner"].concat();
    let _census = crate::custody_mounts::seam::install_list(vec![b"/".to_vec(), inside]);
    let error = prepare_refusal(
        &capsule,
        &destination.path,
        CustodyRestoreBudgetV1::ceiling(),
    );
    assert!(
        matches!(error, CustodyRestoreErrorV1::MountBoundary(_)),
        "observed {error:?}"
    );
    assert!(!destination.work().exists());
    drop(_census);

    for admitted in [
        |destination: &DestinationV1| destination.path.as_os_str().as_bytes().to_vec(),
        |destination: &DestinationV1| destination.area.as_os_str().as_bytes().to_vec(),
    ] {
        let destination = DestinationV1::new();
        let _census =
            crate::custody_mounts::seam::install_list(vec![b"/".to_vec(), admitted(&destination)]);
        prepared(&capsule, &destination.path);
    }
}

#[test]
fn census_failure_refuses() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let destination = DestinationV1::new();
    let _census = crate::custody_mounts::seam::install(Box::new(|_| {
        Err(crate::custody_mounts::CustodyMountErrorV1::MalformedLine { line: 1 })
    }));

    let error = prepare_refusal(
        &capsule,
        &destination.path,
        CustodyRestoreBudgetV1::ceiling(),
    );
    assert!(
        matches!(error, CustodyRestoreErrorV1::MountBoundary(_)),
        "observed {error:?}"
    );
    assert!(!destination.work().exists());
}

#[test]
fn created_directory_on_another_device_refuses() {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let destination = DestinationV1::new();
    let _device = override_created_dev_for_test(".restore-work", u64::MAX);

    let error = prepare_refusal(
        &capsule,
        &destination.path,
        CustodyRestoreBudgetV1::ceiling(),
    );
    assert!(
        matches!(&error, CustodyRestoreErrorV1::DeviceCrossing { name } if name == ".restore-work"),
        "observed {error:?}"
    );
    assert!(!destination.work().join("plain").exists());
}

// ---------------------------------------------------------------------------------------------
// Task 5: open the control artifacts and bind through 2B1
// ---------------------------------------------------------------------------------------------

const MANIFEST_V1: &str = "control/manifest.json.enc";
const INDEX_V1: &str = "control/capsule-index.json.enc";
const POLICY_V1: &str = "control/restore-policy.json.enc";
const CONTROL_NAMES_V1: [&str; 3] = [INDEX_V1, MANIFEST_V1, POLICY_V1];

fn logical(name: &str) -> LosslessPathV1 {
    LosslessPathV1::from_bytes(name.as_bytes().to_vec())
}

/// The plaintext of a published fixture envelope.
fn plaintext_of(capsule: &Path, name: &str) -> Vec<u8> {
    let ciphertext = std::fs::read(capsule.join(name)).expect("a published ciphertext");
    assert_eq!(&ciphertext[..8], FIXTURE_ENVELOPE_MAGIC_V1);
    ciphertext[8..].to_vec()
}

/// A second fixture-stream export whose generation differs, and its canonical manifest.
fn manifest_of_another_generation() -> Vec<u8> {
    let generation = "generation-2b3a-other";
    let mut other = HarnessV1::new();
    let [unit, run, materialization, _] = IDENTITY_V1;
    other.manifest = manifest_for(
        [unit, run, materialization, generation],
        other.manifest.original_objects().to_vec(),
    );
    other.streams = default_streams(generation);
    let _sealed = other.run_sealed();
    plaintext_of(&other.capsule_dir(), MANIFEST_V1)
}

/// Pins `capsule`, prepares a new destination, and binds the controls, which must bind.
fn bind(
    capsule: &Path,
    opener: &FixtureOpenerV1,
) -> (DestinationV1, BoundControlV1, Vec<StagedPlaintextV1>) {
    let destination = DestinationV1::new();
    let pinned = pinned(capsule);
    let prepared = prepared(&pinned, &destination.path);
    let (control, staged) = bind_control_v1(&prepared, &pinned, opener).expect("the controls bind");
    (destination, control, staged)
}

/// The whole phase V's refusal of `capsule`, with the destination it left. No pack or payload
/// plaintext exists.
fn restore_refusal(
    capsule: &Path,
    opener: &FixtureOpenerV1,
) -> (CustodyRestoreErrorV1, DestinationV1) {
    let destination = DestinationV1::new();
    let error = match restore(capsule, &destination.path, opener) {
        Ok(_) => panic!("the restore must refuse"),
        Err(error) => error,
    };
    for absent in ["plain/payload", "plain/git"] {
        assert!(
            !destination.work().join(absent).exists(),
            "{absent} was staged before the refusal: {error:?}"
        );
    }
    (error, destination)
}

/// Only the three control plaintexts, if any, are staged.
fn staged_controls(destination: &DestinationV1) -> Vec<String> {
    entries_beneath(&destination.work().join("plain"))
}

#[test]
fn binds_an_exported_capsule() {
    let harness = exported();
    let (destination, control, staged) = bind(&harness.capsule_dir(), &FixtureOpenerV1::honest());

    assert_eq!(control.manifest, harness.manifest);
    assert_eq!(
        control.binding.manifest_digest(),
        &harness.manifest.content_digest().unwrap()
    );
    assert_eq!(control.index.artifacts().len(), 6);
    assert_eq!(
        control.policy,
        crate::custody_capsule::CustodyRestorePolicyV1::inert()
    );
    let mut names: Vec<&[u8]> = staged.iter().map(|one| one.name.as_bytes()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        CONTROL_NAMES_V1.map(str::as_bytes).to_vec(),
        "exactly the three controls"
    );
    for one in &staged {
        let plaintext = plaintext_of(&harness.capsule_dir(), &lossy(one.name.as_bytes()));
        assert_eq!(one.length, plaintext.len() as u64);
        assert_eq!(sha256_hex(&one.sha256), Sha256HexV1::digest(&plaintext));
        let mut file = &one.file;
        assert_eq!(file.stream_position().unwrap(), 0, "rewound");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, plaintext);
    }
    assert_eq!(
        staged_controls(&destination),
        [
            "control/",
            "control/capsule-index.json.enc",
            "control/manifest.json.enc",
            "control/restore-policy.json.enc"
        ]
    );
}

/// The staged plaintext and every staging entry are charged: exactly the charge admits, and one
/// byte less refuses before the write that would exceed it.
#[test]
fn staging_ledger_max_and_max_plus_one() {
    let harness = exported();
    let capsule_dir = harness.capsule_dir();
    let plaintext: u64 = CONTROL_NAMES_V1
        .iter()
        .map(|name| plaintext_of(&capsule_dir, name).len() as u64)
        .sum();
    // `.restore-work/`, `plain/`, `plain/control/`, and three leaves.
    let charge = 6 * 64 * 1024 + plaintext;
    let capsule = pinned(&capsule_dir);

    let at_max = DestinationV1::new();
    let prepared = prepare_destination_v1(
        &at_max.path,
        &capsule,
        CustodyRestoreBudgetV1::new(charge).unwrap(),
    )
    .unwrap();
    bind_control_v1(&prepared, &capsule, &FixtureOpenerV1::honest()).expect("exactly the charge");
    assert_eq!(prepared.ledger.borrow().used(), charge);

    let short = DestinationV1::new();
    let prepared = prepare_destination_v1(
        &short.path,
        &capsule,
        CustodyRestoreBudgetV1::new(charge - 1).unwrap(),
    )
    .unwrap();
    let error = bind_control_v1(&prepared, &capsule, &FixtureOpenerV1::honest())
        .expect_err("one byte short refuses");
    assert!(
        matches!(error, CustodyRestoreErrorV1::Budget(_)),
        "observed {error:?}"
    );
    assert!(prepared.ledger.borrow().used() < charge);
}

/// T3: a manifest from another generation, resealed so every ciphertext agrees with the seal.
/// Only the three-way digest disagreement remains.
#[test]
fn tampered_manifest_refuses_before_payload_staging() {
    let harness = exported();
    let foreign = manifest_of_another_generation();
    assert_ne!(foreign, plaintext_of(&harness.capsule_dir(), MANIFEST_V1));
    let area = tempfile::TempDir::new().unwrap();
    let resealed = area.path().join("capsule");
    reseal_for_test(
        &harness.capsule_dir(),
        &resealed,
        &[(MANIFEST_V1, foreign)],
        None,
    );

    let (error, destination) = restore_refusal(&resealed, &FixtureOpenerV1::honest());
    assert!(
        matches!(
            error,
            CustodyRestoreErrorV1::Binding(CustodyCapsuleErrorV1::ThreeWayDigestMismatch)
        ),
        "observed {error:?}"
    );
    assert!(staged_controls(&destination).len() <= 4);
}

#[test]
fn index_mapping_mismatch_refuses() {
    let harness = exported();
    let capsule_dir = harness.capsule_dir();
    let index =
        CustodyCapsuleIndexV1::decode_canonical(&plaintext_of(&capsule_dir, INDEX_V1)).unwrap();
    let rows = index
        .artifacts()
        .iter()
        .filter(|row| row.name().as_bytes() != b"payload/worktree.bin.enc")
        .cloned()
        .collect();
    let shortened = CustodyCapsuleIndexV1::new(index.manifest_digest().clone(), rows)
        .unwrap()
        .encode_canonical()
        .unwrap();
    let area = tempfile::TempDir::new().unwrap();
    let resealed = area.path().join("capsule");
    reseal_for_test(&capsule_dir, &resealed, &[(INDEX_V1, shortened)], None);

    let (error, _destination) = restore_refusal(&resealed, &FixtureOpenerV1::honest());
    assert!(
        matches!(
            error,
            CustodyRestoreErrorV1::Binding(CustodyCapsuleErrorV1::MissingArtifact)
        ),
        "observed {error:?}"
    );
}

/// The policy's fields are closed enums whose only admitted value is `disabled` (or
/// `forbidden`), so an enabled hook is refused at decode.
#[test]
fn non_inert_policy_refuses() {
    let harness = exported();
    let capsule_dir = harness.capsule_dir();
    let policy = String::from_utf8(plaintext_of(&capsule_dir, POLICY_V1)).unwrap();
    assert_eq!(policy.matches("\"hooks\":\"disabled\"").count(), 1);
    let enabled = policy.replace("\"hooks\":\"disabled\"", "\"hooks\":\"enabled\"");
    let area = tempfile::TempDir::new().unwrap();
    let resealed = area.path().join("capsule");
    reseal_for_test(
        &capsule_dir,
        &resealed,
        &[(POLICY_V1, enabled.into_bytes())],
        None,
    );

    let (error, _destination) = restore_refusal(&resealed, &FixtureOpenerV1::honest());
    assert!(
        matches!(&error, CustodyRestoreErrorV1::ControlDecode { name } if name == POLICY_V1),
        "observed {error:?}"
    );
}

/// Review focus 3: the sink measures the bytes the opener wrote, and a receipt that disagrees
/// refuses.
#[test]
fn opener_lying_receipt_refuses() {
    let harness = exported();
    let opener = FixtureOpenerV1::faulted(OpenerFaultV1 {
        lie_about_receipt_length: true,
        ..OpenerFaultV1::default()
    });
    let (error, _destination) = restore_refusal(&harness.capsule_dir(), &opener);
    assert!(
        matches!(error, CustodyRestoreErrorV1::OpenerReceiptMismatch { .. }),
        "observed {error:?}"
    );
}

#[test]
fn opener_refusal_propagates() {
    let harness = exported();
    let opener = FixtureOpenerV1::faulted(OpenerFaultV1 {
        refuse: true,
        ..OpenerFaultV1::default()
    });
    let (error, _destination) = restore_refusal(&harness.capsule_dir(), &opener);
    assert!(
        matches!(
            error,
            CustodyRestoreErrorV1::Open(CustodyCapsuleErrorV1::InvalidInput)
        ),
        "observed {error:?}"
    );
    assert_eq!(opener.calls(), 1, "the first refusal stops the restore");
}

/// Review focus 4: a control plaintext one byte over the read bound refuses at the bound.
#[test]
fn oversize_control_plaintext_refuses() {
    let harness = exported();
    let bound = usize::try_from(RESTORE_CONTROL_READ_BOUND_V1).unwrap();
    let area = tempfile::TempDir::new().unwrap();
    let resealed = area.path().join("capsule");
    reseal_for_test(
        &harness.capsule_dir(),
        &resealed,
        &[(MANIFEST_V1, vec![b' '; bound + 1])],
        None,
    );

    let (error, _destination) = restore_refusal(&resealed, &FixtureOpenerV1::honest());
    assert!(
        matches!(&error, CustodyRestoreErrorV1::ControlOversize { name } if name == MANIFEST_V1),
        "observed {error:?}"
    );
}

/// Counts every byte requested of it, over `length` bytes of data.
struct CountingReaderV1 {
    length: u64,
    position: u64,
    requested: u64,
}

impl Read for CountingReaderV1 {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.requested += buffer.len() as u64;
        let take = (self.length - self.position).min(buffer.len() as u64) as usize;
        buffer[..take].fill(b'x');
        self.position += take as u64;
        Ok(take)
    }
}

/// Fails any read that asks for a byte past `limit`.
struct ProbeReaderV1 {
    limit: u64,
    position: u64,
}

impl Read for ProbeReaderV1 {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.position + buffer.len() as u64 > self.limit {
            return Err(io::Error::other("a read past the probe byte"));
        }
        buffer.fill(b'x');
        self.position += buffer.len() as u64;
        Ok(buffer.len())
    }
}

#[test]
fn bounded_reader_never_requests_past_the_probe() {
    let bound = RESTORE_CONTROL_READ_BOUND_V1;
    let mut counting = CountingReaderV1 {
        length: 4 * 1024 * 1024,
        position: 0,
        requested: 0,
    };
    assert_eq!(
        read_bounded_v1(&mut counting, bound),
        Err(BoundedReadErrorV1::Oversize)
    );
    assert!(
        counting.requested <= bound + 1,
        "{} requested",
        counting.requested
    );

    let mut probe = ProbeReaderV1 {
        limit: bound + 1,
        position: 0,
    };
    assert_eq!(
        read_bounded_v1(&mut probe, bound),
        Err(BoundedReadErrorV1::Oversize)
    );

    for length in [0, 1, bound] {
        let mut exact = CountingReaderV1 {
            length,
            position: 0,
            requested: 0,
        };
        let bytes = read_bounded_v1(&mut exact, bound).expect("within the bound");
        assert_eq!(bytes.len() as u64, length);
        assert!(bytes.capacity() as u64 <= bound + 1);
    }
}

/// T3: a canonical edit of the seal's manifest digest passes task 3 and refuses at binding, with
/// only the control plaintexts staged.
#[test]
fn seal_manifest_digest_edit_refuses_at_binding() {
    let harness = exported();
    let capsule = harness.capsule_dir();
    let seal = read_seal(&capsule);
    write_seal(
        &capsule,
        &rebuilt_seal(
            &seal,
            edited_digest(seal.manifest_digest()),
            seal.artifacts().to_vec(),
        ),
    );

    let (error, destination) = restore_refusal(&capsule, &FixtureOpenerV1::honest());
    assert!(
        matches!(
            error,
            CustodyRestoreErrorV1::Binding(CustodyCapsuleErrorV1::ThreeWayDigestMismatch)
        ),
        "observed {error:?}"
    );
    let staged = staged_controls(&destination);
    assert!(
        staged
            .iter()
            .all(|entry| entry == "control/" || CONTROL_NAMES_V1.contains(&entry.as_str())),
        "{staged:?}"
    );
    assert_eq!(staged.len(), 4, "{staged:?}");
}

/// Review focus 6: after every ciphertext is verified, one control ciphertext is rewritten in place,
/// through the same inode, with a same-length, internally valid envelope of other plaintext. The
/// bytes the opener consumes are re-verified against the seal.
#[test]
fn ciphertext_changed_after_verification_refuses() {
    let harness = exported();
    let _hook = after_ciphertext_verification_for_test(|capsule| {
        let path = capsule.join(POLICY_V1);
        let original = std::fs::read(&path).unwrap();
        let text = String::from_utf8(original[8..].to_vec()).unwrap();
        let other = text.replacen("custody", "CUSTODY", 1);
        assert_ne!(other, text);
        let rewritten = [&FIXTURE_ENVELOPE_MAGIC_V1[..], other.as_bytes()].concat();
        assert_eq!(rewritten.len(), original.len());
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all(&rewritten).unwrap();
        file.sync_all().unwrap();
    });

    let (error, _destination) = restore_refusal(&harness.capsule_dir(), &FixtureOpenerV1::honest());
    assert!(
        matches!(&error, CustodyRestoreErrorV1::CiphertextChanged { name } if name == POLICY_V1),
        "observed {error:?}"
    );
}

/// A control ciphertext truncated in place after verification: the source cannot serve its
/// verified length.
#[test]
fn ciphertext_truncated_after_verification_refuses() {
    let harness = exported();
    let _hook = after_ciphertext_verification_for_test(|capsule| {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(capsule.join(MANIFEST_V1))
            .unwrap();
        file.set_len(8).unwrap();
    });

    let (error, _destination) = restore_refusal(&harness.capsule_dir(), &FixtureOpenerV1::honest());
    assert!(
        matches!(&error, CustodyRestoreErrorV1::CiphertextChanged { name } if name == MANIFEST_V1),
        "observed {error:?}"
    );
}

/// An opener that stops after one chunk of a multi-chunk ciphertext and finishes anyway.
#[test]
fn opener_stopping_early_refuses() {
    let harness = exported();
    let _chunks = override_chunk_bytes_for_test(64);
    let opener = FixtureOpenerV1::faulted(OpenerFaultV1 {
        stop_after_chunks: Some(1),
        ..OpenerFaultV1::default()
    });
    let (error, _destination) = restore_refusal(&harness.capsule_dir(), &opener);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::CiphertextChanged { name } if name == MANIFEST_V1),
        "observed {error:?}"
    );
}

/// With a small chunk, every control is opened across many chunks, the magic split included.
#[test]
fn binds_across_small_chunks() {
    let harness = exported();
    let _chunks = override_chunk_bytes_for_test(3);
    let (_destination, control, _staged) = bind(&harness.capsule_dir(), &FixtureOpenerV1::honest());
    assert_eq!(control.manifest, harness.manifest);
}

/// Pins and prepares the exported capsule, then plants something at `plain/control/` before the
/// first control is staged, and binds.
fn bind_with_planted_control_directory(
    plant: impl FnOnce(&Path),
) -> (CustodyRestoreErrorV1, DestinationV1) {
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let destination = DestinationV1::new();
    let prepared = prepared(&capsule, &destination.path);
    plant(&destination.work().join("plain/control"));
    let error = bind_control_v1(&prepared, &capsule, &FixtureOpenerV1::honest())
        .expect_err("a planted staging directory refuses");
    (error, destination)
}

#[test]
fn shared_staging_directory_custody() {
    // A real directory planted before the first control is staged.
    let (error, destination) = bind_with_planted_control_directory(|path| {
        std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    });
    assert!(
        matches!(&error, CustodyRestoreErrorV1::StagingCollision { name }
            if name == ".restore-work/plain/control"),
        "planted directory: observed {error:?}"
    );
    assert_eq!(staged_controls(&destination), ["control/"]);

    // A symlink to an outside directory planted there.
    let outside = tempfile::TempDir::new().unwrap();
    let target = outside.path().to_path_buf();
    let (error, destination) = bind_with_planted_control_directory(move |path| {
        std::os::unix::fs::symlink(&target, path).unwrap();
    });
    assert!(
        matches!(&error, CustodyRestoreErrorV1::StagingCollision { name }
            if name == ".restore-work/plain/control"),
        "planted symlink: observed {error:?}"
    );
    assert_eq!(entries_beneath(outside.path()), Vec::<String>::new());
    assert_eq!(staged_controls(&destination), ["control"]);

    // After the first control is staged, the created `plain/control/` is renamed away and a
    // fresh directory takes its place.
    let harness = exported();
    let capsule = pinned(&harness.capsule_dir());
    let destination = DestinationV1::new();
    let prepared = prepared(&capsule, &destination.path);
    stage_one_v1(
        &prepared,
        &capsule,
        &logical(MANIFEST_V1),
        &FixtureOpenerV1::honest(),
    )
    .expect("the first control stages");
    let plain = destination.work().join("plain");
    std::fs::rename(plain.join("control"), plain.join("control-moved")).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(plain.join("control"))
        .unwrap();
    let error = stage_one_v1(
        &prepared,
        &capsule,
        &logical(INDEX_V1),
        &FixtureOpenerV1::honest(),
    )
    .expect_err("a replaced staging directory refuses");
    assert!(
        matches!(error, CustodyRestoreErrorV1::IdentityChanged(_)),
        "replaced directory: observed {error:?}"
    );
    assert_eq!(
        staged_controls(&destination),
        [
            "control-moved/",
            "control-moved/manifest.json.enc",
            "control/"
        ]
    );
}

#[test]
fn staging_collision_refuses() {
    let first_leaf = ".restore-work/plain/control/manifest.json.enc";

    // A regular file planted at the first staged leaf.
    let harness = exported();
    let _hook = install_restore_hook_for_test(RestoreHookPointV1::BeforeLeafCreate, |path| {
        if !path.exists() {
            std::fs::write(path, b"planted").unwrap();
        }
    });
    let (error, destination) = restore_refusal(&harness.capsule_dir(), &FixtureOpenerV1::honest());
    drop(_hook);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::StagingCollision { name } if name == first_leaf),
        "planted file: observed {error:?}"
    );
    assert_eq!(
        std::fs::read(destination.path.join(first_leaf)).unwrap(),
        b"planted"
    );

    // A symlink to a file outside the destination planted there.
    let harness = exported();
    let outside = tempfile::TempDir::new().unwrap();
    let target = outside.path().join("outside-file");
    std::fs::write(&target, b"outside").unwrap();
    let link_to = target.clone();
    let _hook = install_restore_hook_for_test(RestoreHookPointV1::BeforeLeafCreate, move |path| {
        if std::fs::symlink_metadata(path).is_err() {
            std::os::unix::fs::symlink(&link_to, path).unwrap();
        }
    });
    let (error, destination) = restore_refusal(&harness.capsule_dir(), &FixtureOpenerV1::honest());
    assert!(
        matches!(&error, CustodyRestoreErrorV1::StagingCollision { name } if name == first_leaf),
        "planted symlink: observed {error:?}"
    );
    assert_eq!(
        std::fs::read_link(destination.path.join(first_leaf)).unwrap(),
        target
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"outside");
    assert_eq!(entries_beneath(outside.path()), ["outside-file"]);
}

// ---------------------------------------------------------------------------------------------
// Task 6: stage the pack and the payloads, then decode-verify every frame
// ---------------------------------------------------------------------------------------------

const WORKTREE_PAYLOAD_V1: &str = "payload/worktree.bin.enc";
const INDEX_PAYLOAD_V1: &str = "payload/index.bin.enc";

fn frame_budget() -> CustodyFrameBudgetV1 {
    CustodyFrameBudgetV1::new(1 << 20, 1 << 30).expect("the fixture frame budget")
}

/// The whole of phase V, at the ceiling budget.
fn restore(
    capsule: &Path,
    destination: &Path,
    opener: &FixtureOpenerV1,
) -> Result<VerifiedCapsuleV1, CustodyRestoreErrorV1> {
    verify_and_stage_v1(
        capsule,
        destination,
        CustodyRestoreBudgetV1::ceiling(),
        frame_budget(),
        opener,
    )
}

/// T1 and T2 refuse before `.restore-work/` exists, and before any open.
fn refused_before_the_work_tree(capsule: &Path) -> CustodyRestoreErrorV1 {
    let destination = DestinationV1::new();
    let opener = FixtureOpenerV1::honest();
    let error = match restore(capsule, &destination.path, &opener) {
        Ok(_) => panic!("the restore must refuse"),
        Err(error) => error,
    };
    assert_eq!(opener.calls(), 0, "an artifact was opened: {error:?}");
    assert_eq!(destination.entries(), Vec::<String>::new(), "{error:?}");
    error
}

/// A rich clone exported through the 2B2b2b2 plan-backed path: every payload a real frame.
struct PlanBackedV1 {
    fixture: PlanFixtureV1,
    manifest: CustodyManifestV1,
    receipts: Vec<crate::custody_coverage::CustodyClassReceiptV1>,
}

impl PlanBackedV1 {
    fn export() -> Self {
        let fixture = PlanFixtureV1::rich_clone();
        let (manifest, capability, receipts) = fixture.planned();
        expect_sealed(
            fixture
                .export(&manifest, capability)
                .expect("the plan-backed export seals"),
        );
        Self {
            fixture,
            manifest,
            receipts,
        }
    }

    /// A second rich clone, planned and exported under `generation`.
    fn export_of_generation(generation: &str) -> Self {
        let fixture = PlanFixtureV1::rich_clone();
        let inventory = fixture.inventory();
        let (plan, sources) = fixture.plan(generation, &inventory);
        let manifest = manifest_from_plan(&plan, generation, inventory);
        let receipts = plan.receipts().to_vec();
        let capability = planned_capability(&manifest, plan, sources);
        expect_sealed(
            fixture
                .export(&manifest, capability)
                .expect("the plan-backed export seals"),
        );
        Self {
            fixture,
            manifest,
            receipts,
        }
    }

    fn capsule(&self) -> PathBuf {
        self.fixture.capsule_dir()
    }

    /// The capsule resealed into `area` with one payload's plaintext replaced.
    fn resealed_with(&self, area: &Path, name: &str, plaintext: Vec<u8>) -> PathBuf {
        let resealed = area.join("capsule");
        reseal_for_test(&self.capsule(), &resealed, &[(name, plaintext)], None);
        resealed
    }
}

fn frame_refusal(capsule: &Path, class: CustodyCoverageClassV1) {
    let (error, _destination) = restore_refusal_after_binding(capsule);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::FrameInvalid { class: refused } if *refused == class),
        "observed {error:?}"
    );
}

/// A refusal past binding: every plaintext is staged, so only the frame check can refuse.
fn restore_refusal_after_binding(capsule: &Path) -> (CustodyRestoreErrorV1, DestinationV1) {
    let destination = DestinationV1::new();
    let error = match restore(capsule, &destination.path, &FixtureOpenerV1::honest()) {
        Ok(_) => panic!("the restore must refuse"),
        Err(error) => error,
    };
    (error, destination)
}

/// The 2B2 harness's fixture streams are not frames, so a fixture-stream capsule cannot be
/// restored: its first coverage payload refuses in phase V.
#[test]
fn verifies_and_stages_a_fixture_stream_capsule() {
    let harness = exported();
    let (error, destination) = restore_refusal_after_binding(&harness.capsule_dir());
    assert!(
        matches!(
            error,
            CustodyRestoreErrorV1::FrameInvalid {
                class: CustodyCoverageClassV1::Index
            }
        ),
        "observed {error:?}"
    );
    assert!(destination.work().join("plain/payload").is_dir());
}

#[test]
fn verifies_and_stages_a_plan_backed_capsule() {
    let exported = PlanBackedV1::export();
    let capsule = exported.capsule();
    let destination = DestinationV1::new();
    let opener = FixtureOpenerV1::honest();
    let verified = restore(&capsule, &destination.path, &opener).expect("phase V passes");

    assert_eq!(verified.control.manifest, exported.manifest);
    assert_eq!(
        verified.staged.len(),
        verified.control.index.artifacts().len()
    );
    assert_eq!(
        opener.calls(),
        verified.staged.len(),
        "each artifact opened once"
    );
    let mut payloads = Vec::new();
    for one in &verified.staged {
        let name = lossy(one.name.as_bytes());
        let plaintext = plaintext_of(&capsule, &name);
        assert_eq!(one.length, plaintext.len() as u64, "{name}");
        assert_eq!(
            sha256_hex(&one.sha256),
            Sha256HexV1::digest(&plaintext),
            "{name}"
        );
        let staged = std::fs::read(destination.work().join("plain").join(&name)).unwrap();
        assert_eq!(staged, plaintext, "{name}");
        let mut file = &one.file;
        assert_eq!(file.stream_position().unwrap(), 0, "{name}: rewound");
        if let CustodyCapsuleArtifactRoleV1::CoveragePayload(class) = one.role {
            let receipt = exported
                .receipts
                .iter()
                .find(|receipt| receipt.class == class)
                .expect("a captured class has a receipt");
            assert_eq!(one.length, receipt.frame_length, "{class:?}");
            assert_eq!(one.sha256, receipt.frame_sha256, "{class:?}");
            payloads.push(class);
        }
    }
    // Staged in index (name) order; the plan's receipts are in class order.
    payloads.sort();
    assert_eq!(
        payloads,
        exported
            .receipts
            .iter()
            .map(|receipt| receipt.class)
            .collect::<Vec<_>>()
    );
    assert_eq!(payloads.len(), 7, "{payloads:?}");
}

#[test]
fn corrupt_frame_trailer_refuses() {
    let exported = PlanBackedV1::export();
    let mut frame = plaintext_of(&exported.capsule(), WORKTREE_PAYLOAD_V1);
    let last = frame.len() - 1;
    frame[last] ^= 0x01;
    let area = tempfile::TempDir::new().unwrap();
    let resealed = exported.resealed_with(area.path(), WORKTREE_PAYLOAD_V1, frame);

    frame_refusal(&resealed, CustodyCoverageClassV1::Worktree);
}

#[test]
fn wrong_class_frame_refuses() {
    let exported = PlanBackedV1::export();
    let other_class = plaintext_of(&exported.capsule(), INDEX_PAYLOAD_V1);
    let area = tempfile::TempDir::new().unwrap();
    let resealed = exported.resealed_with(area.path(), WORKTREE_PAYLOAD_V1, other_class);

    frame_refusal(&resealed, CustodyCoverageClassV1::Worktree);
}

#[test]
fn wrong_generation_frame_refuses() {
    let exported = PlanBackedV1::export();
    let other = PlanBackedV1::export_of_generation("generation-2b3a-other");
    assert_ne!(
        other.manifest.generation_id(),
        exported.manifest.generation_id()
    );
    let other_generation = plaintext_of(&other.capsule(), WORKTREE_PAYLOAD_V1);
    let area = tempfile::TempDir::new().unwrap();
    let resealed = exported.resealed_with(area.path(), WORKTREE_PAYLOAD_V1, other_generation);

    frame_refusal(&resealed, CustodyCoverageClassV1::Worktree);
}

#[test]
fn nothing_outside_restore_work() {
    let exported = PlanBackedV1::export();
    let destination = DestinationV1::new();
    let verified = restore(
        &exported.capsule(),
        &destination.path,
        &FixtureOpenerV1::honest(),
    )
    .expect("phase V passes");

    let entries = destination.entries();
    assert!(
        entries
            .iter()
            .all(|entry| entry == ".restore-work/" || entry.starts_with(".restore-work/")),
        "{entries:?}"
    );
    let mut expected = vec![
        ".restore-work/".to_owned(),
        ".restore-work/plain/".to_owned(),
    ];
    for directory in ["control", "git", "payload"] {
        expected.push(format!(".restore-work/plain/{directory}/"));
    }
    for row in verified.control.index.artifacts() {
        expected.push(format!(
            ".restore-work/plain/{}",
            lossy(row.name().as_bytes())
        ));
    }
    expected.sort();
    assert_eq!(entries, expected);
}

#[test]
fn staged_order_equals_index_order() {
    let exported = PlanBackedV1::export();
    let destination = DestinationV1::new();
    let verified = restore(
        &exported.capsule(),
        &destination.path,
        &FixtureOpenerV1::honest(),
    )
    .expect("phase V passes");

    let staged: Vec<&LosslessPathV1> = verified.staged.iter().map(|one| &one.name).collect();
    let indexed: Vec<&LosslessPathV1> = verified
        .control
        .index
        .artifacts()
        .iter()
        .map(|row| row.name())
        .collect();
    assert_eq!(staged, indexed);
    let roles: Vec<&CustodyCapsuleArtifactRoleV1> =
        verified.staged.iter().map(|one| &one.role).collect();
    let indexed_roles: Vec<&CustodyCapsuleArtifactRoleV1> = verified
        .control
        .index
        .artifacts()
        .iter()
        .map(|row| row.role())
        .collect();
    assert_eq!(roles, indexed_roles);
}

/// S1: the lexically last artifact's ciphertext is corrupted, yet no artifact is opened.
#[test]
fn every_ciphertext_verified_before_the_first_open() {
    let exported = PlanBackedV1::export();
    let capsule = exported.capsule();
    let seal = read_seal(&capsule);
    let last = seal
        .artifacts()
        .last()
        .expect("a sealed artifact")
        .name()
        .clone();
    let path = capsule.join(OsStr::from_bytes(last.as_bytes()));
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[8] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();

    let error = refused_before_the_work_tree(&capsule);
    assert!(
        matches!(&error, CustodyRestoreErrorV1::CiphertextDigest { name }
            if name.as_bytes() == last.as_bytes()),
        "observed {error:?}"
    );
}

/// Every capsule entry by relative path: its kind and mode, its mtime, and its bytes (a file) or
/// its sorted listing (a directory).
fn capsule_snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, i64, i64, Vec<u8>)> {
    use std::os::unix::fs::MetadataExt as _;
    let mut snapshot = BTreeMap::new();
    let mut frontier = vec![root.to_path_buf()];
    while let Some(path) = frontier.pop() {
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        let content = if metadata.is_dir() {
            let mut names: Vec<_> = std::fs::read_dir(&path)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            names.sort();
            for name in &names {
                frontier.push(path.join(name));
            }
            names
                .iter()
                .flat_map(|name| [name.as_bytes(), b"\n"].concat())
                .collect()
        } else {
            std::fs::read(&path).unwrap()
        };
        snapshot.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            (
                metadata.mode(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                content,
            ),
        );
    }
    snapshot
}

/// The capsule is read-only input: on success and on a T1, T2, and T3 refusal, every capsule
/// entry's bytes, mode, and mtime, and every listing, are unchanged.
#[test]
fn capsule_is_never_written() {
    let exported = PlanBackedV1::export();
    let capsule = exported.capsule();
    let area = tempfile::TempDir::new().unwrap();
    let copy = |name: &str| {
        let path = area.path().join(name);
        reseal_for_test(&capsule, &path, &[], None);
        path
    };

    let t1 = copy("t1");
    let mut seal = std::fs::read(t1.join(RESTORE_SEAL_NAME_V1)).unwrap();
    seal.insert(1, b' ');
    std::fs::write(t1.join(RESTORE_SEAL_NAME_V1), seal).unwrap();

    let t2 = copy("t2");
    let path = t2.join(POLICY_V1);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[9] ^= 0x01;
    std::fs::write(&path, bytes).unwrap();

    let t3 = copy("t3");
    let seal = read_seal(&t3);
    write_seal(
        &t3,
        &rebuilt_seal(
            &seal,
            edited_digest(seal.manifest_digest()),
            seal.artifacts().to_vec(),
        ),
    );

    for (row, capsule, succeeds) in [
        ("success", capsule.clone(), true),
        ("T1", t1, false),
        ("T2", t2, false),
        ("T3", t3, false),
    ] {
        let before = capsule_snapshot(&capsule);
        let destination = DestinationV1::new();
        let result = restore(&capsule, &destination.path, &FixtureOpenerV1::honest());
        assert_eq!(result.is_ok(), succeeds, "{row}: {:?}", result.err());
        assert_eq!(
            capsule_snapshot(&capsule),
            before,
            "{row}: the capsule changed"
        );
    }
}

/// The final census recheck: a mount point that appears inside the destination during the
/// restore refuses before `VerifiedCapsuleV1` is returned.
#[test]
fn mount_appearing_during_restore_refuses() {
    let exported = PlanBackedV1::export();
    let destination = DestinationV1::new();
    let inside = [destination.path.as_os_str().as_bytes(), b"/.restore-work"].concat();
    let census = crate::custody_mounts::seam::install(Box::new(move |call| {
        Ok(if call == 1 {
            vec![b"/".to_vec()]
        } else {
            vec![b"/".to_vec(), inside.clone()]
        })
    }));

    let result = restore(
        &exported.capsule(),
        &destination.path,
        &FixtureOpenerV1::honest(),
    );
    assert!(
        matches!(result, Err(CustodyRestoreErrorV1::MountBoundary(_))),
        "observed {:?}",
        result.err()
    );
    assert_eq!(census.calls(), 2);
    assert!(
        destination.work().join("plain/payload").is_dir(),
        "refused only at the end"
    );
}

// ---------------------------------------------------------------------------------------------
// Repair round 1 (finding B1): the final gate re-proves the destination's identity
// ---------------------------------------------------------------------------------------------

/// A racer's swap: moves `path` to `to`, outside the destination, and puts an empty
/// owner-private directory at its name.
fn swap_out(path: &Path, to: &Path) {
    std::fs::rename(path, to).expect("the swapped object moves out");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .expect("an empty replacement at the original name");
}

/// Phase V over a new plan-backed capsule into `destination`, with `swap(leaf)` run at the
/// `BeforeLeafCreate` of the last indexed artifact: after its last pre-create recheck, and before
/// its leaf is created through the retained parent. Returns the result and that artifact's name.
fn restore_with_last_leaf_swap(
    destination: &DestinationV1,
    swap: impl Fn(&Path) + 'static,
) -> (Result<VerifiedCapsuleV1, CustodyRestoreErrorV1>, String) {
    let exported = PlanBackedV1::export();
    let capsule = exported.capsule();
    let seal = read_seal(&capsule);
    let total = seal.artifacts().len();
    // The seal and the index are both in canonical name order, and the controls are staged
    // first, so the seal's last artifact is the last one staged; the hook asserts it.
    let last = lossy(seal.artifacts()[total - 1].name().as_bytes());
    let target = destination.work().join("plain").join(&last);
    let creates = Rc::new(std::cell::Cell::new(0_usize));
    let seen = Rc::clone(&creates);
    let _hook = install_restore_hook_for_test(RestoreHookPointV1::BeforeLeafCreate, move |leaf| {
        seen.set(seen.get() + 1);
        if leaf == target.as_path() {
            assert_eq!(seen.get(), total, "the swapped leaf is the last one staged");
            swap(leaf);
        }
    });
    let result = restore(&capsule, &destination.path, &FixtureOpenerV1::honest());
    assert_eq!(
        creates.get(),
        total,
        "every artifact reached its leaf create"
    );
    (result, last)
}

/// Finding B1: at the last artifact's `BeforeLeafCreate`, the destination root is renamed to a
/// sibling outside it, and an empty directory takes its name. The leaf is created, and its frame
/// verified, through retained descriptors, so only the final gate's root identity check can see
/// that the named destination holds nothing.
#[test]
fn final_gate_refuses_a_destination_root_swapped_at_the_last_leaf() {
    let destination = DestinationV1::new();
    let moved = destination.area.join("dest-moved");
    let (root, moved_to) = (destination.path.clone(), moved.clone());
    let (result, last) =
        restore_with_last_leaf_swap(&destination, move |_| swap_out(&root, &moved_to));

    assert!(
        matches!(&result, Err(CustodyRestoreErrorV1::IdentityChanged(detail))
            if detail.contains("now resolves to a different directory")),
        "observed {:?}",
        result.as_ref().map(|_| "VerifiedCapsuleV1")
    );
    assert!(
        moved.join(".restore-work/plain").join(&last).is_file(),
        "the last leaf was written into the renamed root"
    );
    assert_eq!(destination.entries(), Vec::<String>::new());
}

/// Finding B1: at the same point, the last artifact's retained parent staging directory is
/// renamed out of the destination, and an empty directory takes its name. Only the final gate's
/// staging-chain recheck can see it.
#[test]
fn final_gate_refuses_a_staging_parent_swapped_at_the_last_leaf() {
    let destination = DestinationV1::new();
    let moved = destination.area.join("parent-moved");
    let moved_to = moved.clone();
    let (result, last) = restore_with_last_leaf_swap(&destination, move |leaf| {
        swap_out(leaf.parent().expect("a staged leaf's parent"), &moved_to);
    });

    let parent = Path::new(&last).parent().expect("a sealed parent");
    let changed = format!(".restore-work/plain/{}: ", parent.display());
    assert!(
        matches!(&result, Err(CustodyRestoreErrorV1::IdentityChanged(detail))
            if detail.starts_with(&changed)),
        "observed {:?}",
        result.as_ref().map(|_| "VerifiedCapsuleV1")
    );
    let leaf = destination.work().join("plain").join(&last);
    assert!(
        moved.join(leaf.file_name().expect("a leaf name")).is_file(),
        "the last leaf was written into the renamed parent"
    );
    assert_eq!(
        entries_beneath(leaf.parent().expect("a staged leaf's parent")),
        Vec::<String>::new()
    );
}

/// Finding B1: every frame is verified, and then `.restore-work/` is renamed out of the
/// destination, and an empty directory takes its name, before the final gate. This is the end of
/// the window a swap during frame verification also falls in. Only the final gate's recheck of
/// `.restore-work/` against the root's entry can see it.
#[test]
fn final_gate_refuses_restore_work_swapped_after_the_last_frame() {
    let exported = PlanBackedV1::export();
    let destination = DestinationV1::new();
    let moved = destination.area.join("restore-work-moved");
    let moved_to = moved.clone();
    let _hook = install_restore_hook_for_test(RestoreHookPointV1::BeforeFinalGate, move |root| {
        swap_out(&root.join(".restore-work"), &moved_to);
    });
    let result = restore(
        &exported.capsule(),
        &destination.path,
        &FixtureOpenerV1::honest(),
    );

    assert!(
        matches!(&result, Err(CustodyRestoreErrorV1::IdentityChanged(detail))
            if detail.starts_with(".restore-work: ")),
        "observed {:?}",
        result.as_ref().map(|_| "VerifiedCapsuleV1")
    );
    assert!(
        moved.join("plain/payload").is_dir(),
        "the hook ran after staging"
    );
    assert_eq!(entries_beneath(&destination.work()), Vec::<String>::new());
}

/// The final gate's positive control: the same hooks run at the last leaf and before the gate,
/// and change nothing, so phase V passes, with the last leaf at its named path.
#[test]
fn final_gate_admits_an_unswapped_restore() {
    let destination = DestinationV1::new();
    let gate = Rc::new(std::cell::Cell::new(0_usize));
    let gate_seen = Rc::clone(&gate);
    let _gate = install_restore_hook_for_test(RestoreHookPointV1::BeforeFinalGate, move |_| {
        gate_seen.set(gate_seen.get() + 1);
    });
    let ran = Rc::new(std::cell::Cell::new(false));
    let seen = Rc::clone(&ran);
    let (result, last) = restore_with_last_leaf_swap(&destination, move |_| seen.set(true));

    let verified = result.expect("an unswapped restore passes the final gate");
    assert!(ran.get(), "the last leaf's hook ran");
    assert_eq!(gate.get(), 1, "the final gate's hook ran once");
    assert_eq!(
        verified.staged.len(),
        verified.control.index.artifacts().len()
    );
    assert!(destination.work().join("plain").join(&last).is_file());
}

// ---------------------------------------------------------------------------------------------
// Repair round 2 (finding B1): the final gate's order
// ---------------------------------------------------------------------------------------------

/// Finding B1, round 2: during the final gate's census (the seam's second call), the destination
/// root is renamed to a sibling outside it, an empty directory takes its name, and the census
/// lists only `/`. Only a root identity check after the census can see that the named destination
/// holds nothing.
#[test]
fn final_gate_refuses_destination_root_swapped_during_final_census() {
    let exported = PlanBackedV1::export();
    let capsule = exported.capsule();
    let seal = read_seal(&capsule);
    let destination = DestinationV1::new();
    let moved = destination.area.join("dest-moved");
    let (root, moved_to) = (destination.path.clone(), moved.clone());
    let census = crate::custody_mounts::seam::install(Box::new(move |call| {
        if call == 2 {
            swap_out(&root, &moved_to);
        }
        Ok(vec![b"/".to_vec()])
    }));

    let result = restore(&capsule, &destination.path, &FixtureOpenerV1::honest());
    assert!(
        matches!(&result, Err(CustodyRestoreErrorV1::IdentityChanged(detail))
            if detail.contains("now resolves to a different directory")),
        "observed {:?}",
        result.as_ref().map(|_| "VerifiedCapsuleV1")
    );
    assert_eq!(census.calls(), 2);
    assert_eq!(destination.entries(), Vec::<String>::new());
    for artifact in seal.artifacts() {
        let name = lossy(artifact.name().as_bytes());
        assert!(
            moved.join(".restore-work/plain").join(&name).is_file(),
            "{name} is staged under dest-moved"
        );
    }
}
