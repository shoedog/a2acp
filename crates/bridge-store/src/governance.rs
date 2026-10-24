//! All scopes share the usage sidecar's immediate transaction. JSON preserves full u64 quantities.
use bridge_core::{accounting::UsageAttemptV1, error::BridgeError, governance::*, ids::AttemptId};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='governance_scopes_v1')",
        [],
        |r| r.get(0),
    )
    .map_err(unavailable)
}
fn records<T: DeserializeOwned>(conn: &Connection, sql: &str) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql).map_err(unavailable)?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(unavailable)?;
    rows.map(|r| decode(&r.map_err(unavailable)?)).collect()
}
#[derive(Serialize, Deserialize)]
struct Reserved {
    reservation: BudgetReservationV1,
    liability: BudgetLiabilityV1,
}
#[derive(Serialize, Deserialize)]
struct Binding {
    owner: BudgetScopeOwnerV1,
    rule: BudgetRuleV1,
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
            row.attribution.agent == agent.as_str()
                && reserved.map_or(row.utc_day, |r| r.reservation.admission_day) == *utc_day
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
    saved.liability = saved.liability.reconcile(&row.usage, row.finalized, false);
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
) -> Result<BudgetAdmissionOutcomeV1> {
    use BudgetRefusalReasonV1 as R;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(unavailable)?;
    let now = clock();
    let day = now.div_euclid(86_400_000);
    let owners = owners(context, candidate, day);
    let active = exists(&tx)?;
    let bindings: Vec<Binding> = if active {
        records(&tx, "SELECT record_json FROM governance_scopes_v1")?
    } else {
        vec![]
    };
    for binding in &bindings {
        if let BudgetScopeOwnerV1::WorkflowRun(run) = &binding.owner {
            if context.request == BudgetRequestIdV1::Execution(run.clone())
                && context.workflow_run.as_ref() != Some(run)
            {
                return Err(refuse(R::IdentityMismatch));
            }
        }
    }
    for binding in bindings.iter().filter(|b| owners.contains(&b.owner)) {
        if !policy.is_some_and(|p| p.rules().contains(&binding.rule)) {
            return Err(refuse(R::PolicyMismatch));
        }
    }
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
    let rows: Vec<UsageAttemptV1> = records(&tx, "SELECT record_json FROM usage_attempts_v1")?;
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
    let reserved: Vec<Reserved> = if active {
        records(&tx, "SELECT record_json FROM governance_reservations_v1")?
    } else {
        vec![]
    };
    if row.dispatched || row.finalized || reserved.iter().any(|r| r.reservation.attempt == *attempt)
    {
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
            let saved = reserved
                .iter()
                .find(|r| r.reservation.attempt.as_str() == previous.id);
            if !matches(&owner, previous, saved) {
                continue;
            }
            if previous.started_ms < coverage {
                return Err(refuse(R::IncompleteCoverage));
            }
            if let Some(saved) = saved {
                if rule.limits.terminal_tokens.is_some()
                    && saved.reservation.candidate == *candidate
                    && saved.reservation.scopes.iter().any(|s| s.owner == owner)
                    && saved.reservation.terminal_tokens != allowance
                {
                    return Err(refuse(R::PolicyMismatch));
                }
            }
            if !previous.dispatched {
                continue;
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
    tx.execute_batch("CREATE TABLE IF NOT EXISTS governance_scopes_v1(owner TEXT PRIMARY KEY, record_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS governance_reservations_v1(id TEXT PRIMARY KEY, record_json TEXT NOT NULL);").map_err(unavailable)?;
    for scope in &scopes {
        let rule = policy
            .rules()
            .iter()
            .find(|r| r.id == scope.rule_id)
            .ok_or_else(|| refuse(R::PolicyMismatch))?;
        tx.execute(
            "INSERT OR IGNORE INTO governance_scopes_v1 VALUES(?1, ?2)",
            params![
                encode(&scope.owner)?,
                encode(&Binding {
                    owner: scope.owner.clone(),
                    rule: rule.clone()
                })?
            ],
        )
        .map_err(unavailable)?;
    }
    for previous in &rows {
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
    Ok(BudgetAdmissionOutcomeV1::Admitted(
        BudgetPermitV1::from_committed_admission(reservation),
    ))
}
