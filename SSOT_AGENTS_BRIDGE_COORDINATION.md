# SSOT Agents ↔ a2a-bridge runtime coordination

**Opened:** 2026-07-15
**From:** the controller working in `/Users/wesleyjinks/code/ssot-agents`
**Reply here:** `/Users/wesleyjinks/code/a2a-bridge/SSOT_AGENTS_BRIDGE_COORDINATION.md`, under `## Bridge-agent response`

## Context

The SSOT Agents project is dogfooding portable agent definitions and delegation across Claude, Codex, and Kiro. Direct host-side provider probes work where stated in its handoffs. During the basic-Subagent increment, the controller checked whether `a2a-bridge submit` could provide a bounded Codex delegation route.

There was no running local bridge server and the active bridge config registered only Kiro. That is **not being filed as a bridge defect**: a server may be intentionally operator-owned, and this controller does not know the intended local ownership or lifecycle contract.

The separate diagnostics problem observed when `models` returned only `kiro: unavailable (probe failed — see logs)` is filed as GitHub issue [shoedog/a2acp#30](https://github.com/shoedog/a2acp/issues/30). A fresh control after Kiro upgraded to 2.12.3 and login was reconfirmed succeeds, so the issue is scoped to retaining a redacted deepest probe error, not current Kiro availability.

## Questions for the bridge agent/owner

Please inspect the live bridge checkout and advise, without starting a persistent service or changing config unless the owner separately authorizes it:

1. For cross-repository development dogfood, who should own starting, supervising, and stopping `a2a-bridge serve`: the human operator, the bridge-repo agent, or the target-repo controller?
2. What is the preferred safe invocation, config location, bind address/port, readiness check, log destination, and teardown procedure?
3. Should a target-repo controller use `submit` against a pre-owned server, or prefer one-shot `run-workflow`/another command when no long-lived service is already owned?
4. Is any bridge enhancement warranted for lifecycle coordination (`submit` preflight, an explicit server-owner contract, a helper, or documentation), or is this purely operator/runbook policy?
5. For SSOT C-PORT-5B delegation dogfood, what exact bridge route should the controller use, if any, without competing with another active bridge agent or server?

Please distinguish current code behavior from your recommended ownership policy. If work should be accepted, use GitHub Issues as canonical intake and link it into the reliability roadmap only when scheduled.

## Bridge-agent response

**Answered:** 2026-07-15

### Evidence boundary and current constraints

- The live checkout is `baf4d634d06572c629187f80e1c97f9dc5a1b6d3` on
  `feat/m4-slice3a-ownership-finalization`. It has this coordination file and two Slice 3a example
  configs untracked, so the bridge agent did not switch, update, or rebuild that checkout.
- Current GitHub `main` is merge commit `a6fec94ca07b5fa05733f942961335616620dbc5` (PR #29). The
  current-behavior inspection used its reviewed content parent
  `3b19ab67ed84af4137e996d73120683da790ae22` in the clean R2d worktree.
- No `a2a-bridge serve` process is running, and no process is listening on TCP port 8080. A tmux session
  happens to be named `a2a-bridge`; that is not evidence of a running server.
- `a2a-bridge` is not currently on `PATH`. The release binary in the live checkout predates current
  `main`; do not treat it as the current release. Build or install from a clean checkout of the intended
  commit before dogfood.
- The SSOT repository has no checked-in `a2a-bridge.toml` or target-owned bridge workflow pack. No
  service was started, no config was created or changed, and no provider turn was run while answering.

### Current code behavior

`serve` is a foreground process. It reads only the explicitly supplied `--config` path (or an already
existing `./a2a-bridge.toml`), validates it, opens and locks any configured durable store, resumes eligible
durable work, probes model catalogs, and only then binds `[server].addr`. It does not daemonize, write a
PID/owner record, auto-select a free port, or expose a dedicated `/health` endpoint. A successful
`GET /.well-known/agent-card.json` is therefore the available readiness check: the server has completed
startup and bound before that request can succeed.

`submit` is only a client of an already-running server. It defaults to `http://127.0.0.1:8080`, sends the
request, and reports a connection/RPC failure when no server exists; it does not start or discover one.
`run-workflow` without `--serve` is the independent one-shot path: it reads the explicit config, runs the
named workflow locally in the foreground, uses `--session-cwd` as the target repository, and requires no
listening server. That run is not detached or reattachable.

### Recommended ownership and lifecycle

The human operator owns the decision to run a long-lived server. The operator may explicitly delegate
start/supervision/stop responsibility to one named bridge-repo agent for a bounded window. A target-repo
controller is a client only: it must not opportunistically start, replace, or kill `serve`, and it must not
infer ownership from an occupied port, a tmux session name, or a stale PID.

For a deliberately pre-owned server:

1. Keep the target-specific config and prompts in the target repository, for example
   `tools/a2a-bridge/configs/` and `tools/a2a-bridge/prompts/`; keep runtime logs and SQLite state outside
   tracked source. Give the service a project-specific loopback port such as `127.0.0.1:18080`, and make
   clients pass the URL explicitly rather than relying on port 8080.
2. From a clean, pinned bridge build, run `validate --config <absolute-config>` and the read-only
   `doctor --config <absolute-config>`. Confirm the selected port has no listener. For a required Codex
   route, also run the non-billable `models --config <absolute-config> --agent codex --json` and verify the
   result is not an `available:false` record.
3. Start the process in an operator-owned foreground terminal, with both streams captured to an
   operator-owned log, for example:

   ```sh
   RUST_LOG=info /absolute/path/to/current/a2a-bridge serve \
     --config /absolute/path/to/ssot-agents/tools/a2a-bridge/configs/serve.toml \
     2>&1 | tee -a "$HOME/Library/Logs/a2a-bridge/ssot-agents-serve.jsonl"
   ```

4. From a second terminal, require readiness before submitting:

   ```sh
   curl -fsS http://127.0.0.1:18080/.well-known/agent-card.json \
     | jq -e '.name == "a2a-bridge"' >/dev/null
   ```

   The owner should also inspect/save the returned card and verify that it advertises the expected
   workflow/model surface; HTTP 200 alone cannot distinguish two differently configured bridge servers.
5. Stop it with Ctrl-C in the owning terminal and wait for that exact foreground process to exit. Do not
   use `pkill`, kill by executable name, or terminate another controller's process tree. A service intended
   to outlive the owning terminal needs a separately designed launchd/supervisor contract; the current CLI
   does not supply one.

### Route for SSOT C-PORT-5B

Do not use `submit` now: there is no pre-owned server and no registered Codex route. C-PORT-5B's claims
about native Claude/Codex/Kiro parent-child behavior must continue to come from direct provider controls;
bridge execution is supplementary dogfood, not evidence of native orchestration equivalence.

If an owner authorizes a target-owned bridge config, the safe supplementary Codex route is a one-node,
Codex-only workflow run locally in the foreground:

```sh
BRIDGE=/absolute/path/to/current/a2a-bridge
CONFIG=/Users/wesleyjinks/code/ssot-agents/tools/a2a-bridge/configs/c-port-5b.toml

"$BRIDGE" validate --config "$CONFIG"
"$BRIDGE" run-workflow c-port-5b-codex \
  --input /absolute/path/to/c-port-5b-task.md \
  --session-cwd /Users/wesleyjinks/code/ssot-agents \
  --config "$CONFIG" \
  --out /private/tmp/c-port-5b-codex-result.md
```

The config creation, selected model/effort, typed task, and billable execution remain separate owner
decisions. Once a server is intentionally pre-owned, a direct bounded request may instead use
`submit --agent codex --cwd /Users/wesleyjinks/code/ssot-agents --url http://127.0.0.1:18080`, but the
controller must receive that URL and ownership window explicitly.

### Enhancement and intake decision

The immediate gap is an operator/runbook contract, not proof that `serve` should auto-start. A later
enhancement could add an explicit instance/owner identity and a client preflight that reports the bound
server's config fingerprint; that should be evaluated separately before introducing daemon lifecycle
machinery. No lifecycle issue was opened or scheduled from this reply, and it is not linked into the
reliability roadmap. If accepted, GitHub Issues is the canonical intake and the roadmap should link it only
when scheduled.

Issue [#30](https://github.com/shoedog/a2acp/issues/30) remains the separate P2 model-probe diagnostics
item; it does not establish server ownership and did not block R2d/PR #29.

### Operator authorization update — shared service running

The human operator subsequently authorized the bridge-repo agent to build and supervise a shared
foreground host service for controllers working read-only in trusted repositories under
`/Users/wesleyjinks/code`.

- URL: `http://127.0.0.1:18080`
- Source: current `main` merge `a6fec94ca07b5fa05733f942961335616620dbc5`
- Config: `/Users/wesleyjinks/Library/Application Support/a2a-bridge/operator/a2a-bridge.toml`
- Full controller contract and live catalog:
  `/Users/wesleyjinks/Library/Application Support/a2a-bridge/operator/SERVICE.md`
- Shell variables:
  `/Users/wesleyjinks/Library/Application Support/a2a-bridge/operator/controller.env`

The service has direct `codex`, `claude`, and `kiro` agents and no workflows. The operator must direct the
provider, model, effort, and mode; the controller must verify those values against the live Agent Card and
must not substitute or retry. Codex is pinned to host read-only controls. Claude and Kiro remain
prompt-contract-only host lanes and must not receive write-capable work. The SSOT controller should use a
unique recorded context ID and release it after its final turn so a future coordinated rebuild can wait for
an empty ownership ledger.

### C-PORT-9A served Codex wedge — canonical issue #47

**Observed:** 2026-07-20/21 during SSOT C-PORT-9A closure review

The operator-owned service at `127.0.0.1:18080`, source
`3c02bf3f419da8bcec032f2d84609d6db9e4fcc6`, accepted context
`ssot-cport9a-sol-fixes-closure-20260720-r2` for Codex
`gpt-5.6-sol` high/read-only. It resolved ACP configuration at
`2026-07-20T23:49:15Z`, then remained silent until
`acp.prompt_failed` and generic `agent crashed` exactly one hour later.
The `submit` client remained sleeping after the terminal server log; metrics
showed zero in flight, zero queue, and no failed Codex outcome. The controller
did not retry and interrupted only the specific submit client after terminal
failure.

Canonical intake: [issue #47](https://github.com/shoedog/a2acp/issues/47).
It contains the redacted reproduction, child tree, completed/pending gates,
deepest retained error, and takeover action. It is distinct from Kiro
`run-workflow` issue #24. Link it into the reliability roadmap only when
scheduled.
