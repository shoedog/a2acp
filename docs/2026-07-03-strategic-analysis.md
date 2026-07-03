# a2a-bridge — Strategic Analysis (2026-07-03)

**Status:** Analysis / recommendation document.
**Method:** Six independent lenses, synthesized: two Opus 4.8 sub-agents (architecture; performance), three Sonnet 5 sub-agents (container posture; spec-evolution maintainability; DX + workflow/prompt management), and one codex gpt-5.5 xhigh pass dispatched **through the bridge itself** (`run-workflow`, read-only sandbox, watchdog) — dogfooding the product to analyze the product. Every load-bearing claim below was grounded in code the analyst actually read (file:line) or a command actually run. Confidence levels (0–100) are stated per claim; where lenses disagreed, the disagreement was resolved by reading the code, not averaged away — the most consequential case being the `SessionManager` checkout lock scope (§3), where the codex lens caught a real serialization point the host lens had cleared.

---

## 1. Executive summary

1. **The hexagon is real, and it held under extreme velocity.** `bridge-core` has zero protocol-SDK dependencies (verified by grep and `Cargo.toml`), the crate graph is a clean DAG, and ~1,100 commits landed in 90 days without corrupting the core. The architecture's weakness is not the design — it's that **consolidation lagged extension**: the Coordinator seam is only 2/3 realized, and ~6,400 lines of product logic accreted into the binary. *(Confidence 90)*
2. **Performance is strong where it matters and weak in cheap-to-fix places.** No per-delta DB writes, turn I/O runs lock-free, deadlock-safe batch fan-out. The fixes: one verified serialization point (`checkout_turn_inner` holds the global session lock across agent spawn/configure — codex-found, synthesis-verified), two SQLite pragmas, a `spawn_blocking` sweep, and a bin-crate split that also fixes the `-j 1` dev-loop OOM. *(Confidence 85)*
3. **The container posture is not overprotective — but it is not honest with itself.** Every load-bearing layer traces to a concrete, previously-real hole. Meanwhile the repo's own slices were mostly built with *host-run* codex under `danger-full-access` and a prompt-level "don't commit." The two-tier reality (containers for untrusted content, host-native for trusted own-repo work) is already practice; it just isn't policy. Formalize it instead of relaxing the container tier. *(Confidence 80)*
4. **The prompts/workflows question has a clear answer:** per-target-repo customizations belong in the **target repo** (`tools/a2a-bridge/{configs,prompts}/` — zero loader changes needed; versions with the code it reviews); the ~15 reusable product workflows stay here; the ~185 one-shot slice artifacts should leave `examples/`/`prompts/` (the hygiene guard freezes the clutter, it doesn't shrink it). Defer any pack/registry machinery until a second consuming repo proves the need. *(Confidence 80)*
5. **The biggest unasked question is what this project *is*.** Zero release tags, no CONTRIBUTING, a README describing the tool as it was in May, no evals measuring whether the self-hosted reviews are actually good, no cost guardrails. Whether a2a-bridge is a personal power tool or an OSS product determines half the roadmap below — and it's currently undecided by default, not by choice. *(Confidence 85)*

---

## 2. Architecture

### Strong

| Claim | Evidence | Why it matters | Conf. |
|---|---|---|---|
| `bridge-core` is genuinely pure; the anti-corruption boundary holds | 0 `agent_client_protocol` refs, 0 `a2a` type usage in core (one comment); core's `Cargo.toml` has zero protocol deps; domain `A2aState` mapped to wire `a2a::TaskState` at the boundary | This is the README's load-bearing claim and it is true — spec churn cannot reach the domain | 95 |
| Clean acyclic crate graph | core ← adapters ← coordinator/workflow ← inbound/mcp ← bin; no cycles | Refactors stay local; no distributed-monolith failure mode | 95 |
| Session/task lifecycle is centralized, not smeared | `SessionManager` (bridge-coordinator) owns claim states (`Idle`/`Resetting`/`Compacting`), generation guards, cancel tokens, inject queue; referenced elsewhere only as display strings | The hardest-won invariants (slices 3/4/9, cancel-tokens) live in one auditable place | 88 |
| Backend decoration follows a clean port contract | `WorktreeBackend`, `ContainerRwBackend` both wrap `Arc<dyn AgentBackend>`; shared warm-dispatch substrate with RAII guards | New isolation/behavior layers have an established pattern | 85 |
| Test density is a hidden strength | `server.rs` is ~64% tests, `session_manager.rs` ~70%, `config.rs` ~58%; ~1,367 test fns; ACP has golden-frame *and* captured-real-corpus wire tests | The scary line counts are mostly tests; the wire-conformance corpus caught real drift during the ACP 1.x bump | 90 |
| Batch logic is shared, not duplicated | All 11 batch ops are free fns over `BatchDeps` in one file; both Coordinator and InboundServer call them | The "implemented on both" smell is dependency-wiring duplication only — the fix is the Coordinator migration, not batch surgery | 90 |

### Weak

| Claim | Evidence | Why it matters | Conf. |
|---|---|---|---|
| The god-binary is real, and it's product logic, not wiring | bin crate = 19,017 lines; `main.rs` ~4,800 production lines / 234 fns; `implement.rs` 1,190 + `merge.rs` 973 + `review.rs` 927 + `tweak.rs` 840 + `verify.rs` 506 + `resilient.rs` 541 ≈ **6,400 lines of controller-loop features trapped in the bin** | Untestable behind no public API; unusable from MCP/other surfaces; every feature recompiles the world (dev-loop pain, see §3) | 90 |
| The Coordinator seam is only 2/3 realized — A2A was never migrated | `InboundServer` holds its own parallel `session_manager`/`task_store`/`permission_registry`/`batch`/`bindings`; reimplements warm dispatch via free fns; duplicates `batch_deps()`; the deferred "T9 handler-thinning" is admitted in a code comment | Every new turn-lifecycle feature must be built twice or reach around the seam — exactly what batch had to do | 88 |
| `AgentBackend` shows accretion rings | 10 methods, most with default impls and per-slice provenance tags in doc comments | Still thematically coherent, but each slice was absorbed rather than the trait re-segmented (e.g., a `WarmSession` capability trait) | 75 |
| Isolation layers can't stack | Container-rw is a separate `AgentKind`, not a decorator; watchdog is baked into `AcpBackend` as a config field, not a decorator; composition is hardcoded in an inline match at 2 SpawnFn sites | "Containerized + worktree" and "watchdog on non-ACP backends" are impossible today; adding a decorator means editing the composition-root match twice | 78 |
| `lsp-mcp` is an architecturally orphaned co-tenant | 4,326 lines, zero `bridge-*` deps; the bin only uses its `lang::detect` and shells out to its binary | Workspace noise + build coupling for a tool that is not part of the bridge | 80 |
| The advertised typestate is not the shipped state machine | README leads with the `Task`/`Session` typestate; README:425 admits it is "a compile-time spec artifact, not yet load-bearing" — the runtime lifecycle is `SessionManager`'s claim-state enum | Docs honesty issue more than a design issue, but new contributors will study the wrong mechanism | 85 |
| `bridge-core` is SDK-free but no longer *domain-only* | Core now also hosts sandbox composition, reapers, run identity, process helpers, MCP/profile/catalog pieces, task-store abstractions (codex lens, `bridge-core/src/lib.rs`) | The anti-corruption boundary (no wire types) held; the *scope* boundary (domain vs ops substrate) drifted — two different disciplines, only one enforced | 80 |

### Refactor / rearchitect verdicts

**Needs doing (ranked by value):**

1. **Finish the Coordinator migration** — make `InboundServer` a thin adapter holding `Arc<Coordinator>`. This is *completing* the architecture already ruled on (slice 8's "co-equal thin adapters"), not new architecture. Risk of doing: M-H (the warm-path cancel/binding invariants were historically bug-prone — use the same spec→plan→dual-review→live-gate loop that shipped the slices). Risk of not: every lifecycle feature ships twice. *(Conf. 80)*
2. **Extract the controller loops from the bin** into a `bridge-controller` (or similar) crate with a public API; leave `main.rs` as actual composition. Also the single biggest dev-loop fix. *(Conf. 82)*
3. **Make isolation composable** — watchdog as a decorator; container-rw as a decorator rather than an `AgentKind`; a backend-builder that assembles worktree ∘ container ∘ watchdog outside the match. *(Conf. 70)*
4. **Split `server.rs` by concern** (transport / dispatch / batch / workflow / reattach) — partly subsumed by #1. *(Conf. 80)*
5. **Evict `lsp-mcp` to its own repo/workspace.** Mechanical. *(Conf. 80)*

**Does NOT need doing:** a store rewrite (SQLite is the right call at this scale — see §3), a workflow-engine replacement, or reopening greenfield-vs-fork (settled, ADR-0008). The architecture needs *completion and consolidation*, not re-architecture. *(Conf. 85)*

---

## 3. Performance

Workload frame: agent turns are seconds-to-minutes and LLM-bound; the bridge must add no stalls, deadlocks, unbounded memory, or serialization points. Verdicts are judged against that, not against a web-service profile.

### Strong

- **The streaming hot path never touches the DB per delta.** The translator coalesces text (≤1,200-char chunks); journal writes fire only for rich events (plan/tool_call); durable checkpoints are per-node-completion. *(Conf. 90)*
- **The turn itself runs lock-free.** In `reconcile`/`release`/`reset` the `by_context` guard is explicitly dropped before backend `.await`s, and the agent turn (the seconds-to-minutes part) never runs under the session lock. *(Conf. 90 — but see the checkout exception below)*
- **Batch fan-out is deadlock-safe and cap-correct** — serve-wide `Semaphore` + drain-while-acquiring `select!` (the documented cross-batch-deadlock fix), per-child failure isolation. *(Conf. 85)*
- **SSE reattach is the right shape** — subscribe-before-snapshot, durable cursor, dedup floor, lag → retryable error rather than memory growth; bounded hubs (broadcast 256 / mpsc 64) at node granularity. *(Conf. 85; both host and codex lenses independently)*
- **Registry hot path is exemplary**: `ArcSwap` lock-free reads, lazy `OnceCell` spawn, explicit guard-drop-before-await discipline, lease-before-retired-check. The codex lens called it "the strongest concurrency discipline in the repo" — it is the local model the session-manager fix below should copy. *(Conf. 90)*

### Weak (with honest impact assessment)

| Risk | Evidence | Real impact | Conf. |
|---|---|---|---|
| **`checkout_turn_inner` holds the global session lock across agent I/O** — the one real serialization point | `session_manager.rs:386` takes `by_context.lock()`, then holds it across `registry.resolve().await` (which **lazy-spawns the agent process** on first use) and `backend.configure_session().await` (`:554–579`) — found by the codex lens, contradicting the host lens, **verified directly in this synthesis** | One slow agent spawn (container pull, node startup, ACP handshake) blocks *every* context checkout serve-wide — bites exactly when concurrency matters (batch, MCP, multi-context serve). Fix by copying the registry crate's own clone-out-then-drop-guard discipline; mind the claim-state TOCTOU on re-acquire | 90 |
| Single `Arc<Mutex<rusqlite::Connection>>`, **no WAL**, `synchronous=FULL`, **zero `spawn_blocking` in the workspace** | Only pragma set is `foreign_keys`; every async store method does blocking SQLite I/O on a tokio worker | Not a throughput bottleneck at current volume, but each durable-first write eats an fsync before its progress frame publishes, and batch fan-out serializes N children on one blocking connection | 85 |
| Worktree git ops block the runtime | `HostGitWorktree::add/remove` call sync `std::process::Command` inside `async fn` | `git worktree add` = hundreds of ms–seconds parking a tokio worker; once per session, opt-in — real but bounded | 80 |
| Per-turn container spawn, no amortization | `ContainerRwBackend` opens a new container per turn; warm-pool deferred (scaffolding exists) | 0.5–3 s fixed cost per turn — fine for long turns, painful for short ones | 75 |
| Unbounded per-turn event channel | `mpsc::unbounded_channel::<TurnEvent>()` between ACP handler and translator | The one truly unbounded buffer on the hot path; practically low (token-rate producers) | 55 |
| Journal never pruned; each reattach re-folds the full journal slice; unary buffers whole turns in memory | No `DELETE FROM task_journal` anywhere; `journal_fold_inputs` reads all rows per `SubscribeToTask`; `collect_turn` accumulates the full turn | Negligible today (node-granularity events, short-lived tasks); a scaling cliff with long tool-call histories or many reconnecting subscribers | 65 |
| Translator accumulation on very large outputs | Lenses disagreed: host lens — non-issue (acc capped at ~1,200 chars between flushes); codex lens — full artifact text is retained and `chars().count()/skip().collect()` churn can go quadratic on large unary artifacts | Streaming path is fine; the *unary/artifact* path is the open question. Benchmark before optimizing | 60 |
| Same-target write locks for concurrent write-capable runs are deferred | ADR-0025 explicitly defers them; two batch children (or batch + implement) targeting one repo can mutate concurrently | A data-safety gap more than a performance one — flagged by the codex lens | 85 |

### Dev-loop performance is a first-class finding

The documented `cargo test --all-targets` OOM (forcing `-j 1`) has identifiable structural causes: the monolithic bin crate (any change recompiles/links the largest serial unit), **~29 integration-test files = ~29 separate binaries each statically linking the full workspace + bundled SQLite C amalgamation**, and giant files with inlined `#[cfg(test)]`. The fixes (bin split; test-binary consolidation; optionally mold/lld) overlap almost perfectly with the architecture refactors — one effort, two payoffs. *(Conf. 80)*

### Non-issues (looked slow, don't matter here)

Per-delta `chars().count()` on a ≤1,200-char streaming buffer, node-granularity DB writes, broadcast sizing, the async fire-and-forget reaper, and the session mutex *on the turn path* (it is dropped before turn I/O — the checkout path above is the exception, not the rule). No store rewrite, no async-SQLite migration, no premature caching. *(Conf. 85)*

---

## 4. Containerized agents: overprotective?

**Verdict: No — but the posture is two-tier in practice and one-tier on paper.** *(Conf. 75)*

Every load-bearing layer traces to a concrete hole that actually existed, several found only by dogfooding:

| Layer | Verdict | Why |
|---|---|---|
| `:ro` kernel mounts | **Load-bearing** | Agent CLIs have no tool-restriction flags (`claude-agent-acp` has none at all) — the kernel RO mount is the *only* hard guarantee during "read-only" review |
| Default-deny egress proxy | **Load-bearing** (untrusted content), defense-in-depth (own code) | Exfiltration path if an injected instruction bypasses the prompt contract |
| `[sandbox]` two-layer validation (S0–S6) | **Load-bearing** | S6 (nested-`volumes` re-mounting the `:ro` repo rw) was caught by the project's own dogfooded review, not design review |
| Verify creds-XOR-egress split | **Load-bearing regardless of trust** | The threat is agent-*authored* code (LLM output nobody has read yet), not the agent's own trustworthiness |
| `implement` quarantine clone + hook neutralization | **Load-bearing** (workflow safety as much as security) | Git hooks execute arbitrary code at commit time |
| Cred discipline (isolated writable copies, never mount `~`) | **Load-bearing, near-zero cost** | — |
| Runtime allowlist (S3) | Ceremony-leaning | Guards operator typos, not adversaries |
| Reaper | Ops hygiene, not security | Closed a real 15-container leak |

**The empirical counter-evidence:** eleven `*-impl-codex.toml` configs — the ones that actually built slices 0–10 — run codex **on the host** with `sandbox_mode="danger-full-access"` and only a prompt-level "do not commit." Review configs run host-side with codex's *native* read-only sandbox. The project already operates a trusted-repo fast tier; the ADRs just never formalized it. *(Conf. 68 on "unreconciled practice" vs "deliberate unstated policy" — worth the owner confirming.)*

**Where the posture is weaker than it looks** (the real findings — the risk runs opposite to "overprotective"):

1. `[sandbox]` is entirely **opt-in per agent entry** — an entry without the block gets a name-allowlist check, not containment.
2. The host-run implementer's "no commit / no git mutation" constraint is **prompt-level only**; nothing enforces it.
3. `mount`/`allowed_cwd_root` are boot-fixed — hot-edits silently don't apply until restart.
4. `ollama cloud` bypasses the proxy entirely (documented, easy to forget).
5. `run-workflow` doesn't enforce the cwd gate (only serve+A2A does).

**Safe relaxations:** for read-only review of *your own trusted code*, host-run + agent-native sandbox (codex) or the tools-off pattern (claude) is already proven practice — formalize it as a named tier. Opening egress on trusted-repo profiles buys little (the allowlist is a one-time scripted cost). **Never relax:** write-capable agents on unreviewed repos; any content an adversary could have authored (third-party PRs, deps, issues); the verify creds split; cred discipline; the symlink canonicalization (free).

---

## 5. Usefulness and usability

**Where DX is strong:** `init` scaffolds a working config + 14 prompts + README and `validate` passes immediately (live-tested); config errors are above-average actionable (bad prompt ref → the workflow id, node id, bad value, *and* the valid list); `prompt_file` resolves relative to the config file (not CWD) — the right rule; the registry hot-reloads; unknown subcommands hard-error instead of falling through. *(Conf. 85)*

**Sharp edges, ranked:**

1. **README.md is actively misleading** — still frames the project at "Increment 3b," claims the store is in-memory-only and calls MCP/containers "deferred" (all long shipped), and never mentions `run-workflow`, `implement`, `task-spec`, `[sandbox]`, or `mcp`. It's the first file GitHub shows. Also omits 7 of 15 crates. *(Conf. 95)*
2. **`--help` is inconsistent**: 9 subcommands have real usage; `submit`, `task`, `session`, `serve`, `merge` error on `--help` with three different failure shapes. *(Conf. 90)*
3. **Bare `a2a-bridge` silently writes a config file** to CWD as a side effect of probing. *(Conf. 85)*
4. **No troubleshooting doc, no sample output, no binary distribution** (zero version tags; `cargo build` + pinned 1.94.0 toolchain is the only install path). *(Conf. 90)*
5. **No single reference for the full TOML surface** — knowledge is spread across README/AGENTS/6 docs/31 ADRs; `validate` is the de-facto schema. *(Conf. 85)*
6. **The docs contradict each other on operational rules** (codex lens): `docs/onboarding.md` still says same-config parallel container runs collide, while ADR-0025's label/lease scheme lifted that rule; ADR-0031 says container-rw ACP MCP delivery is unwired while the code now wires it for `McpDelivery::Acp`. Stale *operational* guidance is worse than stale prose — operators act on it. *(Conf. 90)*
7. **No preflight diagnostics.** The recurring first-run failures (agent not on PATH / not in `allowed_cmds`, expired creds, missing runtime, egress misconfigured, MCP env stripped) are all machine-checkable. An `a2a-bridge doctor` command was the codex lens's #2 usability recommendation (conf. 92) and this synthesis agrees — the containerized-mcp-env-trap class of bug cost real debugging days and is exactly what a preflight catches. *(Conf. 85)*

**Usefulness** (what would make it more valuable, beyond fixing the above): formalizing the trusted-repo tier (§4) removes the biggest friction from daily use; a warm container pool makes short containerized turns viable; the eval harness (§8, M3) turns "the reviews feel good" into evidence; ACP-registry-aware agent onboarding (§10, L3) removes the per-agent setup tax.

---

## 6. Maintainability as A2A and ACP evolve

**The empirical anchor:** the ACP 0.12.1→1.0.1 bump (commit `1c6e6b7`) cost **34 files, +1,076/−586, across 8 crates** — and rippled into crates that import zero SDK types (`bridge-core/orch.rs`, `session_manager.rs`, `executor.rs`, `bridge-api`). Lesson: the adapter confines wire *types* perfectly, but SDK *semantics* (effort/model handling, session lifecycle) still propagate. Budget spec bumps as slice-sized work with a live gate, never as "chore" commits. *(Conf. 90)*

**Leak audit:** `agent_client_protocol` — perfectly confined to `bridge-acp` (0 leaks). `a2a::` — confined to its two legitimate adapter homes (`bridge-a2a-inbound`, `bridge-a2a-outbound` — the latter undocumented as a sanctioned home) **except** `bin/a2a-bridge/src/main.rs`: 35 wire-type refs forming a hand-rolled A2A HTTP client that duplicates `bridge-a2a-outbound`, plus a **hand-duplicated version pin** in the bin's `Cargo.toml` that a bump could silently miss. This is the single highest-leverage leak fix. *(Conf. 85)*

**Wire-compat discipline is asymmetric:** ACP has `golden_frames.rs` (spec-referenced serialized-shape assertions) *and* `corpus_replay.rs` (captured real bytes from 4 agents replayed through the actual parse path) — both changed during the 1.x bump, proving they gate drift. **A2A has no equivalent**: no golden fixtures, no captured-peer corpus, no `A2A_WIRE_V`. `ORCH_V` versions the internal journal only. *(Conf. 85)*

**CI gaps:** coverage floors exist (≥85–90% per crate) and fmt/clippy/deny/hygiene run, but CI installs `@stable` while the repo pins 1.94.0 (latent divergence), no `rust-version` MSRV field exists, and the live-gate (real agents) is manual and unscheduled — an agent shipping a breaking ACP change is invisible to CI until the next hand-run gate. Agent-version drift is the un-covered axis: the corpus is a point-in-time capture. *(Conf. 80)*

**Sustainability of the process itself:** MEMORY.md (the project's own working memory) is 38.9KB against a 24.4KB limit and degrading; 31 ADRs + 14 handoff docs have no index; the README drifted badly in six weeks; and at least two docs now contradict shipped behavior (onboarding vs ADR-0025 concurrency; ADR-0031 vs the container MCP code — see §5). The documentation system that enabled the velocity is itself accumulating debt. A cheap partial fix: extend the existing `validate --repo-hygiene` gate with an ADR/doc staleness check (the codex lens's suggestion), since the hygiene-gate pattern demonstrably works here. *(Conf. 80)*

---

## 7. Workflow & prompt management: the ruling

Three artifact kinds are currently mixed in `examples/` + `prompts/` (~215 files):

- **(i) Reusable product workflows** (~15 files: code-review, spec/plan-review, design, panel + canonical prompts) — now wired through E8a's `[[prompts]]` registry. **Right place: this repo**, shipped as reference material with the binary that runs them. They version with the engine, and `init` copies them out.
- **(ii) One-shot dev-process artifacts** (~185 slice-N/e-N review configs+prompts) — every one targets *this repo reviewing its own development*, hardcodes this machine's absolute paths, and will never rerun. The hygiene guard (208-entry allowlist) **freezes** this set; it does not shrink it. **Right place: not `examples/`/`prompts/`** — delete them (git history preserves them) or move to `docs/history/`. The 13:1 noise ratio is the whole discoverability problem.
- **(iii) Per-target-repo customizations** — the docs already prescribe `tools/a2a-bridge/{configs,prompts}/` in the owning repo, and **the loader already supports it perfectly**: config paths resolve relative to the config file, so a target repo's config+prompts are self-contained with zero bridge changes. No example of kind (iii) exists yet in the wild — the pattern is prescribed but unvalidated.

**Ruling: hybrid (b)+(a), and defer (c)/(d).** Target-repo-owned dirs for per-repo work (option b — zero code, correct versioning semantics: a prompt referencing "this repo's acceptance criteria" must move with that repo's history); this repo keeps only kind (i); a one-time purge handles kind (ii). A separate pack repo (c) or user-level `~/.config` layer (d) each require loader machinery (search paths, pack resolution) that solves a sharing problem **no second consuming repo yet demonstrates**. When two consuming repos genuinely need the same prompts, the cheap first move is promoting that subset into kind (i) here — and only if that fails, build the search path. *(Conf. 80 on the ruling; 60 on delete-vs-archive for kind (ii) — owner's call on audit value.)*

Two lenses reached this same hybrid independently (the DX lens from the loader code, the codex lens from README/onboarding intent — both citing the config-directory-relative resolution as the deciding fact). They split only on *timing* for packs: codex would build a pack system medium-term on accumulation-pressure grounds (conf. 88); the DX lens defers it for lack of a second consumer (conf. 75). This synthesis sides with deferral — accumulation pressure is solved by the purge + target-repo pattern, and pack machinery built before a real second consumer exists would be designed against guesses — but promotes it explicitly to the long-term list (L9) with a named trigger: *build packs when the second consuming repo demonstrably duplicates prompts.*

---

## 8. What is NOT being asked

1. **What is this project?** Personal power tool, OSS product, or reference implementation? Zero release tags, no CONTRIBUTING, no distribution, version 0.1.0 everywhere. Every undecided week decides it by default. The answer reprioritizes half this document. *(Conf. 85)*
2. **Are the reviews actually good?** The entire strategic bet is self-hosted review/design quality, and the evidence is anecdotes ("the dogfooded review caught S6"). There is no eval corpus (seeded-bug regression sets), no yield measurement per agent/prompt/effort, no A/B between prompt versions. The one asset that would compound — measured review quality — doesn't exist. *(Conf. 85)*
3. **Cost governance.** Usage telemetry is plumbed (slice 2, panel weights) but there are no budgets, no per-batch caps, no quota-aware admission. A misconfigured batch can silently burn a day of subscription quota. *(Conf. 80)*
4. **Operational observability.** `bridge-observ` is 110 lines of JSON tracing. For a long-running serve executing batches: no metrics endpoint, no queue-depth/turn-latency/failure-rate counters, no dashboard. *(Conf. 85)*
5. **Agent-version drift.** Agents update weekly; the corpus is point-in-time; live-gates are manual. No compatibility matrix ("codex-acp ≥X, claude-agent-acp ≥Y known-good"), no scheduled canary run. *(Conf. 80)*
6. **The two orchestration systems.** The TOML DAG engine (workflows) and the Rust-coded controller loops (`implement`→review→tweak→merge) are separate machineries with separate resume stories. Should the loop become a workflow primitive (cyclic graphs / conditional edges), or stay code? Nobody has ruled. *(Conf. 75)*
7. **Inbound security lifecycle** — bearer token exists, but no rotation story, no TLS, no authz beyond all-or-nothing, fine for loopback; a blocker the moment a second machine appears. A2A v1.0's signed Agent Cards and multi-tenancy are exactly this gap, standardized. *(Conf. 80)*
8. **Retention** — journal and task history grow forever; no pruning, no archival policy. Trivial now, decided-by-default later. *(Conf. 75)*
9. **Windows** (or an explicit non-goal statement). `libc`, process-group kill, Unix paths throughout. *(Conf. 85)*
10. **The docs/memory system's own health** — the process that produced the velocity (handoffs, MEMORY.md, ADRs) is over its own limits and un-indexed. *(Conf. 80)*

**Needs research / evidence before deciding:**
- **A small benchmark suite** (the codex lens elevated this to a next step; this synthesis keeps it here as the gate for three decisions): cold vs warm container start, ACP handshake cost, SQLite under batch load before/after WAL, reattach fold cost vs journal length, translator cost on large unary artifacts. Every performance recommendation above marked "inferred" should be confirmed or killed by numbers before optimization work is scheduled.
- Audit whether codex's native `sandbox_mode` is trustworthy enough to be a *sanctioned* tier (it's already a de-facto one). `claude-agent-acp` has no equivalent — its tier ceiling is tools-off prompts or containers.
- **Periodic red-team validation of the containment claims** — including the inherent residual channel the egress design cannot close: an injected agent can still exfiltrate *to its own model provider* through the allowlisted endpoint (codex lens, ADR-0013 grounds). Worth writing down as accepted risk even if nothing changes.
- Whether `bridge-a2a-outbound::PeerDelegation` covers the CLI's streaming needs (prerequisite to the leak fix).
- ACP remote-transport maturity and the ACP registry API (both on the ecosystem roadmap; both would reshape agent onboarding).
- Whether an official A2A conformance suite / TCK exists to test the inbound surface against (both host and codex lenses flagged conformance-vs-local-goldens independently).
- Whether test-binary consolidation actually cures the link-memory OOM (measure before committing).
- Store lifecycle: schema versioning is additive-only today; backup/retention policy is undecided as task history becomes product data.

---

## 9. Top 10 next steps

| # | Action | Why | Cost / LOE | Benefit | Risk |
|---|---|---|---|---|---|
| 1 | **SQLite hardening: `journal_mode=WAL`, `busy_timeout`, `synchronous=NORMAL`** at connection open | Highest payoff-to-effort ratio in the repo; erases the fsync-serialize + blocking risk under batch fan-out | **S** (two pragmas + tests) | Durable-first writes stop gating progress frames; batch children stop serializing | Near-zero; WAL is the default choice for this shape |
| 2 | **Async-hygiene pass**: fix the `checkout_turn_inner` lock scope (drop the guard before `registry.resolve`/`configure_session`, re-check claim state on re-acquire — copy the registry crate's own discipline) + `spawn_blocking` for worktree git ops and boot-time docker sweeps | The checkout lock is the one verified serialization point (one slow agent spawn blocks all checkouts serve-wide); the blocking calls park tokio workers; zero `spawn_blocking` exists workspace-wide today | **S–M** | Concurrency headroom exactly where batch/MCP/serve need it | The re-acquire TOCTOU on claim states needs care — this is warm-path invariant territory, test accordingly |
| 3 | **README rewrite + troubleshooting section + sample output** | The front door is actively wrong (in-memory store, "deferred" MCP/containers) and hides 7 crates and the flagship commands | **S** | New-user path stops being misleading; onboarding <30 min becomes real | None |
| 4 | **One-time artifact purge**: move/delete the ~185 one-shot slice configs+prompts; shrink the hygiene allowlist to kind (i) | The guard freezes clutter; only a purge fixes the 13:1 noise ratio and makes `examples/` mean "examples" | **S–M** (owner sign-off on delete vs `docs/history/`) | Discoverability; the hygiene gate starts guarding a clean set | Losing casual browsability of dev history (git keeps everything) |
| 5 | **Formalize the sandbox tier model** in an ADR + config presets: Tier 0 tools-off, Tier 1 host + agent-native RO sandbox, Tier 2 container `:ro`+egress, Tier 3 container `:rw`+verify-split; document *which content classes require which tier* | The two-tier reality already exists but is unwritten; the real risk found is under-enforcement (opt-in `[sandbox]`, prompt-level-only constraints), not over-protection | **M** | Daily-use friction drops with a sanctioned fast tier; the never-relax lines get written down | Formalizing might reveal disagreements — that's the point |
| 6 | **A2A golden wire fixtures + captured-peer corpus** (`golden_wire.rs` mirroring `golden_frames.rs`) | ACP proved the pattern catches real drift during bumps; A2A (the *product surface*) has no equivalent and a-lf 0.3.0→1.x is coming | **S–M** | The next A2A bump gets the same safety net the ACP bump had | None |
| 7 | **CLI polish + `a2a-bridge doctor`**: `--help` normalization (top-level dispatcher check) + real usage for `submit`/`task`/`session`/`serve`/`merge`; stop bare-invocation writing a config file silently; add a `doctor` preflight (runtime present, agents on PATH + in `allowed_cmds`, creds valid/unexpired, egress reachable, MCP/LSP readiness) | Three different failure shapes for the same user intent; a silent file write on a probe; every recurring first-run failure is machine-checkable (the containerized-MCP-env-trap class cost real debugging days) | **S** (polish) + **M** (doctor) | Uniform discoverability; first-run failures become one command instead of a debugging session | Probe flakiness — keep doctor read-only and advisory |
| 8 | **CI/toolchain hardening**: pin CI to 1.94.0 (or read `rust-toolchain.toml`), add `rust-version` MSRV, add a plain `cargo test` job, fix the duplicated `a2a` pin in the bin's Cargo.toml | Latent CI/local divergence; a hand-duplicated version pin a bump can miss | **S** | Reproducibility; one less silent-drift channel | None |
| 9 | **Extract the controller loops from the bin** (`implement`/`review`/`tweak`/`merge`/`verify`/`resilient` → a library crate with a public API; thin `main.rs`) | 6,400 lines of product logic untestable behind no API; the largest link unit in the OOM; blocks reuse from MCP | **L** | Testability, reuse, and the single biggest dev-loop improvement | M — untangling `main.rs` helpers; do it with the established slice discipline |
| 10 | **Finish the Coordinator migration** (InboundServer → thin adapter over `Arc<Coordinator>`; delete the parallel state + duplicate `batch_deps`) | The architecture's one incomplete ruling; every lifecycle feature currently ships twice | **L** | A2A becomes co-equal with CLI/MCP as designed; future features ship once | M-H — warm-path invariants are subtle; needs the full spec→dual-review→live-gate loop |

Items 1–8 are a week-scale cleanup wave; 9–10 are the two real slices to schedule next.

## 10. Five medium-term ideas

| # | Idea | Why | Cost/LOE | Benefit | Risk |
|---|---|---|---|---|---|
| M1 | **Composable isolation stack**: watchdog-as-decorator, container-as-decorator, backend builder; then the **warm container pool with leases, same-target write locks, and idle expiry** (pool scaffolding exists; write locks are the ADR-0025 deferral) | Unlocks "containerized + worktree," watchdog on all backend kinds; pool amortizes the 0.5–3 s per-turn spawn; write locks close the concurrent-mutation data-safety gap that warm pools would otherwise widen | **M–L** | Isolation becomes a menu, not a fork; short containerized turns become viable; concurrent write runs become safe by construction | Decorator ordering semantics and lease lifecycle need care (who wraps whom; leaks/races if rushed) |
| M2 | **Release engineering**: version tags, GitHub Releases with prebuilt binaries (cargo-binstall-compatible), CHANGELOG, CONTRIBUTING | Forces the §8-Q1 identity decision; removes the toolchain from the install path | **M** | Distribution; the project becomes adoptable by non-Rust users | Maintenance expectation ratchets up — that's the decision being forced |
| M3 | **Review-quality eval harness**: seeded-bug corpora per workflow; measure catch-rate/precision per agent × prompt × effort; run on a schedule | The strategic bet (self-hosted review) is currently unmeasured; also the *only* way to know if prompt changes regress | **M–L** | Prompt/agent choices become evidence-based; marketing-grade numbers if OSS | Eval design is genuinely hard; a bad corpus measures the wrong thing |
| M4 | **Operational observability**: metrics endpoint (Prometheus/OTel), turn-latency/failure/queue-depth counters, per-task cost rollups on `session/status` | A long-running serve executing concurrent batches is currently a black box between log lines | **M** | Batches become monitorable; cost governance (§8-Q3) gets its substrate | Low |
| M5 | **Scheduled live-gate canary**: nightly CI job running the golden workflows against real current agents (codex/claude/kiro), diffing against the corpus | Agent-version drift is the un-covered failure axis; live-gates are manual today | **M** | Breakage found the morning it ships upstream, not mid-slice | Flaky-agent noise; needs quarantine logic and cost caps |

## 11. Ten long-term ideas

| # | Idea | Why | Cost/LOE | Benefit | Risk |
|---|---|---|---|---|---|
| L1 | **A2A v1.0 federation surface**: signed Agent Cards, multi-tenancy, TLS/real authz | The moment a second machine or user appears, the loopback bearer-token posture is the blocker; the spec standardized exactly this | **L** | The bridge becomes a real network node, not a localhost tool | Scope creep into infra; do it only when a second node exists |
| L2 | **gRPC binding for A2A** | A2A v1.0 made the same logical agent exposable over JSON-RPC and gRPC; SDK support will mature | **M–L** | Interop with gRPC-first platforms | Wait for a2a-rs to carry it; don't hand-roll |
| L3 | **ACP-registry-aware agent onboarding**: `a2a-bridge agents discover/install` against the public ACP registry | 25+ ACP agents exist; the registry (Jan 2026) is machine-readable; today each agent is hand-configured | **M** | New-agent setup drops to one command; the bridge rides ecosystem growth | Registry API stability |
| L4 | **Remote ACP transports** (drive agents on other machines) when the spec ships them | Currently on the official ACP roadmap; would let one serve orchestrate agents across boxes without containers-per-host | **L** | Fleet topology without SSH hacks | Spec timing; don't front-run it |
| L5 | **Unify the orchestration engines**: make the review→tweak loop expressible in the workflow DAG (bounded cycles / conditional edges), or explicitly rule that loops stay code. The codex lens proposes the concrete mechanism: teach the workflow executor to run on *supplied warm sessions*, so `implement`'s edit/fix turns stop being off-executor (conf. 93) | Two engines with two resume stories is a standing tax; today workflow features (checkpoints, panel, journal) don't apply to implementation turns; either outcome beats undecided | **L** | One mental model; loops get crash-resume and observability for free | DAG-with-cycles is a real design problem; the "rule it stays code" outcome is cheap and legitimate |
| L6 | **Web/TUI operations panel**: task watch, batch progress, E2 permission approvals, cost dashboards | The interactive-permission seam (slice 9) and panel weights (slice 10) already exist server-side with no human-friendly client | **L** | The serve becomes operable by humans, not just CLIs | Front-end maintenance surface |
| L7 | **Team/multi-user mode**: per-caller identity, quotas, task ownership, shared warm pools | Follows L1; the single-user assumptions (one bearer token, one owner lock) are load-bearing today | **L** | The obvious growth path if this becomes a team tool | Big; only behind demonstrated demand |
| L8 | **Cost-aware scheduling**: per-batch/per-context token budgets, quota-aware admission (extend the batch semaphore), spend alerts | Telemetry exists; governance doesn't; batch × xhigh × subscription quotas is a footgun | **M** | Safe unattended operation | Budget semantics across heterogeneous agents are fuzzy |
| L9 | **Workflow packs** (versioned, shareable workflow+prompt bundles with a resolver) | The (c)/(d) options deferred in §7 — build only when ≥2 consuming repos demonstrably share workflows | **M–L** | Cross-repo reuse without copy-paste | Premature machinery if the second consumer never appears — hence long-term |
| L10 | **Ecosystem/conformance posture**: A2A conformance testing of the inbound surface, upstream contributions (kiro auth portability, codex quirks), publish as a reference A2A↔ACP bridge | 150+ orgs on A2A, 25+ ACP agents: this repo sits on a genuinely rare intersection; conformance + visibility compounds | **M** (ongoing) | Credibility, contributors, and early warning on spec direction | Time spent on community instead of features — an identity-question (§8-Q1) dependent |

---

## Appendix: method and sources

Analyst fleet: architecture + performance (Opus 4.8 sub-agents); container posture, spec-evolution maintainability, DX/workflow management (Sonnet 5 sub-agents); an independent whole-repo pass by codex gpt-5.5 (effort xhigh) executed through `a2a-bridge run-workflow` with a read-only sandbox and watchdog — i.e., this document's production itself exercised the bridge's flagship path, end to end, successfully (~50-minute turn, watchdog untripped, bounded-STOP output honored). The codex lens's full verbatim report is preserved as [`analysis-second-opinion.md`](analysis-second-opinion.md). Synthesis and final judgments: Claude (Fable 5).

Cross-lens episodes worth recording: (1) the codex lens found the `checkout_turn_inner` lock-across-await that the host performance lens had explicitly cleared — the synthesis verified it by reading the code, and it is now next-step #2; (2) both the DX and codex lenses independently derived the same workflow/prompt hybrid ruling from different evidence (loader code vs docs intent), disagreeing only on pack-system timing — recorded in §7 with a named trigger. Codex candidates considered but not promoted into the final lists: a typed CLI parser (folded into the bin extraction, #9), translator chunking rework (gated on the benchmark suite, §8), pluggable non-SQLite stores and a distributed runner queue (premature at current scale — revisit behind L1/L7), a secrets broker to replace copied cred files (revisit with L1), and a declarative sandbox/egress policy engine (folded into next-step #5's tier presets).

Ecosystem facts referenced: [A2A Protocol](https://a2a-protocol.org/latest/) v1.0 (signed Agent Cards, multi-tenancy, gRPC binding) under the [Linux Foundation, 150+ orgs](https://www.linuxfoundation.org/press/a2a-protocol-surpasses-150-organizations-lands-in-major-cloud-platforms-and-sees-enterprise-production-use-in-first-year); [Agent Client Protocol](https://zed.dev/acp) with the [ACP Registry](https://zed.dev/blog/acp-registry) (Jan 2026) and [JetBrains ACP support](https://blog.jetbrains.com/ai/2026/01/acp-agent-registry/); remote transports on the ACP roadmap.
