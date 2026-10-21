//! Fixture-only standalone smoke entry point, reusable by later accounting slices.
use bridge_core::accounting::UsageAttemptV1;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
pub struct SmokeCli {
    pub dir: tempfile::TempDir,
    pub config: PathBuf,
    prompts: PathBuf,
    pub store: PathBuf,
    pub platform: PathBuf,
}
impl SmokeCli {
    pub fn new(configured: bool, interrupt: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let adapter = dir.path().join("fixture-acp");
        let prompts = dir.path().join("prompts.jsonl");
        // Observe the exact delivered prompt independently of accounting. Both usage sources
        // travel through the real ACP mapper: occupancy/cost notification, terminal token totals.
        fs::write(&adapter, r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -E 's/^\{"jsonrpc":"2.0","id":([^,]+),"method".*/\1/')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]},"id":%s}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","result":{"sessionId":"fixture","models":{"availableModels":[{"modelId":"good","name":"Good"}],"currentModelId":"good"}},"id":%s}\n' "$id" ;;
    *'"method":"session/set_model"'*|*'"method":"session/set_mode"'*|*'"method":"session/set_config_option"'*) printf '{"jsonrpc":"2.0","result":{},"id":%s}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' "$line" >> "$1"
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture","update":{"sessionUpdate":"usage_update","used":99,"size":1000,"cost":{"amount":0.25,"currency":"USD"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"PONG"}}}}'
      if [ "$2" = true ]; then exit 0; fi
      printf '{"jsonrpc":"2.0","result":{"stopReason":"end_turn","usage":{"totalTokens":15,"inputTokens":10,"outputTokens":5,"thoughtTokens":2,"cachedReadTokens":3,"cachedWriteTokens":4}},"id":%s}\n' "$id" ;;
  esac
done
"#).unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        crate::text_file_busy::warm_up_script(&adapter);
        let config = dir.path().join("a2a-bridge.toml");
        let store_config = if configured {
            "[store]\npath = \"tasks.sqlite\"\n"
        } else {
            ""
        };
        fs::write(
            &config,
            format!(
                r#"
default = "agent"
{store_config}
[registry]
allowed_cmds = [{adapter:?}]
[metrics]
enabled = false
[traces]
enabled = false
[[agents]]
id = "agent"
cmd = {adapter:?}
args = [{prompts:?}, "{interrupt}"]
pre_authenticated = true
[server]
addr = "127.0.0.1:0"
"#
            ),
        )
        .unwrap();
        let platform = dir.path().join("platform");
        fs::create_dir(&platform).unwrap();
        fs::set_permissions(&platform, fs::Permissions::from_mode(0o700)).unwrap();
        let store = if configured {
            dir.path().join("tasks.sqlite")
        } else {
            platform.join("workflow-history.sqlite")
        };
        Self {
            dir,
            config,
            prompts,
            store,
            platform,
        }
    }
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_a2a-bridge"));
        cmd.args([
            "smoke",
            "--agent",
            "agent",
            "--model",
            "good",
            "--timeout-secs",
            "5",
            "--config",
        ])
        .arg(&self.config)
        .current_dir(self.dir.path())
        .env("A2A_BRIDGE_STATE_DIR", &self.platform);
        cmd
    }
    pub fn prompts(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&self.prompts)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub fn rows(&self) -> Vec<UsageAttemptV1> {
        let path = self.store.with_extension("sqlite.usage-v1.sqlite");
        if !path.exists() {
            return vec![];
        }
        let conn =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let rows = conn
            .prepare("SELECT record_json FROM usage_attempts_v1")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect();
        rows
    }
}
