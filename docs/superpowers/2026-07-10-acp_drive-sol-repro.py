#!/usr/bin/env python3
"""Drive codex-acp in a2a-toolchain through a minimal ACP session.

Usage: acp_drive-sol-repro.py <launch-model>
Optional environment: SWITCH_MODEL, SWITCH_EFFORT, WITH_MCP, AUTHENTICATE.
AUTHENTICATE is intentionally opt-in because mounted credentials are already authenticated.
"""
import subprocess, json, sys, threading, time

model = sys.argv[1] if len(sys.argv) > 1 else "gpt-5.6-sol"
CREDS = "/Users/wesleyjinks/.config/a2a-creds/codex/auth.json"

launch = (
    'cd /tmp && exec codex-acp '
    f'-c model="{model}" '
    '-c model_reasoning_effort="xhigh" '
    '-c approval_policy="never" '
    '-c sandbox_mode="danger-full-access"'
)
cmd = ["docker", "run", "--rm", "-i",
       "-v", f"{CREDS}:/root/.codex/auth.json",
       "a2a-toolchain:latest", "sh", "-lc", launch]

p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.PIPE, text=True, bufsize=1)

err_lines = []
def pump_err():
    for line in p.stderr:
        err_lines.append(line.rstrip())
        print("STDERR:", line.rstrip(), flush=True)
threading.Thread(target=pump_err, daemon=True).start()

def send(obj):
    print("SEND:", json.dumps(obj)[:160], flush=True)
    p.stdin.write(json.dumps(obj) + "\n"); p.stdin.flush()

def read_until(pred, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        line = p.stdout.readline()
        if not line:
            print("<<stdout EOF>>", flush=True); return None
        line = line.strip()
        if not line: continue
        try: msg = json.loads(line)
        except Exception:
            print("RECV(raw):", line[:200], flush=True); continue
        open("/tmp/acp_full.jsonl","a").write(json.dumps(msg)+"\n")
        print("RECV:", json.dumps(msg)[:220], flush=True)
        if pred(msg): return msg
    print("<<timeout>>", flush=True); return None

try:
    send({"jsonrpc":"2.0","id":1,"method":"initialize",
          "params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":False,"writeTextFile":False},"terminal":False}}})
    read_until(lambda m: m.get("id")==1)
    import os
    if os.environ.get("AUTHENTICATE"):
        send({"jsonrpc":"2.0","id":2,"method":"authenticate","params":{"methodId":"chat-gpt"}})
        read_until(lambda m: m.get("id")==2)
    mcp = []
    if os.environ.get("WITH_MCP"):
        mcp = [{"name":"lsp","command":"/usr/local/bin/lsp-mcp",
                "args":["--repo","/tmp","--lang","auto","--target-cache","/tmp/lsp-target"],
                "env":[]}]
    r = send({"jsonrpc":"2.0","id":3,"method":"session/new","params":{"cwd":"/tmp","mcpServers":mcp}})
    resp = read_until(lambda m: m.get("id")==3)
    sid = (resp or {}).get("result",{}).get("sessionId")
    print("SESSION ID:", sid, flush=True)
    if sid:
        switch = os.environ.get("SWITCH_MODEL")
        if switch:
            # reproduce the BRIDGE's in-session model switch (acp_backend.rs:618)
            send({"jsonrpc":"2.0","id":"sw","method":"session/set_config_option",
                  "params":{"sessionId":sid,"configId":"model","value":switch}})
            print("SWITCH RESULT:", json.dumps(read_until(lambda m: m.get("id")=="sw", timeout=60) or {"NO_RESPONSE":True}), flush=True)
        effort = os.environ.get("SWITCH_EFFORT")
        if effort:
            # reproduce the bridge's effort reconciliation after model selection
            send({"jsonrpc":"2.0","id":"se","method":"session/set_config_option",
                  "params":{"sessionId":sid,"configId":"reasoning_effort","value":effort}})
            print("EFFORT RESULT:", json.dumps(read_until(lambda m: m.get("id")=="se", timeout=60) or {"NO_RESPONSE":True}), flush=True)
        send({"jsonrpc":"2.0","id":"m","method":"session/set_mode","params":{"sessionId":sid,"modeId":"agent-full-access"}})
        read_until(lambda m: m.get("id")=="m", timeout=15)
        send({"jsonrpc":"2.0","id":4,"method":"session/prompt",
              "params":{"sessionId":sid,"prompt":[{"type":"text","text":"Reply with exactly the single word PONG and nothing else."}]}})
        read_until(lambda m: m.get("id")==4, timeout=90)
    time.sleep(2)
finally:
    try: p.stdin.close()
    except Exception: pass
    time.sleep(1)
    p.terminate()
    print("\n=== EXIT CODE:", p.poll(), "===", flush=True)
    print("=== STDERR TAIL ===", flush=True)
    for l in err_lines[-25:]: print(l, flush=True)
