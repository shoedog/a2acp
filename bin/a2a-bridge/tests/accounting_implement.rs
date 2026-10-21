//! Fixture-only production CLI accounting, independent of optional telemetry.
#![cfg(unix)]
#[path = "accounting/implement_support.rs"]
mod support;
use support::*;

#[test]
fn fresh_edit_review_fix_approval_is_durable() {
    let f = Fixture::new(3);
    let out = f.run(false, "");
    success(&out);
    assert_eq!(
        f.prompts().len(),
        4,
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let rows = f.rows();
    assert_eq!(
        rows.len(),
        4,
        "every production prompt needs one durable row"
    );
    assert!(rows.iter().all(|r| r.dispatched && r.finalized));
    assert_eq!(f.setups().iter().filter(|s| s["writer"] == true).count(), 1);
    for (row, prompt) in rows.iter().zip(f.prompts()) {
        assert_eq!(row.attribution.effective_effort.as_deref(), Some("high"));
        assert_eq!(row.attribution.effective_mode.as_deref(), Some("write"));
        assert_eq!(prompt["config"]["effort"], "high");
        assert_eq!(prompt["config"]["mode"], "write");
    }
    let owner = bridge_controller::implement_resume::load_accounting_owner(&f.clone_path())
        .unwrap()
        .unwrap();
    for row in &rows {
        assert_eq!(
            row.attribution.execution.as_deref(),
            Some(owner.execution_id.as_str())
        );
        assert_eq!(
            row.attribution.run_attempt.as_deref(),
            Some(owner.attempt_id.as_str())
        );
        assert_eq!(row.attribution.effective_model.as_deref(), Some("good"));
        assert_eq!(row.usage.terminal.as_ref().unwrap().total_tokens, 15);
    }
    assert_eq!(rows[0].attribution.attempt_kind, "implement_edit");
    assert_eq!(rows[2].attribution.attempt_kind, "implement_fix");
    for i in [1, 3] {
        assert_eq!(rows[i].attribution.agent, "reviewer");
        assert_eq!(
            rows[i].attribution.workflow.as_deref(),
            Some("implement-review")
        );
        assert_eq!(rows[i].attribution.node.as_deref(), Some("review"));
    }
    assert!(String::from_utf8_lossy(&out.stdout).contains("Approve"));
}

#[test]
fn resume_and_legacy_admit_before_subsequent_prompts() {
    for legacy in [false, true] {
        let f = Fixture::new(1);
        success(&f.run(false, ""));
        let previous = f.rows();
        assert_eq!(previous.len(), 2);
        let clone = f.clone_path();
        let old = bridge_controller::implement_resume::load_accounting_owner(&clone)
            .unwrap()
            .unwrap();
        let path = bridge_controller::implement_resume::checkpoint_path(&clone);
        let mut ck: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // Reopen the committed revision as a stranded in-loop checkpoint.
        ck["phase"] = "InLoop".into();
        ck["loop_max_attempts"] = 3.into();
        if legacy {
            ck.as_object_mut().unwrap().remove("accounting_owner");
        }
        std::fs::write(path, serde_json::to_vec(&ck).unwrap()).unwrap();
        std::fs::write(f.dir.path().join("reviews"), "0").unwrap();
        let lookups = f.dir.path().join("subject-lookups");
        let _ = std::fs::remove_file(&lookups);
        success(&f.run(true, ""));
        let observations = std::fs::read_to_string(lookups).unwrap();
        let state: serde_json::Value =
            serde_json::from_str(observations.lines().next().unwrap()).unwrap();
        assert!(
            state.is_null() || state["paused"] == true,
            "subject lookup raced a writer: {state}"
        );
        let rows = f.rows();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[..2], previous);
        let owner = bridge_controller::implement_resume::load_accounting_owner(&clone)
            .unwrap()
            .unwrap();
        assert_ne!(owner.attempt_id, old.attempt_id);
        assert_eq!(owner.execution_id == old.execution_id, !legacy);
        for row in &rows[2..] {
            assert_eq!(
                row.attribution.execution.as_deref(),
                Some(owner.execution_id.as_str())
            );
            assert_eq!(
                row.attribution.run_attempt.as_deref(),
                Some(owner.attempt_id.as_str())
            );
            assert!(row.dispatched && row.finalized);
        }
        // Resume prepares the first fix before the review decides whether it is needed.
        assert_eq!(rows[2].attribution.attempt_kind, "implement_fix");
    }
}

#[test]
fn setup_and_writer_admission_fail_before_prompts() {
    for setup in [false, true] {
        let f = Fixture::new(3);
        if setup {
            let creds = f.dir.path().join("expired.json");
            std::fs::write(&creds, r#"{"claudeAiOauth":{"expiresAt":1}}"#).unwrap();
            let config = std::fs::read_to_string(&f.config)
                .unwrap()
                .replace("cmd = \"fixture\"", "cmd = \"claude-agent-acp\"")
                .replace(
                    "access = \"rw\"",
                    &format!(
                        "access = \"rw\"\nvolumes = [{:?}]",
                        format!("{}:/root/.claude/.credentials.json", creds.display())
                    ),
                );
            std::fs::write(&f.config, config).unwrap();
        } else {
            std::fs::create_dir(f.dir.path().join("history.sqlite.usage-v1.sqlite")).unwrap();
        }
        let out = f.run(false, "");
        assert!(!out.status.success());
        assert!(f.prompts().is_empty());
        if setup {
            let rows = f.rows();
            assert_eq!(rows.len(), 1);
            assert!(!rows[0].dispatched && rows[0].finalized);
        }
    }
    let f = Fixture::new(1);
    success(&f.run(false, ""));
    let clone = f.clone_path();
    let path = bridge_controller::implement_resume::checkpoint_path(&clone);
    let mut ck: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    ck["phase"] = "InLoop".into();
    ck["accounting_owner"] = "invalid".into();
    std::fs::write(&path, serde_json::to_vec(&ck).unwrap()).unwrap();
    let before = f.rows();
    assert!(!f.run(true, "").status.success());
    assert_eq!(f.prompts().len(), 2);
    assert_eq!(f.rows(), before);
}

#[test]
fn lazy_acp_setup_rejection_keeps_zero_usage_pending_and_publishes_abort() {
    let f = Fixture::new(3);
    let config = std::fs::read_to_string(&f.config).unwrap().replacen(
        "model = \"good\"",
        "model = \"unavailable\"",
        1,
    );
    std::fs::write(&f.config, config).unwrap();
    let out = f.run(false, "");
    assert!(f.prompts().is_empty());
    assert!(!f.dir.path().join("prompt-starts").exists());
    let rows = f.rows();
    assert_eq!(rows.len(), 1, "{}", String::from_utf8_lossy(&out.stderr));
    // The existing capture contract marks dispatch before entering lazy setup.
    // No Done reached capture, so the unchanged schema retains a pending zero-usage row.
    assert!(rows[0].dispatched && !rows[0].finalized, "{:?}", rows[0]);
    assert_eq!(rows[0].usage, Default::default());
    let ck = bridge_controller::implement_resume::load_checkpoint(&f.clone_path()).unwrap();
    assert_eq!(
        ck.phase,
        bridge_controller::implement_resume::ImplementPhase::Abort
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("prompt_acceptance: Excluded"));
    assert!(f.dir.path().join("evidence").exists());
}

#[test]
fn resumable_ownership_write_failure_refuses_before_provider_preparation() {
    let f = Fixture::new(1);
    success(&f.run(false, ""));
    f.strand();
    let clone = f.clone_path();
    let path = bridge_controller::implement_resume::checkpoint_path(&clone);
    let owner = bridge_controller::implement_resume::load_accounting_owner(&clone).unwrap();
    let before = f.rows();
    let setups = f.setups();
    std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
    let out = f.run(true, "");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("checkpoint create"));
    assert_eq!(f.setups(), setups);
    assert_eq!(f.prompts().len(), 2);
    assert_eq!(f.rows(), before);
    assert_eq!(
        bridge_controller::implement_resume::load_accounting_owner(&clone).unwrap(),
        owner
    );
}

#[test]
fn resumed_approval_settles_the_unused_fix_preparation() {
    let f = Fixture::new(1);
    success(&f.run(false, ""));
    f.strand();
    let before = f.rows();
    success(&f.run(true, ""));
    let rows = f.rows();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[..2], before);
    assert_eq!(f.prompts().len(), 3);
    assert_eq!(rows[2].attribution.agent, "writer");
    assert_eq!(rows[2].attribution.attempt_kind, "implement_fix");
    assert!(!rows[2].dispatched && rows[2].finalized);
    assert_eq!(rows[3].attribution.agent, "reviewer");
    assert!(rows[3].dispatched && rows[3].finalized);
    // Lazy setup: approval never needs the prepared resume writer.
    assert_eq!(f.setups().iter().filter(|s| s["writer"] == true).count(), 1);
}

#[test]
fn registry_refusal_precedes_container_run_and_preparation() {
    for invalid_default in [false, true] {
        let f = Fixture::new(3);
        let config = std::fs::read_to_string(&f.config).unwrap();
        let config = if invalid_default {
            config.replace("default = \"writer\"", "default = \"missing\"")
        } else {
            config.replace(
                &format!("allowed_cmds = [{:?}]", f.dir.path().join("runtime")),
                "allowed_cmds = [\"excluded-runtime\"]",
            )
        };
        std::fs::write(&f.config, config).unwrap();
        let out = f.run(false, "");
        assert!(!out.status.success(), "registry must refuse");
        assert!(!f.dir.path().join("runs").exists());
        assert!(f.prompts().is_empty());
        assert!(f.rows().is_empty());
        let ck = bridge_controller::implement_resume::load_checkpoint(&f.clone_path()).unwrap();
        assert_eq!(
            ck.phase,
            bridge_controller::implement_resume::ImplementPhase::Abort
        );
        assert!(f.dir.path().join("evidence").exists());
    }
}

#[test]
fn production_usage_write_failure_keeps_typed_persistence_and_pending_charge() {
    let f = Fixture::new(3);
    let out = f.run(false, "usage_write_fail");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("persistence: Some(StoreFailure)"),
        "{stderr}"
    );
    assert!(!stderr.contains("host custody unavailable"), "{stderr}");
    assert_eq!(f.prompts().len(), 1);
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].dispatched && !rows[0].finalized);
    let ck = bridge_controller::implement_resume::load_checkpoint(&f.clone_path()).unwrap();
    assert_eq!(
        ck.phase,
        bridge_controller::implement_resume::ImplementPhase::Abort
    );
    assert!(f.dir.path().join("evidence").exists());
}
