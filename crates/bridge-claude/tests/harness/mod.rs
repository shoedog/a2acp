//! Materializes a python fake `claude` + a per-spawn JSON config, returns a
//! (cmd, ClaudeConfig) the backend can spawn. No env vars, no cargo bin.
#![allow(dead_code)]
use bridge_claude::ClaudeConfig;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Behavior knobs for one spawned fake process.
#[derive(Default, Clone)]
pub struct FakeSpec {
    pub no_init: bool,              // skip the init line → init never captured (lazy)
    pub hang: bool, // read the turn but never emit ANYTHING → turn-timeout / cancel→EOF
    pub stall: bool, // emit assistant text, then NO result → a STARTED, mid-flight turn
    pub exit_before_init: bool, // exit(0) immediately, before init/read → EOF-before-init (not auth)
    pub result_err: Option<String>, // emit result subtype = this (e.g. "error_during_execution")
    pub reply: Option<String>,  // fixed assistant text; default = remembered number or "OK"
    pub init_sid: String,       // session id in the init line
}
impl FakeSpec {
    pub fn new() -> Self {
        Self {
            init_sid: "fake-sid".into(),
            ..Default::default()
        }
    }
}

const FAKE_PY: &str = r#"#!/usr/bin/env python3
import sys, json, os
def cfg():
    for i, a in enumerate(sys.argv):
        if a == "--fake-config":
            return json.load(open(sys.argv[i+1]))
    return {}
c = cfg()
out = sys.stdout
if c.get("exit_before_init", False):
    sys.exit(0)   # close stdout before init/read → models "not authenticated"/immediate exit
if not c.get("no_init", False):
    out.write(json.dumps({"type":"system","subtype":"init","session_id":c.get("init_sid","fake-sid")})+"\n"); out.flush()
memory = None
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    try: v = json.loads(line)
    except Exception: continue
    try: text = v["message"]["content"][0]["text"]
    except Exception: text = ""
    for w in text.split():
        if w.lstrip("-").isdigit(): memory = w
    if c.get("hang", False):
        continue
    reply = c.get("reply") or (memory if memory is not None else "OK")
    out.write(json.dumps({"type":"assistant","message":{"content":[{"type":"text","text":reply}]}})+"\n")
    out.flush()
    if c.get("stall", False):
        continue   # emitted text but no result → the turn is started + mid-flight
    if c.get("result_err"):
        out.write(json.dumps({"type":"result","subtype":c["result_err"]})+"\n")
    else:
        out.write(json.dumps({"type":"result","subtype":"success","stop_reason":"end_turn"})+"\n")
    out.flush()
"#;

/// Write the script + config to a unique temp dir; return (script_path, config).
pub fn fake(name: &str, spec: FakeSpec) -> (String, ClaudeConfig) {
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("v3c-fake-{}-{}-{}", std::process::id(), name, seq));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake_claude.py");
    std::fs::write(&script, FAKE_PY).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg_path = dir.join("config.json");
    let mut f = std::fs::File::create(&cfg_path).unwrap();
    let json = serde_json::json!({
        "no_init": spec.no_init, "hang": spec.hang, "stall": spec.stall,
        "exit_before_init": spec.exit_before_init,
        "result_err": spec.result_err, "reply": spec.reply, "init_sid": spec.init_sid,
    });
    write!(f, "{json}").unwrap();
    let config = ClaudeConfig {
        cwd: PathBuf::from("."),
        extra_args: vec![
            "--fake-config".into(),
            cfg_path.to_string_lossy().into_owned(),
        ],
        ..ClaudeConfig::default()
    };
    (script.to_string_lossy().into_owned(), config)
}

/// Convenience: a default fake that echoes the remembered number.
pub fn fake_default(name: &str) -> (String, ClaudeConfig) {
    fake(name, FakeSpec::new())
}
