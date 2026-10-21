#![allow(dead_code)]
use bridge_core::accounting::UsageAttemptV1;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub config: PathBuf,
}
impl Fixture {
    pub fn new(max_attempts: u32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("source")).unwrap();
        git(&root.join("source"), &["init", "-q"]);
        git(&root.join("source"), &["config", "user.name", "Fixture"]);
        git(
            &root.join("source"),
            &["config", "user.email", "fixture@example.test"],
        );
        fs::write(root.join("source/work"), "base").unwrap();
        git(&root.join("source"), &["add", "work"]);
        git(&root.join("source"), &["commit", "-qm", "base"]);
        git(root, &["init", "--bare", "-q", "remote.git"]);
        git(
            &root.join("source"),
            &[
                "remote",
                "add",
                "origin",
                root.join("remote.git").to_str().unwrap(),
            ],
        );
        git(&root.join("source"), &["push", "-q", "origin", "HEAD"]);
        fs::write(root.join("task.md"), "---\ntask-type: implement\n---\n# Fixture\n\n## Description\nEdit work.\n\n## Acceptance Criteria\nWork is fixed.\n").unwrap();
        let runtime = root.join("runtime");
        fs::write(&runtime, PROVIDER).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|path| path.join("git"))
            .find(|path| path.is_file())
            .unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let wrapper = root.join("bin/git");
        fs::write(
            &wrapper,
            format!(
                r#"#!/usr/bin/env python3
import sys, os, json, pathlib
root = pathlib.Path(__file__).parent.parent
if sys.argv[-3:] == ['log', '-1', '--format=%s']:
    state = root/'container'
    with (root/'subject-lookups').open('a') as f:
        f.write((state.read_text() if state.exists() else 'null')+'\n')
os.execv({git:?}, [{git:?}] + sys.argv[1:])
"#
            ),
        )
        .unwrap();
        fs::set_permissions(wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let config = root.join("config.toml");
        fs::write(
            &config,
            format!(
                r#"
default = "writer"
allowed_cwd_root = {root:?}
[registry]
allowed_cmds = [{runtime:?}]
[store]
path = "history.sqlite"
[metrics]
enabled = false
[traces]
enabled = false
[storage]
admission_floor_gib = 0
[custody]
push = false
[evidence]
path = {evidence:?}
[implement]
max_attempts = {max_attempts}
[[agents]]
id = "writer"
kind = "container_rw"
cmd = "fixture"
model = "good"
effort = "high"
mode = "write"
[agents.sandbox]
runtime = {runtime:?}
image = "fixture"
mount = {root:?}
access = "rw"
egress = "open"
[[agents]]
id = "reviewer"
cmd = {runtime:?}
args = ["adapter"]
pre_authenticated = true
model = "good"
effort = "high"
mode = "write"
[[workflows]]
id = "implement-edit"
[[workflows.nodes]]
id = "edit"
agent = "writer"
prompt_text = "EDIT"
inputs = []
[[workflows]]
id = "implement-fix"
[[workflows.nodes]]
id = "fix"
agent = "writer"
prompt_text = "FIX {{{{input}}}}"
inputs = []
[[workflows]]
id = "implement-review"
[[workflows.nodes]]
id = "review"
agent = "reviewer"
prompt_text = "REVIEW {{{{input}}}}"
inputs = []
[review]
workflow = "implement-review"
slice_timeout_secs = 1
[server]
"#,
                root = root,
                evidence = root.join("evidence")
            ),
        )
        .unwrap();
        Self { dir, config }
    }
    pub fn run(&self, resume: bool, mode: &str) -> Output {
        let Some(binary) = option_env!("CARGO_BIN_EXE_a2a-bridge") else {
            panic!("run is integration-only; unit tests share fixture construction");
        };
        let mut cmd = Command::new(binary);
        cmd.arg("implement");
        if resume {
            cmd.arg("--resume")
                .arg(self.clone_path().file_name().unwrap());
        } else {
            cmd.arg("--input")
                .arg(self.dir.path().join("task.md"))
                .arg("--repo")
                .arg(self.dir.path().join("source"))
                .args(["--lang", "none", "--depth", "light"]);
        }
        cmd.arg("--config").arg(&self.config);
        let mut paths = vec![self.dir.path().join("bin")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        cmd.env("PATH", std::env::join_paths(paths).unwrap());
        cmd.env("FIXTURE_MODE", mode)
            .env("A2A_BRIDGE_STATE_DIR", self.dir.path().join("state"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }
    pub fn clone_path(&self) -> PathBuf {
        fs::read_dir(self.dir.path().join(".a2a-implement"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.join(".git").is_dir())
            .unwrap()
    }
    pub fn strand(&self) {
        let path = bridge_controller::implement_resume::checkpoint_path(&self.clone_path());
        let mut ck: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        ck["phase"] = "InLoop".into();
        ck["loop_max_attempts"] = 3.into();
        fs::write(path, serde_json::to_vec(&ck).unwrap()).unwrap();
    }
    pub fn setups(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.dir.path().join("setups"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub fn prompts(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.dir.path().join("prompts"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    pub fn rows(&self) -> Vec<UsageAttemptV1> {
        let path = self.dir.path().join("history.sqlite.usage-v1.sqlite");
        if !path.exists() {
            return vec![];
        }
        let conn = rusqlite::Connection::open(path).unwrap();
        let rows = conn
            .prepare("SELECT record_json FROM usage_attempts_v1 ORDER BY rowid")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect();
        rows
    }
}
pub fn git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
pub fn success(out: &Output) {
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

const PROVIDER: &str = r#"#!/usr/bin/env python3
import sys, os, json, pathlib, subprocess, sqlite3
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
mode = (root/'mode').read_text() if (root/'mode').exists() else os.environ.get('FIXTURE_MODE', '')
state = root / 'container'
if not args: sys.exit(0)
if args[0] == 'container': args = args[1:]
if args[0] in ['info','ps']: sys.exit(0)
if args[0] == 'rm':
    state.unlink(missing_ok=True)
    sys.exit(0)
if args[0] in ['pause','unpause']:
    if mode == 'custody_refuse' and args[0] == 'unpause': sys.exit(7)
    data = json.loads(state.read_text()); data['paused'] = args[0] == 'pause'
    state.write_text(json.dumps(data)); sys.exit(0)
if args[0] == 'inspect':
    if not state.exists(): sys.exit(1)
    data = json.loads(state.read_text())
    fmt = args[args.index('--format')+1]
    if 'Config.Labels' in fmt: print(data['id']+'\t'+json.dumps(data['labels']))
    elif 'State.Paused' in fmt: print(data['id']+' true '+str(data['paused']).lower())
    elif 'State.Status' in fmt: print('running')
    else: sys.exit(2)
    sys.exit(0)
if args[0] == 'run':
    with (root/'runs').open('a') as f: f.write('run\n')
    labels = dict(args[i+1].split('=',1) for i,a in enumerate(args) if a == '--label')
    state.write_text(json.dumps({'id':args[args.index('--name')+1], 'labels':labels, 'paused':mode == 'custody_refuse'}))
cwd = None
applied = {'model':'good', 'mode':'ask', 'effort':'low'}
def effort(value):
    return {'id':'effort','name':'Effort','type':'select','currentValue':value,
            'options':[{'value':v,'name':v} for v in ['low','high']]}
for line in sys.stdin:
    req = json.loads(line); method = req.get('method'); ident = req.get('id')
    if ident is None: continue
    result = {}
    if method == 'initialize': result = {'protocolVersion':1,'agentCapabilities':{},'authMethods':[]}
    elif method == 'session/new':
        cwd = pathlib.Path(req['params']['cwd'])
        with (root/'setups').open('a') as f: f.write(json.dumps({'writer':args[0] == 'run','cwd':str(cwd)})+'\n')
        model = 'other' if mode == 'replacement_reject' else 'good'
        result = {'sessionId':'fixture','models':{'availableModels':[{'modelId':model,'name':model}],'currentModelId':model},
                  'modes':{'currentModeId':'ask','availableModes':[{'id':m,'name':m} for m in ['ask','write']]},
                  'configOptions':[effort('low')]}
    elif method == 'session/set_mode': applied['mode'] = req['params']['modeId']
    elif method == 'session/set_model': applied['model'] = req['params']['modelId']
    elif method == 'session/set_config_option':
        applied['effort'] = req['params']['value']
        result = {'configOptions':[effort(applied['effort'])]}
    elif method == 'session/prompt':
        text = req['params']['prompt'][0]['text']
        with (root/'prompt-starts').open('a') as f: f.write(text+'\n')
        if mode == 'terminal_error':
            print(json.dumps({'jsonrpc':'2.0','id':ident,'error':{'code':-32603,'message':'fixture terminal error'}}),flush=True)
            continue
        with (root/'prompts').open('a') as f: f.write(json.dumps({'text':text,'cwd':str(cwd),'config':applied})+'\n')
        if text.startswith(('EDIT','FIX')):
            (cwd/'work').write_text('fixed' if text.startswith('FIX') else 'edited')
            subprocess.run(['git','-C',str(cwd),'add','work'],check=True)
            (cwd/'.git/A2A_COMMIT_MSG').write_text('fixture change')
            answer = 'edited'
        else:
            count = root/'reviews'; n = int(count.read_text()) if count.exists() else 0
            count.write_text(str(n+1))
            answer = 'VERDICT: '+('APPROVE' if n else 'REJECT')+'\nSUMMARY: fixture review'
        print(json.dumps({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'fixture','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':answer}}}}),flush=True)
        if mode == 'usage_write_fail':
            with sqlite3.connect(root/'history.sqlite.usage-v1.sqlite') as db:
                db.execute("CREATE TRIGGER fail_usage BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(FAIL, 'fixture usage failure'); END")
        result = {'stopReason':'end_turn','usage':{'totalTokens':15,'inputTokens':10,'outputTokens':5}}
    print(json.dumps({'jsonrpc':'2.0','id':ident,'result':result}),flush=True)
"#;
