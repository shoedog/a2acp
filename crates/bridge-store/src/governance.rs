//! Deferred: indexed history lookup and coverage evidence for legacy pre-coverage owners.
//! All scopes share the usage sidecar's immediate transaction. JSON preserves full u64 quantities.
use bridge_core::{accounting::UsageAttemptV1, error::BridgeError, governance::*, ids::AttemptId};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;

type Result<T> = std::result::Result<T, BridgeError>;
fn unavailable(_: impl std::fmt::Debug) -> BridgeError {
    BridgeError::StoreFailure
}
fn refuse(reason: BudgetRefusalReasonV1) -> BridgeError {
    BridgeError::BudgetRefused {
        refusal: Box::new(BudgetRefusalV1::new(reason)),
    }
}
fn decode<T: DeserializeOwned>(json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(unavailable)
}
fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(unavailable)
}
fn exists(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='governance_reservations_v1')",
        [],
        |r| r.get(0),
    )
    .map_err(unavailable)
}
fn records<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql).map_err(unavailable)?;
    let rows = stmt
        .query_map(args, |r| r.get::<_, String>(0))
        .map_err(unavailable)?;
    rows.map(|r| decode(&r.map_err(unavailable)?)).collect()
}
#[derive(Serialize, Deserialize)]
struct Reserved {
    reservation: BudgetReservationV1,
    liability: BudgetLiabilityV1,
}
fn owners(
    context: &BudgetContextV1,
    candidate: &BudgetCandidateV1,
    day: i64,
) -> Vec<BudgetScopeOwnerV1> {
    let mut owners = vec![
        BudgetScopeOwnerV1::Request(context.request.clone()),
        BudgetScopeOwnerV1::UtcDayAgent {
            agent: candidate.agent.clone(),
            utc_day: day,
        },
    ];
    if let Some(run) = &context.workflow_run {
        owners.push(BudgetScopeOwnerV1::WorkflowRun(run.clone()));
    }
    owners
}
fn owner_for(rule: &BudgetRuleV1, owners: &[BudgetScopeOwnerV1]) -> Option<BudgetScopeOwnerV1> {
    owners
        .iter()
        .find(|o| match (&rule.selector, o) {
            (BudgetSelectorV1::EachRequest, BudgetScopeOwnerV1::Request(_))
            | (BudgetSelectorV1::EachWorkflowRun, BudgetScopeOwnerV1::WorkflowRun(_)) => true,
            (BudgetSelectorV1::UtcDayAgent(a), BudgetScopeOwnerV1::UtcDayAgent { agent, .. }) => {
                a == agent
            }
            _ => false,
        })
        .cloned()
}
fn matches(owner: &BudgetScopeOwnerV1, row: &UsageAttemptV1, reserved: Option<&Reserved>) -> bool {
    match owner {
        BudgetScopeOwnerV1::Request(BudgetRequestIdV1::Execution(id))
        | BudgetScopeOwnerV1::WorkflowRun(id) => {
            row.attribution.execution.as_deref() == Some(id.as_str())
        }
        BudgetScopeOwnerV1::UtcDayAgent { agent, utc_day } => {
            // Unreserved unfinished dispatch blocks every day; counted work keeps its day.
            row.attribution.agent == agent.as_str()
                && ((reserved.is_none() && row.dispatched && !row.finalized)
                    || reserved.map_or(row.utc_day, |r| r.reservation.admission_day) == *utc_day)
        }
        _ => false,
    }
}
/// Reconcile only accounting rows, in the same transaction as their update. Returns one completion notice.
pub(crate) fn reconcile(tx: &Transaction<'_>, row: &UsageAttemptV1) -> Result<bool> {
    if !exists(tx)? {
        return Ok(false);
    }
    let json: Option<String> = tx
        .query_row(
            "SELECT record_json FROM governance_reservations_v1 WHERE id=?1",
            [&row.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(unavailable)?;
    let Some(json) = json else {
        return Ok(false);
    };
    let mut saved: Reserved = decode(&json)?;
    let notice = row.finalized
        && !matches!(
            saved.liability,
            BudgetLiabilityV1::Known(_) | BudgetLiabilityV1::Unknown { .. }
        )
        && row
            .usage
            .terminal
            .as_ref()
            .is_some_and(|t| t.total_tokens > saved.reservation.terminal_tokens)
        && saved.reservation.terminal_tokens > 0;
    let next = saved.liability.reconcile(&row.usage, row.finalized, false);
    if next == saved.liability {
        return Ok(false);
    }
    saved.liability = next;
    tx.execute(
        "UPDATE governance_reservations_v1 SET record_json=?2 WHERE id=?1",
        params![row.id, encode(&saved)?],
    )
    .map_err(unavailable)?;
    Ok(notice)
}

pub(crate) fn admit(
    conn: &mut Connection,
    policy: Option<&BudgetPolicyV1>,
    attempt: &AttemptId,
    context: &BudgetContextV1,
    candidate: &BudgetCandidateV1,
    clock: &dyn Fn() -> i64,
    owner_covered: &dyn Fn(&UsageAttemptV1, i64) -> Result<bool>,
) -> Result<BudgetAdmissionOutcomeV1> {
    use BudgetRefusalReasonV1 as R;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let now = clock();
    let day = now.div_euclid(86_400_000);
    let owners = owners(context, candidate, day);
    let active = exists(&tx)?;
    let reserved: HashMap<String, Reserved> = if active {
        records::<Reserved>(
            &tx,
            "SELECT record_json FROM governance_reservations_v1",
            [],
        )?
        .into_iter()
        .map(|r| (r.reservation.attempt.as_str().to_owned(), r))
        .collect()
    } else {
        HashMap::new()
    };
    // Committed reservations retain owner coverage independently of policy rule names.
    let mut bindings = reserved
        .values()
        .flat_map(|r| &r.reservation.scopes)
        .map(|s| &s.owner);
    for binding in bindings.clone() {
        if let BudgetScopeOwnerV1::WorkflowRun(run) = binding {
            if context.request == BudgetRequestIdV1::Execution(run.clone())
                && context.workflow_run.as_ref() != Some(run)
            {
                return Err(refuse(R::IdentityMismatch));
            }
        }
    }
    if active && policy.is_none() {
        let json: String = tx
            .query_row(
                "SELECT record_json FROM usage_attempts_v1 WHERE id=?1",
                [attempt.as_str()],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        let row: UsageAttemptV1 = decode(&json)?;
        if !matches(
            &BudgetScopeOwnerV1::Request(context.request.clone()),
            &row,
            None,
        ) || row.attribution.agent != candidate.agent.as_str()
            || row.attribution.effective_model != candidate.model
        {
            return Err(refuse(R::IdentityMismatch));
        }
    }
    let rebound = if active {
        let digest: String = tx
            .query_row(
                "SELECT digest_json FROM governance_policy_v1 WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        if policy.map(BudgetPolicyV1::digest) == Some(decode::<[u8; 32]>(&digest)?) {
            false
        } else {
            let unfinished: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM usage_attempts_v1 WHERE json_extract(record_json, '$.dispatched')=1 AND json_extract(record_json, '$.finalized')=0)",
                [], |r| r.get(0)
            ).map_err(unavailable)?;
            if unfinished
                || reserved
                    .values()
                    .any(|r| matches!(r.liability, BudgetLiabilityV1::Pending(_)))
            {
                return Err(refuse(R::PolicyMismatch));
            }
            policy.is_some()
        }
    } else {
        false
    };
    let Some(policy) = policy else {
        return Ok(BudgetAdmissionOutcomeV1::Ungoverned);
    };
    let BudgetRequestIdV1::Execution(execution) = &context.request else {
        return Err(refuse(R::UnsupportedRoute));
    };
    // Local workflows use one execution ID under distinct request/workflow tags. Other routes
    // lack persisted parentage in UsageAttemptV1 and cannot establish historical coverage yet.
    if context
        .workflow_run
        .as_ref()
        .is_some_and(|run| run != execution)
    {
        return Err(refuse(R::UnsupportedRoute));
    }
    let day_agent = policy
        .rules()
        .iter()
        .any(|r| matches!(&r.selector, BudgetSelectorV1::UtcDayAgent(a) if a == &candidate.agent))
        .then_some(candidate.agent.as_str());
    let extra = if active {
        " OR id IN (SELECT id FROM governance_reservations_v1 WHERE json_extract(record_json, '$.reservation.candidate.agent')=?2 AND json_extract(record_json, '$.reservation.admission_day')=?3)"
    } else {
        ""
    };
    let rows: Vec<UsageAttemptV1> = records(&tx, &format!("SELECT record_json FROM usage_attempts_v1 WHERE id=?4 OR json_extract(record_json, '$.attribution.execution')=?1 OR (json_extract(record_json, '$.attribution.agent')=?2 AND (json_extract(record_json, '$.utc_day')=?3 OR (json_extract(record_json, '$.dispatched')=1 AND json_extract(record_json, '$.finalized')=0))){extra}"), params![execution.as_str(), day_agent, day, attempt.as_str()])?;
    let row = rows
        .iter()
        .find(|r| r.id == attempt.as_str())
        .ok_or_else(|| refuse(R::IdentityMismatch))?;
    if row.attribution.execution.as_deref() != Some(execution.as_str())
        || row.attribution.agent != candidate.agent.as_str()
        || row.attribution.effective_model != candidate.model
    {
        return Err(refuse(R::IdentityMismatch));
    }
    if row.dispatched || row.finalized || reserved.contains_key(attempt.as_str()) {
        return Err(refuse(R::DuplicateAttempt));
    }
    let allowance = policy
        .allowance_for(candidate, context.workflow_run.is_some())
        .map_err(|refusal| BridgeError::BudgetRefused {
            refusal: Box::new(refusal),
        })?
        .map_or(0, |n| n.get());
    let coverage: i64 = tx
        .query_row(
            "SELECT started_ms FROM usage_coverage_v1 WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(unavailable)?;
    // Existing owner bindings are durable coverage proofs. For a new owner, require the
    // primary ledger's initial attempt, rather than treating an empty usage result as proof.
    let owner_scope = |o: &BudgetScopeOwnerV1| {
        matches!(
            o,
            BudgetScopeOwnerV1::Request(_) | BudgetScopeOwnerV1::WorkflowRun(_)
        )
    };
    if policy
        .rules()
        .iter()
        .any(|r| owner_for(r, &owners).is_some_and(|o| owner_scope(&o)))
        && !bindings.any(|b| owners.contains(b) && owner_scope(b))
        && !owner_covered(row, coverage)?
    {
        return Err(refuse(R::IncompleteCoverage));
    }
    let mut scopes = Vec::new();
    for rule in policy.rules() {
        let Some(owner) = owner_for(rule, &owners) else {
            continue;
        };
        let scope = BudgetScopeKeyV1 {
            rule_id: rule.id.clone(),
            owner: owner.clone(),
        };
        if let BudgetScopeOwnerV1::UtcDayAgent { utc_day, .. } = &owner {
            if utc_day
                .checked_mul(86_400_000)
                .is_none_or(|start| coverage > start)
            {
                return Err(refuse(R::IncompleteCoverage));
            }
        }
        let (mut turns, mut tokens) = (0_u64, 0_u64);
        let mut unknown = false;
        for previous in &rows {
            let saved = reserved.get(&previous.id);
            if !matches(&owner, previous, saved) {
                continue;
            }
            if !previous.dispatched {
                continue;
            }
            // Preparation before lazy initialization has no liability. Historical dispatch
            // before coverage, or pending work without a token reservation, is unverifiable.
            if (saved.is_none() && previous.started_ms < coverage)
                || (rule.limits.terminal_tokens.is_some()
                    && !previous.finalized
                    && saved.is_some_and(|r| r.reservation.terminal_tokens == 0))
            {
                return Err(refuse(R::IncompleteCoverage));
            }
            turns = turns.checked_add(1).ok_or_else(|| refuse(R::Overflow))?;
            let liability = if let Some(saved) = saved {
                saved
                    .liability
                    .reconcile(&previous.usage, previous.finalized, false)
            } else {
                if !previous.finalized {
                    return Err(refuse(R::IncompleteCoverage));
                }
                BudgetLiabilityV1::Pending(0).reconcile(&previous.usage, true, false)
            };
            unknown |= matches!(liability, BudgetLiabilityV1::Unknown { .. });
            if rule.limits.terminal_tokens.is_some() {
                tokens = tokens
                    .checked_add(liability.retained())
                    .ok_or_else(|| refuse(R::Overflow))?;
            }
        }
        for (metric, limit, used, additional) in [
            (BudgetMetricV1::Turns, rule.limits.turns, turns, 1),
            (
                BudgetMetricV1::TerminalTokens,
                rule.limits.terminal_tokens,
                tokens,
                allowance,
            ),
        ] {
            let Some(limit) = limit else {
                continue;
            };
            let reason = if metric == BudgetMetricV1::TerminalTokens && unknown {
                Some(R::UnknownMeasurement)
            } else {
                let total = used
                    .checked_add(additional)
                    .ok_or_else(|| refuse(R::Overflow))?;
                (total > limit).then_some(R::LimitExceeded)
            };
            if let Some(reason) = reason {
                return Err(BridgeError::BudgetRefused {
                    refusal: Box::new(BudgetRefusalV1 {
                        reason,
                        scope: Some(scope),
                        metric: Some(metric),
                    }),
                });
            }
        }
        scopes.push(scope);
    }
    // Schema, policy bindings, all liabilities and dispatch intent publish together or not at all.
    tx.execute_batch("CREATE TABLE IF NOT EXISTS governance_reservations_v1(id TEXT PRIMARY KEY, record_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS governance_policy_v1(singleton INTEGER PRIMARY KEY CHECK(singleton=1), digest_json TEXT NOT NULL);").map_err(unavailable)?;
    tx.execute(
        "INSERT INTO governance_policy_v1 VALUES(1, ?1) ON CONFLICT(singleton) DO UPDATE SET digest_json=excluded.digest_json",
        [encode(&policy.digest())?],
    ).map_err(unavailable)?;
    for previous in rows.iter().filter(|r| {
        scopes
            .iter()
            .any(|s| matches(&s.owner, r, reserved.get(&r.id)))
    }) {
        reconcile(&tx, previous)?;
    }
    let reservation = BudgetReservationV1 {
        attempt: attempt.clone(),
        context: context.clone(),
        candidate: candidate.clone(),
        policy_digest: policy.digest(),
        admitted_ms: now,
        admission_day: day,
        scopes,
        turns: 1,
        terminal_tokens: allowance,
    };
    tx.execute(
        "INSERT INTO governance_reservations_v1 VALUES(?1, ?2)",
        params![
            attempt.as_str(),
            encode(&Reserved {
                reservation: reservation.clone(),
                liability: BudgetLiabilityV1::Pending(allowance)
            })?
        ],
    )
    .map_err(unavailable)?;
    let mut dispatched = row.clone();
    dispatched.dispatched = true;
    tx.execute(
        "UPDATE usage_attempts_v1 SET record_json=?2 WHERE id=?1",
        params![attempt.as_str(), encode(&dispatched)?],
    )
    .map_err(unavailable)?;
    tx.commit().map_err(unavailable)?;
    if rebound {
        tracing::info!("governance policy rebound at store quiescence");
    }
    Ok(BudgetAdmissionOutcomeV1::Admitted(
        BudgetPermitV1::from_committed_admission(reservation),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::{accounting::UsageAttributionV1, ids::AttemptIdentity};

    #[test]
    fn commit_failure_never_mints_a_permit_or_publishes_dispatch() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE usage_attempts_v1(id TEXT PRIMARY KEY, record_json TEXT NOT NULL);
            CREATE TABLE usage_coverage_v1(singleton INTEGER PRIMARY KEY, started_ms INTEGER NOT NULL);
            INSERT INTO usage_coverage_v1 VALUES(1, 50);").unwrap();
        let owner = AttemptIdentity::initial().unwrap();
        let attempt = AttemptId::mint().unwrap();
        let candidate = BudgetCandidateV1 {
            agent: bridge_core::ids::AgentId::parse("a").unwrap(),
            model: None,
        };
        let context = BudgetContextV1 {
            request: BudgetRequestIdV1::Execution(owner.execution_id.clone()),
            workflow_run: None,
        };
        let row = UsageAttemptV1 {
            id: attempt.as_str().into(),
            attribution: UsageAttributionV1 {
                execution: Some(owner.execution_id.as_str().into()),
                agent: "a".into(),
                ..Default::default()
            },
            started_ms: 100,
            utc_day: 0,
            dispatched: false,
            finalized: false,
            usage: Default::default(),
            cost_invalid: false,
        };
        conn.execute(
            "INSERT INTO usage_attempts_v1 VALUES(?1, ?2)",
            params![row.id, encode(&row).unwrap()],
        )
        .unwrap();
        let p = BudgetPolicyV1::new(
            vec![BudgetRuleV1 {
                id: "r".into(),
                selector: BudgetSelectorV1::EachRequest,
                limits: BudgetLimitsV1 {
                    turns: Some(1),
                    terminal_tokens: None,
                },
            }],
            vec![],
        )
        .unwrap();
        conn.commit_hook(Some(|| true)); // SQLite rejects the actual COMMIT, after all writes succeed.
        assert!(matches!(
            admit(
                &mut conn,
                Some(&p),
                &attempt,
                &context,
                &candidate,
                &|| 100,
                &|_, _| Ok(true)
            ),
            Err(BridgeError::StoreFailure)
        ));
        assert!(!exists(&conn).unwrap());
        let rows: Vec<UsageAttemptV1> =
            records(&conn, "SELECT record_json FROM usage_attempts_v1", []).unwrap();
        assert!(!rows[0].dispatched);
        conn.commit_hook(None::<fn() -> bool>);
        assert!(matches!(
            admit(
                &mut conn,
                Some(&p),
                &attempt,
                &context,
                &candidate,
                &|| 100,
                &|_, _| Ok(true)
            )
            .unwrap(),
            BudgetAdmissionOutcomeV1::Admitted(_)
        ));
    }
}
