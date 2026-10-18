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

Freeze bounded UTF-8 skill bytes, ID and digest at admission. Resolve operator-selected regular files relative to their
declaring config, never the checkout. Proposed limits: 32 KiB per skill / 64 KiB total. Missing, malformed or oversized selected
skills fail before provider work; no network retrieval. Initially support instruction-only single files: no asset copying or
script execution; relative references are not delivered. Validate frontmatter name/description and unique IDs.

Append a clearly delimited “Attached skills” section after workflow template expansion, before dispatch; skill braces are
literal, never workflow variables. Identify delivered MCP names, actual session checkout, and relevant tool-name conventions
beside the unmodified body. Preserve task restrictions and output format. The skill is advisory procedure, not authority to
edit, install, access the network or relax a sandbox. Cover workflow and direct-agent turns, implement initial/fix turns, and
warm continuation; emit one attachment section per outgoing prompt, without mutating stored task input or reusing another
session's rendered text. Record digest/size/delivery mode; never log source bytes or host paths by default. Changed skill
content on resume requires explicit re-admission, not a silent change beneath a retained session or approved prompt. B has two
workflow rendering paths (`crates/bridge-workflow/src/executor.rs:2932,3601`); a single CLI-only append would miss
served/dispatcher paths. The common dispatch contract must cover each entrance.

## 2. Generic configuration and binding

Options: repeated per-agent entries, reusable bundles, or hard-coded Prism. **Start with `agents.skills`, then bundles.** Cost
if wrong: excessive merge/session-identity complexity; exclude nested bundles and arbitrary inheritance.

| Proposed scope | Fields and meaning |
|---|---|
| `[[agents.skills]]` | `id`, `path`, `delivery="prompt"`; direct attachment, usable independently in PR 1. |
| `[[attachments]]` | `id`; `skills` entries with the same shape; `mcp` entries with existing ADR-0028 fields. |
| Agent or workflow node | `attachments=[IDs]`, optional `role`; explicit `attachments_disabled=true` suppresses inherited bundles. |
| `attachment_defaults` | Role-to-bundle lists for `code-review`, `spec-review`, `plan-review`, `design`, `implement`. |
| Operator policy file | Bundle definitions, defaults, exact workflow/node or agent selectors, optional provider delivery overrides. |
| MCP lifecycle policy | Optional `warm` readiness tool/arguments/status selectors/deadline; resource reservation independent of warming; optional bounded retirement observer. |

Ship generic examples using a fictitious navigator. The operator's policy owns the Prism executable, `--repo {cwd} --shared`,
compatible cache/refresh settings, skill path, tool aliases and readiness selectors. Do not vendor the Prism skill or add
project-specific workflows under `examples/` (AGENTS.md:18–23).

An explicitly selected `--attachments-file` (or `A2A_BRIDGE_ATTACHMENTS_FILE`) supplies one operator policy across existing
configs. Absent selection is byte-compatible behavior; no automatic scan of home directories. Select it once in the operator
launcher/service, persist its resolved digest with the run, and apply it to local workflows, serve/MCP controllers, direct
dispatch and implement/resume. No client request may supply new file paths, commands or bundles; it may select only already
admitted roles/attachments. Resolution is deterministic: explicit disable wins; role defaults plus explicit bundle IDs form an
ordered, deduplicated union; equal repeated definitions coalesce; conflicting MCP names or skill IDs fail validation. Direct
entries do not silently override bundle contents. A new admission is needed to change policy on resume.

Resolve (including resource-limited omissions) before effective-entry validation, preflight and authority binding. Preserve
command allowlists, secret handling, managed-loopback markers and native cwd restrictions. Include attachment content/delivery
identity in affected session reuse and preflight keys; native process-bound MCP changes require a distinct adapter instance,
never mutation of a shared live registry entry. Skill text cannot make an unsupported MCP transport supported. B's binder
already freezes rendered MCP payloads (`crates/bridge-core/src/execution_policy.rs:2410–2442`); skill/default resolution must
feed that boundary. The current continuation fingerprint covers agent/model/effort/mode/cwd only
(`crates/bridge-core/src/session_fingerprint.rs:7–33`); add attachment identity rather than assuming coverage.

## 3. Optional pre-warm and resource lifetime

Options: operator pre-warm as today; raise adapter startup timeouts; or bridge-managed readiness before dispatch. **Recommend
optional bridge-managed warming after delivery works.** Keep operator warming available; timeout inflation alone does not teach
an agent to retry a `warming` result. Cost if wrong: extra delay, extra processes and retained GiBs without better reviews.
Default warm off; initially propose a 180 s total deadline, bounded by the remaining run deadline, and 10 s retry interval. No
provider calls or unbounded background retries.

Use the declared MCP command in the same execution namespace, canonical checkout, cache and environment as the eventual agent.
Initialize a bridge-owned client, invoke a declared harmless readiness tool, and classify an explicitly configured structured
response as ready/retryable/failed. Initialization alone is not readiness for lazy indexes. Cap attempts (18 including the
first), response bytes and per-call time within one deadline; unknown/malformed responses degrade. Only configured retryable
statuses retry; no retry on build failure. This is a generic MCP client contract, not a shell hook or hard-coded Prism protocol.

Deduplicate one warm lease per checkout **and effective server identity** across simultaneous bridge runs; retain the client
through agent connection/last-use so lazy shared state is not immediately retired. Prism's `--shared` lets agent shims join the
same index; standalone servers would duplicate memory and must not advertise this optimization. P's identity includes canonical
path, filesystem identity, analyzer build/protocol, cache base and refresh policy. One daemon per checkout is therefore a policy
invariant: require the same effective identity for all attached Prism clients in a namespace; refuse conflicting managed
attachment identities instead of silently launching another index. Host and container namespaces are separate pools unless
socket/cache sharing is explicitly configured and verified; do not claim cross-pool sharing.

A timeout/failure releases the warm client and records `unavailable`; dispatch continues with the skill's bounded
retry/CLI/search fallback. Reuse a failed warm outcome for that run so every node does not repeat it. Cancellation releases
owned clients and children; never kill an existing shared daemon or a sibling's client. At final lease release, close bridge
clients; daemon idle retirement is the default (Prism defaults to 900 s). An opt-in identity-bound draining shutdown may shorten
retention; Prism exposes pid/start-time guards and never disconnects a client (`P:src/bin/prism-mcp.rs:98–119`). Never use a
broad process-name kill.

TTL alone does not bound concurrent memory. Apply an operator-wide slot/reservation budget to **all managed attachments**, not
only warmers, using cross-process leases in each execution namespace. Proposed initial pool: two distinct checkout identities,
estimated 3 GiB each; this is admission accounting, not a hard RSS cap. Reserve before spawning either warmer or agent MCP. At
saturation, omit that optional MCP attachment for the turn and report `resource_limited`; do not skip warming then let the agent
start the same index unaccounted. Keep reservations while daemon retirement is unconfirmed; unknown ownership retains the slot.
External daemons and CLI fallback builds are outside this bound; avoid automatic CLI indexing at saturation, use search instead.
An optional allowlisted, read-only lifecycle-observer argv returns bounded identity/alive/unknown JSON; the operator supplies
its server-specific mapping (Prism has `daemon-status`, `P:src/bin/prism-mcp.rs:76–95`). No observer means operator reconciliation before reclaiming
slots; expiry alone is not proof. Validate this dependency before enabling the pool. Actual memory/build peaks must be measured
before increasing the cap. Idle pooled adapter sessions can keep MCP clients alive: release them or count their reservations
until adapter retirement confirms disconnection.

The existing LSP dependency warm (`B:bin/a2a-bridge/src/main.rs:5016–5074`) is not an MCP readiness facility: it targets
selected container_rw agents. Extend dispatch preparation for actual selected agents/checkouts, including implement clones after
creation and worktrees after allocation, rather than warming the source repo.

## 4. Review-role defaults and prompt migration

Options: edit every config, infer review intent, or explicit roles plus central selectors. **Use explicit shipped-node roles and
exact operator selectors for legacy configs.** One policy attaches bundles to review nodes and implement initial/fix turns.
Custom configs need central mappings or agent-wide defaults; never infer intent from names containing “review”. Cost if wrong:
missed reviews or expensive synth nodes. Preview the resolved workflow/node/agent/role/digest/MCP/delivery/cwd matrix and report
unmatched selectors.

Eleven prompt files mention Prism at B, including the differently worded `prompts/review-sol-risk.md:5–8`. Replace duplication
with a conditional generic pointer: “Follow attached navigation skills when relevant; if none are attached, use the permitted
read-only tools.” Remove the contradictory “do not investigate absent tools” clause for declared attachments: allow bounded
deferred-tool discovery, not environment troubleshooting. Keep review prohibitions and exploration budgets. Scope this exception
explicitly: ToolSearch and the declared read-only MCP/CLI query are permitted, subject to deadline/resource limits; denial means
fall back, not bypass. The delivered P skill supplies names (`mcp__plugin_prism_prism__nav_*`, `mcp__prism__nav_*`, bare Kiro
names), wrong-checkout checks and retry guidance. OpenCode aliases must be reported from its actual tool catalog, not guessed.
Preserve the no-skill path, and count a justified docs-only skip as success rather than demanding ceremonial calls.

## 5. Usage measurement from bridge events

Options: grep titles now; structured optional event identity; or proxy every MCP call. **Recommend structured event identity
plus an explicitly heuristic legacy report.** A full proxy gives stronger wire evidence but adds transport/lifecycle complexity;
defer it unless adapters cannot expose identity. Cost if wrong: false adoption counts or an apparent zero caused by missing
telemetry. Existing events retain title/status and call IDs, not server/tool identity
(`B:crates/bridge-core/src/orch.rs:137–205`; ACP mapper `acp_backend.rs:6986–7018`).

Add bounded optional server/tool identity at the ACP mapping seam, extracting only allowlisted fields from adapter-provided
structured metadata/raw input; never persist arguments or raw output for this metric. X already renders MCP input as `{server,
tool, arguments}` (`dist/index.js:29520–29524`); preserve only the first two. Use version-tested parsers for other adapters;
absent identity stays unknown. Plugin aliases and bare Kiro names are not sufficient proof without a unique admitted catalog
mapping. ToolSearch is discovery, not a Prism call. Warm-up calls are bridge-originated and excluded from agent counts.

Persist through the existing journal (`B:crates/bridge-store/src/sqlite.rs:3656–3666`) and retain identity through
live/served/replay projections. Count distinct `(run,node,session,turn,tool_call_id)` attempts; reconcile start, updates and
terminal state without double counting retries/replay. Report attempted/completed/failed/unfinished, with incomplete coverage
for truncated IDs or missing events; identical truncated IDs must not become exact counts. Also report identity coverage,
attachment availability, skill digest and first-call latency. A completed call is not necessarily useful: a
warming response can be protocol-successful. Readiness/usefulness remains unknown unless bounded structured status is available;
CLI use and reviewer-reported skips are separate fields.

Collect ten consecutive eligible reviews; record role/provider/diff class/version, covering all four adapters and spec/code
review or naming gaps. Compare a fixed prior sample with the same classifier and legacy uncertainty. Report sessions with
identified navigation calls, useful-result evidence, timeouts, justified skips, elapsed time and context overhead. Owner judges
relevance; counts do not prove quality or causality. Provisional target: useful navigation in 8/10 eligible sessions, with
delivery/coverage recorded for all ten.

## 6. Four bounded PR slices

Cap each slice at **two review rounds**; disclose bounded extensions for converging closed findings, otherwise return to design
without restarting. Require same-environment fail-before/pass-after regressions, negative/edge cases, full workspace test totals
and CONTRIBUTING.md:26–31,47–62 gates. Report unavailable platform/container checks.

1. **Per-agent prompt skills.** Own skill parsing/freezing and shared dispatch composition, including warm implement/fix and
   both workflow routes; keep current MCP configuration. Value alone: existing MCP agents receive the maintained skill instead
   of relying on installed home files. Tests: exact body reaches four fake adapters; missing/oversized/malformed skill; literal
   template braces; no cwd writes; direct/served/workflow/implement paths; continuation digest change; duplicate prevention;
   no-config byte parity.
2. **Bundles, role policy and prompt pointers.** Own effective attachment resolution and binding before validation/preflight,
   policy persistence, generic examples and eleven prompts. Exclude warming and metrics. Tests: old configs inherit via one
   policy; opt-out/conflicts/unknown selectors; source-to-clone/worktree cwd; two roles sharing an agent cannot leak MCP state;
   resume drift; OpenCode explicit delivery; Kiro sandbox refusal; managed-loopback/secret checks still run. Migration output
   proves every selected role is covered.
3. **Event identity and ten-review report.** Own optional event fields, ACP extraction and journal/projection report. Exclude
   transport proxying. Tests: adapter fixtures identify real calls, title/ToolSearch false positives, bare-name collision,
   updates/replay/repeated IDs across sessions, missing starts, truncated IDs, old journals, failed/warming results and secret
   arguments absent from output. Unknown coverage stays unknown.
4. **Optional warm leases and admission limits.** Own readiness client and per-namespace resource lifecycle; reuse attachment
   bindings. Exclude native skill delivery and arbitrary shutdown hooks. Tests: fake MCP warming→ready,
   malformed/failure/timeout/cancel, concurrent same-checkout single warm, distinct clones, shared namespace mismatch, saturated
   pool, retained adapter clients, crash recovery and sibling-safe release. No real provider needed for regressions; separately
   authorize exact-adapter/live adoption trials.

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

Owner decisions: (1) accept prompt-first delivery and defer native optimizations; (2) opt into one operator policy with explicit
legacy-role mappings; (3) accept provisional 180 s warming / two-checkout reservations or choose different limits; (4) authorize
a separately bounded ten-review trial and agree eligibility/8-of-10 criteria. No implementation, live trial, full-suite
execution or publication is authorized by this design task.
