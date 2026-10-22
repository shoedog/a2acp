//! Offline presentation only: all accounting totals come from the typed ledger.
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{config, BoxError, CONFIG_PATH};
use bridge_core::{
    accounting::{UsageBucketV1, UsageComponentV1, UsageQueryV1, UsageSummaryV1},
    task_store::TaskStore,
};
use bridge_store::sqlite::{HistoryAllocationKind, SqliteStore};

pub const USAGE: &str = "\
usage: a2a-bridge usage [--config <path> | --store <absolute-path>]
    [--execution <id>] [--attempt <id>] [--node <id>] [--agent <id>] [--model <id>]
    [--workflow <id>] [--day <YYYY-MM-DD> | --from-day <YYYY-MM-DD> --through-day <YYYY-MM-DD>]
    [--group-by <agent|model|workflow|day>]... [--json]

Read the typed local usage ledger offline. Default grouping: agent, model, workflow, UTC day.
Client reporting requires --usage-config <path> or --usage-store <absolute-path>.";

type Selection = Result<Option<(PathBuf, Option<HistoryAllocationKind>)>, BoxError>;

/// Shared with local execution: relative stores belong to the supplied config directory.
/// In particular, a symlinked config does not move its relative store to the link target.
pub fn configured_store_path(cfg: &config::RegistryConfig, base: &Path) -> Option<PathBuf> {
    cfg.store.as_ref().map(|store| {
        let path = PathBuf::from(&store.path);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    })
}

fn select(config: Option<&Path>, store: Option<&Path>) -> Selection {
    if config.is_some() && store.is_some() {
        return Err("select exactly one reporting config or store".into());
    }
    if let Some(path) = store {
        if !path.is_absolute() {
            return Err("reporting store must be absolute".into());
        }
        return Ok(Some((path.to_owned(), None)));
    }
    let Some(path) = config else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(path).map_err(|_| "cannot read reporting config")?;
    let cfg = config::RegistryConfig::parse(&raw).map_err(|_| "invalid reporting config")?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if let Some(path) = configured_store_path(&cfg, base) {
        Ok(Some((path, Some(HistoryAllocationKind::Configured))))
    } else {
        let path = bridge_store::sqlite::platform_history_path()
            .map_err(|_| "cannot select platform store")?;
        Ok(Some((path, Some(HistoryAllocationKind::Platform))))
    }
}

fn open(path: &Path, kind: Option<HistoryAllocationKind>) -> Result<SqliteStore, BoxError> {
    match kind {
        Some(HistoryAllocationKind::Configured) => {
            SqliteStore::open_configured_history_read_only(path)
        }
        Some(HistoryAllocationKind::Platform) => SqliteStore::open_platform_history_read_only(path),
        None => SqliteStore::open_history_read_only(path),
    }
    .map_err(|_| "store missing, unrelated or incompatible".into())
}

/// Strip reporting flags before any request/positional parser sees them. A bad reporting
/// selection affects only stderr reporting, never the original request or its result.
pub fn client_args(args: &[String]) -> (Vec<String>, Selection) {
    let mut rest = Vec::new();
    let (mut config, mut store) = (None, None);
    let mut invalid = false;
    let mut i = 0;
    while i < args.len() {
        let slot = match args[i].as_str() {
            "--usage-config" => Some(&mut config),
            "--usage-store" => Some(&mut store),
            _ => None,
        };
        if let Some(slot) = slot {
            i += 1;
            if let Some(value) = args.get(i).filter(|v| !v.starts_with("--")) {
                invalid |= slot.replace(PathBuf::from(value)).is_some();
            } else {
                invalid = true;
                continue;
            }
        } else {
            rest.push(args[i].clone());
        }
        i += 1;
    }
    let selection = if invalid {
        Err("invalid reporting store selection".into())
    } else {
        // Served clients must resolve relative stores from the same config as serve.
        let config = config
            .map(|path| std::fs::canonicalize(path).map_err(|_| "cannot resolve reporting config"));
        match config.transpose() {
            Ok(config) => select(config.as_deref(), store.as_deref()),
            Err(error) => Err(error.into()),
        }
    };
    (rest, selection)
}

fn quoted<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("usage presentation is JSON serializable")
}
fn component(c: &UsageComponentV1) -> String {
    format!(
        "{{reported_sum:{},missing_count:{},partial:{}}}",
        c.reported_sum, c.missing_count, c.partial
    )
}
fn bucket(b: &UsageBucketV1) -> String {
    format!(
        "agent={} model={} workflow={} utc_day={} billing={} attempts={} dispatched={} pending={} unmapped={} total_tokens={} input_tokens={} output_tokens={} thought_tokens={} cached_read_tokens={} cached_write_tokens={} costs={} cost_known={} cost_missing={} cost_invalid={} cost_partial={}",
        quoted(&b.keys.agent), quoted(&b.keys.effective_model), quoted(&b.keys.workflow), quoted(&b.keys.utc_day),
        b.keys.billing.as_ref().map_or_else(|| "unmapped".into(), quoted),
        b.attempted, b.dispatched, b.pending, b.unmapped,
        component(&b.total_tokens), component(&b.input_tokens), component(&b.output_tokens),
        component(&b.thought_tokens), component(&b.cached_read_tokens), component(&b.cached_write_tokens),
        quoted(&b.cost_by_currency), b.cost_known, b.cost_missing, b.cost_invalid, b.cost_partial
    )
}

async fn selected_line(selection: &Selection, execution: &str) -> Result<String, BoxError> {
    let (path, kind) = selection
        .as_ref()
        .map_err(|e| e.to_string())?
        .as_ref()
        .ok_or("no reporting store selected")?;
    let store = open(path, *kind)?;
    let summary = store
        .usage_summary_v1(&UsageQueryV1 {
            execution: Some(execution.to_owned()),
            group_by_agent: true,
            group_by_model: true,
            ..Default::default()
        })
        .await
        .map_err(|_| "usage ledger incompatible or unreadable")?;
    line(&summary, &format!("execution={}", quoted(&execution)), true)
}
fn line(summary: &UsageSummaryV1, target: &str, require_rows: bool) -> Result<String, BoxError> {
    if require_rows && summary.buckets.is_empty() {
        return Err("no matching usage rows".into());
    }
    Ok(format!(
        "usage {target} attempts={} coverage_start_ms={} historical_incomplete={} groups=[{}]",
        summary.buckets.iter().map(|b| b.attempted).sum::<u64>(),
        summary.coverage_start_ms,
        summary.historical_incomplete,
        summary
            .buckets
            .iter()
            .map(bucket)
            .collect::<Vec<_>>()
            .join("; ")
    ))
}
fn print_line(result: Result<String, BoxError>, writer: &mut impl Write) {
    match result {
        Ok(line) => {
            let _ = writeln!(writer, "{line}");
        }
        // Errors here are closed, static reasons. Keep exactly one bounded line.
        Err(error) => {
            let _ = writeln!(writer, "usage unavailable: {error}");
        }
    }
}
pub async fn print_selected(selection: &Selection, execution: &str) {
    print_line(selected_line(selection, execution).await, &mut io::stderr());
}
pub async fn print_local_node(store: &dyn TaskStore, execution: &str, node: &str) {
    print_line(
        local_node_line(store, execution, node).await,
        &mut io::stderr(),
    );
}
async fn local_node_line(
    store: &dyn TaskStore,
    execution: &str,
    node: &str,
) -> Result<String, BoxError> {
    let summary = store
        .usage_summary_v1(&UsageQueryV1 {
            execution: Some(execution.to_owned()),
            node: Some(node.to_owned()),
            group_by_agent: true,
            group_by_model: true,
            ..Default::default()
        })
        .await;
    match summary {
        Ok(summary) => line(&summary, &format!("node={}", quoted(&node)), true),
        Err(_) => Err("usage ledger unreadable".into()),
    }
}

/// Gregorian civil date to Unix UTC day; reject invalid dates instead of normalizing them.
fn day(raw: &str) -> Result<i64, BoxError> {
    let invalid = || -> BoxError { "usage: invalid day (expected YYYY-MM-DD)".into() };
    let bytes = raw.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return Err(invalid());
    }
    let y: i64 = raw[..4].parse().map_err(|_| invalid())?;
    let m: i64 = raw[5..7].parse().map_err(|_| invalid())?;
    let d: i64 = raw[8..].parse().map_err(|_| invalid())?;
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let limit = match m {
        2 => 28 + i64::from(leap),
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return Err(invalid()),
    };
    if d < 1 || d > limit {
        return Err(invalid());
    }
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    Ok(era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + d - 1 - 719468)
}

#[derive(serde::Serialize)]
struct Report {
    #[serde(flatten)]
    summary: UsageSummaryV1,
    empty_state: Option<&'static str>,
}
fn empty_state(s: &UsageSummaryV1, q: &UsageQueryV1) -> Option<&'static str> {
    if !s.buckets.is_empty() {
        return None;
    }
    // Zero is the read-only store sentinel for an absent accounting sidecar.
    if s.coverage_start_ms == 0 && s.historical_incomplete {
        return Some("historical-incomplete");
    }
    if q.utc_day_through
        .is_some_and(|d| (d + 1) * 86_400_000 <= s.coverage_start_ms)
    {
        Some("before-coverage")
    } else if !s.historical_incomplete
        || q.utc_day_from
            .is_some_and(|d| d * 86_400_000 >= s.coverage_start_ms)
    {
        Some("covered-empty")
    } else {
        Some("historical-incomplete")
    }
}

pub async fn usage_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let (mut config, mut store) = (None, None);
    let mut q = UsageQueryV1::default();
    let (mut json, mut grouped, mut exact_day) = (false, false, false);
    let mut seen = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag != "--group-by" && !seen.insert(flag) {
            return Err("usage: duplicate flag".into());
        }
        if flag == "--json" {
            json = true;
            i += 1;
            continue;
        }
        i += 1;
        let value = args
            .get(i)
            .filter(|v| !v.starts_with("--"))
            .ok_or("usage: flag requires a value")?;
        match flag {
            "--config" => config = Some(PathBuf::from(value)),
            "--store" => store = Some(PathBuf::from(value)),
            "--execution" => q.execution = Some(value.clone()),
            "--attempt" => q.run_attempt = Some(value.clone()),
            "--node" => q.node = Some(value.clone()),
            "--agent" => q.agent = Some(value.clone()),
            "--model" => q.effective_model = Some(value.clone()),
            "--workflow" => q.workflow = Some(value.clone()),
            "--day" => {
                exact_day = true;
                q.utc_day_from = Some(day(value)?);
                q.utc_day_through = q.utc_day_from;
            }
            "--from-day" => q.utc_day_from = Some(day(value)?),
            "--through-day" => q.utc_day_through = Some(day(value)?),
            "--group-by" => {
                grouped = true;
                match value.as_str() {
                    "agent" => q.group_by_agent = true,
                    "model" => q.group_by_model = true,
                    "workflow" => q.group_by_workflow = true,
                    "day" => q.group_by_utc_day = true,
                    _ => return Err("usage: invalid grouping".into()),
                }
            }
            _ => return Err(format!("usage: unknown argument {flag:?}").into()),
        }
        i += 1;
    }
    if (exact_day && (seen.contains("--from-day") || seen.contains("--through-day")))
        || matches!((q.utc_day_from, q.utc_day_through), (Some(a), Some(b)) if a > b)
    {
        return Err("usage: incompatible day range".into());
    }
    if !grouped {
        q.group_by_agent = true;
        q.group_by_model = true;
        q.group_by_workflow = true;
        q.group_by_utc_day = true;
    }
    if config.is_none() && store.is_none() {
        config = Some(PathBuf::from(CONFIG_PATH));
    }
    let result = async {
        let (path, kind) =
            select(config.as_deref(), store.as_deref())?.ok_or("no reporting store selected")?;
        let summary = open(&path, kind)?
            .usage_summary_v1(&q)
            .await
            .map_err(|_| "usage ledger incompatible or unreadable")?;
        Ok::<_, BoxError>(Report {
            empty_state: empty_state(&summary, &q),
            summary,
        })
    }
    .await
    .map_err(|e| format!("usage unavailable: {e}"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(state) = result.empty_state {
        println!(
            "usage: {state} coverage_start_ms={} historical_incomplete={}",
            result.summary.coverage_start_ms, result.summary.historical_incomplete
        );
    } else {
        println!("{}", line(&result.summary, "", false)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::{accounting::*, orch::*};

    #[tokio::test]
    async fn node_line_sums_usage_across_attempts() {
        let store = SqliteStore::open_in_memory().unwrap();
        let owner = bridge_core::ids::AttemptIdentity::initial().unwrap();
        for tokens in [3, 5] {
            let row = UsageAttemptV1 {
                id: bridge_core::ids::AttemptId::mint().unwrap().as_str().into(),
                attribution: UsageAttributionV1 {
                    execution: Some(owner.execution_id.as_str().into()),
                    run_attempt: Some(owner.attempt_id.as_str().into()),
                    node: Some("draft".into()),
                    ..Default::default()
                },
                started_ms: 1,
                utc_day: 0,
                dispatched: false,
                finalized: false,
                usage: Default::default(),
                cost_invalid: false,
            };
            store.start_usage_attempt_v1(&row).await.unwrap();
            let usage = UsageSnapshot {
                terminal: Some(TerminalUsage {
                    total_tokens: tokens,
                    input_tokens: tokens,
                    output_tokens: 0,
                    thought_tokens: None,
                    cached_read_tokens: None,
                    cached_write_tokens: None,
                }),
                ..Default::default()
            };
            store
                .update_usage_attempt_v1(&row.id, true, Some(usage), true)
                .await
                .unwrap();
        }
        let line = local_node_line(&store, owner.execution_id.as_str(), "draft")
            .await
            .unwrap();
        assert!(line.contains("node=\"draft\" attempts=2"), "{line}");
        assert!(
            line.contains("total_tokens={reported_sum:8,missing_count:0,partial:false}"),
            "{line}"
        );
    }
}
