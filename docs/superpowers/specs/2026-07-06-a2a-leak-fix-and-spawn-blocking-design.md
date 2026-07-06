# Design — C: eliminate the bin's hand-rolled A2A client · B2: blocking-call offload

**Status:** DESIGN (Fable architecture, opus-adjudicated — all load-bearing claims verified
against source). Next: codex xhigh review → implement. Repo `a2a-bridge`, base `main`.

## 0. Premise corrections (verified against source — reshape both changes)

- **P1 — B2 headline is stale (VERIFIED).** `crates/bridge-worktree/src/host_git.rs`
  production uses `tokio::process::Command` (host_git.rs:6, `run_git` :22-30 `.output().await`);
  the blocking `std::process::Command` at :141/:187/:204 is `#[cfg(test)]` (mod tests :122).
  Commit `1b2c0134` ("async git process calls; stop parking tokio workers") already shipped
  this. `sweep.rs:9-15` documents that its git stays sync (Drop context). **B2 ≈ 80% done;
  only a small residue remains (§2.2).**
- **P2 — "35 a2a:: refs" is a line count.** Non-test surface is 22 lines / **26 occurrences**
  (test module starts main.rs:6242). Targets below are occurrences.
- **P3 — "wire bytes unchanged" is the WRONG bar for C (VERIFIED).** Typed `a2a::Message`
  construction cannot be byte-identical to the hand-rolled `serde_json::json!` request: typed
  adds required `message_id`+`role`, `Part::text` serializes `{"text"}` (no `"kind"`), key
  order differs. The server never strict-deserializes `Message` — `parts_from_params`
  (server.rs:3569-3594) reads from the raw `Value`, accepting `kind=="text" | None`;
  `task_id_from_params`/`context_id_from_params` likewise. **Correct bar: structural
  (`serde_json::Value`) equality modulo an explicit allowed delta (G-C1).**

## 1. ARCHITECTURE — C (eliminate hand-rolled A2A client)

### 1.1 Central ruling: does `A2aClient` cover the CLI's streaming needs? — NO (3 verified gaps)
1. `send_streaming` hardcodes `context_id: Some(new_context_id())`, `task_id: None`,
   `metadata: None` (client.rs:50-62). CLI needs caller `contextId` (`--context` mandatory),
   a freshly-minted `taskId` (**load-bearing** — server's fresh-send fallback synthesizes the
   constant stub `TaskId::parse("task-1")`, server.rs:3421-3424; omitting collides concurrent
   `--serve` runs in the durable store — guarded by `serve_client_requests_have_distinct_task_ids`),
   and metadata `a2a-bridge.skill`/`.cwd`.
2. `A2aClient::new` sets a client-wide **total** timeout (client.rs:33-35) that spans the
   response body → would abort a long workflow SSE stream. Hand-rolled path uses no timeout
   (main.rs:2831). **Biggest silent-regression risk.**
3. `open_stream` yields lossy `bridge_core::translator::Event` (DelegationStream), applying a
   delegation policy (Completed→empty end, Failed/Canceled/**Rejected**→`Err`, unknown→skip).
   CLI needs the opposite: per-frame status print, terminal recorded only when
   `status.message.is_none() && state∈{Completed,Failed,Canceled}` (Rejected NOT terminal),
   read-to-EOF, distinguish Failed/Canceled/non-terminal/none for exit codes, hard-fail on
   any undecodable frame. None survives a trip through `Event`.

**Seam:** the reusable outbound client owns TRANSPORT — URL, POST, JSON-RPC envelope+id,
`A2A-Version` header, auth policy, timeout policy, and SSE event framing (bytes→`data:`/`id:`,
blank-line boundaries, EOF flush). The CLI keeps INTERPRETATION — typed `a2a::StreamResponse`
decode per SSE payload, terminal semantics, artifact/text extraction, exit-code mapping.
Frame-typing stays per-caller (the two consumers have incompatible decode policies; `task watch`
doesn't type-decode at all). The genuine shared layer is SSE framing, not typed frames.

### 1.2 Target public API on `bridge-a2a-outbound`
New `src/sse.rs`: `pub struct SseEvent { id: Option<String>, data: String }`;
`pub fn sse_events(resp: reqwest::Response) -> impl Stream<Item=Result<SseEvent, SseError>>`
(spec framing: accumulate `data:` lines, dispatch on blank line, flush at EOF, ignore comments).

Extended `A2aClient`:
- `new(base_url, auth, timeout)` — UNCHANGED (peer path).
- `loopback(base_url)` — NO Authorization header, NO total timeout.
- `send_streaming_with(&[Part], SendOpts) -> Result<StreamingReply, ClientError>` — owns Message
  construction, ids, headers, content-type discrimination. `SendOpts { context_id: Option<String>,
  task_id: TaskIdMode(Mint|None), metadata: Option<Map> }`. `StreamingReply { Events(stream) | Json(Value) }`
  (Json = server answered unary instead of streaming).
- `rpc(method, params) -> Result<Value, ClientError>` — generic unary JSON-RPC (replaces `rpc_call`).
  **CONTRACT (codex #1):** MUST parse+return the JSON body **regardless of HTTP status** (do NOT
  `.error_for_status()`). The server rides JSON-RPC errors for invalid params/request on **HTTP 400**
  with an `error` member (server.rs:3362-3366); `rpc_call` never checks status (main.rs:3355) and
  callers inspect `v["error"]`. Using status-error handling would drop the JSON-RPC error body and
  regress user-facing errors for run-batch/batch/session. `ClientError` is only for transport
  failures (unreachable/timeout/undecodable body).
- `subscribe_sse(method, params, last_event_id) -> Result<StreamingReply, ClientError>` — replaces
  `task watch` transport (Last-Event-ID header).
- `send_streaming`/`open_stream`/`cancel` — signatures + observable behavior UNCHANGED, internally
  re-expressed over `send_streaming_with` + `sse_events`, `ClientError→UpstreamA2aError` at old
  boundaries so `PeerDelegation` is untouched.

`ClientError` = new enum carrying `reqwest::Error`/status so the bin keeps its exact user-facing
strings ("cannot reach serve at {url} — is `a2a-bridge serve` running? ({e})"). Loopback paths do
NOT route through `BridgeError::UpstreamA2aError` (discards detail). `testpeer.rs` stays
`#[cfg(test)] pub(crate)`; bin parity tests keep their wiremock harness.

### 1.3 Bin after C
- Delete `build_run_workflow_streaming_request` (main.rs:2772) + `rpc_call` (:3333-3358) + the
  hand-rolled SSE loops. `run_workflow_serve_client` builds `SendOpts{context_id:Some(context),
  task_id:Mint, metadata:skill/cwd}` and decodes each `ev.data` with
  `serde_json::from_str::<a2a::StreamResponse>` (strict — hard error preserved).
- `rpc_call`'s ten callers → `client.rpc(method, params)`; result-shape interpretation stays in bin.
- `task_watch_cmd` → `subscribe_sse`; keeps its data-print loop + `SseEvent.id` for `--from`. Fix in
  passing: `a2a::methods::SUBSCRIBE_TO_TASK` instead of hardcoded `"SubscribeToTask"` (main.rs:3992).
- **Residual `a2a::` target: ≤ 19 occurrences non-test (from 26)**, HARD ZERO for
  `SVC_PARAM_VERSION`, `a2a::VERSION`, `a2a::new_*`, `methods::SEND_STREAMING_MESSAGE`, and
  `reqwest::Client` in the bin. Kept (legit interpretation): `Artifact`, `Message`(:2813 text
  helper), `Option<TaskState>`, `StreamResponse` + arms, `methods::{SEND_MESSAGE,GET_TASK,
  LIST_TASKS,CANCEL_TASK,SUBSCRIBE_TO_TASK}`. Drop bin's direct `[dependencies] reqwest` (keep
  dev-dep). Delete the redundant `[dev-dependencies] a2a` line (Cargo.toml:56 — benign but dead).

### 1.4 Site classification
main.rs:3344 = `rpc_call` (generic loopback JSON-RPC for submit/task/batch/session) — IN scope.
main.rs:3996 = `task_watch_cmd` SSE (third copy of the line-splitter) — IN scope via `subscribe_sse`
(defensible cut line if a smaller C is wanted, but recommended in).

### 1.5 Drift table (verified)
| Axis | Hand-rolled | A2aClient today | Ruling |
|---|---|---|---|
| A2A-Version | "1.0" | sent | identical, keep |
| Authorization | never sent | always bearer | `loopback()` sends none (moot today — AlwaysGrant only — but preserve) |
| Timeout | none | client-wide total | **trap** — loopback sets none |
| JSON-RPC id | `1` | `"req-1"` | server only echoes; parity tests exclude id |
| Part shape | `{kind:text,text}` | `{text}` | allowed delta (server lenient) |
| messageId/role | absent | present | allowed delta (extracted by key) |
| error text | rich | UpstreamA2aError (dropped) | ClientError carries source |

## 2. ARCHITECTURE — B2 (offload)

### 2.1 Seam: do NOT touch `WorktreeProvider` (already async). Two classes:
- **Class 1 must stay sync** — anything reachable from a Drop impl (`WorktreeRunEndGuard::drop`
  sweep.rs:109-124, `RunEndGuard::drop`, reaper primitives reaper.rs:58-139). Keep
  `sweep_orphans`/`recover_orphans` internals sync functions shared by Drop guards + async wrappers.
- **Class 2 offload at the async CALL SITE** with a tiny bin helper:
  `async fn run_blocking<T:Send+'static>(f: impl FnOnce()->T + Send + 'static) -> T { spawn_blocking(f).await.expect("blocking task panicked") }`

### 2.2 Include / exclude (verified)
**IN (blocking on a live async runtime):**
1. `recover_orphans` in the serve **hot-reload watcher** (main.rs:6030-6040 — `tokio::spawn`ed,
   runs on every config change while serve is live; calls blocking reaper docker CLI). Fix:
   `run_blocking(move || recover_orphans(&snap2,…)).await;` **then** `reg.apply(snap).await`
   (preserves recover-before-apply ordering).
2. `cleanup_failed_add`'s `std::fs::remove_dir_all(wt)` (host_git.rs:40 — async per-turn
   `configure_session` path). Fix: `tokio::fs::remove_dir_all(wt).await` (tokio::fs is
   spawn_blocking under the hood).

**OUT (evidence in §2.2 of Fable's doc):** serve/mcp BOOT sweeps (one-shot, empty runtime, before
bind); all CLI one-shot commands (implement/implement_resume/run_workflow-local/containers/doctor/
config/merge — parked worker has no victim); micro-blocking `std::fs` metadata syscalls;
`WorktreeProvider`/`WorktreeBackend` (already async).

### 2.3 Hazards
- Cancellation: spawn_blocking runs to completion even if the awaiter drops. Both IN items are
  best-effort/idempotent (recover_orphans documented idempotent; cleanup_failed_add discarded) —
  safe. No cancellation-needing op moves to the pool.
- Ordering: recover-before-apply preserved by awaiting inline (never detached); watcher is serial.
- env/cwd: spawn_blocking is in-process (full env; not the containerized-MCP-env-trap class); git/
  docker argv use absolute paths / `-C`.
- Panic: `.expect` restores today's panic-propagation.

### 2.4 PRs: TWO. **PR-B2 first** (~20 lines: helper + hot-reload wrap + `tokio::fs` + boot comments).
**PR-C** second, 2 commits (commit 1 = outbound crate: sse.rs + SendOpts/StreamingReply/ClientError
+ loopback/rpc/subscribe_sse + re-express send_streaming/open_stream + tests; commit 2 = bin swap +
deletes + Cargo dedupe + parity tests). Out of scope both: bridge-a2a-inbound (golden_wire must stay
green), PeerDelegation semantics, CLI terminal rules/exit codes, testpeer export, reaper/sweep async.

## 3. GUARDRAILS
- **G-C1 (wire):** request structurally identical modulo EXACTLY: +messageId, **+role:"ROLE_USER"**
  (codex #3 — `a2a::Role::User` serializes `"ROLE_USER"`, NOT `"user"`, a2a-lf types.rs:58; server
  ignores role, reads raw Value), parts[0] loses `kind:text`, JSON-RPC id, key order. Preserve+assert: method
  "SendStreamingMessage", contextId, fresh taskId, both metadata keys, single text part,
  A2A-Version 1.0, NO Authorization.
- **G-C2 (terminal parity):** keep the CLI rules (message.is_none() gate; Rejected NOT terminal;
  read-to-EOF; artifacts concat in arrival order; bad frame = hard error; exit map). Do NOT
  substitute open_stream's rules.
- **G-C3 (timeout):** loopback client sets NO total timeout. The copy-paste
  `A2aClient::new(url,"",30s)` is the expected wrong impl — passes fast tests, kills real
  workflows. Regression test required (T-C6).
- **G-C4 (peer path):** send_streaming/open_stream/cancel signatures + header + BridgeError + SSE
  mapping unchanged; existing client.rs tests pass UNMODIFIED; PeerDelegation untouched.
- **G-C5:** shared SSE decoder = spec framing; assert parity on the emission shapes serve actually
  produces (single-line data per event).
- **G-B1 (Drop-sync):** nothing reachable from a Drop impl becomes async/spawn_blocking-dependent.
- **G-B2 (ordering):** recover completes before reg.apply; wrap awaited inline, never detached.
- **G-B3 (worktree semantics):** cleanup_failed_add sequence (remove then prune) + add-retry loop
  + Reserving state machine unchanged; only remove_dir_all → tokio::fs.
- **Non-goals:** no new streaming semantics, no auth feature, no client retry, no async
  WorktreeProvider redesign, no perf work beyond the named offloads.

## 4. ACCEPTANCE / TEST CRITERIA
**Verify triad every PR:** `cargo fmt --check` && `cargo clippy --workspace -j 1` &&
`cargo test --workspace -j 1` (`-j 1` mandatory — linker OOM).

**C:** T-C1 request parity (extend `serve_client_builds_streaming_message` + `..._distinct_task_ids`;
wiremock: A2A-Version present, Authorization ABSENT). T-C2 the four SSE/exit characterization tests
pass UNMODIFIED. T-C3 new bin tests: (a) "A"+"B"+Completed→"AB"; (b) artifact+EOF-no-terminal→"stream
ended without terminal status"; (c) Completed WITH message → not terminal → same as (b); (d)
undecodable frame → "bad SSE data frame". T-C4 outbound: existing client.rs tests green unmodified +
new sse.rs unit tests (single/multi-line data, CRLF, id capture, comment/event ignored, EOF flush) +
rpc()/loopback() tests (version header sent, Authorization None). T-C5 residual-ref gates: non-test
`grep -c 'a2a::' main.rs` ≤ 19; ZERO for
`SVC_PARAM_VERSION|a2a::VERSION|a2a::new_|methods::SEND_STREAMING_MESSAGE|reqwest::Client`;
golden_wire.rs untouched. T-C6 timeout regression (loopback stall > would-be-default). T-C7 owner
live gate: serve + run-workflow --serve + task watch (loopback, no containers).

**B2:** T-B1 offload evidence — `run_blocking` on a current_thread runtime concurrent with a ticker,
assert ticks progress during the blocking sleep. T-B2 all worktree tests green unmodified (esp.
`unborn_head_add_errors_cleanly` which exercises cleanup_failed_add). T-B3 ordering by inspection
(await inline). T-B4 mechanical: `spawn_blocking` ≥1; `std::fs::remove_dir_all` in host_git.rs = 0.

## 5. Reviewer/implementer flags
1. Server `"task-1"` stub → keep client-side taskId minting (distinct-taskId test is the tripwire).
2. `a2a` = `a2a-lf` pinned `=0.3.0`; re-verify P3 serde claims if the pin moves.
3. open_stream treats Rejected terminal; CLI does not — preserved by design, don't "fix" inside C.
4. AlwaysGrant is the only prod AuthMiddleware — no-auth-header ruling is functionally moot today.
5. Micro-blocking std::fs left in async worktree paths — deliberate; one-line tokio::fs swaps if
   ever profiled.
6. implement's warm-loop blocking git — OUT now; first place to revisit if implement becomes
   serve-resident.
7. Parity tests compare serde_json::Value, never strings (serde_json Map = BTreeMap).

**Confidence:** B2 premise 98 · C seam 90 · timeout trap 95 · allowed-delta 93 · residual target 90
· two-PR split 85.

## 6. Codex xhigh design review — 5 findings FOLDED IN (opus-verified; supersede above where conflicting)
Architecture ENDORSED ("mostly matches the source"); the following tighten the C guardrails/tests
+ one fact fix. All verified against source.
1. **(MAJOR) `rpc()` HTTP-400 body contract** — folded into §1.2 (parse body regardless of status).
   **Add T-C8:** a wiremock test where the server returns HTTP 400 with a JSON-RPC `error` body for
   an invalid-params call → `rpc()` returns the parsed `Value` (with `error`), NOT a `ClientError`;
   the bin surfaces the JSON-RPC error message unchanged.
2. **(MAJOR) `task watch --from` under-tested** — **strengthen T-C7:** the live gate MUST exercise
   `task watch --from <id>`; **add a wiremock/unit test** asserting the `Last-Event-ID` header is
   sent when `--from` is set (server consumes it as the replay cursor, server.rs:880) and that
   `SseEvent.id` updates the printed resume hint on close.
3. **(MAJOR/fact) role serializes `ROLE_USER`** — folded into G-C1. **T-C1 asserts `role ==
   "ROLE_USER"`**, not `"user"`.
4. **(MINOR) T-C5 grep gate counts the test module** — the criterion is: count `a2a::` occurrences
   ONLY before the test module (`bin/a2a-bridge/src/main.rs` `#[cfg(test)]` at :6242) — e.g.
   `head -n 6241 main.rs | grep -c 'a2a::'` ≤ 19 (and the hard-zero greps likewise scoped to
   non-test), NOT a raw whole-file `grep -c`.
5. **(MINOR) T-B4 same scoping** — assert `std::fs::remove_dir_all` is absent specifically in
   `cleanup_failed_add` (or scope the grep to non-test lines); tests legitimately call it
   (host_git.rs:216/232).

**Verdict folded:** ready to implement with §6 applied. No architectural change — C's seam, B2's
scope, the two-PR split, and the guardrails stand; §6 tightens tests + fixes the role fact + pins
the rpc-400 contract.
