//! Production-inactive slice 2d claim-exchange regressions.
use bridge_core::domain::{AgentEntry, AgentKind};
use bridge_core::execution_policy::{
    freeze_node_execution_identity_v1, freeze_provider_attempt_v1, freeze_worktree_checkout_v1,
    resolve_execution_policy_v1, BoundWorktreeCustodyV1, DeadlineActivationV2,
    ExecutionPolicyInvocationV1, FrozenCheckoutEffectV1, FrozenProviderLogicalSessionV1,
    FrozenR2f1bContractV1, FrozenWorktreeCustodyPlanV1, HistoryAllocationKindV1, LedgerAdmissionV1,
    PolicyActivationV1, PolicyNodeRefV1, ProviderFreezeInputV1, Sha256HexV1,
    WorkflowControlDefaultsV1, WorktreeCheckoutInputV1, WorktreeCustodyIdV1,
};
use bridge_core::ids::{AgentId, AttemptId, AttemptIdentity, ExecutionId, NodeId, WorkflowId};
use bridge_core::liveness::{acquire_lease_in, FsLeaseProbe};
use bridge_core::mcp::McpDelivery;
use bridge_core::SessionCwd;
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
use bridge_workflow::run_spec::{WorkflowRunSpecV1, WorkflowSnapshotV3};
use bridge_worktree::custody::{
    custody_record_path, probe_custody_record_presence, read_custody_record_in,
    PreservationReasonV1, RecoveryLocatorV1, WorktreeCustodyStateKindV1,
};
use bridge_worktree::custody_lock::{try_acquire_custody_lock_in, CUSTODY_LOCK_DIR_NAME};
use bridge_worktree::custody_writer::{
    observed_identity, ClaimExchangeOutcomeV1, ClaimExchangeRequestV1, MaterializedIdentitiesV1,
    PreservationOutcomeV1, WorktreeCustodianV1,
};
use bridge_worktree::sweep::{sweep_orphans, WorktreeRunEndGuard};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ROOT_ATTEMPT: &str = "attempt-11111111111111111111111111111111";
const RETRY_ATTEMPT: &str = "attempt-22222222222222222222222222222222";

struct ExchangeFixture {
    root: PathBuf,
    target: PathBuf,
    lease_dir: PathBuf,
    predecessor: WorkflowSnapshotV3,
    successor: WorkflowSnapshotV3,
    predecessor_binding: BoundWorktreeCustodyV1,
    successor_binding: BoundWorktreeCustodyV1,
    retained: MaterializedIdentitiesV1,
}

fn unique_root(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "a2a-bridge-claim-exchange-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::canonicalize(root).unwrap()
}

fn graph() -> WorkflowGraph {
    WorkflowGraph {
        id: WorkflowId::parse("claim-exchange").unwrap(),
        nodes: vec![WorkflowNode {
            id: NodeId::parse("node").unwrap(),
            agent: AgentId::parse("codex").unwrap(),
            prompt_template: "{{input}}".to_string(),
            inputs: vec![],
            retry: None,
            harvest_sanitization: None,
        }],
        panel: None,
        controls: Some(WorkflowControlDefaultsV1::default()),
    }
}

fn entry() -> AgentEntry {
    AgentEntry {
        id: AgentId::parse("codex").unwrap(),
        cmd: Some("codex-acp".to_string()),
        base_url: None,
        api_key_env: None,
        args: vec![],
        kind: AgentKind::Acp,
        model_provider: None,
        model: Some("gpt-5.6-sol".to_string()),
        effort: None,
        mode: None,
        preflight: false,
        fallback_models: vec![],
        cwd: None,
        session_cwd: None,
        sandbox: None,
        watchdog: None,
        mcp: vec![],
        mcp_delivery: McpDelivery::Acp,
        auth_method: None,
        pre_authenticated: true,
        host_fallback_eligible: false,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        extensions: BTreeMap::new(),
    }
}

fn state(root: &Path, target: &Path) -> Option<WorktreeCustodyStateKindV1> {
    let pinned =
        bridge_core::fs_custody::PinnedDirectoryV1::open(root, "claim exchange test").ok()?;
    let name: OsString = Path::new(&custody_record_path(&target.to_string_lossy()))
        .file_name()?
        .to_os_string();
    read_custody_record_in(&pinned, &name)
        .ok()
        .map(|record| record.state.kind())
}

fn fixture(name: &str) -> ExchangeFixture {
    let root = unique_root(name);
    let source = root.join("source");
    let common = root.join("common");
    let lease_dir = root.join("leases");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&common).unwrap();

    let node = PolicyNodeRefV1::from_node_id(0, "node");
    let origin_attempt = AttemptId::parse(ROOT_ATTEMPT).unwrap();
    let logical_session = FrozenProviderLogicalSessionV1::Execute {
        candidate_ordinal: 0,
    };
    let checkout = freeze_worktree_checkout_v1(&WorktreeCheckoutInputV1 {
        attempt_id: origin_attempt.clone(),
        node: node.clone(),
        logical_session,
        source_cwd: SessionCwd::parse(&source.to_string_lossy()).unwrap(),
        canonical_source_cwd: SessionCwd::parse(&source.to_string_lossy()).unwrap(),
        canonical_worktree_root: SessionCwd::parse(&root.to_string_lossy()).unwrap(),
        worktree_owner: "claim-exchange".to_string(),
    })
    .unwrap();
    let FrozenCheckoutEffectV1::Worktree {
        target_cwd,
        checkout_digest,
        ..
    } = &checkout
    else {
        unreachable!("worktree freezer must produce a worktree checkout")
    };
    let target = PathBuf::from(target_cwd.as_str());
    std::fs::create_dir_all(&target).unwrap();
    let plan = FrozenWorktreeCustodyPlanV1 {
        custody_id: WorktreeCustodyIdV1::parse(format!("custody-{}", "a".repeat(64))).unwrap(),
        checkout_fingerprint: checkout_digest.clone(),
        target_cwd: target_cwd.clone(),
    };
    let contract = FrozenR2f1bContractV1::with_computed_fingerprint(
        DeadlineActivationV2::ManualOnlyR2f1a,
        vec![plan.clone()],
    )
    .unwrap();
    let provider_attempt = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
        entry: &entry(),
        overrides: None,
        node: node.clone(),
        logical_session,
        checkout,
        provider_effect_key: None,
    })
    .unwrap();
    let execution =
        freeze_node_execution_identity_v1(node.clone(), vec![provider_attempt]).unwrap();
    let controls = resolve_execution_policy_v1(
        graph().controls.as_ref().unwrap(),
        &ExecutionPolicyInvocationV1::default(),
        false,
        PolicyActivationV1::Production,
    )
    .unwrap();
    let spec = WorkflowRunSpecV1::build(
        origin_attempt.clone(),
        graph(),
        controls,
        Some(SessionCwd::parse("/repo").unwrap()),
        vec![execution],
        LedgerAdmissionV1::HistoryLedgerAdmitted {
            kind: HistoryAllocationKindV1::Configured,
        },
    )
    .unwrap();
    let initial = AttemptIdentity {
        execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
        attempt_id: origin_attempt.clone(),
        ordinal: 0,
        parent_attempt_id: None,
    };
    let predecessor = WorkflowRunSpecV1::decode_snapshot_v3(
        &spec.encode_snapshot_v3(initial.clone(), contract).unwrap(),
    )
    .unwrap();
    let successor = WorkflowSnapshotV3 {
        attempt: AttemptIdentity {
            execution_id: initial.execution_id.clone(),
            attempt_id: AttemptId::parse(RETRY_ATTEMPT).unwrap(),
            ordinal: 1,
            parent_attempt_id: Some(initial.attempt_id.clone()),
        },
        delivery_spec: predecessor.delivery_spec.clone(),
        predecessor_snapshot_digest: Some(predecessor.digest().unwrap()),
        r2f1b: predecessor.r2f1b.clone(),
    };
    let predecessor_binding = BoundWorktreeCustodyV1 {
        attempt: predecessor.attempt.clone(),
        origin_attempt_id: origin_attempt.clone(),
        node: node.clone(),
        plan: plan.clone(),
    };
    let successor_binding = BoundWorktreeCustodyV1 {
        attempt: successor.attempt.clone(),
        origin_attempt_id: origin_attempt,
        node,
        plan,
    };
    let retained = MaterializedIdentitiesV1 {
        source: observed_identity(&source.to_string_lossy()),
        root: observed_identity(&root.to_string_lossy()),
        worktree: observed_identity(&target.to_string_lossy()),
        common_dir: observed_identity(&common.to_string_lossy()),
    };
    let custodian = WorktreeCustodianV1::enter(
        &root,
        &target.to_string_lossy(),
        predecessor_binding.clone(),
    )
    .unwrap();
    custodian.publish_protection_prepared().unwrap();
    custodian.replace_materializing().unwrap();
    custodian.replace_live_protected(&retained).unwrap();
    drop(custodian);

    ExchangeFixture {
        root,
        target,
        lease_dir,
        predecessor,
        successor,
        predecessor_binding,
        successor_binding,
        retained,
    }
}

fn exchange(fixture: &ExchangeFixture) -> ClaimExchangeOutcomeV1 {
    WorktreeCustodianV1::claim_exchange_for_successor(ClaimExchangeRequestV1 {
        worktree_root: &fixture.root,
        worktree_path: &fixture.target.to_string_lossy(),
        predecessor_snapshot: &fixture.predecessor,
        successor_snapshot: &fixture.successor,
        predecessor_binding: &fixture.predecessor_binding,
        successor_binding: fixture.successor_binding.clone(),
        retained: &fixture.retained,
        lease_namespace_dir: &fixture.lease_dir,
    })
}

fn exchange_with(
    fixture: &ExchangeFixture,
    worktree_root: &Path,
    worktree_path: &str,
    retained: &MaterializedIdentitiesV1,
    lease_namespace_dir: &Path,
) -> ClaimExchangeOutcomeV1 {
    WorktreeCustodianV1::claim_exchange_for_successor(ClaimExchangeRequestV1 {
        worktree_root,
        worktree_path,
        predecessor_snapshot: &fixture.predecessor,
        successor_snapshot: &fixture.successor,
        predecessor_binding: &fixture.predecessor_binding,
        successor_binding: fixture.successor_binding.clone(),
        retained,
        lease_namespace_dir,
    })
}

fn record_path_in(worktree_root: &Path, worktree_path: &str) -> PathBuf {
    worktree_root.join(
        Path::new(&custody_record_path(worktree_path))
            .file_name()
            .unwrap(),
    )
}
fn custody_lock_entries(worktree_root: &Path) -> Vec<OsString> {
    let mut entries = std::fs::read_dir(worktree_root.join(CUSTODY_LOCK_DIR_NAME))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}
#[derive(Default)]
struct CountingProvider {
    calls: Cell<usize>,
    state_at_call: RefCell<Option<WorktreeCustodyStateKindV1>>,
}

impl CountingProvider {
    fn configure_after_exchange(&self, root: &Path, target: &Path) {
        self.calls.set(self.calls.get() + 1);
        self.state_at_call.replace(state(root, target));
    }
}

/// Test-only shape of slice 5's later provider continuation. It may run only after the
/// exchange has returned its lease-owning ready token; all other outcomes stop before it.
fn exchange_then_apply_provider_effect(
    fixture: &ExchangeFixture,
    provider: &CountingProvider,
) -> ClaimExchangeOutcomeV1 {
    let outcome = exchange(fixture);
    if let ClaimExchangeOutcomeV1::Exchanged(_) = &outcome {
        provider.configure_after_exchange(&fixture.root, &fixture.target);
    }
    outcome
}

#[test]
fn successor_attempt_and_claim_exchange_validates_before_any_provider_call() {
    let fixture = fixture("ordering");
    let provider = CountingProvider::default();
    let outcome = exchange_then_apply_provider_effect(&fixture, &provider);
    let ClaimExchangeOutcomeV1::Exchanged(ready) = outcome else {
        panic!("valid exchange refused");
    };
    assert_eq!(
        ready.predecessor_claim_digest(),
        &fixture.predecessor.digest().unwrap()
    );
    assert_eq!(ready.successor_attempt(), &fixture.successor.attempt);
    assert!(ready.successor_live_lease_path().exists());

    assert_eq!(provider.calls.get(), 1);
    assert_eq!(
        *provider.state_at_call.borrow(),
        Some(WorktreeCustodyStateKindV1::RecoveredLive),
        "the only provider-shaped effect is after the durable exchange"
    );
    drop(ready);
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn invalid_successor_claim_exchanges_refuse_before_provider_or_custody_effects() {
    for case in [
        "reused",
        "wrong-origin",
        "wrong-digest",
        "wrong-lineage",
        "wrong-parent",
        "wrong-node",
    ] {
        let mut fixture = fixture(case);
        match case {
            "reused" => {
                fixture.successor.attempt.attempt_id =
                    fixture.predecessor.attempt.attempt_id.clone()
            }
            "wrong-origin" => {
                fixture.successor_binding.origin_attempt_id =
                    AttemptId::parse(format!("attempt-{}", "8".repeat(32))).unwrap();
            }
            "wrong-digest" => {
                fixture.successor.predecessor_snapshot_digest = Some(Sha256HexV1::digest(b"wrong"));
            }
            "wrong-lineage" => fixture.successor.attempt.ordinal = 2,
            "wrong-parent" => {
                fixture.successor.attempt.parent_attempt_id =
                    Some(AttemptId::parse(format!("attempt-{}", "9".repeat(32))).unwrap());
            }
            "wrong-node" => {
                let wrong = PolicyNodeRefV1::from_node_id(1, "other-node");
                fixture.predecessor_binding.node = wrong.clone();
                fixture.successor_binding.node = wrong;
            }
            _ => unreachable!(),
        }
        let record = custody_record_path(&fixture.target.to_string_lossy());
        let before = std::fs::read(&record).unwrap();
        let provider = CountingProvider::default();
        let outcome = exchange_then_apply_provider_effect(&fixture, &provider);
        assert!(
            matches!(outcome, ClaimExchangeOutcomeV1::Refused(_)),
            "{case}: unexpected outcome"
        );
        assert_eq!(
            std::fs::read(&record).unwrap(),
            before,
            "{case}: exchange wrote custody"
        );
        assert!(
            !fixture.lease_dir.exists(),
            "{case}: validation must precede successor lease acquisition"
        );
        assert_eq!(
            provider.calls.get(),
            0,
            "{case}: provider must not be called"
        );
        std::fs::remove_dir_all(&fixture.root).unwrap();
    }
}

/// Sol S-1: even a caller-provided pristine root must stay untouched when snapshot validation
/// refuses. This catches moving `enter` ahead of the validation set.
#[test]
fn invalid_exchange_on_a_pristine_root_creates_no_custody_lock_directory() {
    let mut fixture = fixture("pristine-invalid");
    let pristine_root = unique_root("pristine-invalid-target");
    let lease_namespace_dir = pristine_root.join("leases");
    fixture.successor.attempt.ordinal = 2;

    assert!(matches!(
        exchange_with(
            &fixture,
            &pristine_root,
            &fixture.target.to_string_lossy(),
            &fixture.retained,
            &lease_namespace_dir,
        ),
        ClaimExchangeOutcomeV1::Refused(_)
    ));
    assert!(
        !pristine_root.join(CUSTODY_LOCK_DIR_NAME).exists(),
        "validation must run before either custody-cell directory is created"
    );
    assert!(
        !lease_namespace_dir.exists(),
        "validation must run before either recovery lease is acquired"
    );
    std::fs::remove_dir_all(&fixture.root).unwrap();
    std::fs::remove_dir_all(pristine_root).unwrap();
}

#[test]
fn a_live_predecessor_lease_refuses_without_writing_then_the_same_request_exchanges() {
    let fixture = fixture("live-predecessor");
    let record = custody_record_path(&fixture.target.to_string_lossy());
    let before = std::fs::read(&record).unwrap();
    let lock_entries_before = custody_lock_entries(&fixture.root);
    let successor_lease = fixture
        .lease_dir
        .join(format!("{}.lock", fixture.successor.attempt.run_id()));
    let predecessor_lease =
        acquire_lease_in(&fixture.lease_dir, fixture.predecessor.attempt.run_id()).unwrap();

    assert!(matches!(
        exchange(&fixture),
        ClaimExchangeOutcomeV1::Refused(_)
    ));
    assert_eq!(std::fs::read(&record).unwrap(), before);
    assert_eq!(custody_lock_entries(&fixture.root), lock_entries_before);
    assert!(
        !successor_lease.exists(),
        "a live predecessor must refuse before a successor lease is created"
    );

    drop(predecessor_lease);
    let ClaimExchangeOutcomeV1::Exchanged(ready) = exchange(&fixture) else {
        panic!("the identical request must exchange once the predecessor lease is free");
    };
    drop(ready);
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn successful_exchange_holds_the_successor_lease_until_the_ready_token_drops() {
    let fixture = fixture("successor-lease-held");
    let ClaimExchangeOutcomeV1::Exchanged(ready) = exchange(&fixture) else {
        panic!("valid exchange refused");
    };
    let successor_run_id = fixture.successor.attempt.run_id().to_string();
    assert!(ready.successor_live_lease_path().exists());
    assert!(
        acquire_lease_in(&fixture.lease_dir, &successor_run_id).is_err(),
        "the ready token must retain the successor flock"
    );
    drop(ready);
    let released = acquire_lease_in(&fixture.lease_dir, &successor_run_id)
        .expect("the successor flock must be acquirable after the ready token drops");
    drop(released);
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn frozen_graph_wrong_root_refuses_before_locks_record_or_lease() {
    let fixture = fixture("wrong-root");
    let foreign_root = unique_root("wrong-root-foreign");
    let foreign_record = record_path_in(&foreign_root, &fixture.target.to_string_lossy());
    std::fs::copy(
        custody_record_path(&fixture.target.to_string_lossy()),
        &foreign_record,
    )
    .unwrap();
    let before = std::fs::read(&foreign_record).unwrap();
    let foreign_lease_namespace = foreign_root.join("leases");

    assert!(matches!(
        exchange_with(
            &fixture,
            &foreign_root,
            &fixture.target.to_string_lossy(),
            &fixture.retained,
            &foreign_lease_namespace,
        ),
        ClaimExchangeOutcomeV1::Refused(_)
    ));
    assert_eq!(std::fs::read(&foreign_record).unwrap(), before);
    assert!(
        !foreign_root.join(CUSTODY_LOCK_DIR_NAME).exists(),
        "wrong-root validation must refuse before lock-directory creation"
    );
    assert!(
        !foreign_lease_namespace.exists(),
        "wrong-root validation must refuse before recovery-lease acquisition"
    );
    std::fs::remove_dir_all(&fixture.root).unwrap();
    std::fs::remove_dir_all(foreign_root).unwrap();
}

#[test]
fn frozen_graph_wrong_record_worktree_refuses_without_effects() {
    let fixture = fixture("wrong-record-worktree");
    let unrelated_worktree = fixture.root.join("unrelated-worktree");
    std::fs::create_dir_all(&unrelated_worktree).unwrap();
    let record = custody_record_path(&fixture.target.to_string_lossy());
    let mut record_value = bridge_worktree::custody::WorktreeCustodyRecordV1::decode_canonical(
        &std::fs::read(&record).unwrap(),
    )
    .unwrap();
    record_value.worktree = observed_identity(&unrelated_worktree.to_string_lossy());
    std::fs::write(&record, record_value.encode_canonical().unwrap()).unwrap();
    let before = std::fs::read(&record).unwrap();
    let lock_entries_before = custody_lock_entries(&fixture.root);

    assert!(matches!(
        exchange_with(
            &fixture,
            &fixture.root,
            &fixture.target.to_string_lossy(),
            &fixture.retained,
            &fixture.lease_dir,
        ),
        ClaimExchangeOutcomeV1::Refused(_)
    ));
    assert_eq!(std::fs::read(&record).unwrap(), before);
    assert_eq!(custody_lock_entries(&fixture.root), lock_entries_before);
    assert!(!fixture.lease_dir.exists());
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn frozen_graph_swapped_retained_source_and_root_refuses_without_effects() {
    let fixture = fixture("swapped-retained");
    let mut retained = fixture.retained.clone();
    std::mem::swap(&mut retained.source, &mut retained.root);
    let record = custody_record_path(&fixture.target.to_string_lossy());
    let before = std::fs::read(&record).unwrap();
    let lock_entries_before = custody_lock_entries(&fixture.root);

    assert!(matches!(
        exchange_with(
            &fixture,
            &fixture.root,
            &fixture.target.to_string_lossy(),
            &retained,
            &fixture.lease_dir,
        ),
        ClaimExchangeOutcomeV1::Refused(_)
    ));
    assert_eq!(std::fs::read(&record).unwrap(), before);
    assert_eq!(custody_lock_entries(&fixture.root), lock_entries_before);
    assert!(!fixture.lease_dir.exists());
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn terminal_preserved_claim_is_not_exchanged() {
    let fixture = fixture("terminal");
    let custodian = WorktreeCustodianV1::enter(
        &fixture.root,
        &fixture.target.to_string_lossy(),
        fixture.predecessor_binding.clone(),
    )
    .unwrap();
    assert_eq!(
        custodian.preserve_after_cancel(
            PreservationReasonV1::NodeFailure,
            &fixture.retained,
            RecoveryLocatorV1::RegisteredWorktree {},
            1_700_000_000_000,
        ),
        PreservationOutcomeV1::Preserved
    );
    drop(custodian);

    let record = custody_record_path(&fixture.target.to_string_lossy());
    let before = std::fs::read(&record).unwrap();
    assert!(matches!(
        exchange(&fixture),
        ClaimExchangeOutcomeV1::Refused(_)
    ));
    assert_eq!(std::fs::read(&record).unwrap(), before);
    assert_eq!(
        state(&fixture.root, &fixture.target),
        Some(WorktreeCustodyStateKindV1::Preserved)
    );
    let provider = CountingProvider::default();
    assert_eq!(provider.calls.get(), 0);
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn crash_after_claim_exchange_remains_recovered_live_and_sweep_protected() {
    let fixture = fixture("crash-after-exchange");
    let ready = match exchange(&fixture) {
        ClaimExchangeOutcomeV1::Exchanged(ready) => ready,
        _ => panic!("valid exchange must complete"),
    };
    sweep_orphans(&fixture.root.to_string_lossy(), "test-host", &FsLeaseProbe);
    let guard = WorktreeRunEndGuard::new(
        fixture.root.to_string_lossy().into_owned(),
        fixture.successor.attempt.run_id().to_string(),
    );
    guard.settle();
    drop(guard);

    assert_eq!(
        state(&fixture.root, &fixture.target),
        Some(WorktreeCustodyStateKindV1::RecoveredLive)
    );
    assert!(
        fixture.target.exists(),
        "both sweep paths retain RecoveredLive"
    );
    assert!(Path::new(&custody_record_path(&fixture.target.to_string_lossy())).exists());
    drop(ready);
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn claim_synced_but_lease_untransferred_keeps_both_protections() {
    let fixture = fixture("lease-window");
    let held_custody_cell =
        try_acquire_custody_lock_in(&fixture.root, &fixture.predecessor_binding.plan.custody_id)
            .unwrap();
    let predecessor_lease = fixture
        .lease_dir
        .join(format!("{}.lock", fixture.predecessor.attempt.run_id()));
    let parked_lease_namespace = fixture.root.join("parked-lease-namespace");
    let outcome = std::thread::scope(|scope| {
        let exchange_thread = scope.spawn(|| exchange(&fixture));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predecessor_lease.exists() {
            assert!(
                Instant::now() < deadline,
                "exchange never acquired the predecessor recovery lease"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::rename(&fixture.lease_dir, &parked_lease_namespace).unwrap();
        std::fs::write(&fixture.lease_dir, b"not a directory").unwrap();
        drop(held_custody_cell);
        exchange_thread.join().unwrap()
    });
    assert!(matches!(
        outcome,
        ClaimExchangeOutcomeV1::LeaseUnavailable(_)
    ));
    assert_eq!(
        state(&fixture.root, &fixture.target),
        Some(WorktreeCustodyStateKindV1::RecoveredLive)
    );
    assert!(
        !probe_custody_record_presence(&fixture.target.to_string_lossy())
            .authorizes_checkout_removal(),
        "the durable gate input remains refusing while the successor lease is unavailable"
    );
    sweep_orphans(&fixture.root.to_string_lossy(), "test-host", &FsLeaseProbe);
    assert!(
        fixture.target.exists(),
        "the boot sweep must retain the lease-transfer window"
    );
    let record = custody_record_path(&fixture.target.to_string_lossy());
    let published = std::fs::read(&record).unwrap();
    #[cfg(unix)]
    let published_inode = {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata(&record).unwrap().ino()
    };
    std::fs::remove_file(&fixture.lease_dir).unwrap();
    std::fs::rename(&parked_lease_namespace, &fixture.lease_dir).unwrap();
    let ClaimExchangeOutcomeV1::Exchanged(ready) = exchange(&fixture) else {
        panic!("a stranded RecoveredLive record must re-enter after the lease directory is fixed");
    };
    assert_eq!(
        std::fs::read(&record).unwrap(),
        published,
        "RecoveredLive re-entry must acquire only the successor lease, not re-publish the record"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(
            std::fs::metadata(&record).unwrap().ino(),
            published_inode,
            "RecoveredLive re-entry must not atomically replace an identical record"
        );
    }
    drop(ready);
    std::fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn recovered_live_reentry_refuses_a_different_successor_attempt() {
    let mut fixture = fixture("mismatched-reentry");
    let ClaimExchangeOutcomeV1::Exchanged(ready) = exchange(&fixture) else {
        panic!("valid exchange refused");
    };
    drop(ready);
    let record = custody_record_path(&fixture.target.to_string_lossy());
    let before = std::fs::read(&record).unwrap();
    fixture.successor.attempt.attempt_id =
        AttemptId::parse(format!("attempt-{}", "3".repeat(32))).unwrap();
    fixture.successor_binding.attempt = fixture.successor.attempt.clone();

    assert!(matches!(
        exchange(&fixture),
        ClaimExchangeOutcomeV1::Refused(_)
    ));
    assert_eq!(std::fs::read(&record).unwrap(), before);
    assert!(
        !fixture
            .lease_dir
            .join(format!("{}.lock", fixture.successor.attempt.run_id()))
            .exists(),
        "a mismatched re-entry must not mint a lease for a new successor"
    );
    std::fs::remove_dir_all(&fixture.root).unwrap();
}
