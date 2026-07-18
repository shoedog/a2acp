# a2a-bridge — Improvements & New-Features Roadmap (proposal)

**Created:** 2026-07-17
**Status:** non-normative proposal. This complements — it does not replace —
[`roadmap.md`](roadmap.md) (the priority cursor) and
[`reliability-execution-roadmap.md`](reliability-execution-roadmap.md) (the active P0 program).
Per project convention, GitHub Issues is the canonical intake; items here are linked into
[`roadmap.md`](roadmap.md) only when actually scheduled. Issue stubs for the near-term items live in
[`roadmap-issues/`](roadmap-issues/).
**Scope:** everything *other than* the reliability program (R0–R4), which is owned by its own track.

**Source basis:** current code (`~99.5k` LOC, 16 crates, v0.2.1, AGPL-3.0), the maintainer's own
[`2026-07-03-strategic-analysis.md`](2026-07-03-strategic-analysis.md) (M1–M5 / L1–L10),
[`analysis-second-opinion.md`](analysis-second-opinion.md),
[`orchestration-improvements-2026-06-17.md`](orchestration-improvements-2026-06-17.md), the 33 ADRs under
[`adr/`](adr/), [`m4-observability-roadmap.md`](m4-observability-roadmap.md), and the `SSOT_AGENTS_BRIDGE_COORDINATION.md`
cross-repo request (a working-tree coordination doc, not tracked on `main`).

---

## How to read this

**Detail is front-loaded.** Near-term / higher-priority items carry full value + design detail.
Detail tapers with distance; 6+-month items are one-liners by design (they will be re-specced when they
approach, and the ecosystem they depend on will have moved).

**Horizon calibration.** Velocity is ~430 commits / 30 days, single-maintainer, agent-assisted, but every
substantive slice runs the full spec → dual-adversarial-review → live-gate loop, so *reviewed* throughput is
roughly one non-trivial slice every few days to two weeks. Horizons below are expressed in that currency,
and every near-term horizon assumes it competes with the reliability P0 for the same review budget.

**Relationship to the reliability program (Track A — in progress, not owned here).**
R3d (owner-bound scheduled canaries) is starting; R3e (OpenRouter) and R3f (OpenCode) follow; R4
(reproducible release + promotion gate) closes the program. Several items here have a hard
**"resume-after"** dependency on Track A deliverables (the compatibility matrix, phase-specific errors,
pinned/floating lanes, the smoke harness) — those dependencies are called out per item. Nothing in this
roadmap should merge changes into the reliability slices' scope.

**Priority legend:** ★★★ do-next-after-reliability-core · ★★ high-value mid-term · ★ opportunistic / gated.

---

## Horizon 0 — Finish-line & cheap debt (interleave now; hours-to-days each)

These are low-risk, high-leverage cleanups that either close a *stated* gap or remove a source of future
agent confusion. They can slot between reliability slices without competing for the heavy review budget.

### H0-1 ★★★ Reconcile docs with the shipped Coordinator + controller extractions
**Value.** The two largest deferred items in the July-3 strategic analysis — #10 (make `InboundServer` a
thin adapter over `Coordinator`) and #9 (extract the ~6.4k-line implement/review/tweak/merge/verify loop
into a `bridge-controller` library) — have **substantially shipped in code** (`InboundServer::from_coordinator`,
the `#10 slice 7` comment, `bridge-a2a-inbound` re-exporting the coordinator's session manager;
`crates/bridge-controller` with pure loops + ports). But `README.md`'s "Known limitations" still says the
inbound server "has not been migrated onto Coordinator," and the crate table omits `bridge-controller`. Doc
drift is already flagged as a *systemic* risk (onboarding contradicted ADR-0025; ADR-0031 contradicts shipped
MCP code). Stale docs actively mislead the next agent — expensive when agents plan from them.
**Design considerations.** (a) Update README architecture + crate table + limitations, `onboarding.md`, and
mark ADR-0031's "no in-container MCP for claude/kiro" note as superseded (code now wires the ACP delivery
path). (b) Add a `validate --repo-hygiene` sub-check that flags ADRs/handoffs whose claims contradict a
grep-able code marker, so drift is caught mechanically rather than by memory — this is the durable fix, the
one-time doc edit is the stopgap. (c) Don't overstate: the migration is *structurally* done but not *clean*
(see H0-2), so describe it as "adapter-over-Coordinator, accessor cleanup pending," not "complete."

### H0-2 ★★ Close the Coordinator `*_ref()` accessor leak (finish #10 cleanly)
**Value.** `Coordinator` exposes ~20 `*_ref()` accessors handing out `Arc<Mutex<HashMap<…>>>` internals
(bindings, cancel maps, progress hubs) to the adapters. So "thin adapter over one service API" leaks mutable
shared state through handles instead of methods — the exact coupling the migration was meant to remove. Left
alone, every new adapter (H3 federation, H3 team-mode, future MCP tools) re-couples to internals and the
"one stable service API" guarantee erodes.
**Design considerations.** Inventory each accessor's real call-site intent and replace it with an intent-named
`Coordinator` method (e.g. `cancel_turn(ctx, gen)` instead of handing out the cancel map). Do this
incrementally, one accessor family per change, each behind the existing coordinator-migration live-gate
(boot→submit→restart→resume, mid-turn force-reset, warm cancel — recorded in
`evals/2026-07-05-coordinator-migration-livegate.md`). Watch the warm-path claim-state invariants
(`Idle/Running/Resetting/Compacting`) — this is where the migration was deferred because it's bug-prone.

### H0-3 ★★★ Enforce `allowed_cwd_root` on the local `run-workflow` path
**Value.** A genuine security *under*-enforcement gap the project has self-diagnosed: the served HTTP/A2A
path gates target cwd against `allowed_cwd_root`, but local `run-workflow` does **not**. Any workflow run
locally can target an arbitrary repo. Small change, real containment win, and it removes an asterisk from the
Tier model.
**Design considerations.** Route local `run-workflow` cwd through the same gate the coordinator applies; the
seam already exists (the gate currently lives on the adapter, and `build_coordinator` passes `None` for
`allowed_cwd_root` in serve). Decide the default: fail-closed with an explicit `--allow-cwd-root` escape hatch
for the trusted-own-repo case is consistent with ADR-0032's "under-enforcement is the risk" stance. Add a
regression that a local run outside the root is refused.

### H0-4 ★ Repo hygiene: evict root scratch binaries; decide `lsp-mcp`'s home
**Value.** `check_boxerr` / `check_okor` / `check_pathbuf` / `check_vecstr` are stray ~440 KB binaries at the
repo root — noise the hygiene gate exists to prevent. Separately, `crates/lsp-mcp` (~1.5k LOC, its own
binary) has **zero bridge dependencies** — it's an orphaned co-tenant that inflates the workspace build (a
contributor to the `cargo test -j1` OOM). Cheap to remove; the LSP-home decision is a small architectural call.
**Design considerations.** Delete the scratch binaries (git-ignore the pattern). For `lsp-mcp`: either
(a) move it to its own repo and consume the built binary via `[[languages]].lsp_env` (cleanest — it's already
delivered as a standalone binary into the toolchain container), or (b) keep it but document it as an
independent tool. Option (a) also shrinks the static-link surface that forces `-j1`. Coordinate with the
in-container-nav story (ADR-0031) since `implement`'s verify container bundles the `lsp-mcp` binary.

---

## Horizon 1 — Near-term, highest priority (next 1–3 months, post-reliability-core)

These are the four I'd put first once the R3 core lands. Each is high-value, builds on a seam that already
exists, and is either already-designed or a "productize what's already measured" play. Issue stubs:
[`roadmap-issues/`](roadmap-issues/).

### H1-1 ★★★ M4 Slice 3b — TTL retention (bounded storage)
**Status.** *Designed and signed off, not implemented.* Design of record:
`docs/superpowers/specs/2026-07-10-m4-slice3-retention-design-rev6.md` (6 adversarial revisions); resume
checklist in [`m4-observability-roadmap.md`](m4-observability-roadmap.md). Slice 3a (retention *safety*
foundation — ownership, finalization barriers, recency, DDL-only migration, **no deletion**) already merged
via PR #19.

**Value.** This is the single most concrete unbuilt operational item. **The SQLite store grows without bound
today** — `task_journal`, `task_node_checkpoints`, `turn_log`, and artifacts accumulate forever. For a service
the operator now runs long-lived (a shared operator on `127.0.0.1:18080` is already a standing deployment),
unbounded growth is a when-not-if failure. The design is done and the safety substrate is merged, so this is
high value at comparatively low residual risk — mostly execution against a reviewed contract.

**Why it sits in H1 not H0.** [`roadmap.md`](roadmap.md)'s explicit resume rule gates 3b behind the
reliability program delivering the smoke harness, compatibility matrix, phase-specific errors, and
pinned/floating lanes — i.e. it resumes *after* the R3 core, which is exactly the H1 window.

**Technical design considerations.**
- Implement the rev6 contract *as written* — it supersedes older sketches. Only `[storage].artifact_retention_days`
  (default 14, 0=off) is valid; the older `artifact_retention_max_bytes` / `purge_terminal_tasks_days` knobs
  must be **rejected** (`deny_unknown_fields`). Do not reintroduce size eviction / DB ceilings / VACUUM /
  redaction / OTLP — all explicitly out of scope for 3b.
- Enforce the mandatory 24-hour wall-clock floor; run one bounded boot pass **before** the Prometheus
  counter rebuild (so rebuilt counters don't double-count rows you're about to delete), then bounded hourly
  sweeps (`RetentionService`, batch cap 10,000), all under `BEGIN IMMEDIATE`.
- Single source of truth for eligibility: the `retention_artifact_eligibility` SQLite view drives both
  candidate listing *and* the delete-time re-check; mirror it in the memory store for parity. **Never delete
  `tasks` rows.** Retain pending/ambiguous legacy `NULL task_id` rows.
- Route semantics: current row → 200; absent row on a task with the set-once `artifacts_purged_at` marker →
  **task-level 410**; unknown → 404. Per-artifact 404-vs-410 precision is an accepted non-goal (owner decision).
- Fold the three 3a carry-ins: task-level 410 contract, closing the read-to-commit recency window
  (commit-adjacent clock read or fail-closed, with a stalled-writer regression), and re-reading the purge
  marker after an absent body (purge-between-reads race tests).

### H1-2 ★★★ Cost & quota governance (productize the cost data already captured)
**Value.** `turn_log` already records `tokens` and `cost` per turn, and `bridge_turn_cost_total` /
`bridge_turn_tokens_total` metrics already exist — but **nothing enforces a budget**. The self-diagnosed
risk is blunt: "a misconfigured batch can burn a day of quota." A single fan-out or `run-batch` with a bad
concurrency setting is an unbounded spend. This is a high-leverage item precisely because the *measurement*
substrate is already built (Slice 1) — this is enforcement + surfacing on top of existing columns, not new
plumbing. It also composes with the reliability program's cost-bound concerns (R3 canaries already carry cost
budgets and USD accounting).
**Technical design considerations.**
- Two layers: (1) **admission budgets** — extend the batch semaphore (`crates/bridge-coordinator/batch.rs`,
  `[batch]`) and the fan-out path with per-run / per-time-window cost ceilings; refuse or queue admission when
  the projected spend exceeds headroom, mirroring R3's "prospective budget headroom at admission and again
  after hashing" pattern so the two accounting models stay consistent. (2) **visibility** — surface per-node
  token/cost in `run-workflow --out` and `task get` (today evals gate on turn *count* because dollar usage
  isn't surfaced on that path).
- Cost truth is provider-dependent and sometimes absent (recall the R3 incident where cost was observed as
  zero, and the sticky non-finite/non-USD-cost handling R3b added). Reuse that sticky, fail-closed cost
  serializer rather than inventing a second one; treat unknown cost conservatively (block or warn, per policy)
  rather than as zero.
- Keep it advisory-configurable: a hard ceiling that aborts vs. a soft warn threshold
  (`warm_usage_warn_fraction` already exists as prior art for soft thresholds).
- Wire-shape caution: exposing task usage on A2A `tasks/get` is a wire-shape change deferred in M4 — decide
  whether cost surfacing rides that change or stays operator-side (metrics + journal) first.

### H1-3 ★★★ Review-quality eval harness expansion (M3)
**Value.** Self-hosted multi-agent review *is* the product thesis, and it is almost unmeasured: the
`evals/` harness grades only the `code-review` workflow, over a 15-item seeded-defect corpus, last run
2026-07-04, and it does **not** yet consume the `turn_log` eval columns (`prompt_id` / `model` / `effort`)
that Slice 1 built *specifically* to enable per-cell analysis. Without this, every prompt / model / effort /
depth decision is a guess, and there's no regression signal when upstream agents drift — which is the entire
premise of the reliability program. This is the measurement layer that makes every other quality decision
data-driven.
**Technical design considerations.**
- Expand corpora beyond `review-seeded-v1`: add design-review and implement-outcome tasksets, and a
  judge-quality eval (who watches the kiro judge?). Keep the blind cross-family judge discipline and the
  family-overlap guard that already exist.
- Wire the harness to `turn_log`: GROUP BY `prompt_id`/`model`/`effort` to compute catch-rate, precision,
  and false-finding rate per cell — the substrate exists, no consumer does. This closes the loop between the
  metrics investment and actual decisions.
- Make it schedulable but never automatic-in-CI (token spend). A nightly or weekly run against pinned agents
  becomes the **quality canary** that complements the reliability program's *compatibility* canary — drift in
  review catch-rate is as important as drift in "does the agent still start."
- Budget discipline is already good (`--cap` on turn count, `--smoke`, `--dry-run`); extend the cap to dollar
  budgets once H1-2 surfaces cost.
- Field note to design around (FN-1): read-only reviewer `cargo test` stalls at `_dyld_start` on macOS, so
  reviews are code-trace-verified not run-verified — an eval that depends on the reviewer running tests needs
  the dedicated non-RO build/test step or a containerized-Linux verify.

### H1-4 ★★ serve lifecycle & operator ergonomics
**Value.** There is a live, cross-repo request for exactly this (the `SSOT_AGENTS_BRIDGE_COORDINATION.md`
working-tree coordination doc): a second project wanted a bounded delegation route and hit the fact that
`serve` is a **bare foreground process** — no daemonization,
no PID/owner record, no auto-port selection, no `/health` endpoint, and no way for a client to verify *which
config* a bound server is running (HTTP 200 on the Agent Card "cannot distinguish two differently configured
bridge servers"). The operator is compensating with hand-written `SERVICE.md` conventions, an
ownership-ledger-by-context-id, and a creds-refresh launchd plist that hardcodes a checkout path. This is
friction *today*, not hypothetically.
**Technical design considerations.**
- **Sequence deliberately** (the maintainer's stated preference): ship *identity + preflight* before any
  daemon-lifecycle machinery. Concretely: (1) add a config-fingerprint to the Agent Card so a client can
  confirm it's talking to the intended server; (2) add a `submit`/client preflight that checks that
  fingerprint and the bound config; (3) add a real readiness signal (a `/health` endpoint or bless the
  existing `GET /.well-known/agent-card.json` as the contract). Only *then* evaluate a supervised
  launchd/systemd contract with a PID/owner record — introducing daemon machinery is a separate, later call.
- **Do not** let a client infer ownership from an occupied port, a tmux name, or a stale PID — the coordination
  doc is explicit that clients must not opportunistically start/replace/kill `serve`. Encode an explicit
  server-owner contract instead.
- Generalize the ownership ledger the operator is hand-running (context-id acquire/release so a coordinated
  rebuild can wait for an empty ledger) into a first-class, queryable serve concept — this is the seed of
  H3 team/multi-user mode, so design the identity model with that in mind.
- Fix the creds-refresh plist's hardcoded path (`/Users/wesleyjinks/code/a2a-bridge/...`) as part of this —
  it's a deployment-portability bug.
- File the accepted work as GitHub issues (canonical intake) and link into the reliability roadmap only when
  scheduled, per the coordination doc's own rule.

---

## Horizon 2 — Mid-term (roughly 3–6 months; medium detail)

### H2-1 ★★ Merge Mode B + same-target write locks (data-safety)
**Value.** ADR-0027 ships Merge Mode A (fast-forward `commit-tree` + `push --force-with-lease`) and records
that it "is not production-resilient under heavy concurrency"; ADR-0025 explicitly **defers same-target write
locks**, so two concurrent write-capable runs can mutate one repo. This is a correctness/data-safety gap, not
an ergonomic one. It's H2 rather than H1 only because single-operator usage rarely hits concurrent same-repo
writes *today* — its priority rises sharply the moment batch `implement` or team mode (H3) lands.
**Design considerations.** Add Mode B (`--as-branch` staging branches) as the concurrency-resilient path;
add a per-`<id>` advisory lock (SQLite run registry or flock, consistent with ADR-0025's existing lease
mechanism) so resume/merge and concurrent writers serialize on the target. Decide auto-replay-on-StaleLease
policy (currently deferred). Regression: two batch children targeting one repo must not interleave commits.

### H2-2 ★★ Composable isolation stack + warm container pool for writers
**Value.** Today "containerized + worktree" and "watchdog on a containerized agent" are impossible
combinations (M1), and every per-turn `container_rw` outside `implement` pays a 0.5–3 s cold-start. A
decorator stack (watchdog-as-decorator, container-rw-as-decorator, backend builder) plus a warm writer pool
(leases + idle expiry — scaffolding exists, deferred since ADR-0018) removes both limits.
**Design considerations.** The decorator pattern is already proven (`WorktreeBackend` decorates any backend);
generalize it into a composable builder so isolation layers stack. For the warm pool, reuse the `implement`
warm-session state machine (ADR-0024) but add lease ownership + idle expiry + the concurrency-safety
guarantees from the R2b warm-cleanup work (the reliability program has already paid down enormous warm-path
concurrency debt — build on that, don't re-derive it). Watch the watchdog blast-radius constraint: SIGKILL on
a shared process-backed ACP backend takes sibling turns down, so watchdog stays safe only on per-turn-isolated
agents — the decorator must encode that.

### H2-3 ★★ Structured / JSON workflow output (W2)
**Value.** ADR-0012 deliberately did *not* build structured output, with a clear re-trigger: "a real
deterministic consumer (CI gate, dashboard, SARIF API)." H1-3 (evals wanting machine-readable findings) and a
future CI-review integration *are* that consumer. Structured review findings unlock SARIF export → GitHub code
scanning, programmatic gating, and dashboards.
**Design considerations.** Follow the ADR-0012 doctrine — structure only at the deterministic boundary,
per-workflow, via a dedicated structuring node + constrained decoding on the `kind=api` backend, not a generic
"make everything JSON" layer. Add a small schema registry + validation at that node only. Start with
`code-review` → SARIF as the first concrete consumer so the design is pulled by a real format, not speculative.

### H2-4 ★★ OTLP / OpenTelemetry exporter (consume the traceparent that's already plumbed)
**Value.** The bridge already parses and persists W3C `traceparent` on every turn, and carries 4 correlation
ids in `task_span()` — but there is **no consumer**: it's dead-end data. An OTLP exporter turns the existing
lifecycle events + turn spans into distributed traces viewable in any standard backend, which is what makes a
multi-agent workflow debuggable across nodes.
**Design considerations.** Add an `opentelemetry` adapter behind the existing `Observer` seam
(`bridge-observ`), gated off by default like `/metrics`. Emit a span tree per workflow run (run → node →
turn) using the already-stored ids. Keep it additive — the metrics deferral ledger lists "OTLP exporter / span
tree" as explicitly out of Slice-1/2 scope, so it's a clean new adapter, not a refactor. Consider a separate
metrics/trace bind port (also on the deferral ledger).

### H2-5 ★ Bidirectional clarify channel (B1)
**Value.** Today an agent that needs clarification mid-run can't ask — it guesses or fails, wasting a
(possibly billable) run. A `question`/`flag` channel where the agent emits and waits for an operator answer
mid-turn makes long autonomous runs far more robust.
**Design considerations.** The permission/`request_permission` reverse channel + `session permit` machinery is
the natural substrate — a clarify request is structurally a permission-suspend with a free-text payload. Reuse
the resumable-suspend path the Translator already implements for permission denials. Decide timeout/default
behavior (the permission path already has `permission_timeout_ms`).

### H2-6 ★★ Release & supply-chain hardening (M2 completion)
**Value.** Releases build for 3 targets and extract notes from the changelog, but there's no signing
(cosign/minisign), no SBOM, no dependabot/renovate, and no scheduled `cargo-deny`/audit — so a new RUSTSEC
advisory between commits goes unnoticed, and release artifacts are unverifiable. For an AGPL tool with a
CLA that already names `dependabot[bot]`, this is low-hanging credibility + safety.
**Design considerations.** Add dependabot/renovate + a scheduled `cargo deny check advisories` job; add
artifact signing + `SHA256SUMS` signature; generate an SBOM (cargo-sbom/syft). The **reproducible container
image** gap (transitive npm + kiro resolve floating at build time; no lock manifest / pinned digest) overlaps
directly with reliability R4 (reproducible dependency/image pins + promotion gate) — coordinate so this and R4
don't double-build the same machinery. Keep `multiple-versions` moving toward `deny` once the tree allows.

### H2-7 ★ Dev-loop: fix the `cargo test -j1` OOM (test-binary consolidation)
**Value.** `cargo test --all-targets` OOMs and forces `-j1` because the monolithic bin + ~29 integration-test
binaries each statically link the workspace + bundled SQLite. This slows every contributor and every CI-like
local gate.
**Design considerations.** Consolidate integration-test binaries (a single harness binary with modules, or
fewer larger ones) and/or split the bin (overlaps H3-4). The strategic analysis flags "verify whether
test-binary consolidation actually cures the OOM" as research-first — benchmark before committing to the
restructure.

---

## Horizon 3 — Further out (roughly 6–9 months; lighter detail)

### H3-1 ★★ Unify the two orchestration engines (L5)
The TOML workflow DAG and the Rust-coded `implement → review → tweak` loop are two engines with separate
resume stories (ADR-0024 records this as a HARD CONSTRAINT: workflow features don't reach edit/fix turns).
Unifying — the implement loop as a workflow primitive with cyclic graphs / conditional edges, an executor
running on supplied warm sessions — gives one resume story and kills a large duplication surface.
*Value:* removes the biggest structural fork in the codebase. *Design:* big and risky (rated "L5", codex
confidence 93 that it's viable); needs its own spec → dual-review → live-gate loop; do it only after the warm
pool (H2-2) and controller cleanup (H0-2) stabilize the pieces it would unify.
See the runtime-workflows + Agent/Provider-split RFC ([`rfc-agents-workflows.md`](rfc-agents-workflows.md),
[diagram](rfc-agents-workflows-diagram.html), and [Part II — memory/delegation](rfc-agents-workflows-part2-memory-delegation.md)),
which concludes those capability shifts are **orthogonal to L5** and can land first; it also surfaced a
Phase-0 registry defect ([issue #35](https://github.com/shoedog/a2acp/issues/35)).

### H3-2 ★★ A2A federation v1 (L1)
Signed Agent Cards, per-caller identity (not just a forwarded bearer), TLS/authz, multi-tenancy. Called out
as "the blocker the moment a second machine appears." *Value:* the gate to any networked/multi-host use.
*Design:* keep the A2A boundary clean so this is an outbound-client + inbound-authz addition, not a rework;
per-entry AgentCards + JWT/mTLS are already seamed-for but unimplemented.

### H3-3 ★★ Team / multi-user mode (L7)
Per-caller identity, quotas, shared warm pools. Builds directly on H1-4's ownership ledger, H1-2's budgets,
and H3-2's identity. *Value:* the step from personal power tool to shared service. *Design:* the identity
model from H1-4/H3-2 must land first; quotas extend H1-2's admission budgets per-caller.

### H3-4 ★ God-file decomposition + typed CLI parser + `AgentBackend` trait split
`server.rs` (12.9k), `main.rs` (9.4k, hand-rolled subcommand parsing for 18 commands), `acp_backend.rs`
(6.9k), `config.rs` (4.1k, schema+loader+validator+prompt-registry+watcher in one). The `AgentBackend` trait
is a 10-method kitchen sink. *Value:* maintainability + the typed-parser cleanup also helps the test OOM.
*Design:* opportunistic, incremental, behind existing gates; decompose `AgentBackend` into capability traits;
adopt clap in the bin (clap already used in `lsp-mcp`).

### H3-5 ★ Web / TUI ops panel (L6)
Permission approvals + cost dashboards + run status. *Value:* the interactive surface for the `session permit`
/ budget / trace data that already exists server-side. *Design:* server-side seams (permission suspend/resume,
`/metrics`, drill-down routes) already exist — this is a client, keep orchestration out of it.
A concrete design with rendered mockups now exists: [`operator-ui-design.md`](operator-ui-design.md) (TUI-first,
web→Grafana, mobile as a primary conversing-first client over Tailscale), with mockups in
[`operator-ui-mockups.html`](operator-ui-mockups.html) (TUI cockpit) and
[`operator-ui-mobile-mockups.html`](operator-ui-mobile-mockups.html) (mobile app).

### H3-6 ★ Linux rootless podman + per-language verify hardening
ADR-0030 validated only macOS `podman machine`; Linux rootless (uid/SELinux `:z/:Z`) is a separate spike.
Verify-container cache volumes are never reaped (disk growth); `--verify-strict`/`--no-verify` and per-language
verify locks are deferred. *Value:* portability + disk safety. *Design:* per ADR-0030/0020 deferrals.

### H3-7 ★ Smaller orchestration UX wins (A4 / E4 / E5 / C5)
Context-budget exposure + `compact`/`clear` ops (A4), content-hash-gated repo-read caching (E4),
dry-run/plan-only mode (E5), transcript fetch (C5). *Value:* each is a modest ergonomics win; several are
cheap enough to pull earlier if a slice has slack. *Design:* mostly additive on existing session/journal
surfaces.

---

## Horizon 4 — 6+ months / speculative (one-liners; re-spec when they approach)

- **gRPC A2A binding (L2)** — wait for `a2a-rs` to stabilize the binding; don't front-run the spec.
- **ACP-registry-aware `agents discover/install` (L3)** — the ACP registry launched Jan 2026; integrate once it's proven.
- **Remote ACP transports (L4)** — don't front-run the ACP spec's transport story.
- **Workflow packs / user-level `~/.config` prompt layer (L9)** — gated on a *second* consuming repo actually duplicating prompts; per-target-repo `tools/a2a-bridge/{configs,prompts}/` is the ruled interim pattern.
- **Pluggable non-SQLite stores** — `TaskStore`/`SessionStore` traits already abstract it; add a second impl only when scale demands.
- **Secrets broker** replacing copied creds (kills the `sync-creds.sh` staleness class); **declarative sandbox/egress policy engine**; **signed workflow-pack registry**; **external red-team loop** against the containment claims.
- **Distributed runner queue** / multi-host execution — standing non-goal until a concrete cross-host need.
- **Mesh participation (NANDA / BeeAI)** — explicit non-goal until a concrete cross-org dependency or a NANDA-grade standard exists; keep the A2A boundary clean so it's a later additive client.
- **SDK-compatibility CI matrix** — a build/test matrix across ACP/A2A SDK versions; complements the reliability compatibility matrix.
- **Visual workflow builder / traces** — UI on top of the DAG + journal.
- **Windows support** — currently unaddressed (`libc`, process-group kill, Unix paths); needs an explicit *non-goal* statement rather than silent absence.
- **fs / terminal ACP capabilities** — the bridge deliberately advertises none ("no-fs posture"); adding them requires a *new* ADR.
- **Conductor pattern partial-adopt** — full fork permanently rejected (ADR-0008); only adopt specific proxy-chain patterns if one of the five recorded re-trigger conditions fires.

---

## Cross-cutting themes (true regardless of horizon)

1. **Docs-as-code drift is a recurring, compounding cost.** Multiple analyses independently flagged
   README/ADR/onboarding contradicting shipped code. The mechanical fix (H0-1's hygiene sub-check) pays for
   itself across every future agent-planned slice.
2. **The measurement gap undercuts the core thesis.** The product is self-hosted review; until H1-3 lands, its
   quality is unmeasured and drift is invisible. Prioritize measurement (H1-3) alongside the reliability
   program's compatibility canary — they are the two halves of "is this still good."
3. **Under-enforcement, not over-protection, is the self-diagnosed security posture.** H0-3 (cwd gate),
   same-target write locks (H2-1), and the model-endpoint exfiltration accepted-risk all point the same way:
   containment is opt-in and has holes. Close the cheap ones early.
4. **Build on the reliability program's concurrency payoff.** R2b/R2c/R3 have paid down enormous warm-session,
   cleanup-ordering, and cost-accounting debt with adversarial rigor. The warm pool (H2-2), budgets (H1-2), and
   engine unification (H3-1) should reuse those primitives, never re-derive them.
