# a2a-bridge Operator UI — Design & Mockups

**Created:** 2026-07-17 · **Realizes roadmap item [H3-5](roadmap-improvements.md) (operator ops panel).**
Grounded in the bridge's existing operator surfaces (permission suspend/resume, `session`/`task`/`batch`
clients, `/metrics`, the traces drill-down routes) — the UI is a **client** over those seams and contains no
orchestration.

**Rendered mockups:** [`operator-ui-mockups.html`](operator-ui-mockups.html) — open in a browser to see the
TUI cockpit (overview, permission inbox, implement supervision) and the mobile screens as styled frames. This
markdown is the full spec; the HTML is the visual companion.

Three form factors were evaluated: **TUI (recommended, ships now)**, **web (deferred to Grafana + an optional
thin cost page)**, and a **mobile companion (post-federation remote surface)**. Full treatment below.

---

## 1. Recommendation: TUI first (ratatui), web deferred to Grafana + a later thin cost page

**Build a TUI.** Justification, tied to this operator and these seams:

1. **The operator lives in tmux.** The killer surface is the `permission_policy="defer"` inbox: an agent's ACP `request_permission` **suspends** and waits, with `permission_timeout_ms` (default 120 000 ms — `bin/a2a-bridge/src/main.rs:864`). Approval latency is the product. A tmux pane with a terminal bell and tmux activity-flag is glanceable in <1s; a browser tab is where notifications go to die. A web UI would need OS notification plumbing to match what `\a` gives a TUI for free.
2. **Every surface is already CLI/JSON-RPC/SSE.** `task watch` is reattachable SSE with a `Last-Event-ID` cursor (ADR-0015); `session status` already returns pending-permission views (`PermissionView` in `crates/bridge-core/src/permission.rs`, verified by `session_status_lists_pending_permissions`); traces are plain GET routes. A TUI is a thin terminal client over these. A web UI would additionally force CORS/static-serving decisions into `bridge-a2a-inbound` — orchestration-adjacent surface area we explicitly want to keep out of the core.
3. **The web UI's one real advantage — charts — is already better served.** `/metrics` is Prometheus format with labeled series (`bridge_turn_cost_total{agent,currency,model}`). The operator can point Grafana at it today. Building a bespoke chart web app duplicates Grafana badly; a TUI with sparklines + a Grafana link covers 95% of JTBD-5.
4. **Cheapest first build.** ratatui + the JSON-RPC client the CLI already embodies (`submit`/`task`/`session` are thin HTTP clients per the README architecture section). No new server, no bundler, no asset pipeline.

**The hybrid, sketched:** keep `/metrics` → Prometheus → Grafana for historical cost/latency dashboards (zero code). If a native web page is ever wanted, it is a **separate** `bridge-webui` binary serving one static self-contained HTML page that proxies `/metrics` + the traces routes — never routes added to `serve`.

---

## 2. Information architecture

```
                       ┌──────────────────────────────────────────────┐
                       │ Global chrome: server identity + fingerprint │
                       │ + SSE health + pending-permission badge      │
                       └──────────────────────────────────────────────┘
   [1] Overview ── glance (JTBD-1)
   [2] Inbox ───── permission approvals (JTBD-2)        ← global hotkey `p` from anywhere
   [3] Runs ────── list ──▶ Run Detail (live tail)
                     │        ├─▶ Implement Supervision (JTBD-3)   (auto for implement runs)
                     │        └─▶ Investigate (JTBD-4)             (auto-offered on Failed/wedged)
   [4] Cost ────── burn vs budget (JTBD-5)
   [5] Fleet ───── Sessions tab (JTBD-6) │ Agents tab (JTBD-7)
```

- Number keys `1–5` switch top-level screens; `Enter` drills in, `Esc` backs out. Investigate and Implement Supervision are drill-ins of a run, not tabs — they inherit the run's context.
- A **pending-permission badge** `⚠ 2` renders in the header on *every* screen, with remaining-timeout countdown of the oldest request. `p` jumps to the Inbox from anywhere. Terminal bell rings once per new pending request.
- Read-mostly: every screen is safe to leave open. All writes are keyed actions that map 1:1 onto existing CLI verbs and (except approve/deny, which are the designed fast path) require a confirm.

---

## 3. Core screens (mockups)

Palette conventions used below: **green** = healthy/running/approved, **yellow** = waiting/warn (usage fraction over `warm_usage_warn_fraction`, timeout <30s), **red** = failed/denied/wedged/over-budget, **dim** = terminal/idle/historic. `⚠` items blink-highlight. Bottom bar always shows contextual keybinds.

### 3a. Overview / Dashboard

**Purpose:** JTBD-1 — one glance: what's running, turns in flight, cost burn, queue depth, anything needing me.

```
┌ a2a-bridge ▏ 127.0.0.1:8080 ▏ card ✓ a2acp ▏ fp 9f3c…41e1 ✓ ▏ ● SSE ▏ ⚠ 2 pending (78s) ┐
│ [1]Overview  [2]Inbox●2  [3]Runs  [4]Cost  [5]Fleet                          14:32:07 │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ NOW                                      │ ATTENTION                                  │
│  turns in flight   3                     │  ⚠ permit req-7f21  codex/impl  78s left   │
│  queue depth       4  (batch cap 2)      │  ⚠ permit req-7f22  codex/impl  91s left   │
│  cost this hour    $1.84   ▂▃▅▇▅▃▂▁      │  ✗ t-01JX2M failed: implement verify node  │
│  tokens this hour  312k in / 41k out     │  ! warm ctx c-9a01 usage 0.83 (warn ≥0.80) │
├──────────────────────────────────────────┴────────────────────────────────────────────┤
│ ACTIVE RUNS                                                                            │
│  ID        KIND        AGENT   NODE/STATE            TTFT    DUR     COST    STATUS    │
│▸ t-01JX3A  implement   codex   review (att 2/3)      1.2s    4m12s   $0.61   ● running │
│  t-01JX3B  code-review claude  synth                 0.8s    1m03s   $0.22   ● running │
│  t-01JX3C  batch #b-12 kiro    queued 3/7            —       —       —       ○ queued  │
│  t-01JX2M  implement   codex   verify ✗              1.1s    9m44s   $1.13   ✗ failed  │
├───────────────────────────────────────────────────────────────────────────────────────┤
│ SESSIONS  idle 2 · running 3 · compacting 0     AGENTS  codex ✓  claude ✓  kiro ✓ (9/9)│
├───────────────────────────────────────────────────────────────────────────────────────┤
│ j/k move · Enter open run · p inbox · c cancel run · r refresh · ? help · q quit       │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

**Interactions:** `j/k` select run, `Enter` → Run Detail (implement runs open Supervision, failed runs open Investigate), `c` → confirm-gated `task cancel`, `p` → Inbox.
**Data sources:** gauges from `GET /metrics` poll @5s (`bridge_turns_in_flight`, `bridge_queue_depth`; hourly cost/token deltas computed client-side from `bridge_turn_cost_total` / `bridge_turn_tokens_total` counters). Run table from A2A `task list` (JSON-RPC `GetTask`/list against the durable store) merged with `batch list`. Attention pane from `session status` (pending `PermissionView`s + `warm_usage_warn_fraction` breaches) and failed tasks from `task list`. Sessions/agents strip from `session status` + Agent Card `agent-models` extension.

### 3b. Permission Inbox — the defer inbox

**Purpose:** JTBD-2 — resolve suspended `request_permission` calls before `permission_timeout_ms` expires, with enough context to decide safely.

```
┌ a2a-bridge ▏ … ▏ ⚠ 2 pending (78s) ┐  INBOX                                            │
├─ PENDING (2) ────────────────────────────┬─ REQUEST req-7f21 ──────────────────────────┤
│▸ req-7f21  codex @ t-01JX3A (implement)  │ context   c-impl-4420   gen 3   op op-11    │
│    write outside workspace   ⏳ 78s ████▁ │ agent     codex (tier3-impl, container :rw) │
│  req-7f22  codex @ t-01JX3A (implement)  │ title     "permission req-7f21"             │
│    run `cargo publish`       ⏳ 91s █████ │ tool call                                   │
│                                          │   fs/write  /repo/../../.ssh/config         │
│─ RESOLVED (this session) ────────────────│                    ▲ outside clone root     │
│  req-7f19  ✓ approved  edit src/lib.rs   │ OPTIONS (from agent)                        │
│  req-7f18  ✗ denied    curl example.com  │  [1] allow-once          [2] allow-always   │
│  req-7f15  ✓ approved  cargo test        │  [3] reject-once ◀ default                  │
│                                          │ RUN CONTEXT (last 6 journal lines)          │
│                                          │  edit: patched retry backoff in client.rs   │
│                                          │  verify: cargo test 212 passed              │
│                                          │  review: MAJOR — hardcoded path…            │
├──────────────────────────────────────────┴─────────────────────────────────────────────┤
│ a approve · d deny · m modify (pick 1-3) · e escalate · R reason · j/k · Enter detail  │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

**Interactions:** `a`/`d` resolve immediately with a 400ms visual confirm flash (no modal — this path is latency-critical by design); `m` then `1–3` picks an agent-offered option id; `e` escalates; `R` attaches a `--reason` before resolving. Countdown bar turns yellow <30s, red <10s, rings the bell again at 15s.
**Data sources:** pending list = `session status` poll @1s (returns `PermissionView`: requestId, generation, operationId, offered options, title — `crates/bridge-core/src/permission.rs`). Resolution = the exact `SessionPermit` JSON-RPC the CLI sends: `session permit <requestId> --context <ctxId> --generation <n> --op <opId> --approve|--deny|--modify <optId>|--escalate [--reason <txt>]`. Run-context tail = `GET /tasks/:id/journal.jsonl` (last N lines). Tier badge from local config (see §3f caveat).

### 3c. Implement Supervision — supervised implement with live veto

**Purpose:** JTBD-3 — watch clone→edit→verify→review→tweak, read the diff and verdicts, and steer (`session inject`) or veto (`task cancel` / deny at the inbox) without ever driving the loop from the UI.

```
┌ … ▏ ⚠ 0 ┐  RUN t-01JX3A · implement · codex @ tier3-impl · --resume ok · att 2/3       │
├─ PIPELINE ─────────────────────────────────────────────────────────────────────────────┤
│  clone ✓ 4s → edit ✓ 2m10s → verify ✓ 41s → review ● standard 1m02s → tweak → merge    │
│                                    212 tests ✓        2 reviewers + synth      (gate)   │
├─ LIVE STREAM (review node, attempt 2) ────────────┬─ DIFF (att 2)  +84 −12 · 3 files ──┤
│ 14:31:40 ttft 1.2s                                │ M src/client.rs        +61 −9      │
│ [claude-lens] The retry loop now honors           │ M src/config.rs        +19 −3      │
│ Retry-After but the jitter calc can overflow      │ A tests/retry.rs       +4  −0      │
│ on u32::MAX… checking blast radius via prism      │                                    │
│ diff-slice…                                       │ REVIEW VERDICTS                    │
│ [codex-lens] verify passed; reading taint         │  att 1  ✗ Changes-Requested        │
│ slice at .git/a2a-bridge/review-slices/…          │    MAJOR overflow in jitter calc   │
│ ▌                                                 │  att 2  ● in progress (standard)   │
├───────────────────────────────────────────────────┴────────────────────────────────────┤
│ STEER  i inject note → session inject c-impl-4420 --input - --append                   │
│ VETO   x cancel run (confirm) · merge gate: NEVER auto — `merge t-01JX3A` offered only │
│        after Approved verdict, confirm-typed                                           │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ i inject · x cancel · D full diff · V verdict detail · J journal · f follow · Esc back │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

**Interactions:** `i` opens a one-line editor whose submit is exactly `session inject <contextId> --input - --append [--dedupe <key>]` — a steering note the *loop* consumes; the UI never edits the plan itself. `D` fetches the full diff artifact; `V` shows the synth verdict text; `x` = confirm-gated `task cancel`. When the run reaches Approved, a `merge` affordance appears requiring the run id to be re-typed (maps to `a2a-bridge merge <id> [--onto <branch>]`).
**Data sources:** pipeline + per-node/attempt status and TTFT/duration/cost from the run's SSE stream (`task watch <id>` semantics: A2A `SubscribeToTask` with `Last-Event-ID` reattach, ADR-0015). Diff/verdict artifacts from `GET /tasks/:id/artifacts/:node`. Journal tail from `GET /tasks/:id/journal.jsonl`. Depth badge (light/standard/thorough) rides the per-node events.

### 3d. Investigate — journal + deepest error

**Purpose:** JTBD-4 — for a failed/wedged run, surface the **deepest** error (today the real root cause is often swallowed after a node wedges), with a per-node timeline and the raw journal one keypress away.

```
┌ … ┐  INVESTIGATE t-01JX2M · implement · ✗ Failed · failure_class: verify_error          │
├─ DEEPEST ERROR (auto-extracted, depth 3) ──────────────────────────────────────────────┤
│ ▶ verify › cargo test › proc exit 101                                       14:19:02   │
│   error[E0308]: mismatched types — expected `Duration`, found `u64`                    │
│   src/client.rs:147:31                                                                 │
│   ⓘ terminal task status said only "verify node failed"; this frame was 3 levels        │
│     deeper in the journal and 41s earlier than the wrapping error.                     │
├─ NODE TIMELINE ────────────────────────────────────────────────────────────────────────┤
│  14:08:11 clone    ✓  4s                                                               │
│  14:08:15 edit     ✓  6m02s   warm container a2a-run-8812-edit                         │
│  14:14:17 verify   ✗  41s     exit 101  ◀ deepest error here                           │
│  14:14:58 (wedge)  ⚠  4m46s   no further frames; turn held Running until timeout       │
│  14:19:44 task     ✗  terminal StatusUpdate: Failed                                    │
├─ JOURNAL (raw, cursor at deepest frame) ───────────────────────────────────────────────┤
│ {"ts":"14:19:02","node":"verify","attempt":1,"stream":"stderr","data":"error[E0308]…"} │
│ {"ts":"14:19:02","node":"verify","attempt":1,"event":"proc_exit","code":101}           │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ e next/prev error · t turn row · A artifacts · y yank resume cmd · / grep · Esc back   │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

**Interactions:** `e`/`E` cycle error frames deepest-first; `t` fetches the turn row for token/cost forensics; `y` yanks `a2a-bridge implement --resume t-01JX2M …` to the clipboard (the fix path stays in the CLI); `/` greps the journal client-side.
**Data sources:** `GET /tasks/:id/journal.jsonl` (whole file, parsed client-side; deepest-error heuristic = last `stderr`/`error` frame *preceding* the wedge gap, not the terminal `StatusUpdate`), `GET /turns/:turn_id` for per-turn rows, `GET /tasks/:id/artifacts/:node` for node outputs, `task get` for terminal status + `failure_class`. Requires opt-in `[traces]`; if disabled the screen degrades to `task get` + a banner saying which config block to enable.

### 3e. Cost / Quota

**Purpose:** JTBD-5 — watch burn against a budget; per-agent/model attribution; feed the cost-governance roadmap item without pretending the server enforces budgets yet.

```
┌ … ┐  COST                                     budget: $10.00/day (client-side, .abtui) │
├─ BURN ─────────────────────────────────────────────────────────────────────────────────┤
│  today  $6.41 / $10.00  ████████████████░░░░░░░░  64%      proj. EOD $9.80 ⚠           │
│  hour   ▂▂▃▅▇▅▃▂▁▂▃▅  $1.84    turns/hr 14    avg $0.13/turn                           │
├─ BY AGENT/MODEL (today) ────────────────────────┬─ TOP RUNS (today) ────────────────── ┤
│  AGENT   MODEL         TURNS  TOKENS     COST   │  t-01JX2M implement    $1.13         │
│  codex   gpt-5.6-sol     31   1.2M/210k  $3.90  │  t-01JX3A implement    $0.61 ●       │
│  claude  opus-4.8        12   410k/88k   $2.21  │  t-01JX1F code-review  $0.44         │
│  kiro    (default)        9   150k/12k   $0.30  │  b-12 batch (7 runs)   $0.38         │
├─────────────────────────────────────────────────┴──────────────────────────────────────┤
│  outcome mix: success 46 ▏ error 4 ▏ canceled 2      cost_dropped_total: 0 ✓           │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ b set budget · G open Grafana · Enter run detail · Tab by-model/by-agent · Esc back    │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

**Interactions:** `b` edits the local budget (stored in the TUI's own config — the UI must not invent server-side governance; when the roadmap's server budgets land, this becomes read-through). `G` prints/yanks the Grafana URL for deep history.
**Data sources:** `GET /metrics` poll @5s with bearer token: `bridge_turn_cost_total{agent,currency,model}`, `bridge_turn_tokens_total`, `bridge_turns_total{agent,effort,model,outcome}`, `bridge_turn_cost_dropped_total` (a nonzero value = attribution is lossy, shown red). Sparkline/EOD projection = client-side counter deltas. Top-runs list from per-turn cost on the SSE events + `GET /turns/:turn_id`. Counters reset on `serve` restart — the header shows counter age and the screen labels totals "since serve start" when < a day.

### 3f. Fleet — Sessions tab (Agents tab sketched)

**Purpose:** JTBD-6/7 — warm-session hygiene and agent/model health in one place.

```
┌ … ┐  FLEET   [Sessions] Agents                                                         │
├─ WARM SESSIONS ────────────────────────────────────────────────────────────────────────┤
│  CONTEXT       AGENT   STATE       IDLE-TTL   USAGE          PENDING   TASK            │
│▸ c-impl-4420   codex   ● Running   —          0.62 ██████░░   2 ⚠      t-01JX3A        │
│  c-9a01        claude  ○ Idle      12m/30m    0.83 ████████⚠  0        —               │
│  c-rev-118     kiro    ◐ Compacting —         0.41 ████░░░░   0        —               │
├─ ACTIONS (selected: c-9a01) ───────────────────────────────────────────────────────────┤
│  usage 0.83 ≥ warn 0.80 → recommend: compact                                           │
│  [C]ompact   [R]elease   [K]clear   [X]cancel   [I]nject      (all confirm-gated)      │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ Agents tab (Tab):                                                                      │
│   AGENT   KIND          TIER  MODELS (advertised)          EFFORT        DOCTOR        │
│   codex   acp           1 RO  gpt-5.6-sol ✓                low…xhigh     9/9 ✓         │
│   impl    container_rw  3 RW  gpt-5.6-sol ✓                low…xhigh     9/9 ✓         │
│   claude  acp           0     opus-4.8, sonnet-4.6         low…max       8/9 ⚠ auth    │
│   card fp 9f3c…41e1 matches local config ✓ (H1-4 fingerprint: pinned at connect)       │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ C compact · R release · K clear · X cancel · I inject · d run doctor · m probe models  │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

**Interactions:** every action is a confirm-gated 1:1 CLI verb: `session compact <contextId>`, `session release`, `session clear`, `session cancel`, `session inject`. `d` shells out to `a2a-bridge doctor --json` (read-only 9-check preflight, host-vs-sandbox aware); `m` runs `models --json` (live probe — on demand only, it spawns agents).
**Data sources:** Sessions = `session status` poll @2s (claim-state enum Idle/Running/Resetting/Compacting, idle TTL, usage fraction vs `warm_usage_warn_fraction`, pending-permission count). Agents = Agent Card `agent-models` extension (`capabilities.extensions[].params.agents` — free, no probe) refreshed on demand; `doctor --json` / `models --json` shell-outs. **Tier badges** come from parsing the *local* config file, since tiers aren't on the wire — displayed with a `local-config` provenance mark, and cross-checked against the config-fingerprint once roadmap H1-4 lands; on mismatch the badge greys out rather than lying.

---

## 4. Interaction model

**Keybinding scheme (vim-ish, two layers):**
- *Global:* `1–5` screens, `p` inbox-jump, `?` help overlay, `q` quit, `:` command palette (palette entries are literal CLI verbs — `:cancel t-01JX3A`, `:compact c-9a01` — reinforcing the 1:1 mapping), `y` yank the CLI equivalent of the selected entity/action.
- *Local:* `j/k/g/G` list nav, `Enter`/`Esc` drill/back, `/` filter, `f` follow-tail toggle, `Tab` sub-tabs.
- *Writes:* inbox `a/d/m/e` are single-key with a visual confirm flash (latency-critical by design); everything else (`cancel`, `release`, `clear`, `merge`) opens a confirm modal; `merge` requires re-typing the run id.

**How live data arrives:**

| Feed | Transport | Cadence |
|---|---|---|
| Run/turn/node events, TTFT, tokens, cost, outcome | SSE — A2A `SubscribeToTask` per watched run, reattach via `Last-Event-ID` cursor persisted to the TUI's state file (ADR-0015) | push |
| Pending permissions, claim states, usage fractions | `session status` JSON-RPC poll | 1 s (2 s when inbox empty) |
| Gauges/counters | `GET /metrics` (bearer) | 5 s |
| Task/batch inventory | `task list` / `batch list` | 5 s + on SSE terminal events |
| Journal / turn rows / artifacts | `GET /tasks/:id/journal.jsonl`, `/turns/:turn_id`, `/tasks/:id/artifacts/:node` | on demand |
| Doctor / model probe | `doctor --json`, `models --json` shell-out | on demand only |

**Permission round-trip, end to end:** agent calls ACP `request_permission` → `PolicyEngine` defers, registers a gen+op-keyed rendezvous in the permission registry → next `session status` poll (≤1s) shows the `PermissionView` → TUI bells, badges, renders options → operator hits `a` → TUI sends the `SessionPermit` JSON-RPC (requestId + `--context` + `--generation` + `--op` + decision, exactly the CLI's payload) → registry resolves exact-once, suspended ACP call returns → next poll confirms removal; on `permission_timeout_ms` expiry the entry moves to Resolved as `timed-out (denied)` in red. Generation stamping means a stale approve after a session reset is rejected server-side — the TUI just displays the error, never retries.

---

## 5. Design principles & phased build

**Principles:**
1. **Read-mostly; explicit control plane.** Every mutation is a 1:1 mapping onto an existing verb (`permit`, `inject`, `cancel`, `release`, `clear`, `compact`, `merge`). The UI contains zero orchestration — no retry logic, no workflow decisions, no auto-merge, ever.
2. **Fail closed on server identity.** On connect, fetch `/.well-known/agent-card.json` and pin it (and the H1-4 config-fingerprint once it ships). If the card/fingerprint changes mid-session, drop to a read-only banner state until the operator re-acknowledges — never send a permit to a server you can't identify.
3. **Deepest error first.** Failure UI leads with the extracted deepest journal frame, not the wrapping node status — directly targeting the known swallowed-error bugs.
4. **Everything reattachable, nothing owned.** SSE cursors persist; killing the TUI loses nothing; the durable SQLite store (server-side) is the only source of truth. The TUI never opens the SQLite file — it would violate the single-writer lock.
5. **The CLI is the escape hatch.** `y` yanks the equivalent command everywhere; the TUI accelerates the CLI, it doesn't replace it.
6. **Latency budget on the inbox:** new pending request visible ≤2s, bell ≤2s, resolution round-trip ≤1 keypress + 1 RPC.

**Phased build:**
- **Phase 0 — Inbox (cheapest first screen, highest value):** one poll (`session status`) + one RPC (`SessionPermit`) + bell. Shippable in days; immediately makes `permission_policy="defer"` livable.
- **Phase 1 — Overview + Runs + live tail:** `task list`/`batch list` + one `SubscribeToTask` SSE consumer with cursor persistence.
- **Phase 2 — Implement Supervision + Investigate:** traces routes, diff/verdict artifacts, deepest-error extractor, `session inject` steering.
- **Phase 3 — Cost + Fleet:** `/metrics` poller, sparklines, budget file, doctor/models shell-outs, tier badges.
- **Phase 4 (optional) — web cost page:** separate static-serving binary; or just ship a Grafana dashboard JSON in `deploy/` and skip it.

## 6. Tech sketch

- **New crate `crates/bridge-tui`, binary `a2a-bridge-tui`** (or `a2a-bridge tui` dispatching to it) — a separate client crate; nothing added to `bridge-core`/`bridge-a2a-inbound`.
- **Stack:** `ratatui` + `crossterm` (event loop), `tokio` (one task per feed → `mpsc` into a single `AppState` reducer), `reqwest` + `reqwest-eventsource` (SSE with `Last-Event-ID`), `serde_json`, `arboard` (yank), optional `tui-textarea` for the inject editor.
- **Client reuse, not new orchestration:** extract the JSON-RPC request builders the CLI already has in `bin/a2a-bridge/src/main.rs` (`build_session_permit_rpc`, `build_session_inject_rpc`, the `task`/`session`/`batch` payloads) into a small shared `bridge-client` crate consumed by both the CLI subcommands and the TUI — guaranteeing the TUI can never send anything the CLI couldn't.
- **State file:** `~/.config/a2a-bridge-tui/state.toml` — server URL, bearer token env-var *name*, pinned card/fingerprint, SSE cursors, local budget. No secrets stored.
- **Web alternative (if ever):** `bridge-webui` binary embedding one self-contained static HTML page (no CDN), reverse-proxying `/metrics` + traces routes to sidestep CORS changes in the core; charts only, no control plane — permits stay in the TUI/CLI where identity pinning is enforced.

---

## 7. Mobile companion app — a post-federation remote, not a third cockpit

**Honest fit assessment:** mobile is the *weakest* of the three form factors for this tool and should be built last, if at all — with one exception that genuinely justifies it: **approving a deferred permission while away from the desk.** `permission_timeout_ms` defaults to 120s; if the operator steps out for coffee mid-`implement`, today the request times out and the run dies. That single moment — approve/deny from a lock screen — is the entire reason this app deserves to exist. Everything else on a phone is a worse version of the TUI.

### 7.1 The hard constraint: mobile is gated on H3-2

The bridge binds `127.0.0.1:8080` behind NAT. A phone cannot reach it, period. Mobile therefore requires what the TUI gets for free:

- **(a) Secure remote reach** — Tailscale/WireGuard mesh, or an authenticated tunnel, so the phone can address the bridge at all.
- **(b) TLS + per-caller auth + signed, pinned server identity** — you cannot put a *permission control plane* on a network path with only a shared bearer token and an unauthenticated Agent Card. This is exactly roadmap **H3-2 (A2A federation v1)**: per-caller identity, mTLS/JWT enforcement (explicitly listed as "not implemented" in the README's Known limitations), and the H1-4 config-fingerprint so the phone provably talks to the bridge running the config the operator thinks it is.

**Sequencing consequence, stated plainly:** the TUI ships now against today's localhost trust model. The mobile app is a **post-federation surface** — starting it before H3-2 lands means either building throwaway auth or, worse, shipping approve/deny over a weakly authenticated network path. Don't.

### 7.2 The push problem

Real-time approval needs the phone to *learn* about a pending request within seconds. The local bridge cannot push to a phone (no APNs/FCM credentials, no public endpoint), and a phone cannot hold a reliable long-lived connection to a home machine (OS kills background sockets). Two transport architectures, honestly traded:

| | **Direct-over-Tailscale (poll/SSE over tunnel)** | **Relay for push (bridge → relay → APNs/FCM)** |
|---|---|---|
| How | Phone polls `session status` / holds SSE over the mesh while app is foregrounded | Tiny relay (cloud or always-on host) with APNs/FCM creds; bridge POSTs pending-permission envelopes to it; relay pushes |
| Wakes a locked phone | **No** — background polling is throttled to minutes by iOS/Android; useless for a 120s timeout | **Yes** — this is the only way to hit the lock screen in seconds |
| New infrastructure | None (mesh already exists for reach) | A new always-on component + push credentials |
| Security surface | Control plane stays inside the mesh; nothing leaves | Notification *envelope* transits third-party push infra — must carry **only** `{requestId, agent, title, deadline}`, never diff content, paths, or repo names; the **resolution RPC always goes back over the authenticated tunnel, never through the relay** |
| Failure mode | Miss the request unless app is open | Relay down → degrade to in-app poll; bridge must treat relay as fire-and-forget, never block the permission registry on it |

**Recommendation:** both, layered — relay for the wake-up (content-free envelope), tunnel for everything else including the `SessionPermit` RPC itself. The relay is a doorbell, not a door. The security implication is accepted knowingly: putting approve/deny on the network at all is a real expansion of the control plane's attack surface, which is precisely why per-device revocable auth and generation stamping (§7.6) are non-negotiable, and why deny-by-timeout remains the fail-safe.

### 7.3 Scope: the phone gets four jobs, not seven

**In:** (1) permission approve/deny with push — the killer case; (2) glance/overview — is anything running, anything red; (3) cost/quota alert — budget breach push + a burn readout; (4) **cancel a runaway run** — the one write beyond permit that belongs on a phone (a run burning $2/hr while you're at lunch).

**Out, deliberately:** journal/deepest-error investigation, diff reading, review verdicts, implement supervision, session hygiene (compact/release/inject), doctor/model probes. Why: these are *forensic reading and steering* tasks — multi-pane, grep-driven, clipboard-heavy, consequence-laden. A 6-inch screen degrades them into skimming, and skim-approving a merge or skim-reading a diff-verdict is exactly how a supervised loop stops being supervised. The mobile rule: **decide only what was designed to be decided in one glance** (an agent-offered permission option, a cancel). Anything requiring synthesis waits for the desk. `session inject` is excluded for the same reason — steering text composed on a phone keyboard under time pressure is a liability, not a feature.

### 7.4 Mockups

**(a) Lock-screen push — pending permission** (content-free envelope + inline actions):

```
┌──────────────────────────────┐
│  🔒  09:41                   │
│ ┌──────────────────────────┐ │
│ │ ⚠ a2a-bridge      now    │ │
│ │ Permission request       │ │
│ │ codex · implement run    │ │
│ │ req-7f21 · expires 78s   │ │
│ │ ▓▓▓▓▓▓▓▓░░░░  ⏳          │ │
│ │                          │ │
│ │  [ Approve ]  [ Deny ]   │ │
│ │  [ Open in app…       ]  │ │
│ └──────────────────────────┘ │
│   inline actions require     │
│   device unlock (FaceID)     │
└──────────────────────────────┘
```

Inline Approve/Deny fire the `SessionPermit` RPC over the tunnel after biometric unlock; if the tunnel is down the action fails **loudly** into the app rather than queueing. **Data source:** envelope from the relay (`{requestId, agent, runKind, deadline}`); actions → `SessionPermit` JSON-RPC over the mesh.

**(b) In-app approval detail** (mobile analog of Inbox §3b — full context arrives over the tunnel, not the relay):

```
┌──────────────────────────────┐
│ ◀ Inbox        req-7f21   ⚠  │
├──────────────────────────────┤
│ ⏳ 71s   ▓▓▓▓▓▓▓░░░░░        │
│                              │
│ codex @ t-01JX3A (implement) │
│ tier3-impl · container :rw   │
│ ctx c-impl-4420 · gen 3      │
├──────────────────────────────┤
│ TOOL CALL                    │
│  fs/write                    │
│  /repo/../../.ssh/config     │
│  ⚠ outside clone root        │
├──────────────────────────────┤
│ OPTIONS (from agent)         │
│  ○ allow-once                │
│  ○ allow-always              │
│  ● reject-once  ◀ default    │
├──────────────────────────────┤
│ Run context (journal tail)   │
│  verify: 212 tests passed    │
│  review: MAJOR — hardcoded…  │
├──────────────────────────────┤
│ ┌──────────┐  ┌───────────┐  │
│ │ ✗ DENY   │  │ ✓ APPROVE │  │
│ └──────────┘  └───────────┘  │
│      [ escalate to desk ]    │
└──────────────────────────────┘
```

"Escalate to desk" maps to `--escalate` — the honest mobile answer when the request needs the diff. **Data sources:** `session status` (PermissionView: requestId/gen/op/options), `GET /tasks/:id/journal.jsonl` tail — both over the tunnel; resolution via `SessionPermit` with `--context --generation --op`.

**(c) Glance + cost alert:**

```
┌──────────────────────────────┐
│ a2a-bridge   ● tunnel ✓ fp ✓ │
├──────────────────────────────┤
│ NOW                          │
│  3 running · 4 queued        │
│  ⚠ 1 pending permit (71s)    │
├──────────────────────────────┤
│ COST TODAY                   │
│  $6.41 / $10.00              │
│  ▓▓▓▓▓▓▓▓▓▓▓▓░░░░░  64%      │
│  ⚠ projected EOD $9.80       │
├──────────────────────────────┤
│ RUNS                         │
│  ● t-01JX3A impl  4m  $0.61  │
│    review · attempt 2/3      │
│                 [ CANCEL ]   │
│  ● t-01JX3B rev   1m  $0.22  │
│  ✗ t-01JX2M impl  — failed   │
│    → investigate on desktop  │
├──────────────────────────────┤
│  Inbox(1)   Glance   Cost    │
└──────────────────────────────┘
```

CANCEL requires a swipe-confirm and maps to `task cancel`. Failed runs deep-link nowhere — they say "investigate on desktop" on purpose. **Data sources:** `task list`, `session status`, `/metrics` counters (`bridge_turns_in_flight`, `bridge_queue_depth`, `bridge_turn_cost_total` deltas), all polled over the tunnel while foregrounded; budget-breach push via the relay.

### 7.5 Position

Mobile is a **remote companion for the latency-critical subset** — approve/deny, glance, cost alarm, emergency cancel — gated on H3-2 federation, while the TUI remains the primary cockpit and ships now. It is not a third place to do the job; it is a pager with a yes/no button. Both clients speak through the same seam: the shared `bridge-client` request builders (§6) define the *only* payloads either can send — `SessionPermit`, `TaskCancel`, `session status`, `task list`, `/metrics`, `journal.jsonl` — so the mobile app inherits the same 1:1-verb discipline and can never invent orchestration the CLI doesn't have.

### 7.6 Tech sketch

- **Client:** native (SwiftUI first, given the operator's macOS environment; Kotlin later) over a PWA. The deciding factor is push: actionable lock-screen notifications with inline approve/deny and biometric gating require native APNs categories; a PWA over the tunnel cannot wake reliably and reduces to "poll while open," which forfeits the killer case.
- **Relay:** a ~200-line stateless service (fly.io/small VPS, or an always-on home host with APNs/FCM creds). Bridge-side: a fire-and-forget `[notify]` webhook POST on permission-registry insert and budget breach — envelope only, no content, and the registry never blocks on it. Relay compromise therefore leaks *that* a request happened, never what, and grants no control.
- **Auth:** per-device tokens minted by the H3-2 caller-identity layer, revocable individually (`device revoke` on the desk), scoped to the mobile verb subset (permit, cancel, read) — a stolen phone cannot `merge`, `inject`, or `clear`. Server identity pinned via TLS cert + H1-4 config-fingerprint at pairing (QR code on the desk shows fingerprint + pairing token); on mismatch the app goes read-only, same fail-closed rule as the TUI (§5.2).
- **Staleness:** the mobile `SessionPermit` carries the same `--context --generation --op` stamping as the TUI/CLI — a phone approving from a 90-second-old notification after the session reset gets a clean server-side rejection from the gen+op-keyed registry (`crates/bridge-core/src/permission.rs`), not a misapplied grant. The exact-once rendezvous also makes phone-and-desk racing safe: first resolver wins, the loser gets an error, nothing double-fires.
