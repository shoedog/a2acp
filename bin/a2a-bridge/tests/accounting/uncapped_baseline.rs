//! Black-box baseline from the independent G-min.3 binary at a75cd3f2.
//! Only declared fixture identities, roots and clock readings are canonicalized;
//! messages, output bytes, accounting values and artifact contents stay exact.
use super::{locator, Cli};
use bridge_core::{execution_policy::NodeTerminalV1, ids::*};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs};

fn projection(cli: &Cli, file_output: bool) -> Value {
    let mut command = cli.command();
    if file_output {
        command.args(["--out", "result.md"]);
    }
    let output = command.output().unwrap();
    let (execution, attempt) = locator(&output);
    ExecutionId::parse(&execution).unwrap();
    AttemptId::parse(&attempt).unwrap();
    let rows = cli.rows();
    assert_eq!(rows.len(), 1);
    let concrete = rows[0].id.clone();
    AttemptId::parse(&concrete).unwrap();
    let canonical_root = fs::canonicalize(cli.dir.path()).unwrap();
    let replace_identities = |text: &str| {
        text.replace(canonical_root.to_str().unwrap(), "$FIXTURE")
            .replace(cli.dir.path().to_str().unwrap(), "$FIXTURE")
            .replace(&execution, "exec-11111111111111111111111111111111")
            .replace(&attempt, "attempt-22222222222222222222222222222222")
            .replace(&concrete, "attempt-33333333333333333333333333333333")
    };
    let stdout = replace_identities(std::str::from_utf8(&output.stdout).unwrap());
    let mut stderr = String::new();
    let mut public_terminals = Vec::new();
    for line in std::str::from_utf8(&output.stderr)
        .unwrap()
        .split_inclusive('\n')
    {
        let mut line = replace_identities(line);
        if let Some(start) = line.find("coverage_start_ms=") {
            let start = start + "coverage_start_ms=".len();
            let end = line[start..].find(' ').unwrap() + start;
            line[start..end].parse::<i64>().unwrap();
            line.replace_range(start..end, "0");
        }
        if let Some((prefix, encoded)) = line.split_once(",terminal_json=") {
            let mut terminal =
                NodeTerminalV1::decode_canonical(encoded.strip_suffix("}\n").unwrap().as_bytes())
                    .unwrap();
            terminal.cleanup.duration_ms = 0;
            let encoded = String::from_utf8(terminal.encode_canonical().unwrap()).unwrap();
            public_terminals.push(serde_json::from_str::<Value>(&encoded).unwrap());
            line = format!("{prefix},terminal_json={encoded}}}\n");
        }
        stderr.push_str(&line);
    }
    let conn =
        rusqlite::Connection::open(cli.store.with_extension("sqlite.usage-v1.sqlite")).unwrap();
    let accounting_records: Vec<String> = rows
        .iter()
        .map(|row| {
            let raw: String = conn
                .query_row(
                    "SELECT record_json FROM usage_attempts_v1 WHERE id=?1",
                    [&row.id],
                    |r| r.get(0),
                )
                .unwrap();
            replace_identities(&raw)
                .replace(
                    &format!(r#""started_ms":{}"#, row.started_ms),
                    r#""started_ms":0"#,
                )
                .replace(&format!(r#""utc_day":{}"#, row.utc_day), r#""utc_day":0"#)
                .replace(&format!(r#""at_ms":{}"#, row.usage.at_ms), r#""at_ms":0"#)
        })
        .collect();
    let accounting: Vec<Value> = rows
        .into_iter()
        .map(|mut row| {
            row.started_ms = 0;
            row.utc_day = 0;
            row.usage.at_ms = 0;
            serde_json::from_str(&replace_identities(&serde_json::to_string(&row).unwrap()))
                .unwrap()
        })
        .collect();
    let artifacts = artifact_files(cli.dir.path(), cli.dir.path());
    // New governance records or artifacts in an uncapped run are observable regressions too.
    let governance_tables: u64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'governance_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    json!({"status":output.status.code(), "stdout":stdout, "stderr":stderr,
        "artifacts":artifacts, "accounting":accounting, "accounting_records":accounting_records, "public_failure_projection":public_terminals,
        "governance_tables":governance_tables})
}

fn artifact_files(root: &std::path::Path, directory: &std::path::Path) -> BTreeMap<String, String> {
    let mut artifacts = BTreeMap::new();
    for entry in fs::read_dir(directory).unwrap().map(Result::unwrap) {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
        if [
            "fixture-acp",
            "input.md",
            "a2a-bridge.toml",
            "prompts.jsonl",
        ]
        .contains(&relative)
            || relative.starts_with("tasks.sqlite")
        {
            continue;
        }
        if path.is_dir() {
            artifacts.extend(artifact_files(root, &path));
        } else {
            artifacts.insert(relative.into(), fs::read_to_string(path).unwrap());
        }
    }
    artifacts
}

#[test]
fn uncapped_cli_bytes_match_independent_success_and_failure_baselines() {
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/uncapped-gmin3.json")).unwrap();
    let actual = json!({
        "success":projection(&Cli::new(true, false, "good"), true),
        "failure":projection(&Cli::new(true, false, "missing"), true),
    });
    // Explicit alternate binaries support independent base controls; this never updates fixtures.
    if std::env::var_os("A2A_GMIN4_TEST_BINARY").is_some() {
        eprintln!("baseline_projection={actual}");
    }
    assert_eq!(actual, expected["projections"]);
    for (case, model) in [("success", "good"), ("failure", "missing")] {
        let mut stdout_expected = expected["projections"][case].clone();
        let terminal_bytes = stdout_expected["artifacts"]["result.md"]
            .as_str()
            .unwrap()
            .to_owned();
        let stdout = stdout_expected["stdout"].as_str().unwrap().to_owned() + &terminal_bytes;
        stdout_expected["stdout"] = json!(stdout);
        stdout_expected["stderr"] = json!(stdout_expected["stderr"]
            .as_str()
            .unwrap()
            .replace("[brief-lint] wrote artifact result.brief-lint.json\n", ""));
        stdout_expected["artifacts"] = json!({});
        assert_eq!(
            projection(&Cli::new(true, false, model), false),
            stdout_expected
        );
    }
    let mutated = Cli::new(true, false, "good");
    let adapter = mutated.dir.path().join("fixture-acp");
    fs::write(
        &adapter,
        fs::read_to_string(&adapter)
            .unwrap()
            .replace("answer", "answeR"),
    )
    .unwrap();
    super::text_file_busy::warm_up_script(&adapter);
    assert_ne!(
        projection(&mutated, true),
        expected["projections"]["success"]
    );
    // A drift in any required surface must fail the exact comparison, rather than be
    // swallowed by substring checks or clock/identity canonicalization.
    for case in ["success", "failure"] {
        for field in [
            "stdout",
            "stderr",
            "artifacts",
            "accounting",
            "accounting_records",
            "public_failure_projection",
        ] {
            let mut mutated = actual.clone();
            match field {
                "stdout" | "stderr" => {
                    mutated[case][field] =
                        json!(format!("{}\n", mutated[case][field].as_str().unwrap()))
                }
                "artifacts" => {
                    mutated[case][field]["result.md"] = json!(format!(
                        "{}\n",
                        mutated[case][field]["result.md"].as_str().unwrap()
                    ))
                }
                "accounting_records" => {
                    mutated[case][field][0] =
                        json!(format!("{} ", mutated[case][field][0].as_str().unwrap()));
                }
                "accounting" => {
                    mutated[case][field][0]["dispatched"] =
                        json!(!mutated[case][field][0]["dispatched"].as_bool().unwrap())
                }
                _ => mutated[case][field][0]["primary"] = json!("intentional_projection_mutation"),
            }
            assert_ne!(
                mutated, expected["projections"],
                "comparison missed {case}/{field}"
            );
        }
    }
}
