//! Context fork over a served `message/send`, against the real `a2a-bridge serve` binary and an offline
//! ACP adapter (`fixtures/fork-acp`) that speaks the real `session/fork` JSON-RPC shape. No provider
//! credentials or live models.
#![cfg(unix)]
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Served {
    dir: tempfile::TempDir,
    child: Child,
    url: String,
    peer: std::net::TcpListener,
}

impl Served {
    /// `agent` and `other` advertise session/fork; `plain` is the same adapter without the capability.
    fn start() -> Self {
        Self::start_with("")
    }

    /// [`Self::start`] with `extra` appended to the config (for example governance rules).
    fn start_with(extra: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let adapter = dir.path().join("fork-acp");
        fs::write(&adapter, include_str!("fixtures/fork-acp")).unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let peer = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        peer.set_nonblocking(true).unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let config = format!(
            "default='agent'\nallowed_cwd_root={root:?}\n[store]\npath='tasks.sqlite'\n\
             [delegation]\npeer_url='http://{peer_addr}'\nauth='fixture'\n\
             [[agents]]\nid='agent'\ncmd={adapter:?}\nargs=['wire','fork']\nmodel='good'\npre_authenticated=true\n\
             [[agents]]\nid='plain'\ncmd={adapter:?}\nargs=['wire-plain','nofork']\nmodel='good'\npre_authenticated=true\n\
             [[agents]]\nid='other'\ncmd={adapter:?}\nargs=['wire-other','fork']\nmodel='good'\npre_authenticated=true\n\
             [[billing_identities]]\nagent='agent'\nmodel='good'\naccount_group='fixture'\nbilling_mode='metered'\n\
             [[billing_identities]]\nagent='agent'\nmodel='other'\naccount_group='fixture'\nbilling_mode='metered'\n\
             [[billing_identities]]\nagent='other'\nmodel='good'\naccount_group='fixture'\nbilling_mode='metered'\n\
             [[workflows]]\nid='flow'\n[[workflows.nodes]]\nid='review'\nagent='agent'\nprompt_text='{{{{input}}}}'\ninputs=[]\n\
             [server]\naddr='{addr}'\n{extra}",
            root = dir.path()
        );
        fs::write(dir.path().join("config.toml"), config).unwrap();
        let stderr = fs::File::create(dir.path().join("stderr")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(["serve", "--config", "config.toml"])
            .current_dir(dir.path())
            .env_remove("A2A_BRIDGE_MCP_CALL_DEPTH")
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap();
        let mut served = Self {
            dir,
            child,
            url: format!("http://{addr}"),
            peer,
        };
        for _ in 0..400 {
            assert!(
                served.child.try_wait().unwrap().is_none(),
                "startup failed: {}",
                served.stderr()
            );
            if std::net::TcpStream::connect(addr).is_ok() {
                return served;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("serve startup timed out: {}", served.stderr());
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.dir.path().join("stderr")).unwrap_or_default()
    }

    fn wire(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.dir.path().join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The effects of one kind, in order: `forks` is every `fork ...` line of the `agent` adapter.
    fn effects(&self, kind: &str) -> Vec<String> {
        self.wire("wire")
            .into_iter()
            .filter(|line| line.split(' ').next() == Some(kind))
            .collect()
    }

    /// The agent session each prompt reached, in order.
    fn prompted(&self) -> Vec<String> {
        self.effects("prompt")
            .into_iter()
            .map(|line| line.trim_start_matches("prompt ").to_owned())
            .collect()
    }

    async fn call(&self, method: &str, params: Value) -> Value {
        reqwest::Client::new()
            .post(&self.url)
            .header("A2A-Version", "1.0")
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    fn message(n: u32, context: Option<&str>, metadata: Value) -> Value {
        let mut md = json!({
            "a2a-bridge.execution_id": format!("exec-{n:032}"),
            "a2a-bridge.attempt_id": format!("attempt-{n:032}"),
        });
        for (key, value) in metadata.as_object().unwrap() {
            md[key] = value.clone();
        }
        let mut message = json!({"text": "go", "taskId": format!("exec-{n:032}"), "metadata": md});
        if let Some(context) = context {
            message["contextId"] = json!(context);
        }
        json!({ "message": message })
    }

    /// One unary turn. `n` makes the request's execution identity unique.
    async fn send(&self, n: u32, context: Option<&str>, metadata: Value) -> Value {
        self.call("SendMessage", Self::message(n, context, metadata))
            .await
    }

    /// One streaming turn: the raw SSE body.
    async fn send_stream(&self, n: u32, context: Option<&str>, metadata: Value) -> String {
        reqwest::Client::new()
            .post(&self.url)
            .header("A2A-Version", "1.0")
            .json(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "SendStreamingMessage",
                          "params": Self::message(n, context, metadata)}),
            )
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    fn rows(&self) -> Vec<Value> {
        let conn = rusqlite::Connection::open(self.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
            .unwrap();
        let mut query = conn
            .prepare("SELECT record_json FROM usage_attempts_v1 ORDER BY rowid")
            .unwrap();
        query
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(args)
            .current_dir(self.dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn peer_untouched(&self) -> bool {
        matches!(self.peer.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text(reply: &Value) -> &str {
    reply["result"]["artifact"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no answer in {reply}"))
}

fn reason(reply: &Value) -> &str {
    reply["error"]["data"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("no refusal reason in {reply}"))
}

#[tokio::test]
async fn fork_branches_an_idle_context_and_every_turn_reaches_its_own_session() {
    let s = Served::start();
    assert_eq!(
        text(&s.send(1, Some("src"), json!({})).await),
        "answer from mint-0"
    );
    let minted = s.effects("new").len();

    // A fork that names its new context: exactly one session/fork, of the source's agent session.
    let a = s
        .send(2, Some("fork-a"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(text(&a), "answer from fork-0");
    assert_eq!(a["result"]["task"]["contextId"], "fork-a");
    assert_eq!(s.effects("fork").len(), 1);
    assert!(
        s.effects("fork")[0].starts_with("fork mint-0 fork-0 /"),
        "{:?}",
        s.effects("fork")
    );

    // A fork that omits the context id gets one, returned as usual.
    let b = s
        .send(3, None, json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(text(&b), "answer from fork-1");
    let allocated = b["result"]["task"]["contextId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!allocated.is_empty() && allocated != "src" && allocated != "fork-a");

    // The source, fork A and the allocated fork continue independently.
    assert_eq!(
        text(&s.send(4, Some("src"), json!({})).await),
        "answer from mint-0"
    );
    assert_eq!(
        text(&s.send(5, Some("fork-a"), json!({})).await),
        "answer from fork-0"
    );
    assert_eq!(
        text(&s.send(6, Some(&allocated), json!({})).await),
        "answer from fork-1"
    );
    assert_eq!(
        s.prompted(),
        ["mint-0", "fork-0", "fork-1", "mint-0", "fork-0", "fork-1"]
    );
    assert_eq!(
        s.effects("fork").len(),
        2,
        "only the two fork requests forked"
    );
    assert_eq!(
        s.effects("new").len(),
        minted,
        "no fork was minted by session/new"
    );
}

#[tokio::test]
async fn a_forked_turn_applies_its_overrides_and_is_accounted_on_its_own() {
    let s = Served::start();
    s.send(1, Some("src"), json!({})).await;
    let forked = s
        .send(
            2,
            Some("fork"),
            json!({
                "a2a-bridge.fork_from": "src",
                "a2a-bridge.model": "other",
                "a2a-bridge.mode": "ask",
            }),
        )
        .await;
    assert_eq!(text(&forked), "answer from fork-0");
    let wire = s.wire("wire");
    let fork_at = wire.iter().position(|l| l.starts_with("fork ")).unwrap();
    assert!(
        wire[fork_at + 1..].contains(&"set_mode fork-0 ask".to_owned()),
        "{wire:?}"
    );
    assert!(
        wire[fork_at + 1..].contains(&"set_model fork-0 other".to_owned()),
        "{wire:?}"
    );
    assert!(
        wire.iter()
            .all(|l| l != "set_model mint-0 other" && !l.starts_with("set_mode mint-0")),
        "the source session keeps its own config: {wire:?}"
    );

    let rows = s.rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let [source, fork] = [&rows[0]["attribution"], &rows[1]["attribution"]];
    assert_eq!(fork["agent"], "agent");
    assert_eq!(fork["execution"], format!("exec-{:032}", 2));
    assert_eq!(fork["effective_model"], "other");
    assert_eq!(fork["effective_mode"], "ask");
    assert_eq!(source["execution"], format!("exec-{:032}", 1));
    assert_eq!(source["effective_model"], "good");
    assert!(rows
        .iter()
        .all(|row| row["finalized"] == true && row["dispatched"] == true));
}

#[tokio::test]
async fn a_fork_inherits_the_source_config_unless_the_message_overrides_it() {
    let s = Served::start();
    s.send(1, Some("src"), json!({"a2a-bridge.model": "other"}))
        .await;
    s.send(2, Some("fork"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    let rows = s.rows();
    assert_eq!(rows[1]["attribution"]["effective_model"], "other");
    assert!(
        s.wire("wire")
            .contains(&"set_model fork-0 other".to_owned()),
        "the fork keeps the source's model: {:?}",
        s.wire("wire")
    );
}

#[tokio::test]
async fn refusals_carry_stable_reasons_and_send_no_fork_or_prompt() {
    let s = Served::start();
    s.send(1, Some("src"), json!({})).await;
    s.send(2, Some("plain-src"), json!({"a2a-bridge.agent": "plain"}))
        .await;
    let settled = s.wire("wire");
    let settled_plain = s.wire("wire-plain");

    let refused = |reply: &Value| {
        assert_eq!(reply["error"]["code"], -32600, "{reply}");
        reason(reply).to_owned()
    };
    // Unknown source.
    let reply = s
        .send(3, Some("t1"), json!({"a2a-bridge.fork_from": "nobody"}))
        .await;
    assert_eq!(refused(&reply), "fork_source_not_found");
    // Target already in use.
    s.send(4, Some("taken"), json!({})).await;
    let settled = {
        let mut wire = settled;
        wire.extend(s.wire("wire").into_iter().skip(wire.len()));
        wire
    };
    let reply = s
        .send(5, Some("taken"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(refused(&reply), "fork_target_in_use");
    // The source id itself is a target in use too.
    let reply = s
        .send(6, Some("src"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(refused(&reply), "fork_target_in_use");
    // An adapter that did not advertise session/fork.
    let reply = s
        .send(
            7,
            Some("t2"),
            json!({"a2a-bridge.fork_from": "plain-src", "a2a-bridge.agent": "plain"}),
        )
        .await;
    assert_eq!(refused(&reply), "fork_adapter_unsupported");
    // A cwd outside allowed_cwd_root.
    let reply = s
        .send(
            8,
            Some("t3"),
            json!({"a2a-bridge.fork_from": "src", "a2a-bridge.cwd": "/definitely/not/under/the/root"}),
        )
        .await;
    assert_eq!(refused(&reply), "fork_cwd_outside_root");
    // Backends and routes with no session to fork: delegation (and, with it, fan-out and workflows).
    for skill in ["delegate", "fan-out", "flow"] {
        let reply = s
            .send(
                9,
                Some("t4"),
                json!({"a2a-bridge.fork_from": "src", "a2a-bridge.skill": skill}),
            )
            .await;
        assert_eq!(refused(&reply), "fork_backend_unsupported", "{skill}");
    }
    assert!(
        s.peer_untouched(),
        "a refused fork never reaches an outbound peer"
    );

    assert_eq!(s.wire("wire"), settled, "no refusal reached the adapter");
    assert_eq!(s.wire("wire-plain"), settled_plain);
    assert!(s.effects("fork").is_empty());

    // The refused source is untouched: it still takes its own turn, then forks.
    assert_eq!(
        text(&s.send(10, Some("src"), json!({})).await),
        "answer from mint-0"
    );
    let reply = s
        .send(11, Some("t5"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(text(&reply), "answer from fork-0");
}

const GOVERNED: &str =
    "\n[[governance.rules]]\nid='request'\nselector='each_request'\nlimits={turns=1}\n";

#[tokio::test]
async fn a_fork_that_names_no_agent_takes_the_source_agent() {
    for governed in [false, true] {
        let s = Served::start_with(if governed { GOVERNED } else { "" });
        s.send(1, Some("src"), json!({"a2a-bridge.agent": "other"}))
            .await;
        // The registry default is `agent`; the source ran on `other`.
        let reply = s
            .send(2, Some("child"), json!({"a2a-bridge.fork_from": "src"}))
            .await;
        assert_eq!(
            text(&reply),
            "answer from fork-0",
            "governed={governed}: {reply}"
        );
        let streamed = s
            .send_stream(3, Some("child-2"), json!({"a2a-bridge.fork_from": "src"}))
            .await;
        assert!(
            streamed.contains("answer from fork-1"),
            "governed={governed}: {streamed}"
        );
        let forks: Vec<_> = s
            .wire("wire-other")
            .into_iter()
            .filter(|l| l.starts_with("fork "))
            .collect();
        assert_eq!(forks.len(), 2, "{:?}", s.wire("wire-other"));
        assert!(
            s.effects("fork").is_empty(),
            "the default agent never saw a fork"
        );
        // Accounting follows the inherited agent.
        let rows = s.rows();
        assert!(
            rows.iter()
                .all(|row| row["attribution"]["agent"] == "other"),
            "governed={governed}: {rows:?}"
        );
        // An explicit, conflicting agent is refused with its stable reason, unary and streaming.
        let other_wire = s.wire("wire-other");
        let reply = s
            .send(
                4,
                Some("child-3"),
                json!({"a2a-bridge.fork_from": "src", "a2a-bridge.agent": "agent"}),
            )
            .await;
        assert_eq!(
            reason(&reply),
            "fork_agent_mismatch",
            "governed={governed}: {reply}"
        );
        let body = s
            .send_stream(
                5,
                Some("child-4"),
                json!({"a2a-bridge.fork_from": "src", "a2a-bridge.agent": "agent"}),
            )
            .await;
        assert!(
            body.contains("\"reason\":\"fork_agent_mismatch\""),
            "governed={governed}: {body}"
        );
        assert_eq!(
            s.wire("wire-other"),
            other_wire,
            "governed={governed}: a refused fork reached the source agent"
        );
        assert!(
            s.effects("fork").is_empty() && s.effects("prompt").is_empty(),
            "governed={governed}: a refused fork reached the named agent"
        );
        assert_eq!(
            s.wire("wire-other")
                .iter()
                .filter(|l| l.starts_with("fork "))
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn streaming_refusals_carry_the_stable_reason_too() {
    for governed in [false, true] {
        let s = Served::start_with(if governed { GOVERNED } else { "" });
        s.send(1, Some("src"), json!({})).await;
        let before = s.wire("wire");
        let body = s
            .send_stream(2, Some("t"), json!({"a2a-bridge.fork_from": "nobody"}))
            .await;
        assert!(
            body.contains("\"reason\":\"fork_source_not_found\""),
            "governed={governed}: {body}"
        );
        let body = s
            .send_stream(3, Some("src"), json!({"a2a-bridge.fork_from": "src"}))
            .await;
        assert!(
            body.contains("\"reason\":\"fork_target_in_use\""),
            "governed={governed}: {body}"
        );
        assert_eq!(s.wire("wire"), before, "no refusal reached the adapter");
    }
}

#[tokio::test]
async fn a_source_with_a_turn_in_flight_is_refused_until_it_settles() {
    let s = Served::start();
    s.send(1, Some("src"), json!({})).await;
    fs::write(s.dir.path().join("wire.block"), "").unwrap();
    let url = s.url.clone();
    let running = tokio::spawn(async move {
        reqwest::Client::new()
            .post(&url)
            .header("A2A-Version", "1.0")
            .json(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "SendMessage", "params": {"message": {
                "text": "go", "contextId": "src", "taskId": format!("exec-{:032}", 2),
                "metadata": {
                    "a2a-bridge.execution_id": format!("exec-{:032}", 2),
                    "a2a-bridge.attempt_id": format!("attempt-{:032}", 2),
                }}}}),
            )
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });
    for _ in 0..400 {
        if s.prompted().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        s.prompted().len(),
        2,
        "the source's second turn is in flight"
    );
    let reply = s
        .send(3, Some("t"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(reason(&reply), "fork_source_busy");
    assert!(s.effects("fork").is_empty());
    assert_eq!(s.prompted().len(), 2, "the refusal sent no prompt");

    fs::remove_file(s.dir.path().join("wire.block")).unwrap();
    assert!(running.await.unwrap().contains("answer from mint-0"));
    let reply = s
        .send(4, Some("t"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(text(&reply), "answer from fork-0");
}

#[tokio::test]
async fn forks_and_their_source_release_and_cancel_independently() {
    let s = Served::start();
    s.send(1, Some("src"), json!({})).await;
    s.send(2, Some("fork"), json!({"a2a-bridge.fork_from": "src"}))
        .await;

    // Releasing the source leaves the fork usable ...
    let released = s.call("SessionRelease", json!({"contextId": "src"})).await;
    assert_eq!(released["result"]["released"], true, "{released}");
    assert_eq!(
        text(&s.send(3, Some("fork"), json!({})).await),
        "answer from fork-0"
    );
    // ... and the released source is gone for forking.
    let reply = s
        .send(4, Some("late"), json!({"a2a-bridge.fork_from": "src"}))
        .await;
    assert_eq!(reason(&reply), "fork_source_not_found");

    // Forking again from the surviving fork, then releasing it, leaves its own fork usable.
    s.send(
        5,
        Some("grandchild"),
        json!({"a2a-bridge.fork_from": "fork"}),
    )
    .await;
    s.call("SessionRelease", json!({"contextId": "fork"})).await;
    assert_eq!(
        text(&s.send(6, Some("grandchild"), json!({})).await),
        "answer from fork-1"
    );
    assert!(s.stderr().is_empty() || !s.stderr().contains("panicked"));
}

#[tokio::test]
async fn the_fork_capability_is_reported_for_the_adapter_that_advertises_it() {
    let s = Served::start();
    let out = s.cli(&["models", "--config", "config.toml", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let agents: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(agents["agent"]["fork"], true, "{agents}");
    assert!(agents["plain"].get("fork").is_none(), "{agents}");

    let card: Value = reqwest::get(format!("{}/.well-known/agent-card.json", s.url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let models = card["capabilities"]["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|ext| {
            ext["uri"]
                .as_str()
                .is_some_and(|uri| uri.contains("agent-models"))
        })
        .expect("agent-models extension");
    assert_eq!(models["params"]["agents"]["agent"]["fork"], true, "{card}");
    assert!(
        models["params"]["agents"]["plain"].get("fork").is_none(),
        "{card}"
    );
}

#[tokio::test]
async fn submit_fork_from_sets_the_key_and_prints_the_new_context() {
    let s = Served::start();
    s.send(1, Some("src"), json!({})).await;
    let input: PathBuf = s.dir.path().join("input.md");
    fs::write(&input, "go").unwrap();
    let args = |extra: &[&str]| {
        let mut args = vec![
            "submit",
            "--input",
            input.to_str().unwrap(),
            "--url",
            &s.url,
            "--no-heartbeat",
        ];
        args.extend_from_slice(extra);
        args.into_iter().map(str::to_owned).collect::<Vec<_>>()
    };
    let run = |args: Vec<String>| {
        Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(&args)
            .current_dir(s.dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = run(args(&["--fork-from", "src", "--context", "fork"]));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("context_id=fork"), "{stdout}");
    assert!(stdout.contains("answer from fork-0"), "{stdout}");

    // Without --context the server allocates the new context and submit prints it.
    let out = run(args(&["--fork-from", "src"]));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("context_id="), "{stdout}");
    assert!(stdout.contains("answer from fork-1"), "{stdout}");

    // A refused fork surfaces its stable reason.
    let out = run(args(&["--fork-from", "nobody"]));
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("fork_source_not_found"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let help = s.cli(&["submit", "--help"]);
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(help.contains("--fork-from"), "{help}");
}
