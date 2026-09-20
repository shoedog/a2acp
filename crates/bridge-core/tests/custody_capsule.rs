use bridge_core::custody_capsule::{
    CustodyCapsuleArtifactRoleRowV1, CustodyCapsuleArtifactRoleV1 as Role, CustodyCapsuleBindingV1,
    CustodyCapsuleErrorV1 as Error, CustodyCapsuleIndexV1, CustodyCapsuleLayoutV1,
    CustodyCapsuleSealProofV1, CustodyEnvelopeChunkV1, CustodyEnvelopeContextV1,
    CustodyEnvelopeFormatV1, CustodyEnvelopeMetadataV1, CustodyEnvelopeOpenRequestV1,
    CustodyEnvelopeSealReceiptV1, CustodyEnvelopeSinkValidatorV1,
    CustodyEnvelopeSourceDescriptorV1, CustodyEnvelopeSourceValidatorV1,
    CustodyEnvelopeStreamLimitsV1, CustodyEnvelopeStreamReceiptV1, CustodyRestorePolicyV1,
    RestoreDisabledV1, RestoreForbiddenV1,
};
use bridge_core::custody_inventory::{
    CustodyReasonCodeV1 as Reason, CustodyStateClassV1 as State, LosslessPathV1,
};
use bridge_core::custody_seal::{
    CustodyCoverageClassV1 as Coverage, CustodyCoverageEntryV1, CustodyGitObjectFormatV1,
    CustodyGitObjectKindV1 as ObjectKind, CustodyManifestV1, CustodyOriginalObjectV1,
    CustodySealV1, CustodySealedArtifactV1,
};
use bridge_core::execution_policy::Sha256HexV1;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;

struct CountingAllocator;

thread_local! {
    static COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: This test allocator delegates every allocation operation to `System` and only
// increments a thread-local counter before forwarding allocations while counting is enabled.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNT_ALLOCATIONS.with(|enabled| {
            if enabled.get() {
                ALLOCATION_COUNT.with(|count| count.set(count.get().saturating_add(1)));
            }
        });
        // SAFETY: Delegates the requested layout unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: Delegates the pointer and layout unchanged to the system allocator.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

fn digest(byte: u8) -> Sha256HexV1 {
    Sha256HexV1::digest(&[byte])
}

fn object(hex: char, kind: ObjectKind) -> CustodyOriginalObjectV1 {
    CustodyOriginalObjectV1::new(
        CustodyGitObjectFormatV1::Sha1,
        hex.to_string().repeat(40),
        kind,
    )
    .unwrap()
}

fn coverage_with(default: State) -> Vec<CustodyCoverageEntryV1> {
    Coverage::ALL
        .into_iter()
        .map(|class| {
            let reasons = if default == State::Unresolved {
                vec![Reason::ContentUnresolved]
            } else {
                vec![]
            };
            CustodyCoverageEntryV1::new(class, default, reasons, None).unwrap()
        })
        .collect()
}

fn set_coverage(
    mut rows: Vec<CustodyCoverageEntryV1>,
    class: Coverage,
    state: State,
) -> Vec<CustodyCoverageEntryV1> {
    let position = rows.iter().position(|row| row.class() == class).unwrap();
    let reasons = if state == State::Unresolved {
        vec![Reason::ContentUnresolved]
    } else {
        vec![]
    };
    rows[position] = CustodyCoverageEntryV1::new(class, state, reasons, None).unwrap();
    rows
}

fn make_manifest(
    coverage: Vec<CustodyCoverageEntryV1>,
    objects: Vec<CustodyOriginalObjectV1>,
) -> CustodyManifestV1 {
    CustodyManifestV1::new(
        "unit",
        "run",
        "materialization",
        "generation",
        coverage,
        vec![],
        objects,
        vec![],
        vec![],
    )
    .unwrap()
}

fn empty_manifest() -> CustodyManifestV1 {
    make_manifest(coverage_with(State::Empty), vec![])
}

fn full_manifest() -> CustodyManifestV1 {
    make_manifest(
        coverage_with(State::Captured),
        vec![object('a', ObjectKind::Commit)],
    )
}

fn stream_receipt(bytes: &[u8]) -> CustodyEnvelopeStreamReceiptV1 {
    let mut sink = CustodyEnvelopeSinkValidatorV1::new(
        CustodyEnvelopeStreamLimitsV1::new(bytes.len() as u64, bytes.len() as u64, 1).unwrap(),
    );
    sink.accept_chunk(&CustodyEnvelopeChunkV1::new(0, bytes.to_vec(), true).unwrap())
        .unwrap();
    sink.finish().unwrap()
}

fn receipt_for(
    name: &LosslessPathV1,
    manifest_digest: Sha256HexV1,
    format: CustodyEnvelopeFormatV1,
    recipients: Vec<String>,
    bytes: &[u8],
) -> CustodyEnvelopeSealReceiptV1 {
    let context =
        CustodyEnvelopeContextV1::new(name.clone(), manifest_digest, format, recipients).unwrap();
    CustodyEnvelopeSealReceiptV1::new(&context, stream_receipt(bytes)).unwrap()
}

fn proof_for(
    index: &CustodyCapsuleIndexV1,
    manifest_digest: Sha256HexV1,
) -> CustodyCapsuleSealProofV1 {
    let format = CustodyEnvelopeFormatV1::new("capsule-v1", "a2a-bridge", "0.1.0").unwrap();
    let receipts = index
        .artifacts()
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            receipt_for(
                row.name(),
                manifest_digest.clone(),
                format.clone(),
                vec!["recipient-b".to_owned(), "recipient-a".to_owned()],
                &[u8::try_from(offset + 1).unwrap()],
            )
        })
        .collect();
    CustodyCapsuleSealProofV1::from_receipts(receipts).unwrap()
}

fn envelope_format() -> CustodyEnvelopeFormatV1 {
    CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "1").unwrap()
}

fn envelope_context() -> CustodyEnvelopeContextV1 {
    CustodyEnvelopeContextV1::new(
        LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
        digest(1),
        envelope_format(),
        vec!["recipient".to_owned()],
    )
    .unwrap()
}

#[test]
fn layout_derives_exact_control_and_full_artifact_totals() {
    let empty = empty_manifest();
    let empty_layout = CustodyCapsuleLayoutV1::derive(&empty).unwrap();
    assert_eq!(empty_layout.artifact_count(), 3);
    assert_eq!(
        empty_layout
            .index()
            .artifacts()
            .iter()
            .map(|row| row.name().as_bytes())
            .collect::<Vec<_>>(),
        vec![
            b"control/capsule-index.json.enc".as_slice(),
            b"control/manifest.json.enc".as_slice(),
            b"control/restore-policy.json.enc".as_slice(),
        ]
    );

    let full = full_manifest();
    let full_layout = CustodyCapsuleLayoutV1::derive(&full).unwrap();
    assert_eq!(full_layout.artifact_count(), 17);
    assert!(full_layout
        .index()
        .artifacts()
        .iter()
        .any(|row| row.name().as_bytes() == b"git/objects.pack.enc"));
    assert!(full_layout.index().artifacts().iter().any(|row| {
        row.name().as_bytes() == b"payload/worktree.bin.enc"
            && row.role() == &Role::CoveragePayload(Coverage::Worktree)
    }));
}

#[test]
fn layout_refuses_non_sealable_and_object_database_state_mismatches() {
    let unresolved = make_manifest(coverage_with(State::Unresolved), vec![]);
    assert_eq!(
        CustodyCapsuleLayoutV1::derive(&unresolved).unwrap_err(),
        Error::NonSealableManifest
    );

    let captured_empty = make_manifest(coverage_with(State::Captured), vec![]);
    assert_eq!(
        CustodyCapsuleLayoutV1::derive(&captured_empty).unwrap_err(),
        Error::CoverageStateMismatch
    );

    let empty_with_objects = make_manifest(
        coverage_with(State::Empty),
        vec![object('a', ObjectKind::Blob)],
    );
    assert_eq!(
        CustodyCapsuleLayoutV1::derive(&empty_with_objects).unwrap_err(),
        Error::CoverageStateMismatch
    );
}

#[test]
fn layout_refuses_unknown_object_kind() {
    let manifest = make_manifest(
        coverage_with(State::Captured),
        vec![object('a', ObjectKind::Unknown)],
    );
    assert_eq!(
        CustodyCapsuleLayoutV1::derive(&manifest).unwrap_err(),
        Error::UnknownObjectKind
    );
}

#[test]
fn binding_accepts_matching_manifest_index_policy_and_seal() {
    let manifest = full_manifest();
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
    let manifest_digest = manifest.content_digest().unwrap();
    let proof = proof_for(layout.index(), manifest_digest.clone());
    let binding = CustodyCapsuleBindingV1::new(
        &manifest,
        layout.index(),
        &CustodyRestorePolicyV1::default(),
        &proof,
    )
    .unwrap();
    assert_eq!(binding.manifest_digest(), &manifest_digest);
    assert_eq!(binding.artifact_names().len(), 17);
}

#[test]
fn binding_refuses_a_foreign_index_manifest_digest() {
    let manifest = full_manifest();
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
    let manifest_digest = manifest.content_digest().unwrap();
    let proof = proof_for(layout.index(), manifest_digest);
    let foreign_index =
        CustodyCapsuleIndexV1::new(digest(8), layout.index().artifacts().to_vec()).unwrap();

    assert_eq!(
        CustodyCapsuleBindingV1::new(
            &manifest,
            &foreign_index,
            &CustodyRestorePolicyV1::default(),
            &proof,
        )
        .unwrap_err(),
        Error::ThreeWayDigestMismatch
    );
}

#[test]
fn binding_refuses_a_foreign_seal_manifest_digest() {
    let manifest = full_manifest();
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
    let foreign_proof = proof_for(layout.index(), digest(7));

    assert_eq!(
        CustodyCapsuleBindingV1::new(
            &manifest,
            layout.index(),
            &CustodyRestorePolicyV1::default(),
            &foreign_proof,
        )
        .unwrap_err(),
        Error::ThreeWayDigestMismatch
    );
}

#[test]
fn binding_refuses_a_foreign_manifest_while_index_and_seal_agree() {
    let manifest = full_manifest();
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
    let proof = proof_for(layout.index(), manifest.content_digest().unwrap());
    let other_manifest = make_manifest(
        set_coverage(
            coverage_with(State::Captured),
            Coverage::Worktree,
            State::Empty,
        ),
        vec![object('a', ObjectKind::Commit)],
    );

    assert_eq!(
        CustodyCapsuleBindingV1::new(
            &other_manifest,
            layout.index(),
            &CustodyRestorePolicyV1::default(),
            &proof,
        )
        .unwrap_err(),
        Error::ThreeWayDigestMismatch
    );
}

#[test]
fn capsule_seal_proof_requires_receipt_derived_shared_identity() {
    let name = LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec());
    let context = envelope_context();
    let baseline = CustodyEnvelopeSealReceiptV1::new(&context, stream_receipt(&[1, 2, 3])).unwrap();
    let valid = CustodyCapsuleSealProofV1::from_receipts(vec![baseline.clone()]).unwrap();
    assert_eq!(valid.seal().manifest_digest(), context.manifest_digest());
    assert_eq!(valid.seal().artifacts()[0].name(), &name);
    assert_eq!(valid.seal().artifacts()[0].byte_length(), 3);
    assert_eq!(
        valid.seal().artifacts()[0].sha256(),
        &Sha256HexV1::digest(&[1, 2, 3])
    );
    let second_name = LosslessPathV1::from_bytes(b"control/capsule-index.json.enc".to_vec());

    for changed in [
        CustodyEnvelopeContextV1::new(
            second_name.clone(),
            digest(8),
            context.format().clone(),
            context.recipients().to_vec(),
        )
        .unwrap(),
        CustodyEnvelopeContextV1::new(
            second_name.clone(),
            context.manifest_digest().clone(),
            CustodyEnvelopeFormatV1::new("other", "tool", "1").unwrap(),
            context.recipients().to_vec(),
        )
        .unwrap(),
        CustodyEnvelopeContextV1::new(
            second_name.clone(),
            context.manifest_digest().clone(),
            CustodyEnvelopeFormatV1::new("capsule-v1", "other", "1").unwrap(),
            context.recipients().to_vec(),
        )
        .unwrap(),
        CustodyEnvelopeContextV1::new(
            second_name.clone(),
            context.manifest_digest().clone(),
            CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "2").unwrap(),
            context.recipients().to_vec(),
        )
        .unwrap(),
        CustodyEnvelopeContextV1::new(
            second_name,
            context.manifest_digest().clone(),
            context.format().clone(),
            vec!["other-recipient".to_owned()],
        )
        .unwrap(),
    ] {
        let changed = CustodyEnvelopeSealReceiptV1::new(&changed, stream_receipt(&[4])).unwrap();
        assert_eq!(
            CustodyCapsuleSealProofV1::from_receipts(vec![baseline.clone(), changed]).unwrap_err(),
            Error::InvalidInput
        );
    }
}

#[test]
fn binding_reports_missing_extra_unmapped_and_duplicate_artifacts() {
    let manifest = full_manifest();
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
    let manifest_digest = manifest.content_digest().unwrap();
    let proof = proof_for(layout.index(), manifest_digest.clone());

    let mut missing_rows = layout.index().artifacts().to_vec();
    missing_rows.retain(|row| row.name().as_bytes() != b"payload/worktree.bin.enc");
    let missing_index = CustodyCapsuleIndexV1::new(manifest_digest.clone(), missing_rows).unwrap();
    assert_eq!(
        CustodyCapsuleBindingV1::new(
            &manifest,
            &missing_index,
            &CustodyRestorePolicyV1::default(),
            &proof,
        )
        .unwrap_err(),
        Error::MissingArtifact
    );

    let extra_row = CustodyCapsuleArtifactRoleRowV1::new(
        LosslessPathV1::from_bytes(b"payload/reproducible_outputs.bin.enc".to_vec()),
        Role::CoveragePayload(Coverage::ReproducibleOutputs),
    )
    .unwrap();
    let mut empty_rows = CustodyCapsuleLayoutV1::derive(&empty_manifest())
        .unwrap()
        .index()
        .artifacts()
        .to_vec();
    empty_rows.push(extra_row);
    let extra_index =
        CustodyCapsuleIndexV1::new(empty_manifest().content_digest().unwrap(), empty_rows).unwrap();
    let empty_proof = proof_for(&extra_index, empty_manifest().content_digest().unwrap());
    assert_eq!(
        CustodyCapsuleBindingV1::new(
            &empty_manifest(),
            &extra_index,
            &CustodyRestorePolicyV1::default(),
            &empty_proof,
        )
        .unwrap_err(),
        Error::ExtraArtifact
    );

    let empty = empty_manifest();
    let empty_layout = CustodyCapsuleLayoutV1::derive(&empty).unwrap();
    let empty_digest = empty.content_digest().unwrap();
    let format = CustodyEnvelopeFormatV1::new("capsule-v1", "a2a-bridge", "0.1.0").unwrap();
    let mut extra_receipts: Vec<_> = empty_layout
        .index()
        .artifacts()
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            receipt_for(
                row.name(),
                empty_digest.clone(),
                format.clone(),
                vec!["recipient".to_owned()],
                &[u8::try_from(offset + 1).unwrap()],
            )
        })
        .collect();
    extra_receipts.push(receipt_for(
        &LosslessPathV1::from_bytes(b"payload/worktree.bin.enc".to_vec()),
        empty_digest,
        format,
        vec!["recipient".to_owned()],
        &[9],
    ));
    let extra_proof = CustodyCapsuleSealProofV1::from_receipts(extra_receipts).unwrap();
    assert_eq!(
        CustodyCapsuleBindingV1::new(
            &empty,
            empty_layout.index(),
            &CustodyRestorePolicyV1::default(),
            &extra_proof,
        )
        .unwrap_err(),
        Error::UnmappedArtifact
    );

    let duplicate_rows = vec![
        CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"payload/worktree.bin.enc".to_vec()),
            Role::CoveragePayload(Coverage::Worktree),
        )
        .unwrap(),
        CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"payload/worktree.bin.enc".to_vec()),
            Role::CoveragePayload(Coverage::Worktree),
        )
        .unwrap(),
    ];
    assert_eq!(
        CustodyCapsuleIndexV1::new(digest(1), duplicate_rows).unwrap_err(),
        Error::DuplicateArtifactName
    );

    assert_eq!(
        CustodyCapsuleIndexV1::new(digest(1), vec![]).unwrap_err(),
        Error::MissingArtifact
    );
}

#[test]
fn public_rows_enforce_the_reserved_name_role_bijection() {
    for (name, role) in [
        (
            b"payload/worktree-copy.bin.enc".as_slice(),
            Role::CoveragePayload(Coverage::Worktree),
        ),
        (b"control/manifest.json.enc".as_slice(), Role::CapsuleIndex),
        (
            b"payload/object_database.bin.enc".as_slice(),
            Role::CoveragePayload(Coverage::ObjectDatabase),
        ),
        (
            b"payload/index.bin.enc".as_slice(),
            Role::CoveragePayload(Coverage::Worktree),
        ),
    ] {
        assert_eq!(
            CustodyCapsuleArtifactRoleRowV1::new(LosslessPathV1::from_bytes(name.to_vec()), role)
                .unwrap_err(),
            Error::UnmappedArtifact
        );
    }
}

#[test]
fn public_index_cannot_materialize_duplicate_ownership_fixture() {
    let valid_controls = vec![
        CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
            Role::Manifest,
        )
        .unwrap(),
        CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"control/capsule-index.json.enc".to_vec()),
            Role::CapsuleIndex,
        )
        .unwrap(),
        CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"control/restore-policy.json.enc".to_vec()),
            Role::RestorePolicy,
        )
        .unwrap(),
    ];
    let mut rows = valid_controls;
    rows.push(
        CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"payload/worktree.bin.enc".to_vec()),
            Role::CoveragePayload(Coverage::Worktree),
        )
        .unwrap(),
    );
    assert_eq!(
        CustodyCapsuleArtifactRoleRowV1::new(
            LosslessPathV1::from_bytes(b"payload/worktree-copy.bin.enc".to_vec()),
            Role::CoveragePayload(Coverage::Worktree),
        )
        .unwrap_err(),
        Error::UnmappedArtifact
    );
    assert!(CustodyCapsuleIndexV1::new(digest(1), rows).is_ok());
}

#[test]
fn restore_policy_accepts_only_the_closed_inert_values() {
    let policy = CustodyRestorePolicyV1::default();
    let bytes = policy.encode_canonical().unwrap();
    assert_eq!(
        CustodyRestorePolicyV1::decode_canonical(&bytes).unwrap(),
        policy
    );

    assert_eq!(
        CustodyRestorePolicyV1::new(
            RestoreDisabledV1::Enabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreForbiddenV1::Forbidden,
        )
        .unwrap_err(),
        Error::UnsupportedRestoreBehavior
    );
    assert_eq!(
        CustodyRestorePolicyV1::new(
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreDisabledV1::Disabled,
            RestoreForbiddenV1::Allowed,
        )
        .unwrap_err(),
        Error::UnsupportedRestoreBehavior
    );

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["hooks"] = serde_json::json!("active");
    assert_eq!(
        CustodyRestorePolicyV1::decode_canonical(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        Error::InvalidEncoding
    );
}

#[test]
fn envelope_context_matches_seal_canonical_recipient_encoding() {
    let name = LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec());
    let direct = CustodyEnvelopeContextV1::new(
        name.clone(),
        digest(1),
        envelope_format(),
        vec!["z".to_owned(), "a".to_owned(), "a".to_owned()],
    )
    .unwrap();
    assert_eq!(direct.recipients(), &["a".to_owned(), "z".to_owned()]);

    let context = CustodyEnvelopeContextV1::new(
        name.clone(),
        digest(1),
        envelope_format(),
        vec!["z".to_owned(), "a".to_owned(), "a".to_owned()],
    )
    .unwrap();
    let proof = CustodyCapsuleSealProofV1::from_receipts(vec![CustodyEnvelopeSealReceiptV1::new(
        &context,
        stream_receipt(&[2]),
    )
    .unwrap()])
    .unwrap();
    let from_seal = CustodyEnvelopeOpenRequestV1::from_seal_artifact(&proof, name)
        .unwrap()
        .context()
        .clone();
    assert_eq!(
        direct.encode_canonical().unwrap(),
        from_seal.encode_canonical().unwrap()
    );

    for recipients in [vec![], vec![String::new()]] {
        assert_eq!(
            CustodyEnvelopeContextV1::new(
                LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
                digest(1),
                envelope_format(),
                recipients,
            )
            .unwrap_err(),
            Error::InvalidInput
        );
    }

    let non_utf8 = CustodyEnvelopeContextV1::new(
        LosslessPathV1::from_bytes(vec![b'p', b'a', b'y', b'l', b'o', b'a', b'd', b'/', 0xff]),
        digest(1),
        envelope_format(),
        vec!["recipient".to_owned()],
    )
    .unwrap();
    let encoded = non_utf8.encode_canonical().unwrap();
    assert_eq!(
        CustodyEnvelopeContextV1::decode_canonical(&encoded).unwrap(),
        non_utf8
    );
}

#[test]
fn canonical_decode_rejects_unknown_fields_wrong_schema_noncanonical_and_oversized_json() {
    let manifest = empty_manifest();
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
    let bytes = layout.index().encode_canonical().unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["unexpected"] = serde_json::json!(true);
    assert_eq!(
        CustodyCapsuleIndexV1::decode_canonical(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        Error::InvalidEncoding
    );

    let mut wrong_schema: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    wrong_schema["schema"] = serde_json::json!("custody-capsule-index.v2");
    assert_eq!(
        CustodyCapsuleIndexV1::decode_canonical(&serde_json::to_vec(&wrong_schema).unwrap())
            .unwrap_err(),
        Error::WrongSchema
    );

    let mut noncanonical = bytes;
    noncanonical.push(b'\n');
    assert_eq!(
        CustodyCapsuleIndexV1::decode_canonical(&noncanonical).unwrap_err(),
        Error::NonCanonicalEncoding
    );

    let oversized = vec![b' '; 1024 * 1024 + 1];
    assert_eq!(
        CustodyCapsuleIndexV1::decode_canonical(&oversized).unwrap_err(),
        Error::InvalidInput
    );

    let mut policy_value: serde_json::Value = serde_json::from_slice(
        &CustodyRestorePolicyV1::default()
            .encode_canonical()
            .unwrap(),
    )
    .unwrap();
    policy_value["unexpected"] = serde_json::json!(true);
    assert_eq!(
        CustodyRestorePolicyV1::decode_canonical(&serde_json::to_vec(&policy_value).unwrap())
            .unwrap_err(),
        Error::InvalidEncoding
    );

    let format = CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "1").unwrap();
    let mut format_value: serde_json::Value =
        serde_json::from_slice(&format.encode_canonical().unwrap()).unwrap();
    format_value["unexpected"] = serde_json::json!(true);
    assert_eq!(
        CustodyEnvelopeFormatV1::decode_canonical(&serde_json::to_vec(&format_value).unwrap())
            .unwrap_err(),
        Error::InvalidEncoding
    );

    let context = envelope_context();
    let mut context_value: serde_json::Value =
        serde_json::from_slice(&context.encode_canonical().unwrap()).unwrap();
    context_value["unexpected"] = serde_json::json!(true);
    assert_eq!(
        CustodyEnvelopeContextV1::decode_canonical(&serde_json::to_vec(&context_value).unwrap())
            .unwrap_err(),
        Error::InvalidEncoding
    );
}

#[test]
fn envelope_stream_source_accepts_large_artifacts_one_chunk_at_a_time() {
    let total = 1024 * 1024 + 1;
    let limits = CustodyEnvelopeStreamLimitsV1::new(total, 1024 * 1024, 2).unwrap();
    let descriptor = CustodyEnvelopeSourceDescriptorV1::new(total, limits).unwrap();
    let mut validator = CustodyEnvelopeSourceValidatorV1::new(descriptor);
    validator
        .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![7; 1024 * 1024], false).unwrap())
        .unwrap();
    validator
        .accept_chunk(&CustodyEnvelopeChunkV1::new(1, vec![9], true).unwrap())
        .unwrap();
    let receipt = validator.finish().unwrap();
    let mut expected = vec![7; 1024 * 1024];
    expected.push(9);
    assert_eq!(receipt.total_bytes(), total);
    assert_eq!(receipt.sha256(), &Sha256HexV1::digest(&expected));
}

#[test]
fn envelope_stream_source_refuses_ordering_finality_and_total_edges() {
    assert_eq!(
        CustodyEnvelopeChunkV1::new(0, vec![0; 1024 * 1024 + 1], true).unwrap_err(),
        Error::InvalidInput
    );
    assert_eq!(
        CustodyEnvelopeStreamLimitsV1::new(11, 5, 2).unwrap_err(),
        Error::InvalidInput
    );
    assert_eq!(
        CustodyEnvelopeStreamLimitsV1::new(u64::MAX, u64::MAX, u32::MAX).unwrap_err(),
        Error::InvalidInput
    );
    let limits = CustodyEnvelopeStreamLimitsV1::new(10, 5, 2).unwrap();
    assert_eq!(
        CustodyEnvelopeSourceDescriptorV1::new(11, limits).unwrap_err(),
        Error::InvalidInput
    );

    let descriptor = CustodyEnvelopeSourceDescriptorV1::new(2, limits).unwrap();
    let mut gapped = CustodyEnvelopeSourceValidatorV1::new(descriptor.clone());
    assert_eq!(
        gapped
            .accept_chunk(&CustodyEnvelopeChunkV1::new(1, vec![1], false).unwrap())
            .unwrap_err(),
        Error::InvalidInput
    );

    let mut non_final_exact_total = CustodyEnvelopeSourceValidatorV1::new(descriptor.clone());
    assert_eq!(
        non_final_exact_total
            .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![1, 2], false).unwrap())
            .unwrap_err(),
        Error::InvalidInput
    );

    let mut early_final = CustodyEnvelopeSourceValidatorV1::new(descriptor.clone());
    assert_eq!(
        early_final
            .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![1], true).unwrap())
            .unwrap_err(),
        Error::InvalidInput
    );

    let mut missing_final = CustodyEnvelopeSourceValidatorV1::new(descriptor.clone());
    missing_final
        .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![1], false).unwrap())
        .unwrap();
    assert_eq!(missing_final.finish().unwrap_err(), Error::InvalidInput);

    let mut after_final = CustodyEnvelopeSourceValidatorV1::new(descriptor);
    after_final
        .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![1], false).unwrap())
        .unwrap();
    after_final
        .accept_chunk(&CustodyEnvelopeChunkV1::new(1, vec![2], true).unwrap())
        .unwrap();
    assert_eq!(
        after_final
            .accept_chunk(&CustodyEnvelopeChunkV1::new(2, vec![3], true).unwrap())
            .unwrap_err(),
        Error::InvalidInput
    );

    let zero_limits = CustodyEnvelopeStreamLimitsV1::new(1, 1, 1).unwrap();
    let zero_descriptor = CustodyEnvelopeSourceDescriptorV1::new(0, zero_limits).unwrap();
    let mut zero = CustodyEnvelopeSourceValidatorV1::new(zero_descriptor.clone());
    zero.accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![], true).unwrap())
        .unwrap();
    assert_eq!(zero.finish().unwrap().total_bytes(), 0);
    let mut invalid_zero = CustodyEnvelopeSourceValidatorV1::new(zero_descriptor);
    assert_eq!(
        invalid_zero
            .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![], false).unwrap())
            .unwrap_err(),
        Error::InvalidInput
    );
}

#[test]
fn envelope_budget_sink_enforces_budget_finality_and_zero_byte_rules() {
    let limits = CustodyEnvelopeStreamLimitsV1::new(3, 2, 2).unwrap();
    let mut sink = CustodyEnvelopeSinkValidatorV1::new(limits);
    sink.accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![1, 2], false).unwrap())
        .unwrap();
    sink.accept_chunk(&CustodyEnvelopeChunkV1::new(1, vec![3], true).unwrap())
        .unwrap();
    let receipt = sink.finish().unwrap();
    assert_eq!(receipt.total_bytes(), 3);
    assert_eq!(receipt.sha256(), &Sha256HexV1::digest(&[1, 2, 3]));

    let mut missing_final = CustodyEnvelopeSinkValidatorV1::new(limits);
    missing_final
        .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![1, 2], false).unwrap())
        .unwrap();
    assert_eq!(missing_final.finish().unwrap_err(), Error::InvalidInput);

    let mut over_budget = CustodyEnvelopeSinkValidatorV1::new(limits);
    over_budget
        .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![1, 2], false).unwrap())
        .unwrap();
    assert_eq!(
        over_budget
            .accept_chunk(&CustodyEnvelopeChunkV1::new(1, vec![3, 4], true).unwrap())
            .unwrap_err(),
        Error::InvalidInput
    );

    let zero_limits = CustodyEnvelopeStreamLimitsV1::new(1, 1, 1).unwrap();
    let mut zero = CustodyEnvelopeSinkValidatorV1::new(zero_limits);
    zero.accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![], true).unwrap())
        .unwrap();
    assert_eq!(zero.finish().unwrap().total_bytes(), 0);
}

#[test]
fn seal_derived_open_request_binds_membership_limits_and_ciphertext_identity() {
    let name = LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec());
    let context = CustodyEnvelopeContextV1::new(
        name.clone(),
        digest(1),
        envelope_format(),
        vec!["recipient".to_owned()],
    )
    .unwrap();
    let proof = CustodyCapsuleSealProofV1::from_receipts(vec![CustodyEnvelopeSealReceiptV1::new(
        &context,
        stream_receipt(&[1, 2, 3, 4, 5]),
    )
    .unwrap()])
    .unwrap();
    let request = CustodyEnvelopeOpenRequestV1::from_seal_artifact(&proof, name.clone()).unwrap();
    assert_eq!(request.context().artifact_name(), &name);
    assert_eq!(request.context().format(), &envelope_format());
    assert_eq!(request.ciphertext_length(), 5);
    assert_eq!(
        request.ciphertext_sha256(),
        &Sha256HexV1::digest(&[1, 2, 3, 4, 5])
    );

    assert_eq!(
        CustodyEnvelopeOpenRequestV1::from_seal_artifact(
            &proof,
            LosslessPathV1::from_bytes(b"control/capsule-index.json.enc".to_vec()),
        )
        .unwrap_err(),
        Error::MissingArtifact
    );

    let oversized_format = CustodySealV1::new(
        digest(1),
        vec![CustodySealedArtifactV1::new(name.clone(), 5, digest(2)).unwrap()],
        vec!["recipient".to_owned()],
        "c".repeat(4097),
        "tool",
        "1",
    )
    .unwrap();
    assert_eq!(
        CustodyCapsuleSealProofV1::preflight_generic_seal_artifact_for_capsule_v1(
            &oversized_format,
            &name,
        )
        .unwrap_err(),
        Error::SealExceedsV1Limits
    );

    let oversized_artifact = CustodySealV1::new(
        digest(1),
        vec![CustodySealedArtifactV1::new(name.clone(), 10_737_418_241, digest(2)).unwrap()],
        vec!["recipient".to_owned()],
        "capsule-v1",
        "tool",
        "1",
    )
    .unwrap();
    assert_eq!(
        CustodyCapsuleSealProofV1::preflight_generic_seal_artifact_for_capsule_v1(
            &oversized_artifact,
            &name,
        )
        .unwrap_err(),
        Error::SealExceedsV1Limits
    );
}

#[test]
fn generic_capsule_preflight_refuses_over_limit_recipients_without_proportional_allocation() {
    let name = LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec());
    let mut too_many_recipients = Vec::new();
    for index in 0..65 {
        too_many_recipients.push(format!("recipient-{index}"));
    }
    let oversized_recipients = CustodySealV1::new(
        digest(1),
        vec![CustodySealedArtifactV1::new(name, 5, digest(2)).unwrap()],
        too_many_recipients,
        "capsule-v1",
        "tool",
        "1",
    )
    .unwrap();

    ALLOCATION_COUNT.with(|count| count.set(0));
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(true));
    let result =
        CustodyCapsuleSealProofV1::preflight_generic_seal_for_capsule_v1(&oversized_recipients);
    COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
    let allocation_count = ALLOCATION_COUNT.with(Cell::get);

    assert_eq!(result.unwrap_err(), Error::SealExceedsV1Limits);
    assert_eq!(allocation_count, 0, "capsule preflight allocated");
}

#[test]
fn seal_receipt_derives_authenticated_context_and_finalized_ciphertext_identity() {
    let context = envelope_context();
    let finalized = stream_receipt(&[1, 2, 3]);
    let receipt = CustodyEnvelopeSealReceiptV1::new(&context, finalized.clone()).unwrap();
    let cloned = CustodyEnvelopeSealReceiptV1::new(&context, finalized).unwrap();

    assert_eq!(receipt, cloned);
    assert_eq!(receipt.artifact_name(), context.artifact_name());
    assert_eq!(receipt.manifest_digest(), context.manifest_digest());
    assert_eq!(receipt.format(), context.format());
    assert_eq!(receipt.recipients(), context.recipients());
    assert_eq!(receipt.ciphertext_length(), 3);
    assert_eq!(
        receipt.ciphertext_sha256(),
        &Sha256HexV1::digest(&[1, 2, 3])
    );
    assert_eq!(
        receipt.to_sealed_artifact().unwrap().sha256(),
        receipt.ciphertext_sha256()
    );

    let zero_limits = CustodyEnvelopeStreamLimitsV1::new(1, 1, 1).unwrap();
    let mut zero_sink = CustodyEnvelopeSinkValidatorV1::new(zero_limits);
    zero_sink
        .accept_chunk(&CustodyEnvelopeChunkV1::new(0, vec![], true).unwrap())
        .unwrap();
    assert_eq!(
        CustodyEnvelopeSealReceiptV1::new(&context, zero_sink.finish().unwrap()).unwrap_err(),
        Error::InvalidInput
    );
}

#[test]
fn envelope_format_rejects_empty_identity_fields() {
    for (format, tool, version) in [
        ("", "tool", "1"),
        ("capsule-v1", "", "1"),
        ("capsule-v1", "tool", ""),
    ] {
        assert_eq!(
            CustodyEnvelopeFormatV1::new(format, tool, version).unwrap_err(),
            Error::InvalidInput
        );
    }
}

#[test]
fn metadata_bounds_reject_empty_fields_too_many_rows_and_oversized_fields() {
    for (key, value) in [("", "v"), ("k", "")] {
        let mut rows = BTreeMap::new();
        rows.insert(key.to_owned(), value.to_owned());
        assert_eq!(
            CustodyEnvelopeMetadataV1::new(rows).unwrap_err(),
            Error::InvalidInput
        );
    }

    let mut too_many = BTreeMap::new();
    for index in 0..65 {
        too_many.insert(format!("k{index}"), "v".to_owned());
    }
    assert_eq!(
        CustodyEnvelopeMetadataV1::new(too_many).unwrap_err(),
        Error::InvalidInput
    );

    let mut oversized_key = BTreeMap::new();
    oversized_key.insert("k".repeat(4097), "v".to_owned());
    assert_eq!(
        CustodyEnvelopeMetadataV1::new(oversized_key).unwrap_err(),
        Error::InvalidInput
    );

    let mut oversized_value = BTreeMap::new();
    oversized_value.insert("k".to_owned(), "v".repeat(4097));
    assert_eq!(
        CustodyEnvelopeMetadataV1::new(oversized_value).unwrap_err(),
        Error::InvalidInput
    );
}

#[test]
fn index_construction_is_order_independent_and_digest_binds_rows_and_manifest_digest() {
    let manifest = full_manifest();
    let layout = CustodyCapsuleLayoutV1::derive(&manifest).unwrap();
    let forward = layout.index().clone();
    let mut reversed_rows = layout.index().artifacts().to_vec();
    reversed_rows.reverse();
    let reversed =
        CustodyCapsuleIndexV1::new(manifest.content_digest().unwrap(), reversed_rows).unwrap();
    assert_eq!(forward, reversed);
    assert_eq!(
        forward.content_digest().unwrap(),
        reversed.content_digest().unwrap()
    );

    let different_manifest_digest =
        CustodyCapsuleIndexV1::new(digest(5), layout.index().artifacts().to_vec()).unwrap();
    assert_ne!(
        forward.content_digest().unwrap(),
        different_manifest_digest.content_digest().unwrap()
    );

    let mut rows = layout.index().artifacts().to_vec();
    rows.retain(|row| row.name().as_bytes() != b"payload/worktree.bin.enc");
    let different_rows =
        CustodyCapsuleIndexV1::new(manifest.content_digest().unwrap(), rows).unwrap();
    assert_ne!(
        forward.content_digest().unwrap(),
        different_rows.content_digest().unwrap()
    );
}

#[test]
fn envelope_format_and_context_digests_bind_every_field() {
    let format = CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "1").unwrap();
    let baseline_format = format.content_digest().unwrap();
    for changed in [
        CustodyEnvelopeFormatV1::new("other", "tool", "1").unwrap(),
        CustodyEnvelopeFormatV1::new("capsule-v1", "other", "1").unwrap(),
        CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "2").unwrap(),
    ] {
        assert_ne!(baseline_format, changed.content_digest().unwrap());
    }

    let context = envelope_context();
    let baseline_context = context.content_digest().unwrap();
    for changed in [
        CustodyEnvelopeContextV1::new(
            LosslessPathV1::from_bytes(b"control/capsule-index.json.enc".to_vec()),
            digest(1),
            envelope_format(),
            vec!["recipient".to_owned()],
        )
        .unwrap(),
        CustodyEnvelopeContextV1::new(
            LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
            digest(2),
            envelope_format(),
            vec!["recipient".to_owned()],
        )
        .unwrap(),
        CustodyEnvelopeContextV1::new(
            LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
            digest(1),
            CustodyEnvelopeFormatV1::new("other", "tool", "1").unwrap(),
            vec!["recipient".to_owned()],
        )
        .unwrap(),
        CustodyEnvelopeContextV1::new(
            LosslessPathV1::from_bytes(b"control/manifest.json.enc".to_vec()),
            digest(1),
            envelope_format(),
            vec!["other".to_owned()],
        )
        .unwrap(),
    ] {
        assert_ne!(baseline_context, changed.content_digest().unwrap());
    }
}

#[test]
fn direct_deserialization_routes_through_capsule_validation() {
    let row = CustodyCapsuleArtifactRoleRowV1::new(
        LosslessPathV1::from_bytes(b"payload/worktree.bin.enc".to_vec()),
        Role::CoveragePayload(Coverage::Worktree),
    )
    .unwrap();
    let mut row_value = serde_json::to_value(row).unwrap();
    row_value["name"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodyCapsuleArtifactRoleRowV1>(row_value).is_err());

    let index = CustodyCapsuleLayoutV1::derive(&empty_manifest())
        .unwrap()
        .index()
        .clone();
    let mut index_value = serde_json::to_value(index).unwrap();
    index_value["schema"] = serde_json::json!("custody-capsule-index.v2");
    assert!(serde_json::from_value::<CustodyCapsuleIndexV1>(index_value).is_err());

    let mut policy_value = serde_json::to_value(CustodyRestorePolicyV1::default()).unwrap();
    policy_value["hooks"] = serde_json::json!("enabled");
    assert!(serde_json::from_value::<CustodyRestorePolicyV1>(policy_value).is_err());

    let mut format_value =
        serde_json::to_value(CustodyEnvelopeFormatV1::new("capsule-v1", "tool", "1").unwrap())
            .unwrap();
    format_value["capsule_format"] = serde_json::json!("");
    assert!(serde_json::from_value::<CustodyEnvelopeFormatV1>(format_value).is_err());

    let mut context_value = serde_json::to_value(envelope_context()).unwrap();
    context_value["recipients"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodyEnvelopeContextV1>(context_value).is_err());
}
