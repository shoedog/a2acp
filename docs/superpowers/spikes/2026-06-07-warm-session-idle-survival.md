# Spike — warm `:rw` ACP session idle-survival (B2b-3c)

**Date:** 2026-06-07  **Result:** PASS

Question: does a warm `:rw` `claude-agent-acp` container + its ACP session survive the multi-minute
verify+review gap between the implement loop's edit and fix turns? Drove a real container (toolchain
image, fresh creds, egress) over newline-delimited ACP JSON: initialize -> session/new -> prompt ->
**420s (7 min) idle** -> re-prompt the SAME session.

## Transcript (recorded)
```
SEND initialize id 0
init: RESULT
SEND session/new id 1
session: 0cf6b2ea-50b6-4da9-b3d0-391dde9b49a8
SEND session/prompt id 2
prompt1: RESULT end_turn
IDLING 420s (proxy for the verify+review gap) ...
container running after idle: true
SEND session/prompt id 3
IDLE_SURVIVAL: PASS — same warm session answered after 420s idle (stopReason=end_turn)
```

## Script
```python
import json, subprocess, sys, threading, queue, time

CREDS = "/Users/wesleyjinks/.config/a2a-creds/claude/.credentials.json"
NAME = "a2a-spike-idle"
IDLE = 420  # ~7 min, a proxy for the verify+review gap between loop turns

subprocess.run(["docker", "rm", "-f", NAME], capture_output=True)
docker = [
    "docker", "run", "-i", "--rm", "--name", NAME,
    "--network", "a2a-egress-internal",
    "-e", "HTTPS_PROXY=http://a2a-egress-proxy:8888",
    "-e", "HTTP_PROXY=http://a2a-egress-proxy:8888",
    "-e", "NO_PROXY=localhost,127.0.0.1",
    "-v", f"{CREDS}:/root/.claude/.credentials.json",
    "a2a-toolchain:latest", "claude-agent-acp",
]
proc = subprocess.Popen(docker, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE, text=True, bufsize=1)
q = queue.Queue()
def reader(pipe, tag):
    for line in pipe:
        q.put((tag, line.rstrip("\n")))
    q.put((tag, None))
threading.Thread(target=reader, args=(proc.stdout, "out"), daemon=True).start()
threading.Thread(target=reader, args=(proc.stderr, "err"), daemon=True).start()

def send(obj):
    proc.stdin.write(json.dumps(obj) + "\n"); proc.stdin.flush()
    print("SEND", obj.get("method"), "id", obj.get("id"), flush=True)

def wait_result(want_id, timeout):
    end = time.time() + timeout
    while time.time() < end:
        try:
            tag, line = q.get(timeout=max(0.1, min(5, end - time.time())))
        except queue.Empty:
            continue
        if line is None:
            return ("EOF", None)
        if tag == "err":
            print("STDERR", line[:240], flush=True); continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        if msg.get("id") == want_id and ("result" in msg or "error" in msg):
            return ("RESULT", msg)
    return ("TIMEOUT", None)

send({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":False,"writeTextFile":False},"terminal":False}}})
print("init:", wait_result(0, 60)[0], flush=True)
send({"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}})
st, msg = wait_result(1, 60)
if st != "RESULT" or "result" not in (msg or {}):
    print("SESSION_NEW_FAILED", st, msg, flush=True); proc.terminate()
    subprocess.run(["docker","rm","-f",NAME],capture_output=True); sys.exit(1)
sid = msg["result"]["sessionId"]
print("session:", sid, flush=True)
send({"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":"Reply with exactly: A"}]}})
st1, m1 = wait_result(2, 120)
print("prompt1:", st1, (m1.get("result",{}).get("stopReason") if st1=="RESULT" else ""), flush=True)
if st1 != "RESULT":
    print("PROMPT1_FAILED — abort", flush=True); proc.terminate()
    subprocess.run(["docker","rm","-f",NAME],capture_output=True); sys.exit(1)

print(f"IDLING {IDLE}s (proxy for the verify+review gap) ...", flush=True)
time.sleep(IDLE)
alive = subprocess.run(["docker","inspect","-f","{{.State.Running}}",NAME],capture_output=True,text=True).stdout.strip()
print("container running after idle:", alive, flush=True)

send({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":"Reply with exactly: B"}]}})
st2, m2 = wait_result(3, 120)
if st2 == "RESULT" and "result" in m2:
    print(f"IDLE_SURVIVAL: PASS — same warm session answered after {IDLE}s idle (stopReason={m2['result'].get('stopReason')})", flush=True)
elif st2 == "RESULT" and "error" in m2:
    print(f"IDLE_SURVIVAL: FAIL — session errored after idle: {m2['error']}", flush=True)
else:
    print(f"IDLE_SURVIVAL: FAIL — {st2} (container_running={alive})", flush=True)
proc.terminate()
subprocess.run(["docker","rm","-f",NAME],capture_output=True)
```
