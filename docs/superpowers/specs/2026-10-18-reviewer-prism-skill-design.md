# Reviewer and implementer MCP attachments and skills

Date: 2026-10-18 · Status: proposed · Source baseline: `6949fefab2b7f668017c0a01c205b18c8a2eb746`. Decision: deliver
operator-selected skills as prompt text first; reuse ADR-0028 for MCP delivery; then add role defaults, measurable usage, and
optional bounded warming. Native skill packaging is deferred. “Every provider” here means Claude ACP, Codex ACP, host Kiro, and
OpenCode ACP, including their configured model providers. API-only backends and containerized Kiro do not acquire MCP support
through this proposal.

## Evidence and scope

Static inspection only; no builds or provider turns. Installed packages are not receipts for pinned container images.
Owner-supplied observations: 8/13 sessions abandoned `warming`; deferred Claude tools went unloaded; briefs omitted Prism;
13/~88 sampled configs declared it; loaded indexes used approximately 1.2–2.5 GiB. These are motivation, not independently
reproduced measurements. The supplied local skill associated with [Prism PR 488](https://github.com/shoedog/prism/pull/488) was
read; remote equality was not verified. Package/source hashes remain in private lane evidence.

- **B:** bridge baseline above. `crates/bridge-core/src/mcp.rs:138–195,262–343` defines MCP specs, Codex arguments, and Kiro
  skill-resource discovery; `bin/a2a-bridge/src/config.rs:1355–1379` requires an explicit delivery choice for unknown commands,
  including OpenCode.
- **C:** installed `@agentclientprotocol/claude-agent-acp` 0.93.0, `dist/acp-agent.js:8340–8355,8447–8469`; bundled
  `@anthropic-ai/claude-agent-sdk` 0.3.300, `sdk.d.ts:2028–2046,2274–2296`.
- **X:** installed `@agentclientprotocol/codex-acp` 2.2.2, `dist/index.js:35506–35514,35830–35885`.
- **K:** installed `kiro-cli` 2.32.0 `agent --help`/`agent create --help`; these establish agent-config support, not an external
  skill loader. [Kiro skill documentation](https://kiro.dev/docs/skills/) describes specific/glob `skill://` paths; B's renderer
  supplies workspace/global globs today.
- **O:** installed `opencode-ai` 1.18.27 executable's embedded documentation, markers `Register skills from non-default
  locations` and `External skills (auto-loaded)`; `--help` lists ACP. [OpenCode skill
  documentation](https://opencode.ai/docs/skills/) describes discovery and the `skill` tool.
- **P:** inspected Prism tree `c76c4676d36682a0a945caf47003ced7ca027f07`:
  `skills/prism-code-navigation/SKILL.md:26–40,57–73,129–132` covers discovery, checkout identity, CLI fallback, bounded retry,
  review applicability, and empty-result limits. `src/bin/prism-mcp.rs:31–45,98–119` describes shared mode, idle TTL and
  draining shutdown; `src/mcp/shared/identity.rs:114–136,197–215` defines the daemon identity.

## 1. Skill delivery

| Option | Coverage and recommendation | Cost if wrong |
|---|---|---|
| Native external skill location | Possible provider-specific optimizations below; defer as the default. | Discovery can silently fail; isolation, mounts and provider upgrades multiply maintenance. |
| Copy into session cwd | All four can discover provider-specific directories; reject automatic writes, including disposable implement clones. | Pollutes diffs, races parallel work, violates read-only reviews and risks commits containing operator material. |
| Inject one source file into the prompt | **Use for all four now**, through normal text dispatch; works when the agent cannot read the host skill path. | Pays repeated context tokens; instructions can be ignored; not native progressive disclosure. |
| MCP `instructions`, prompts/resources | Supplement only on clients that expose them; guaranteed coverage is currently none. | The skill can remain invisible until discovery succeeds, reproducing the adoption failure. |

MCP prompts require client selection/retrieval ([MCP
specification](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts)); resources require retrieval too. Prism
publishes startup/freshness `instructions` but records unverified ingestion (`P:src/mcp/transport.rs:478–499`). None solves
missing-server configuration. Bridge-side fetching would add initialization and trust dependencies while losing offline skill
availability.

| Adapter | Native route established on disk | Recommended route and remaining uncertainty |
|---|---|---|
| Claude Agent SDK via claude-agent-acp | C accepts `_meta.claudeCode.options`; defaults to user/project/local settings. SDK local `plugins` load skills per session; `skills` selects names, not arbitrary file paths. | Prompt now. Later stage an instruction-only local plugin outside the checkout and pass its path through the adapter; mount read-only in containers. SDK typing and adapter forwarding are verified; actual discovery/activation is not. |
| Codex via codex-acp | X maps additional directories to `.agents/skills` roots via `skills/extraRoots/set`; those directories also enter workspace-write sandbox roots. | Prompt now; do not use extra directories just for skills. A dedicated read-only session skill-root API needs verification before native delivery. Preserve real auth/config home. |
| Kiro | B emits `skill://.kiro/skills/*/SKILL.md` and the global equivalent into managed agent JSON; K documents explicit paths. | Prompt now. Later add an exact staged skill URI to the content-bound managed JSON. Installed help does **not** verify absolute external URI loading; host-only MCP restriction remains. |
| OpenCode | O embeds `skills.paths` recursive discovery and external Claude/agent skill directories. | Prompt plus explicit `mcp_delivery="acp"` now. Later use a bridge-private configuration directory and staged `skills.paths`; ACP process isolation, config merging, and activation remain unverified. |

Codex's documented repository/user locations also support ordinary native discovery, not an isolated bridge delivery contract
([OpenAI skill documentation](https://learn.chatgpt.com/docs/build-skills)). Do not redirect `CODEX_HOME`: ADR-0028:35–40
records the authentication failure that motivated argv overrides. No native route is promoted from source inspection alone; test
the exact host/container adapter separately. Do not install a whole plugin with hooks, extra MCP servers or permissions merely
to deliver one Markdown skill.

Freeze bounded UTF-8 skill bytes, ID and digest at admission. Resolve paths relative to the declaring config, not the session
cwd; trust equals that config's prompt files. Mark checkout-resident sources `origin=checkout` in preview; recommend an
operator-owned policy outside untrusted branches. Reject symlinks with no-follow open and a regular-file check. Limits remain
32 KiB per skill / 64 KiB total; validate frontmatter name/description and unique IDs. Missing/malformed/oversized files fail
before provider work. Instruction-only files: no retrieval, asset copying, script execution or delivered relative references.

After template expansion, prepend exactly one attached-skills section as a separate leading `Part`, with per-prompt nonce
boundaries; never re-expand skill braces or mutate stored input. Keep task, prior findings and output contracts last; brief lint
still sees original input. Test forged headers/closing delimiters in reviewed input. State that the attached ID/digest supersedes
installed copies of that ID. Identify delivered MCP names, session checkout and tool conventions beside the unchanged body.
Skills remain advisory, with bounded discovery only: no added edit, network, installation or sandbox authority. Record digest,
size, delivery mode and repeated-turn overhead, never source bytes or host paths by default. Keep full delivery each task turn;
once-per-session text plus digest reminders is a deferred optimization requiring resumed-session equivalence tests.

Composition belongs above `AgentBackend::prompt` at the production entrances: owned workflow execution (local and coordinator),
implement initial edit, implement fix/resume, and direct coordinator run/continue. Include warm continuations. The dispatcher
workflow route cited previously is test-only, not a second production entrance. Preflight PONG smoke, `/rename` mint prompts and
the warm seed are excluded and must remain byte-identical with attachments enabled. Re-admit changed skills/policy on resume;
never silently change a retained session's instructions.

The existing implement task-file workaround keeps large Unicode task text out of containerized Claude prompts
(`B:bin/a2a-bridge/src/main.rs:3975–3984`). PR 1b must gate the **complete composed initial and fix payload**, including feedback
and maximum multibyte skills: either separately authorized same-environment exact-adapter evidence establishes supported
bounds, or validated lane-specific admission limits reject unsupported combinations before send. Deterministic boundary and
negative tests enforce the selected rule; fake adapters establish byte delivery, not real transport compatibility.

## 2. Generic configuration and binding

Start with `agents.skills`, then reusable bundles; exclude hard-coded Prism, nested bundles and arbitrary inheritance.

| Proposed scope | Fields and meaning |
|---|---|
| `[[agents.skills]]` | `id`, `path`, `delivery="prompt"`; independent per-agent attachment in PR 1a. |
| `[[attachments]]` | `id`, skill entries, MCP entries with ADR-0028 fields in explicit `host` / `container` variants (optionally image-specific). |
| Agent or workflow node | `attachments=[IDs]`, optional `role`; `attachments_disabled=true` suppresses inherited bundles. |
| `attachment_defaults` | Role-to-bundle lists for `code-review`, `spec-review`, `plan-review`, `design`, `implement`. |
| Operator policy | Bundles/defaults, exact workflow/node or agent selectors, provider overrides; optional explicit `replaces_direct` names. |
| MCP lifecycle | Optional readiness tool/args/status/deadline; independent resource reservation; bounded retirement observer. |

Ship generic navigator examples. The operator policy owns Prism's executable, `--repo {cwd} --shared`, cache/refresh settings,
skill path, aliases and readiness selectors. Do not vendor the Prism skill or project-specific workflows under `examples/`.
Resolve each MCP entry in the target execution namespace, including `container_rw` implementers and sandboxed readers. No variant
means omit that MCP attachment with `unsupported_namespace`; the skill may still be sent with “no navigator in this session”.
Preview namespace, selected variant, availability and omission reason; never substitute a host command into a container.

One explicitly selected `--attachments-file` or `A2A_BRIDGE_ATTACHMENTS_FILE` supplies the central policy; no home-directory scan.
No selection preserves byte-compatible behavior. Persist the resolved policy digest for local, served and implement/resume
runs. Clients may select only admitted roles/attachments, never new file paths, commands or bundles. Explicit disable wins;
role defaults plus explicit IDs form an ordered deduplicated union. Equal definitions coalesce; conflicting names/IDs refuse.
Direct entries never silently override bundles. `replaces_direct` explicitly names the displaced direct MCP entries; preview
both identities before staged enablement, preserving opt-out. Replacement still passes ordinary validation and authority gates.

Protect the tiered-review eval explicitly: add `--no-attachments`, suppressing direct skills and policy/bundle additions even
when the launcher exports a policy; leave the eval's declared direct MCP configuration intact. The harness passes this by
default. Attachment-enabled cells must explicitly declare their policy identity; every result records the effective policy
digest (or explicit disabled identity), and integrity checks reject undeclared/mismatched policy. Snapshot current eval prompts
before the eleven shipping prompts change; rebaselining is separately authorized, never an ambient side effect. Preserve the
tools-off judge and deliberately non-navigating scaffolded/synthesis arms unless a cell explicitly changes its contract.

Resolve namespace variants and validate candidates before preflight/authority binding; preserve command allowlists, secrets,
managed-loopback markers and native cwd restrictions. Include skill, policy and MCP delivery identity in reuse/preflight keys.
The binder freezes MCP payloads (`B:crates/bridge-core/src/execution_policy.rs:2402–2470`); PR 4 admits two validated candidates,
with/without the optional resource-managed attachment, rather than editing frozen delivery at dispatch. Each has its own bound
effect/digest and matching prompt availability metadata. Direct/implement turns use **agent-wide attachments only** until they
adopt the V2 binder; client roles cannot change their MCP sets. Role defaults target their configured implement agent at admission.
The continuation fingerprint currently lacks attachment identity (`B:crates/bridge-core/src/session_fingerprint.rs:7–33`);
add it, and never mutate an existing registry entry to change a process-bound MCP set.

Opus's supplied static OpenCode 1.18.27 inspection finds stdio translation into directory-scoped `sdk.mcp.add` registration,
with ignored registration failures. Use one OpenCode process per effective MCP set (`EffectKeyedProcess`), not shared
session-scoped registration across different sets. Model directory-scoped registration in isolation tests. Report delivery
`unverified` until an exact-adapter conformance check demonstrates registration and absence of cross-role leakage; an ACP send
alone proves neither. Continue explicit `mcp_delivery="acp"`; infer aliases only from the actual catalog.

## 3. Optional pre-warm and resource lifetime

Warming defaults off; operator warming remains available. Keep the approved 180 s total budget, bounded by the remaining run
deadline, with 10 s retry intervals and at most 18 attempts including the first. Begin at admission once the frozen checkout is
available, or after clone/worktree creation; overlap spawn/preflight preparation where possible. Block only the first task
prompt send for the remaining budget, never restart the clock. Exclude warm delay from turn-liveness/watchdog clocks and report
it separately; the overall run deadline still applies. Cancellation does not send a prompt. PR 4 initially warms host namespaces
only; container readiness is a named gap until a safe pre-existing execution namespace exists. Container attachments still pay
resource reservations. The existing container LSP dependency warm is not MCP readiness and does not warm the source in lieu of a clone.

Use the selected namespace variant, cache/environment and **frozen effective `{cwd}` string**, with no independent
canonicalization. A bridge-owned MCP client invokes a declared harmless readiness tool; initialization alone is insufficient.
Bound response size and per-call time within the single deadline. Only configured structured retryable statuses retry;
malformed/unknown/build-failed responses degrade. No provider calls, shell hooks or unbounded background retries.

Deduplicate warming by checkout plus effective server identity; retain the client through agent connection/last-use. Prism's
`--shared` joins the same index; standalone servers cannot claim this optimization. Identity includes canonical path, filesystem
identity, analyzer build/protocol, cache base and refresh policy. Require one compatible identity per checkout within a namespace;
refuse conflicting managed identities. Host and container sharing leases remain separate unless socket/cache sharing is verified.
Above these leases, **one aggregate reservation authority per operator memory-bearing host/runtime** enforces the two-checkout
ceiling across namespaces. Independently resident namespace instances each consume a slot even when checkout paths match.
Estimate 3 GiB per slot; accounting is not an RSS guarantee. Measure peaks before increasing the cap.

Reserve before either warmer or agent MCP can spawn, including when warming is off. PR 4 freezes with/without-attachment
candidates at admission. At dispatch, select the with-candidate only under an acquired or retained lease; otherwise select the
without-candidate and journal `resource_limited`, candidate ID/digest and lease decision before provider work. Never patch a
bound payload or bypass its digest check. Early warming acquires and retains the same reservation; an admission that starts no
warmer may encounter saturation later. Preflight, process identity and session reuse must follow the selected candidate.
Unbound direct/implement paths select from their frozen agent-wide candidates before slot creation; retained sessions keep their
candidate and charged reservation. Do not remove MCP from a live session. A crash/replay must not resurrect an unleased candidate.

A failed/timed-out warm records `unavailable`, closes its client and continues with bounded skill retry/search/CLI fallback
while retaining any still-resident reservation; reuse that failure within the run. At saturation use search, never automatic
CLI indexing outside the pool. External daemons/CLI builds remain outside managed accounting. Cancellation releases owned
clients/children only, never shared daemons or sibling clients. Final client release allows idle retirement (Prism defaults to
900 s); optional identity-bound draining shutdown must not disconnect clients. Never kill by process name.

Pool enablement requires a bounded allowlisted read-only lifecycle observer returning identity/alive/unknown; validate its
availability. Reclaim after confirmed retirement, including crash recovery only when the holder is provably dead (PID plus
start time) and an identity probe confirms no daemon. Unknown ownership/retirement retains the reservation and is visible in
run summaries; TTL alone is not proof. Idle adapter clients remain charged until disconnection/retirement. Keep cross-process
lease code `#[cfg(unix)]`; Windows reports pool management unsupported without breaking the non-unix build. Split PR 4 into
accounting then readiness sub-slices so each can converge with warming off or operator pre-warming.

## 4. Review-role defaults and prompt migration

Use explicit shipped-node roles and exact central legacy selectors, never inference from “review” in a name. One operator
policy covers review nodes and agent-wide implement initial/fix attachments; custom configs need mappings or agent defaults.
Preview workflow/node/agent/role/policy/skill/MCP/delivery/cwd, namespace, origin, conflicts and unmatched selectors before staged
rollout. Resolve existing direct Prism conflicts via explicit replacement or per-config opt-out, not mass silent override.

Replace the eleven duplicated Prism prompts, including `review-sol-risk.md`, with a conditional generic pointer to attached
navigation skills and permitted read-only tools when absent. Preserve prohibitions, exploration budgets, prior findings and
output formats. Permit bounded ToolSearch/deferred discovery and declared read-only MCP/CLI queries for attached navigators,
subject to deadline/resource limits; denial means fallback, not environment troubleshooting or bypass. The P skill supplies
aliases, wrong-checkout checks and retry guidance. Preserve the no-skill path. Justified docs-only skips are procedural successes,
not useful-navigation successes; eligibility below prevents ceremonial calls or denominator drift.

## 5. Usage measurement from bridge events

Use structured optional identity plus explicitly heuristic legacy reporting; defer a full transport proxy. Existing served
events retain titles/status/call IDs, not reliable server/tool identity. Local `run-workflow` and `implement` currently drop tool
events: `AttemptTelemetrySink::record` is empty (`B:crates/bridge-core/src/attempt_activity.rs:543–545`), used by
`bin/a2a-bridge/src/main.rs:2995–3003,5311`. **Until PR 3's local sink is delivered and verified, the trial counts served runs only.**

PR 3 adds a bounded identity-only persisting sink for local attempts, in a local journal keyed by `attempt_id`, preserving
existing activity telemetry. Allowlist only node/session/turn/tool_call_id/server/tool/status per event; no arguments or raw
output. Fake-adapter regressions must show a real MCP call through both local `run-workflow` and implement reaches the report.
Retain optional identities through served journal/live/replay projections as well; local absence must never masquerade as zero.

At the ACP seam, extract bounded server/tool identity only from allowlisted structured fields with version-tested parsers.
Aliases/bare Kiro names need a unique admitted catalog mapping; otherwise identity is unknown. ToolSearch is discovery, not a
Prism call; bridge-originated warm calls are excluded. Count distinct `(run,node,session,turn,tool_call_id)` attempts and reconcile
start/update/terminal/replay without double counting. Truncated/colliding IDs or missing events mean incomplete coverage, not
exact counts. Report attempted/completed/failed/unfinished, identity coverage, attachment availability, skill/policy digests and
first-call latency. Protocol completion does not prove usefulness: warming may succeed as a protocol call. Preserve unknown
readiness/usefulness unless bounded status or owner-reviewed result evidence supports it; CLI use and skips remain separate.

Freeze eligibility/classifier before collection: a review needing repository navigation is eligible regardless of delivery
failure or unknown telemetry. Owner-validated docs-only/irrelevant skips are procedural successes outside the denominator; log
all screened reviews and continue until ten consecutive **eligible navigation reviews** are collected. Do not select on call
success. Require useful navigation in at least 8/10, with delivery/coverage for all ten; unknown stays unknown and cannot count
as demonstrated usefulness. Compare a fixed prior sample under the same classifier with legacy uncertainty. Publish per-adapter
rows for Claude/Codex/Kiro/OpenCode, with at least one eligible session each or a named gap; also show role/diff class/version,
spec/code coverage, useful-result evidence, timeouts, skips, elapsed time and context overhead. Owner judges relevance; the
aggregate threshold cannot hide an unsupported adapter or establish quality/causality.

## 6. Bounded PR slices and persisted compatibility

Cap each slice at **two review rounds**; disclose bounded extensions for converging closed findings, otherwise return to design
without restarting. Implementation gates: same-environment fail-before/pass-after regressions, negative/edge cases, full workspace
totals and CONTRIBUTING gates; name unavailable platform/container checks. No provider turn is needed for deterministic tests.

| Slice | Owned behavior and acceptance (later slices must not widen earlier scope) |
|---|---|
| **1a: parse/freeze + workflow** | Skill parsing/freezing, owned workflow composition and attachment fingerprint; current MCP unchanged. Four fake adapters receive exact bodies; local/served route parity, braces, forged delimiters, duplicate prevention, bad/oversized files, no cwd writes, unchanged smoke/rename/seed and no-config bytes. |
| **1b: implement + direct** | Initial edit/fix/resume, direct run/continue and warm continuation composition; agent-wide attachments only. Complete-payload compatibility gate, boundary/negative tests, changed-digest refusal and unchanged prior findings/output contract. No claim of real transport conformance from fakes. |
| **2: bundles + roles** | Namespace variants, central policy/binding/preview, staged direct replacement, eleven prompt pointers and eval isolation/snapshots. Test host-only bundle on container_rw implementer and sandboxed reader (`unsupported_namespace`), valid container variant, clone/worktree cwd, opt-out/conflicts/selectors/resume drift. Model directory-scoped OpenCode registration; mixed-role/shared-agent draft/refine/synthesis graph cannot leak MCP or lose findings/contracts. Keep Kiro sandbox refusal, loopback and secret gates. |
| **3: usage** | Optional ACP identity, local identity-only sink, served journal/projections and ten-review report. Test both local production paths, adapter fixtures, title/ToolSearch false positives, bare-name collisions, updates/replay/repeated IDs across sessions, missing starts/truncation, old journals, failed/warming calls, and absence of secret arguments. Unknown stays unknown. |
| **4a: accounting** | Two frozen candidates, journaled lease selection, aggregate authority, retirement and unix-only crash recovery. Saturate between admission/dispatch: degraded run, no ConfigMismatch or unaccounted spawn. Three container namespaces and host/container overlap: third resident instance limited; cover cancellation, unknown retirement, retained clients, replay and sibling-safe release. |
| **4b: readiness** | Optional host MCP warm client on 4a accounting; frozen cwd, overlap and first-send budget. Test warming→ready, malformed/failure/timeout/cancel, one warm per identical checkout, distinct clones, namespace mismatch, no liveness-clock charge or repeated budget. No native skills/arbitrary shutdown hooks; container warming deferred. |

Enumerate persisted changes explicitly; old-record tests are acceptance gates, not optional migration follow-up:

| Store / owner | New state and old-record test |
|---|---|
| Encoded workflow run spec / 1a, extended by 2 | Frozen skill IDs/bytes/digests/delivery, then resolved policy/roles/namespace variants. Old records decode as no attachments; replay adds no ambient policy and needs no source-file reread. |
| Session fingerprint / 1a; direct consumers 1b | Attachment content/delivery identity in mint/reuse state and any serialized projection. Old state remains attachment-free; attaching or changing a digest requires re-admission, never silent reuse. |
| Implement checkpoint / 1b, extended by 2 | Frozen agent-wide skills, then policy/namespace identity. Old checkpoints resume attachment-free; drift rejects before provider work or requires explicit re-admission. |
| Tool journal/projections + local attempt journal / 3 | Optional identity fields and versioned local rows. Old events remain readable with unknown identity/coverage; missing local journals stay unknown. |
| Frozen delivery candidates + dispatch journal / 4a | Candidate effects/digests and selected candidate/lease decision. Old single-candidate records replay unchanged; never synthesize optional delivery from current policy or replay a stale lease as live. |

## Conflicts and owner decisions

- **WRONG / MATERIAL — rejected native-Codex shortcut (static confidence 99/100).** Supplying a skill directory as an additional
  directory under workspace-write also grants writes there (X:35878–35885). Calling that read-only delivery is incorrect.
  Bounded fix: prompt delivery; regression: no new writable roots. BLOCKER for that shortcut only; proof of a distinct read-only
  loader would remove the objection.
- **SMELL / MATERIAL — prompt discovery conflict (95/100).** Existing silent-fallback language can suppress loading deferred
  tools; owner observations support the risk, not a controlled current-adapter failure. Resolve in PR 2; DEFER from PR 1 while
  its attachment explicitly permits bounded discovery. A same-adapter controlled review would raise/lower confidence; successful
  explicit discovery would refute that instance.
- **SMELL / MATERIAL — historical eager-start assumption (95/100).** ADR-0028:81–87 describes the old cold handshake failure; P
  now supports lazy/background states. Do not equate successful initialization with a warm index. PR 4 should annotate the ADR
  with versioned readiness behavior; source plus a live readiness control would settle deployment applicability. DEFER; no
  current production regression is claimed.
- **SMELL / IMMATERIAL — historical Codex transport premise (90/100).** ADR-0028:21–40 records native delivery as necessary;
  installed X:35437–35482,35519–35537 now translates session MCP entries, including stdio. Keep the working bridge route; DEFER
  transport migration pending an exact-adapter conformance test, which would establish or refute runtime equivalence. Skill
  delivery does not depend on changing this route.

Fixed owner decisions: prompt-first delivery; one central role policy with explicit legacy mappings; optional 180 s warming
and aggregate two-checkout reservations; a separately bounded ten-review trial with 8-of-10 useful navigation. This fold does
not authorize implementation, provider turns, a live trial, full-suite execution or publication; the controller commits.

## Round-1 review fold

Both supplied reviews: APPROVE WITH FIXES. Classifications below are retained; design remedies are future acceptance gates,
not claims that implementation defects or adapter compatibility have already been tested.

| Finding | Change |
|---|---|
| Opus W1 — WRONG / MATERIAL | §5 / PR 3: identity-only local sink and both-path regression; served-only trial until verified. |
| Opus W2 — WRONG / MATERIAL | §§2–3 / 4a: freeze two deliveries, lease-select/journal at dispatch; intervening saturation regression. |
| Opus W3 — WRONG / MATERIAL | §2 / PR 2: namespace variants, unsupported_namespace omission and container implementer/reader tests. |
| Opus S1 — SMELL / MATERIAL | §2 / PR 2: eval --no-attachments, declared policy digest/integrity gate and prompt snapshots; explicit rebaseline. |
| Opus S2 — SMELL / MATERIAL | §2 / PR 2: OpenCode process per MCP set; directory-scoped fake; unverified pending conformance. |
| Opus S3 — SMELL / MATERIAL | §2 / PR 1b: agent-wide attachments for direct/implement until V2 binding. |
| Opus S4 — SMELL / MATERIAL | §1 / PRs 1a–1b: production entrance inventory; byte-identical smoke, rename and seed exclusions. |
| Opus S5 — SMELL / MATERIAL | §1 / PR 1a: leading Part plus nonce boundaries; forged-input regression; preserve output contract. |
| Opus S6 — SMELL / MATERIAL | §§2,4 / PR 2: conflict preview, explicit replaces_direct or opt-out, staged rollout. |
| Opus S7 — SMELL / MATERIAL | §3 / 4b: admission/clone warm, frozen cwd, overlap, first-send remaining budget, separate clocks; host-first. |
| Opus S8 — SMELL / MATERIAL | §3 / 4a: required observer, proven-dead holder plus absent-daemon reclaim, visible retention; unix-only leases. |
| Opus S9 — SMELL / MATERIAL | §6: PR 1a/1b split and explicit persisted-schema/old-record matrix. |
| Opus S10 — SMELL / MATERIAL | §5: per-adapter rows, at least one eligible session each or named gap, unchanged 8/10 threshold. |
| Opus S11 — SMELL / IMMATERIAL | §1: config-equivalent trust, checkout origin preview, external policy recommendation and no-follow files. |
| Opus S12 — SMELL / IMMATERIAL | §§1,5: report repetition overhead; once-per-session optimization considered and explicitly deferred. |
| Opus S13 — SMELL / IMMATERIAL | §1: attached ID/digest supersedes installed same-ID skill. |
| Sol S1 — SMELL / MATERIAL | §1 / PR 1b: full initial/fix multibyte payload gate; validated admission bounds or exact-adapter evidence. |
| Sol S2 — SMELL / MATERIAL | §3 / 4a–4b: aggregate ceiling above namespace leases; three-container/overlap regression; split accounting/readiness. |
| Sol S3 — SMELL / MATERIAL | §5: predeclared eligibility; justified skips outside denominator; unknown is not zero or demonstrated usefulness. |
| Sol source-check recommendations | §§1,2,6: preserve lint/input/findings/contracts and mixed-role graph; named runtime gaps, unchanged safety gates. |
