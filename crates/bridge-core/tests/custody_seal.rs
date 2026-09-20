use bridge_core::custody_inventory::{
    CustodyReasonCodeV1 as Reason, CustodyStateClassV1 as State, LosslessPathV1,
};
use bridge_core::custody_seal::{
    CustodyCoverageClassV1 as Coverage, CustodyCoverageEntryV1, CustodyDependencyV1,
    CustodyExclusionV1, CustodyGitObjectFormatV1 as ObjectFormat,
    CustodyGitObjectKindV1 as ObjectKind, CustodyManifestV1, CustodyOriginalObjectV1,
    CustodyOriginalRefV1, CustodyRefTargetV1, CustodySealErrorV1, CustodySealV1,
    CustodySealabilityV1, CustodySealedArtifactV1,
};
use bridge_core::execution_policy::Sha256HexV1;

fn digest(byte: u8) -> Sha256HexV1 {
    Sha256HexV1::digest(&[byte])
}

fn coverage(state: State) -> Vec<CustodyCoverageEntryV1> {
    Coverage::ALL
        .into_iter()
        .map(|class| {
            let reasons = if state == State::Unresolved {
                vec![Reason::ContentUnresolved]
            } else {
                vec![]
            };
            CustodyCoverageEntryV1::new(class, state, reasons, None).unwrap()
        })
        .collect()
}

fn object(hex: char, kind: ObjectKind) -> CustodyOriginalObjectV1 {
    CustodyOriginalObjectV1::new(ObjectFormat::Sha1, hex.to_string().repeat(40), kind).unwrap()
}

fn manifest_with(
    coverage: Vec<CustodyCoverageEntryV1>,
    refs: Vec<CustodyOriginalRefV1>,
    objects: Vec<CustodyOriginalObjectV1>,
    dependencies: Vec<CustodyDependencyV1>,
    exclusions: Vec<CustodyExclusionV1>,
) -> Result<CustodyManifestV1, CustodySealErrorV1> {
    CustodyManifestV1::new(
        "unit-1",
        "run-1",
        "materialization-1",
        "generation-1",
        coverage,
        refs,
        objects,
        dependencies,
        exclusions,
    )
}

fn artifact(name: &[u8], length: u64) -> CustodySealedArtifactV1 {
    CustodySealedArtifactV1::new(LosslessPathV1::from_bytes(name.to_vec()), length, digest(7))
        .unwrap()
}

fn seal_with(artifacts: Vec<CustodySealedArtifactV1>) -> Result<CustodySealV1, CustodySealErrorV1> {
    CustodySealV1::new(
        digest(1),
        artifacts,
        vec!["recipient-b".to_owned(), "recipient-a".to_owned()],
        "capsule-v1",
        "a2a-bridge",
        "0.1.0",
    )
}

#[test]
fn manifest_is_complete_order_independent_and_canonical() {
    let ref_a = CustodyOriginalRefV1::new(
        LosslessPathV1::from_bytes(b"refs/heads/a".to_vec()),
        CustodyRefTargetV1::Direct(object('a', ObjectKind::Commit)),
    )
    .unwrap();
    let ref_b = CustodyOriginalRefV1::new(
        LosslessPathV1::from_bytes(b"HEAD".to_vec()),
        CustodyRefTargetV1::Symbolic(LosslessPathV1::from_bytes(b"refs/heads/a".to_vec())),
    )
    .unwrap();
    let left = manifest_with(
        coverage(State::Captured).into_iter().rev().collect(),
        vec![ref_a.clone(), ref_b.clone()],
        vec![
            object('b', ObjectKind::Blob),
            object('a', ObjectKind::Commit),
        ],
        vec![],
        vec![],
    )
    .unwrap();
    let right = manifest_with(
        coverage(State::Captured),
        vec![ref_b, ref_a],
        vec![
            object('a', ObjectKind::Commit),
            object('b', ObjectKind::Blob),
        ],
        vec![],
        vec![],
    )
    .unwrap();

    assert_eq!(left, right);
    assert_eq!(
        left.content_digest().unwrap(),
        right.content_digest().unwrap()
    );
    let encoded = left.encode_canonical().unwrap();
    assert_eq!(CustodyManifestV1::decode_canonical(&encoded).unwrap(), left);
}

#[test]
fn manifest_digest_binds_every_constructor_input() {
    let baseline = manifest_with(coverage(State::Captured), vec![], vec![], vec![], vec![])
        .unwrap()
        .content_digest()
        .unwrap();
    let mut changed = Vec::new();
    for ids in [
        ["unit-2", "run-1", "materialization-1", "generation-1"],
        ["unit-1", "run-2", "materialization-1", "generation-1"],
        ["unit-1", "run-1", "materialization-2", "generation-1"],
        ["unit-1", "run-1", "materialization-1", "generation-2"],
    ] {
        changed.push(
            CustodyManifestV1::new(
                ids[0],
                ids[1],
                ids[2],
                ids[3],
                coverage(State::Captured),
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
        );
    }
    let mut empty_coverage = coverage(State::Captured);
    empty_coverage[0] =
        CustodyCoverageEntryV1::new(empty_coverage[0].class(), State::Empty, vec![], None).unwrap();
    changed.push(manifest_with(empty_coverage, vec![], vec![], vec![], vec![]).unwrap());
    changed.push(
        manifest_with(
            coverage(State::Captured),
            vec![CustodyOriginalRefV1::new(
                LosslessPathV1::from_bytes(b"HEAD".to_vec()),
                CustodyRefTargetV1::Unborn,
            )
            .unwrap()],
            vec![],
            vec![],
            vec![],
        )
        .unwrap(),
    );
    changed.push(
        manifest_with(
            coverage(State::Captured),
            vec![],
            vec![object('a', ObjectKind::Blob)],
            vec![],
            vec![],
        )
        .unwrap(),
    );
    changed.push(
        manifest_with(
            coverage(State::Captured),
            vec![],
            vec![],
            vec![CustodyDependencyV1::new(
                "dependency",
                "kind",
                digest(3),
                State::Captured,
                vec![],
            )
            .unwrap()],
            vec![],
        )
        .unwrap(),
    );

    let dependency =
        CustodyDependencyV1::new("dependency", "kind", digest(3), State::Captured, vec![]).unwrap();
    let exclusion =
        CustodyExclusionV1::new("exclusion", "class", "v1", vec!["dependency".to_owned()]).unwrap();
    let mut excluded_coverage = coverage(State::Captured);
    excluded_coverage[0] = CustodyCoverageEntryV1::new(
        excluded_coverage[0].class(),
        State::ExcludedReproducible,
        vec![],
        Some("exclusion".to_owned()),
    )
    .unwrap();
    changed.push(
        manifest_with(
            excluded_coverage,
            vec![],
            vec![],
            vec![dependency],
            vec![exclusion],
        )
        .unwrap(),
    );

    for manifest in changed {
        assert_ne!(baseline, manifest.content_digest().unwrap());
    }
}

#[test]
fn missing_or_duplicate_coverage_class_is_rejected() {
    let mut missing = coverage(State::Captured);
    missing.retain(|row| row.class() != Coverage::Worktree);
    assert_eq!(
        manifest_with(missing, vec![], vec![], vec![], vec![]).unwrap_err(),
        CustodySealErrorV1::IncompleteCoverage
    );

    let mut duplicate = coverage(State::Captured);
    duplicate.push(
        CustodyCoverageEntryV1::new(Coverage::Worktree, State::Captured, vec![], None).unwrap(),
    );
    assert_eq!(
        manifest_with(duplicate, vec![], vec![], vec![], vec![]).unwrap_err(),
        CustodySealErrorV1::DuplicateCoverageClass
    );
}

#[test]
fn coverage_state_and_exclusion_rules_are_closed() {
    assert_eq!(
        CustodyCoverageEntryV1::new(Coverage::Index, State::Unresolved, vec![], None).unwrap_err(),
        CustodySealErrorV1::UnresolvedWithoutReason
    );
    assert_eq!(
        CustodyCoverageEntryV1::new(
            Coverage::Index,
            State::Captured,
            vec![Reason::ContentUnresolved],
            None,
        )
        .unwrap_err(),
        CustodySealErrorV1::ResolvedWithBlockingReasons
    );
    assert_eq!(
        CustodyCoverageEntryV1::new(
            Coverage::ReproducibleOutputs,
            State::ExcludedReproducible,
            vec![],
            None,
        )
        .unwrap_err(),
        CustodySealErrorV1::ExcludedWithoutExclusion
    );
    assert_eq!(
        CustodyCoverageEntryV1::new(
            Coverage::Worktree,
            State::Captured,
            vec![],
            Some("exclusion-1".to_owned()),
        )
        .unwrap_err(),
        CustodySealErrorV1::UnexpectedExclusion
    );
}

#[test]
fn unresolved_coverage_and_dependencies_block_sealability() {
    let mut rows = coverage(State::Captured);
    rows[0] = CustodyCoverageEntryV1::new(
        rows[0].class(),
        State::Unresolved,
        vec![Reason::PrivacyHold, Reason::PrivacyHold],
        None,
    )
    .unwrap();
    let dependency = CustodyDependencyV1::new(
        "dependency-1",
        "alternate-object-store",
        digest(2),
        State::Unresolved,
        vec![Reason::DependencyUnresolved],
    )
    .unwrap();
    let manifest = manifest_with(rows, vec![], vec![], vec![dependency], vec![]).unwrap();
    assert_eq!(
        manifest.sealability(),
        CustodySealabilityV1::Blocked(vec![Reason::DependencyUnresolved, Reason::PrivacyHold])
    );
}

#[test]
fn exclusions_must_be_bidirectionally_bound_to_coverage_and_dependencies() {
    let dependency = CustodyDependencyV1::new(
        "toolchain",
        "reconstruction-input",
        digest(3),
        State::Captured,
        vec![],
    )
    .unwrap();
    let exclusion = CustodyExclusionV1::new(
        "build-cache",
        "target-directory",
        "policy-v1",
        vec!["toolchain".to_owned()],
    )
    .unwrap();
    assert_eq!(
        manifest_with(
            coverage(State::Captured),
            vec![],
            vec![],
            vec![dependency.clone()],
            vec![exclusion.clone()],
        )
        .unwrap_err(),
        CustodySealErrorV1::UnreferencedExclusion
    );

    let mut rows = coverage(State::Captured);
    let position = rows
        .iter()
        .position(|row| row.class() == Coverage::ReproducibleOutputs)
        .unwrap();
    rows[position] = CustodyCoverageEntryV1::new(
        Coverage::ReproducibleOutputs,
        State::ExcludedReproducible,
        vec![],
        Some("build-cache".to_owned()),
    )
    .unwrap();
    assert!(manifest_with(
        rows.clone(),
        vec![],
        vec![],
        vec![dependency],
        vec![exclusion],
    )
    .is_ok());

    let missing_dependency = CustodyExclusionV1::new(
        "build-cache",
        "target-directory",
        "policy-v1",
        vec!["missing".to_owned()],
    )
    .unwrap();
    assert_eq!(
        manifest_with(rows, vec![], vec![], vec![], vec![missing_dependency]).unwrap_err(),
        CustodySealErrorV1::UnknownReconstructionDependency
    );
}

#[test]
fn ref_targets_preserve_symbolic_unborn_and_direct_identity() {
    let name = LosslessPathV1::from_bytes(b"HEAD".to_vec());
    for target in [
        CustodyRefTargetV1::Direct(object('a', ObjectKind::Commit)),
        CustodyRefTargetV1::Symbolic(LosslessPathV1::from_bytes(b"refs/heads/main".to_vec())),
        CustodyRefTargetV1::Unborn,
    ] {
        assert!(CustodyOriginalRefV1::new(name.clone(), target).is_ok());
    }
    assert_eq!(
        CustodyOriginalRefV1::new(
            LosslessPathV1::from_bytes(vec![]),
            CustodyRefTargetV1::Unborn,
        )
        .unwrap_err(),
        CustodySealErrorV1::EmptyRefName
    );
    assert_eq!(
        CustodyOriginalRefV1::new(
            LosslessPathV1::from_bytes(b"HEAD".to_vec()),
            CustodyRefTargetV1::Symbolic(LosslessPathV1::from_bytes(vec![])),
        )
        .unwrap_err(),
        CustodySealErrorV1::EmptySymbolicRefName
    );
}

#[test]
fn conflicting_ref_names_and_object_kinds_are_rejected() {
    let name = LosslessPathV1::from_bytes(b"refs/heads/main".to_vec());
    let left = CustodyOriginalRefV1::new(
        name.clone(),
        CustodyRefTargetV1::Direct(object('a', ObjectKind::Commit)),
    )
    .unwrap();
    let right = CustodyOriginalRefV1::new(
        name,
        CustodyRefTargetV1::Direct(object('b', ObjectKind::Commit)),
    )
    .unwrap();
    assert_eq!(
        manifest_with(
            coverage(State::Captured),
            vec![left, right],
            vec![],
            vec![],
            vec![],
        )
        .unwrap_err(),
        CustodySealErrorV1::ConflictingRefName
    );

    assert_eq!(
        manifest_with(
            coverage(State::Captured),
            vec![],
            vec![
                object('a', ObjectKind::Commit),
                object('a', ObjectKind::Blob)
            ],
            vec![],
            vec![],
        )
        .unwrap_err(),
        CustodySealErrorV1::ConflictingObjectKind
    );
}

#[test]
fn direct_ref_targets_must_exist_in_the_original_object_inventory() {
    let target = object('a', ObjectKind::Commit);
    let direct = CustodyOriginalRefV1::new(
        LosslessPathV1::from_bytes(b"refs/heads/main".to_vec()),
        CustodyRefTargetV1::Direct(target.clone()),
    )
    .unwrap();
    assert_eq!(
        manifest_with(
            coverage(State::Captured),
            vec![direct.clone()],
            vec![],
            vec![],
            vec![],
        )
        .unwrap_err(),
        CustodySealErrorV1::RefTargetObjectMissing
    );
    assert!(manifest_with(
        coverage(State::Captured),
        vec![direct],
        vec![target],
        vec![],
        vec![],
    )
    .is_ok());
}

#[test]
fn git_object_ids_are_exact_lowercase_hex_for_the_selected_format() {
    for (format, value) in [
        (ObjectFormat::Sha1, "a".repeat(39)),
        (ObjectFormat::Sha1, "A".repeat(40)),
        (ObjectFormat::Sha256, "g".repeat(64)),
    ] {
        assert_eq!(
            CustodyOriginalObjectV1::new(format, value, ObjectKind::Unknown).unwrap_err(),
            CustodySealErrorV1::InvalidGitObjectId
        );
    }
    assert!(
        CustodyOriginalObjectV1::new(ObjectFormat::Sha256, "f".repeat(64), ObjectKind::Tree,)
            .is_ok()
    );
}

#[test]
fn canonical_decode_rejects_wrong_schema_and_noncanonical_json() {
    let manifest =
        manifest_with(coverage(State::Captured), vec![], vec![], vec![], vec![]).unwrap();
    let bytes = manifest.encode_canonical().unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["schema"] = serde_json::json!("custody-manifest.v2");
    assert_eq!(
        CustodyManifestV1::decode_canonical(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        CustodySealErrorV1::WrongManifestSchema
    );
    let mut noncanonical = bytes;
    noncanonical.push(b'\n');
    assert_eq!(
        CustodyManifestV1::decode_canonical(&noncanonical).unwrap_err(),
        CustodySealErrorV1::NonCanonicalEncoding
    );
    assert_eq!(
        CustodyManifestV1::decode_canonical(b"{").unwrap_err(),
        CustodySealErrorV1::InvalidEncoding
    );
}

#[test]
fn direct_deserialization_cannot_bypass_manifest_or_seal_validation() {
    let mut coverage_value = serde_json::to_value(
        CustodyCoverageEntryV1::new(Coverage::Index, State::Captured, vec![], None).unwrap(),
    )
    .unwrap();
    coverage_value["state"] = serde_json::json!("unresolved");
    assert!(serde_json::from_value::<CustodyCoverageEntryV1>(coverage_value).is_err());

    let mut object_value = serde_json::to_value(object('a', ObjectKind::Commit)).unwrap();
    object_value["object_id"] = serde_json::json!("A".repeat(40));
    assert!(serde_json::from_value::<CustodyOriginalObjectV1>(object_value).is_err());

    let mut target_value = serde_json::to_value(CustodyRefTargetV1::Symbolic(
        LosslessPathV1::from_bytes(b"HEAD".to_vec()),
    ))
    .unwrap();
    target_value["value"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodyRefTargetV1>(target_value).is_err());

    let reference = CustodyOriginalRefV1::new(
        LosslessPathV1::from_bytes(b"HEAD".to_vec()),
        CustodyRefTargetV1::Unborn,
    )
    .unwrap();
    let mut reference_value = serde_json::to_value(reference).unwrap();
    reference_value["name"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodyOriginalRefV1>(reference_value).is_err());

    let dependency =
        CustodyDependencyV1::new("dependency", "kind", digest(1), State::Captured, vec![]).unwrap();
    let mut dependency_value = serde_json::to_value(dependency).unwrap();
    dependency_value["dependency_id"] = serde_json::json!("");
    assert!(serde_json::from_value::<CustodyDependencyV1>(dependency_value).is_err());

    let exclusion =
        CustodyExclusionV1::new("exclusion", "class", "v1", vec!["dependency".to_owned()]).unwrap();
    let mut exclusion_value = serde_json::to_value(exclusion).unwrap();
    exclusion_value["reconstruction_dependency_ids"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodyExclusionV1>(exclusion_value).is_err());

    let manifest =
        manifest_with(coverage(State::Captured), vec![], vec![], vec![], vec![]).unwrap();
    let mut manifest_value = serde_json::to_value(manifest).unwrap();
    manifest_value["coverage"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodyManifestV1>(manifest_value).is_err());

    let seal = seal_with(vec![artifact(b"data", 1)]).unwrap();
    let mut seal_value = serde_json::to_value(seal).unwrap();
    seal_value["artifacts"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodySealV1>(seal_value).is_err());

    let mut artifact_value = serde_json::to_value(artifact(b"data", 1)).unwrap();
    artifact_value["name"] = serde_json::json!([]);
    assert!(serde_json::from_value::<CustodySealedArtifactV1>(artifact_value).is_err());
}

#[test]
fn seal_digest_binds_artifact_length_and_is_order_independent() {
    let left = seal_with(vec![
        artifact(b"objects/pack", 7),
        artifact(b"manifest.json", 9),
    ])
    .unwrap();
    let right = CustodySealV1::new(
        digest(1),
        vec![artifact(b"manifest.json", 9), artifact(b"objects/pack", 7)],
        vec!["recipient-a".to_owned(), "recipient-b".to_owned()],
        "capsule-v1",
        "a2a-bridge",
        "0.1.0",
    )
    .unwrap();
    assert_eq!(
        left.content_digest().unwrap(),
        right.content_digest().unwrap()
    );

    let changed_length = seal_with(vec![
        artifact(b"objects/pack", 8),
        artifact(b"manifest.json", 9),
    ])
    .unwrap();
    assert_ne!(
        left.content_digest().unwrap(),
        changed_length.content_digest().unwrap()
    );
}

#[test]
fn seal_digest_binds_every_declared_identity_field() {
    let make =
        |manifest_digest, name: &[u8], artifact_digest, recipient: &str, format, tool, version| {
            CustodySealV1::new(
                manifest_digest,
                vec![CustodySealedArtifactV1::new(
                    LosslessPathV1::from_bytes(name.to_vec()),
                    7,
                    artifact_digest,
                )
                .unwrap()],
                vec![recipient.to_owned()],
                format,
                tool,
                version,
            )
            .unwrap()
            .content_digest()
            .unwrap()
        };
    let baseline = make(
        digest(1),
        b"data",
        digest(7),
        "recipient",
        "format",
        "tool",
        "1",
    );
    for changed in [
        make(
            digest(2),
            b"data",
            digest(7),
            "recipient",
            "format",
            "tool",
            "1",
        ),
        make(
            digest(1),
            b"other",
            digest(7),
            "recipient",
            "format",
            "tool",
            "1",
        ),
        make(
            digest(1),
            b"data",
            digest(8),
            "recipient",
            "format",
            "tool",
            "1",
        ),
        make(
            digest(1),
            b"data",
            digest(7),
            "other",
            "format",
            "tool",
            "1",
        ),
        make(
            digest(1),
            b"data",
            digest(7),
            "recipient",
            "other",
            "tool",
            "1",
        ),
        make(
            digest(1),
            b"data",
            digest(7),
            "recipient",
            "format",
            "other",
            "1",
        ),
        make(
            digest(1),
            b"data",
            digest(7),
            "recipient",
            "format",
            "tool",
            "2",
        ),
    ] {
        assert_ne!(baseline, changed);
    }
}

#[test]
fn zero_byte_artifact_is_valid_but_unsafe_relative_names_are_rejected() {
    assert!(seal_with(vec![artifact(b"empty", 0)]).is_ok());
    for name in [
        b"".as_slice(),
        b"/absolute".as_slice(),
        b"a//b".as_slice(),
        b".".as_slice(),
        b"a/../b".as_slice(),
        b"a/".as_slice(),
        b"a\0b".as_slice(),
        b"a\\..\\b".as_slice(),
        b"C:data".as_slice(),
        b"C:/data".as_slice(),
    ] {
        assert_eq!(
            CustodySealedArtifactV1::new(LosslessPathV1::from_bytes(name.to_vec()), 1, digest(9),)
                .unwrap_err(),
            CustodySealErrorV1::UnsafeArtifactName
        );
    }
}

#[test]
fn lossless_non_utf8_artifact_names_round_trip_without_filesystem_interpretation() {
    let seal = seal_with(vec![artifact(&[b'd', b'a', b't', b'a', 0xff], 4)]).unwrap();
    let encoded = seal.encode_canonical().unwrap();
    assert_eq!(CustodySealV1::decode_canonical(&encoded).unwrap(), seal);
}

#[test]
fn artifact_duplicates_and_prefix_conflicts_are_rejected() {
    assert_eq!(
        seal_with(vec![artifact(b"pack", 1), artifact(b"pack", 1)]).unwrap_err(),
        CustodySealErrorV1::DuplicateArtifactName
    );
    assert_eq!(
        seal_with(vec![artifact(b"pack", 1), artifact(b"pack/data", 2)]).unwrap_err(),
        CustodySealErrorV1::ArtifactPrefixConflict
    );
    assert_eq!(
        seal_with(vec![
            artifact(b"pack/data", 2),
            artifact(b"pack!", 3),
            artifact(b"pack", 1),
        ])
        .unwrap_err(),
        CustodySealErrorV1::ArtifactPrefixConflict
    );
}

#[test]
fn seal_requires_recipients_and_versions_and_decodes_only_canonical_schema() {
    for (recipients, format, tool, version, expected) in [
        (
            vec![],
            "capsule-v1",
            "tool",
            "1",
            CustodySealErrorV1::NoRecipients,
        ),
        (
            vec!["".to_owned()],
            "capsule-v1",
            "tool",
            "1",
            CustodySealErrorV1::EmptyRecipient,
        ),
        (
            vec!["r".to_owned()],
            "",
            "tool",
            "1",
            CustodySealErrorV1::EmptyCapsuleFormat,
        ),
        (
            vec!["r".to_owned()],
            "capsule-v1",
            "",
            "1",
            CustodySealErrorV1::EmptyTool,
        ),
        (
            vec!["r".to_owned()],
            "capsule-v1",
            "tool",
            "",
            CustodySealErrorV1::EmptyToolVersion,
        ),
    ] {
        assert_eq!(
            CustodySealV1::new(
                digest(1),
                vec![artifact(b"data", 1)],
                recipients,
                format,
                tool,
                version
            )
            .unwrap_err(),
            expected
        );
    }

    let seal = seal_with(vec![artifact(b"data", 1)]).unwrap();
    let bytes = seal.encode_canonical().unwrap();
    assert_eq!(CustodySealV1::decode_canonical(&bytes).unwrap(), seal);
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["schema"] = serde_json::json!("custody-seal.v2");
    assert_eq!(
        CustodySealV1::decode_canonical(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
        CustodySealErrorV1::WrongSealSchema
    );
}

#[test]
fn empty_manifest_identifiers_and_empty_exclusion_fields_refuse() {
    for empty_index in 0..4 {
        let mut ids = ["unit", "run", "materialization", "generation"];
        ids[empty_index] = "";
        assert_eq!(
            CustodyManifestV1::new(
                ids[0],
                ids[1],
                ids[2],
                ids[3],
                coverage(State::Captured),
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .unwrap_err(),
            CustodySealErrorV1::EmptyManifestIdentity
        );
    }
    assert_eq!(
        CustodyExclusionV1::new("id", "class", "policy", vec![]).unwrap_err(),
        CustodySealErrorV1::NoReconstructionDependencies
    );
}

#[test]
fn dependency_and_exclusion_constructor_branches_are_closed() {
    for (id, kind) in [("", "kind"), ("id", "")] {
        assert_eq!(
            CustodyDependencyV1::new(id, kind, digest(1), State::Captured, vec![]).unwrap_err(),
            CustodySealErrorV1::EmptyDependencyIdentity
        );
    }
    let left = CustodyDependencyV1::new("dependency", "kind-a", digest(1), State::Captured, vec![])
        .unwrap();
    let right =
        CustodyDependencyV1::new("dependency", "kind-b", digest(2), State::Captured, vec![])
            .unwrap();
    assert_eq!(
        manifest_with(
            coverage(State::Captured),
            vec![],
            vec![],
            vec![left, right],
            vec![],
        )
        .unwrap_err(),
        CustodySealErrorV1::ConflictingDependencyId
    );

    for (id, class, policy) in [("", "class", "v1"), ("id", "", "v1"), ("id", "class", "")] {
        assert_eq!(
            CustodyExclusionV1::new(id, class, policy, vec!["dependency".to_owned()]).unwrap_err(),
            CustodySealErrorV1::EmptyExclusionIdentity
        );
    }
    assert_eq!(
        CustodyExclusionV1::new("id", "class", "v1", vec![String::new()]).unwrap_err(),
        CustodySealErrorV1::EmptyReconstructionDependency
    );
    let left = CustodyExclusionV1::new("exclusion", "class-a", "v1", vec!["dependency".to_owned()])
        .unwrap();
    let right =
        CustodyExclusionV1::new("exclusion", "class-b", "v1", vec!["dependency".to_owned()])
            .unwrap();
    assert_eq!(
        manifest_with(
            coverage(State::Captured),
            vec![],
            vec![],
            vec![],
            vec![left, right],
        )
        .unwrap_err(),
        CustodySealErrorV1::ConflictingExclusionId
    );
}

#[test]
fn duplicate_records_and_cross_reference_failures_are_explicit() {
    let target = object('a', ObjectKind::Commit);
    let reference = CustodyOriginalRefV1::new(
        LosslessPathV1::from_bytes(b"refs/heads/main".to_vec()),
        CustodyRefTargetV1::Direct(target.clone()),
    )
    .unwrap();
    assert!(manifest_with(
        coverage(State::Captured),
        vec![reference.clone(), reference],
        vec![target],
        vec![],
        vec![],
    )
    .is_ok());

    let mut rows = coverage(State::Captured);
    let position = rows
        .iter()
        .position(|row| row.class() == Coverage::ReproducibleOutputs)
        .unwrap();
    rows[position] = CustodyCoverageEntryV1::new(
        Coverage::ReproducibleOutputs,
        State::ExcludedReproducible,
        vec![],
        Some("missing".to_owned()),
    )
    .unwrap();
    assert_eq!(
        manifest_with(rows, vec![], vec![], vec![], vec![]).unwrap_err(),
        CustodySealErrorV1::UnknownExclusion
    );

    assert_eq!(
        CustodySealV1::new(
            digest(1),
            vec![],
            vec!["recipient".to_owned()],
            "capsule-v1",
            "tool",
            "1",
        )
        .unwrap_err(),
        CustodySealErrorV1::NoArtifacts
    );
}

#[test]
fn exact_duplicate_objects_dependencies_exclusions_and_reconstruction_ids_deduplicate() {
    let object = object('a', ObjectKind::Blob);
    let object_once = manifest_with(
        coverage(State::Captured),
        vec![],
        vec![object.clone()],
        vec![],
        vec![],
    )
    .unwrap();
    let object_twice = manifest_with(
        coverage(State::Captured),
        vec![],
        vec![object.clone(), object],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(object_once, object_twice);

    let dependency =
        CustodyDependencyV1::new("dependency", "kind", digest(1), State::Captured, vec![]).unwrap();
    let dependency_once = manifest_with(
        coverage(State::Captured),
        vec![],
        vec![],
        vec![dependency.clone()],
        vec![],
    )
    .unwrap();
    let dependency_twice = manifest_with(
        coverage(State::Captured),
        vec![],
        vec![],
        vec![dependency.clone(), dependency.clone()],
        vec![],
    )
    .unwrap();
    assert_eq!(dependency_once, dependency_twice);

    let mut rows = coverage(State::Captured);
    rows[0] = CustodyCoverageEntryV1::new(
        rows[0].class(),
        State::ExcludedReproducible,
        vec![],
        Some("exclusion".to_owned()),
    )
    .unwrap();
    let exclusion = CustodyExclusionV1::new(
        "exclusion",
        "class",
        "v1",
        vec!["dependency".to_owned(), "dependency".to_owned()],
    )
    .unwrap();
    let exclusion_once = manifest_with(
        rows.clone(),
        vec![],
        vec![],
        vec![dependency.clone()],
        vec![exclusion.clone()],
    )
    .unwrap();
    let exclusion_twice = manifest_with(
        rows,
        vec![],
        vec![],
        vec![dependency],
        vec![exclusion.clone(), exclusion],
    )
    .unwrap();
    assert_eq!(exclusion_once, exclusion_twice);
}
