//! Standalone production CLI capture with no metrics, traces, or live provider.
#![cfg(unix)]
use bridge_core::{accounting::*, task_store::TaskStore};
use bridge_store::sqlite::SqliteStore;
use serde_json::Value;
use std::fs;
#[path = "accounting/smoke_support.rs"]
mod smoke_support;
mod text_file_busy;
use smoke_support::SmokeCli;

fn artifact(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "invalid artifact: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[tokio::test]
async fn standalone_smoke_reopens_exact_usage_under_artifact_owner() {
    for configured in [true, false] {
        let cli = SmokeCli::new(configured, false);
        let output = cli
            .command()
            .arg("--acknowledge-billable")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let payload = artifact(&output);
        assert_eq!(payload["success"], true);
        assert_eq!(payload["schema_version"], 2);
        assert_eq!(payload["turn"]["prompt_calls"], 1);
        let prompts = cli.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0]["params"]["prompt"][0]["text"],
            "Reply exactly PONG. Do not use tools."
        );
        let rows = cli.rows();
        for row in &rows {
            assert_eq!(row.attribution.billing.is_some(), configured);
            if let Some(billing) = &row.attribution.billing {
                assert_eq!(billing.account_group, "fixture");
            }
        }
        assert_eq!(
            rows.len(),
            1,
            "every standalone prompt needs durable accounting"
        );
        let row = &rows[0];
        let execution = payload["attempt"]["execution_id"].as_str().unwrap();
        let attempt = payload["attempt"]["attempt_id"].as_str().unwrap();
        assert_eq!(row.attribution.execution.as_deref(), Some(execution));
        assert_eq!(row.attribution.run_attempt.as_deref(), Some(attempt));
        assert_ne!(row.id, attempt);
        assert_eq!(row.attribution.turn.as_ref(), Some(&row.id));
        assert_eq!(row.attribution.agent, "agent");
        assert_eq!(row.attribution.effective_model.as_deref(), Some("good"));
        assert_eq!(row.attribution.attempt_kind, "smoke");
        assert!(row.dispatched && row.finalized);
        let reopened = SqliteStore::open_history_read_only(&cli.store).unwrap();
        let summary = reopened
            .usage_summary_v1(&UsageQueryV1 {
                execution: Some(execution.into()),
                run_attempt: Some(attempt.into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(summary.buckets.len(), 1);
        let b = &summary.buckets[0];
        assert_eq!((b.attempted, b.dispatched, b.pending), (1, 1, 0));
        assert_eq!(
            (
                b.total_tokens.reported_sum,
                b.input_tokens.reported_sum,
                b.output_tokens.reported_sum
            ),
            (15, 10, 5)
        );
        assert_eq!(
            (
                b.thought_tokens.reported_sum,
                b.cached_read_tokens.reported_sum,
                b.cached_write_tokens.reported_sum
            ),
            (2, 3, 4)
        );
        assert_eq!(b.cost_by_currency["USD"], 0.25);
        assert_eq!(row.usage.used, Some(99));
        assert_eq!(row.usage.size, Some(1000));
        assert_eq!(row.usage.terminal.as_ref().unwrap().thought_tokens, Some(2));
        assert_eq!(
            row.usage.terminal.as_ref().unwrap().cached_read_tokens,
            Some(3)
        );
        assert_eq!(
            row.usage.terminal.as_ref().unwrap().cached_write_tokens,
            Some(4)
        );
        assert_eq!(row.usage.terminal.as_ref().unwrap().total_tokens, 15);
        assert_eq!(row.usage.cost.as_ref().unwrap().amount, 0.25);
        assert!(!payload.to_string().contains("attribution"));
        assert_eq!(
            cli.platform.join("workflow-history.sqlite").exists(),
            !configured
        );
        assert_eq!(cli.dir.path().join("tasks.sqlite").exists(), configured);
    }
}

#[test]
fn missing_acknowledgement_never_attempts_or_charges() {
    let cli = SmokeCli::new(true, false);
    let output = cli.command().output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(cli.prompts().is_empty());
    assert!(cli.rows().is_empty());
    assert!(!cli.store.exists());
}

#[test]
fn admitted_resolution_failure_finalizes_without_dispatch() {
    let cli = SmokeCli::new(true, false);
    fs::remove_file(cli.dir.path().join("fixture-acp")).unwrap();
    let output = cli
        .command()
        .arg("--acknowledge-billable")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(cli.prompts().is_empty());
    let payload = artifact(&output);
    assert_eq!(payload["turn"]["prompt_calls"], 0);
    let rows = cli.rows();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].finalized && !rows[0].dispatched);
}

#[tokio::test]
async fn interrupted_usage_remains_pending_after_reopen() {
    let cli = SmokeCli::new(true, true);
    let output = cli
        .command()
        .arg("--acknowledge-billable")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(cli.prompts().len(), 1);
    let rows = cli.rows();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].dispatched && !rows[0].finalized);
    let reopened = SqliteStore::open_history_read_only(&cli.store).unwrap();
    let summary = reopened
        .usage_summary_v1(&Default::default())
        .await
        .unwrap();
    let b = &summary.buckets[0];
    assert_eq!(b.pending, 1);
    assert!(b.cost_partial);
    assert_eq!(rows[0].usage.cost.as_ref().unwrap().amount, 0.25);
}

#[tokio::test]
async fn accounting_write_refusals_never_poll_inner_prompt() {
    for dispatch in [false, true] {
        let cli = SmokeCli::new(true, false);
        let store = SqliteStore::open_shared_history(&cli.store).unwrap();
        store.usage_summary_v1(&Default::default()).await.unwrap();
        drop(store);
        let conn =
            rusqlite::Connection::open(cli.store.with_extension("sqlite.usage-v1.sqlite")).unwrap();
        conn.execute_batch(if dispatch {
            "CREATE TRIGGER refuse_usage BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'dispatch refused'); END;"
        } else {
            "CREATE TRIGGER refuse_usage BEFORE INSERT ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'start refused'); END;"
        }).unwrap();
        drop(conn);
        let output = cli
            .command()
            .arg("--acknowledge-billable")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let payload = artifact(&output);
        assert_eq!(payload["turn"]["prompt_calls"], 0);
        assert_eq!(payload["attempt"]["prompt_may_have_been_accepted"], false);
        assert!(cli.prompts().is_empty());
        let rows = cli.rows();
        assert_eq!(rows.len(), usize::from(dispatch));
        if dispatch {
            assert!(!rows[0].dispatched && !rows[0].finalized);
            assert_eq!(rows[0].usage, Default::default());
        }
    }
}

#[tokio::test]
async fn admitted_configuration_refusals_reopen_one_undispatched_attempt() {
    for refusal in ["cwd", "snapshot", "unknown-agent"] {
        let cli = SmokeCli::new(true, false);
        let raw = fs::read_to_string(&cli.config).unwrap();
        let mut command = cli.command();
        let expected = match refusal {
            "cwd" => {
                command
                    .arg("--session-cwd")
                    .arg(cli.dir.path().join("missing"));
                "smoke.session_cwd"
            }
            "snapshot" => {
                fs::write(
                    &cli.config,
                    format!("watchdog = {{ silence_secs = 1 }}\n{raw}"),
                )
                .unwrap();
                "smoke.config_load"
            }
            "unknown-agent" => {
                fs::write(&cli.config, raw.replace("\"agent\"", "\"other\"")).unwrap();
                "smoke.unknown_agent"
            }
            _ => unreachable!(),
        };
        let output = command.arg("--acknowledge-billable").output().unwrap();
        assert!(!output.status.success(), "{refusal}");
        let payload = artifact(&output);
        assert_eq!(payload["diagnostics"]["failure"]["code"], expected);
        assert_eq!(payload["turn"]["prompt_calls"], 0);
        assert_eq!(payload["attempt"]["prompt_may_have_been_accepted"], false);
        assert!(cli.prompts().is_empty());
        let rows = cli.rows();
        assert_eq!(
            rows.len(),
            1,
            "{refusal}: admitted refusals need accounting"
        );
        assert!(rows[0].finalized && !rows[0].dispatched);
        let reopened = SqliteStore::open_history_read_only(&cli.store).unwrap();
        let summary = reopened
            .usage_summary_v1(&UsageQueryV1 {
                execution: Some(payload["attempt"]["execution_id"].as_str().unwrap().into()),
                run_attempt: Some(payload["attempt"]["attempt_id"].as_str().unwrap().into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(summary.buckets.len(), 1);
        let b = &summary.buckets[0];
        assert_eq!((b.attempted, b.dispatched, b.pending), (1, 0, 0));
    }
}
