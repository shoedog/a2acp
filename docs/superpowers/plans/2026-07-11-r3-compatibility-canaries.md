# R3 — Compatibility manifest and canary implementation plan

- **Status:** overall R3 **IN REVIEW**; R3a **MERGED** at `3927df3f` by PR #31; R3b **ACTIVE** on
  `agent/reliability-r3b-pinned-lane`. Nine pinned rows are implemented; Sol/xhigh approved the pre-live
  deterministic tree. Authorized attempt 1 passed both Codex cases and failed both Fable cases on expired
  OAuth after prompt start. No baseline promotion ran; the post-attempt hardening is in deterministic and
  Sol-review closure after a deadline/config-directory fold.
- **Prerequisite:** R2c/R2d merged (`a6fec94c`, PR #29); R3a merged (`3927df3f`, PR #31)
- **Program source:** [`../../bridge-reliability.md`](../../bridge-reliability.md)
- **Program cursor:** [`../../reliability-execution-roadmap.md`](../../reliability-execution-roadmap.md)
- **Completion shape:** R3a local manifest/runner, R3b pinned lane, R3c floating lane, R3d scheduling,
  R3e OpenRouter, R3f OpenCode

R3 makes upstream drift visible before an unrelated feature branch finds it. It consumes R2c's
single-attempt artifact; it does not invent another prompt engine or retry policy.

## Fixed lane model

| Lane | Purpose | Pin policy | Failure policy |
|---|---|---|---|
| `pinned` | last-known-good production/release candidates | exact adapter/CLI/image/model/config identity | release-blocking for claimed supported paths |
| `floating-current` | detect newly resolved upstream packages/models | deliberately floating candidate inputs, never production defaults | advisory until deliberate promotion |

No canary result automatically rewrites a pin, compatibility row, or support claim.

## R3a — checked-in manifest and local runner

- **Branch:** `agent/reliability-r3a-manifest-runner`

Add:

- `compatibility/manifest.toml` — versioned declarative matrix;
- `compatibility/baselines/pinned.json` — reviewed last-known-good artifact summary;
- `bin/a2a-bridge/src/compatibility.rs` and `a2a-bridge compatibility validate|run|compare`;
- schema/parser fixtures under the owning test module, not generated run output.

The R3a command contract is:

```text
a2a-bridge compatibility validate [--manifest <path>]
a2a-bridge compatibility run [--manifest <path>]
    (--lane pinned|floating-current | --case <id>... | --all)
    --environment-owner <id> --acknowledge-billable --out <path>
a2a-bridge compatibility compare --current <aggregate.json>
    [--baseline <pinned.json>]
```

`validate` performs bounded regular-file parsing only. `run` checks its acknowledgement before reading
the manifest, requires an explicit selection (there is no implicit all-case billing), and pre-creates the
aggregate output as a single-link regular file with mode `0600` before any provider process. The output
immediately contains a valid blocking setup-incomplete aggregate with explicit selected-case rows, so a
later scratch or candidate-staging failure cannot leave a zero-byte artifact. After execution finishes,
the runner writes and syncs the final aggregate plus a separate blocking setup copy to new owner-only
siblings, verifies that all three names still identify the retained files, and atomically replaces the
provisional inode relative to the pinned directory. It then syncs the directory before returning green.
If that post-rename sync fails, it renames the separately synced setup copy back over the output, makes a
best-effort directory sync, and returns an error; if restoration itself fails, it best-effort removes the
green output name and leaves any surviving blocking recovery copy in place. Normal success removes the
rollback sibling best effort. A failed hostile identity rebind may therefore leave one owner-only blocking recovery copy,
but serialization, write, identity, or rename failure never partially overwrites JSON. The output parent
is canonicalized and descriptor-pinned; output and private-scratch entries are created relative to
the retained descriptor, and its identity is rechecked before and during creation, so a name or symlink
retarget cannot redirect an effect into its replacement. Normal worktrees and bare Git repositories are both excluded. Each
eligible `evidence_path = "bridge_smoke"`, `probe = "minimal"` case shells back into the exact candidate
binary's existing R2c `smoke` command once. Before opening the aggregate, the runner takes one bounded
snapshot of the candidate executable and records its SHA-256 and byte length. After allocating the
owner-only aggregate, but before any provider process, it stages those exact bytes as a private
owner-executable/non-writable mode-`0500` file inside the run's mode-`0700` scratch directory. The
creating descriptor remains writable only while the bytes are installed, so the directory entry is
never published with owner-write permission. It rechecks the staged digest
before every spawn, then rechecks cancellation and full declared timeout headroom at the actual
post-hash spawn boundary. It executes the verified file object rather than reopening its mutable name. Smoke
artifacts are opened and removed relative to the retained scratch descriptor, so one aggregate cannot
silently combine different candidate bytes or read evidence from a retargeted scratch pathname.
On Linux, explicit internal argv fields carry the two descriptor numbers only from the compatibility
parent. The staged smoke child proves that the executable descriptor identifies `/proc/self/exe` before
closing it, then proves that the scratch descriptor identifies the opened `--out` parent before closing
that capability. Legacy ambient descriptor environment variables are ignored, and neither internal argv
field is forwarded to an ACP/provider descendant.
Child stdout/stderr is discarded; the runner embeds only the bounded smoke-v2 artifact. Direct-CLI,
direct-ACP, representative-workflow, wrong-platform, wrong-owner, and missing-prerequisite cases remain
explicit `not_run` rows rather than being omitted or routed to a different path.

R3a treats every R2c-backed case as potentially billable, so `billable = true` is required even for a
negative control expected to fail before prompt acceptance. The checked-in R3a manifest and baseline
start with zero cases; R3b deliberately adds supported-path pins, so merely checking out R3a cannot
acquire a billable default. Aggregate artifacts reject secret-shaped content and exact values from the
case's declared `credential_env` before embedding. Credential-shaped names are rejected from the
non-secret structured `required_env` list. Each prerequisite records a `name` plus an optional `one_of`
list of accepted non-secret values; an empty list means presence-only. Ctrl-C lets the one already-started R2c smoke
finish its bounded cleanup/artifact contract, then starts no later case. Total time/token/cost exhaustion
also stops before the next case, and a case is not admitted when its declared token or observable-cost
cap cannot fit the remaining total headroom. Final-case elapsed-time overflow is recorded as blocking.
A missing, malformed, secret-bearing, or exit-inconsistent smoke
artifact is an unaccounted runner failure, so later potentially billable cases are left explicitly
unrun. Embedded smoke evidence has a cumulative 8 MiB bound and the complete aggregate has a 16 MiB
bound, so a valid run cannot emit evidence that its own comparison command refuses or grow without
limit. Pinned comparison retains per-case execution/error/not-run/drift/budget outcomes plus aggregate
success, cancellation, budget exhaustion, and missing-observation counters; variable usage quantities
remain excluded. There is no retry, provider substitution, config rewrite, baseline rewrite, or
compatibility-doc mutation path.

Token accounting prefers smoke-v2's terminal `totalTokens` (input plus output) and falls back to the
fresh session's streamed context `used` value only when terminal accounting is unavailable. Missing
token or cost observations remain explicit counters; caps are observational where a provider does not
report the corresponding metric.

Pre-change evidence: the focused CLI regression failed because `compatibility` was an unknown
subcommand. Initial-review regressions then failed **9** concrete unsafe states on `884bc5f`; that fold
passed compatibility units **30/0**. First-closure mutation locks then failed exactly **5** reviewed
states (**30 passed / 5 failed / 0 ignored**) on `b37147c`; that fold passed compatibility units
**35/0**. Capacity-attempt mutation locks then failed exactly **4** states on `bc9f64c`
(**32 passed / 4 failed / 0 ignored**): default/alias and floating-range pins, contradictory remote-API
direct-control rows, writable candidate publication, and an in-place overwrite after the digest check.
The last exact `c8c9452` fold passed compatibility units **36/0**, the full `a2a-bridge` binary target
**360/0**, and CLI regressions **10/0** at the last exact `c8c9452` boundary. Supplemental Linux/Rust
1.94 passed the same **36/0** units and **10/0** CLI there. Closure re-review of that exact commit
returned `REVISE`. Its new regression set failed pre-fold for raw alias IDs/floating identities,
meaningless remote components, post-hash cancellation and time admission, blocking comparison state,
and zero-byte setup evidence. Exact `a8602bb` then passed **41/0** macOS compatibility units, binary
**365/0**, CLI **10/0**, workspace **2,038/0/12 ignored**, and Linux units **42/0** plus CLI **10/0**;
closure re-review 3 nevertheless returned `REVISE`. Its exactness-range regression and the independently
reproduced pinned-support and atomic-publication regressions failed before their fixes. The Linux ambient
fd and provider-descendant controls cover the remaining descriptor findings. The current local fold passes
**44/0** macOS compatibility units, the full `a2a-bridge` binary target **370/0**, CLI regressions
**10/0**, and the serial workspace suite **2,043/0/12 ignored** across **70** test/doc-test executables.
Linux/Rust 1.94 passes compatibility units **45/0**, smoke CLI **12/0**, and compatibility CLI **11/0**
with `CARGO_PROFILE_TEST_DEBUG=0`; an initial normal-debug compatibility CLI run stopped **8/3** because
the instrumented candidate exceeded the unchanged 256 MiB evidence cap, and the debug-free rerun
separated that fixture artifact from product behavior. The earlier candidate-overwrite control as uid
65534 remains **1/0**, and the Linux directory-sync rollback controls pass **2/0**. The CLI
suite includes a deterministic missing-config control that invokes the nested smoke exactly once, fails
before provider spawn, and preserves the smoke-v2 failure inside an aggregate created mode `0600`; no
live or billable compatibility canary ran, and review turns do not count as compatibility evidence. The
unit suite also proves that the staged candidate is owner-only,
non-writable after publication, and that digest drift refuses before process spawn. Pinned models reject
automatic/default/floating selectors. Alias-shaped raw IDs remain pinnable because ACP resolves an
advertised raw ID before alias fallback; a fallback resolution cannot green because requested and
effective model identities are compared separately. Exact adapter/CLI pins use one complete semantic
`<package>=<version>` and must match one OK agent-specific provenance row; direct CLI requires
an agent-CLI pin, ACP/bridge paths require adapter plus agent-CLI pins, and remote API paths require
dedicated `provider`, `api`, and `api_version` component pins; a generic execution row is insufficient.
Remote-API mode cannot be mislabeled as a direct CLI/ACP control. Prefix
collisions, warning rows, requested/effective model/effort/mode drift,
and exact API-key environment identity/presence fail visibly.

The current review fold also passes format/diff, workspace all-target check, warnings-denied Clippy,
workspace release build, hygiene **37/7**, and release-candidate manifest validation at SHA-256
`f6481b2e88d55ebbdbed33d73bac40b871627ed1ef6779f582c3943858249007`. Sol/xhigh closure re-review 5
and the one clean-room Fable/xhigh adversarial implementation plus release/compatibility review both
approved exact `fba430fe`; this docs fold is the final publication-status review boundary. No ignored or
live test was run or re-baselined, and review turns remain review evidence rather than compatibility
evidence.

Each manifest case records:

- stable case id and lane;
- direct/ACP/bridge evidence path, host/container/API mode, OS/architecture, and environment owner;
- immutable expected image digest for every container case, including floating candidates;
- config/agent id and raw model/effort/mode;
- expected auth path, credential environment-variable name when applicable, and separate structured
  required non-secret prerequisites with optional accepted values;
- minimal versus representative probe type;
- exact config/model pins plus applicable adapter, CLI, image, and component pins for pinned cases;
- billable flag, per-case timeout, cost/token cap when observable, and retry cap fixed at zero;
- expected status (`PASS`, `FAIL`, `UNKNOWN`, `STALE`) and support/non-goal classification;
- artifact retention/redaction policy.

`compatibility validate` is non-billable and rejects duplicate ids, unknown lanes, floating model ids or
missing applicable pins in the pinned lane, unbounded time/cost fields, secret-shaped fields or comments,
arbitrary prompts, retry counts above zero, and container cases without immutable image expectations.

`compatibility run` requires explicit billable acknowledgement, invokes R2c once per selected case,
emits one versioned aggregate JSON artifact, and stops at the configured total cost/time budget. A case
failure is recorded, not retried or normalized into green.

### R3a initial review ledger

The fresh bridge-mediated Sol/xhigh review of exact `884bc5f` returned `REVISE`. Its seven `WRONG`
findings are folded together: exact pinned-model and path-applicable dependency requirements; exact
API-key environment binding; value-aware non-secret environment prerequisites; mutation-sensitive
terminal/diagnostic projection; final-case elapsed-time exhaustion; bare-repository exclusion; and the
stale durable cursor. The same fold closes self-audit defects for effective capability binding,
support-only pinned blocking, prospective token/cost headroom, secret-shaped ids/comments, and canonical
output-parent symlink retargeting. The reviewer `SMELL` is closed by multi-case lane/case/all tests,
all comparison dimensions, and truly cumulative evidence-limit coverage.

### R3a first closure re-review ledger

The fresh bridge-mediated Sol/xhigh closure re-review of exact `b37147c` returned `REVISE`. Four
inherited findings and the inherited coverage `SMELL` were `FIXED`; the dependency-pin finding was
`PARTIAL` because the advertised automatic model id still validated as pinned. That partial is closed
by pinned `auto` rejection. The five new `WRONG` findings are folded together: stable file-object
execution after staged-name retarget; descriptor-relative aggregate/scratch creation and artifact access; complete nested
`failed_phase` comparison while timestamps remain normalized; advisory unsupported-case classification
before prospective budget admission; and one literal `IN REVIEW` token across the roadmap cursor,
dependency graph, and status table. The macOS fold also mutation-locks the platform seam: APFS permits
child output through a stable directory-object path but `canonicalize` rejects that path, so the child
uses the stable path while the parent reads and removes evidence relative to its retained descriptor.

### R3a capacity-ended closure attempt

The fresh bridge-mediated Sol/xhigh attempt on exact `bc9f64c` read the complete branch and reached final
synthesis, but the provider returned capacity before any verdict or gate line. It is therefore not
counted as a completed review. Its concrete partial analysis independently confirmed the inherited
descriptor, diagnostic, budget-ordering, and status-token fixes, then identified three leads that are
folded there: default/floating model selectors and the known alias table were initially rejected, package
ranges were narrowed, remote-API mode
cannot contradict a direct-CLI/direct-ACP evidence row and bypass its applicable pin requirements; and
the staged candidate entry is mode `0500`, preventing an ordinary same-owner writer from reopening the
verified inode across the digest-to-exec boundary. The roadmap dependency graph is also indented as the
literal R2d -> R3 -> R4 prerequisite chain.
The later complete closure review proved alias keys can also be advertised raw identities and that the
range filter remained incomplete; the second closure ledger records the replacement contract.

### R3a second closure re-review ledger

Fresh bridge-mediated Sol/xhigh closure re-review of exact `c8c9452` returned `REVISE`. It marked the
remote/direct contradiction, ordinary-writer staging fix, dependency graph, all six first-closure
findings, and seven of eight initial-review items `FIXED`; exact/applicable pins remained `PARTIAL`.
Five new `WRONG` findings are folded: advertised raw alias IDs remain pinnable while moving model tokens,
dist-tags, abbreviated/ranged package versions fail; remote API requires provider/API/version identity;
cancellation and declared timeout headroom are rechecked after hashing immediately before spawn;
comparison retains blocking case and aggregate outcomes; and a valid blocking aggregate is durable before
scratch/staging can fail. The literal `35/36` cursor mismatch is corrected. The two `SMELL`s are narrowed:
reopened files require one link and the directory helper no longer claims atomic create/open, while Linux
compatibility-only executable/scratch descriptors close inside the staged child before ACP descendants.
Mode `0500` remains an ordinary-writer safeguard, not a claim against root or an actively hostile same-UID
actor that can change permissions.

### R3a third closure re-review ledger

Fresh bridge-mediated Sol/xhigh closure re-review of exact `a8602bb` returned `REVISE`. It marked seven
of the eight inherited second-closure items `FIXED`, kept exact remote pins `PARTIAL` because `|` ranges
still validated, and found three new `WRONG` states: expected `UNKNOWN`/`STALE` let an unexecuted pinned
support case green the release aggregate; ambient Linux fd variables could close an unrelated descriptor;
and an in-place final write could corrupt the already-synced setup artifact. Its one new `SMELL` noted
that the Linux descriptor unit did not exercise the staged-child/provider boundary.

The exact `42523e1` fold closes the four non-exactness items. A pinned support result blocks unless
execution completed and its expectation matched, while pinned non-goals remain advisory. Linux carries
descriptor numbers in explicit internal argv fields, validates the executable against `/proc/self/exe`
and scratch against the opened `--out` parent before closing, and ignores the retired ambient variables;
a Linux integration control starts a fake provider and proves it inherits neither capability. Final
publication uses a synced owner-only sibling plus descriptor-relative atomic rename after both entry
identities are revalidated; an open reader retains the blocking setup inode, and a target-rebinding
negative control refuses without mutating the rebound output or leaving final staging residue.

### R3a fourth closure re-review ledger

Fresh bridge-mediated Sol/xhigh closure re-review of exact `42523e1` returned `REVISE`. It marked pinned
support enforcement, explicit Linux fd authority, atomic sibling publication, and provider-descendant
coverage `FIXED`. Exact remote identities remained `PARTIAL` because `api_version = "v1 or v2"` and
`"v1 v2"` still validated. It found two new `WRONG` states: a directory-sync failure after rename could
leave a green final artifact while the command returned publication failure, and the roadmap's top next
action still named already-completed gate/commit work. One `SMELL` recorded the remaining identity-check
to rename race against an active hostile same-UID actor or root; that is outside the documented
ordinary-writer boundary and remains accepted and nonblocking.

The current fold closes the partial and both `WRONG` findings. Required remote `provider`, `api`, and
`api_version` values must now be one lowercase stable identity; focused pre-change evidence accepted
`"v1 or v2"` and the corrected regression passes. Final publication now keeps the separately synced
blocking setup copy described above; the injected pre-change directory-sync failure left `green final`
at the output, while the corrected test restores `blocking setup` and removes both staging names. A
companion negative control removes the green output name when the rollback name itself vanishes. The
roadmap top cursor now names only the pending exact-head Sol review followed by the single Fable review.
### R3a exact-head approval reviews

Fresh bridge-mediated `gpt-5.6-sol`/xhigh/read-only closure re-review 5 of exact `fba430fe` returned
`APPROVE`. It adjudicated the remote-identity, post-rename sync, and top-cursor findings `FIXED`; retained
the hostile same-UID/root race as accepted/nonblocking; confirmed the four prior fixes did not regress;
and found no new `WRONG`. Its one new nonblocking `SMELL` is symmetric coverage debt: target rebinding
and rollback-name loss are mutation-tested, while final-sibling same-name replacement before validation
is guarded in production but lacks an equivalent direct negative test. The reviewer inspected all 21
changed files, independently confirmed the exact head/base, clean tree, diff check, and manifest hash,
and accepted the supplied test/build totals without rerunning them.

The one independent clean-room bridge-mediated `claude-fable-5[1m]`/xhigh/plan review of the same exact
commit returned release verdict `READY` and gate `APPROVE`, with no `WRONG`. The trusted-own-repo Tier 0
turn used an explicit strict read-only prompt and left the detached worktree clean. It reran only the
manifest SHA-256, accepted the supplied test/build evidence, and reported four minor nonblocking
`SMELL`s: the parent runner has no outer deadline if the already-bounded child smoke itself wedges; the
enumerated secret-shape heuristics are incomplete but remain backstopped by exact credential-value scans,
sensitive-key checks, and R2c redaction; the accepted hostile same-UID/root races remain; and a few
identity/cost edges can false-reject or produce a less-specific visible counter without greening.

These approval findings do not change the R3a merge gate. R3b owns additive final-sibling mutation
coverage and heuristic/diagnostic edge tests before adding real credential-bearing rows. R3d must add a
parent-owned hard deadline plus bounded termination escalation before scheduling unattended canaries;
an internally wedged or stopped smoke must not park the scheduler indefinitely, and repeated operator
cancellation must remain possible. The stronger hostile same-UID/root actor remains an explicit non-goal,
not a silently closed threat. There will be no Fable re-review; this approval-recording docs fold receives
one targeted Sol/xhigh status review before publication. The docs-only fold reruns format/diff, the full
serial workspace **2,043/0/12 ignored** across **70** groups, and hygiene **37/7**; the approved code tree
is unchanged.

## R3b — pinned lane and promotion baseline

- **Branch:** `agent/reliability-r3b-pinned-lane`
- **Implementation state (2026-07-16):** nine pinned rows validate at manifest SHA-256
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. The current post-`427d2ed`
  fold passes binary **394/0**, full workspace **2,070/0/12 ignored** across **70** groups, Linux/Rust 1.94
  binary **395/0**, Linux smoke CLI **15/0**, focused doctor **5/0**, provider/resolve tests **1/0** each,
  and all format/check/Clippy/release/hygiene/manifest/dependency-policy gates.
  An intermediate Linux run was **393/1** only because the container could not resolve this worktree's
  host-absolute `.git` pointer; the exact Git pointer/common-directory mounts restored repo identity and
  the unchanged test passed in the **394/0** rerun.
  The folded release binary is 22,918,128 bytes at SHA-256
  `6cc16d82ec05541dd151e6bf223c28c90104ee4aa9a6c5941e1971845e60a0d1` and has not run a provider turn.
  The pinned baseline remains empty pending a future all-green, separately authorized aggregate. Fresh
  Sol/xhigh closure review of
  exact `c38978a` returned `APPROVE` with no `WRONG`; its sole nonblocking test-coverage `SMELL` is
  closed. A later exact-`f9f3e68` review returned `REVISE` on the pre-recovery deadline gap plus direct
  parser/comment smells. That fold had full host/Linux/merge-policy gates green. Sol subsequently approved
  exact `427d2ed` with all inherited items fixed and no findings.
  A post-review audit then demonstrated immediate-expiry inner-future polling and exact pinned third-party
  provider false-blocks; both are mutation-proven, full-gate green locally, and pending fresh re-review.

The initial fresh one-shot Sol/xhigh review of exact `57f3ee8` returned `REVISE` with two `WRONG`
findings and three `SMELL`s. The branch now keeps invalid negative/non-finite cost history sticky across
later snapshots, aligns both reader-count surfaces, refuses ambiguous duplicate settings provenance,
adds Claude image-label/drift mutation coverage, and test-locks the baseline empty until authorized
promotion. All findings are folded. Fresh Sol/xhigh closure review of exact `c38978a` returned `APPROVE`
with no `WRONG` and one nonblocking `SMELL`: both sticky-cost usage orders were tested only on normal
completion, not the backend-error serializer exit. The current tree drives both orders into an actual
stream error and asserts the terminal state plus rejected-cost artifact. That exact regression passes
**1/0** here and failed **0/1** on pre-fold `9c2b712`, where invalid-then-valid leaked `cost`. The full
host suite then passed **2,060/0/12 ignored** across **70** groups; corrected Linux/Rust 1.94 binary units
passed **388/0**. A reconstructed Linux harness first produced three unrelated execution failures because
its `/tmp` tmpfs was `noexec`; a direct execute probe exited **126** there and **0** with explicit `exec`,
after which the unchanged product tests were green. The separate shared operator pre-prompt crash
is recorded in the central roadmap and did not trigger a replay or process restart.

Authorized live attempt 1 ran exact candidate SHA-256 `d852cc28...4e50` and manifest
`5d18cefe...c235d828` once, with zero retry/fallback. Codex host/reader returned terminal exact `PONG`
in 8.649 s / 4.751 s. Claude 0.44 host and 0.55 reader both initialized, created sessions, applied exact
Fable/xhigh, and reached prompt start, then failed HTTP 401 in 3.117 s / 2.992 s; both cleanup paths
completed. The aggregate ended non-cancelled after 19.512 s, observed 38,053 tokens and zero cost, exhausted
no budget, left all five controls unrun, and is non-promotable. Its mode-`0600` artifact is
`/private/tmp/a2a-bridge-r3b-live.EeBAyf/pinned-aggregate.json`, SHA-256 `7f718f32...1571c1`.

The failure falsified model selection and container-only degradation: model/effort application completed
on both paths and host/reader failed symmetrically. Both access tokens had expired about five hours before
the run. The five-minute launchd sync had succeeded, proving that byte synchronization alone propagated
the stale host state. R3b therefore adds bounded token-blind OAuth metadata parsing, a 16-minute access
runway row for host and exact mounted reader credentials, and a smoke guard that refuses a non-OK row
before adapter resolution. Host automatic auth honors a non-empty absolute `CLAUDE_CONFIG_DIR` and fails
closed on empty/relative ambiguity. The absolute smoke deadline begins before provenance and orphan
recovery, preventing accepted runway from aging behind a fresh timeout; deadline-first resolution refuses
without polling an adapter once expired. Truthy Bedrock/Vertex/Foundry/Anthropic-AWS/Mantle selectors use
external host authentication and bypass first-party file OAuth, while false-like/unknown values and mounted
reader credentials do not. The original spawned regression
failed pre-change **1 passed / 1 failed** because the expired case reached the fake adapter. The newer
config-directory and delayed-recovery regressions also fail pre-change; current focused doctor **5/0**,
provider-auth **1/0**, delayed-recovery CLI **1/0**, and expired/fresh CLI controls **2/0** are green. Finish
fresh Sol re-review, then
require a fresh host login, post-login sync, two green Claude doctors, and new explicit billable
authorization. Attempt 1 must never be replayed or promoted.

Seed rows for every currently claimed path or control in `docs/compatibility.md`:

- Codex host;
- Codex reader/container;
- Claude host ACP last-known-good;
- Claude Fable reader with exact settings prerequisite;
- explicit negative managed-no-egress control where appropriate;
- Kiro remains `STALE` until re-baselined rather than being silently omitted.

R3a deliberately executes only minimal bridge-smoke rows. Historic direct-CLI/direct-ACP controls are
retained as explicit non-goal/unrun rows rather than being relabeled as bridge evidence or invoked by a
new prompt engine. They do not become supported release paths without a separately reviewed bounded
artifact contract.

Before adding credential-bearing rows, extend the deterministic runner controls with symmetric
final-sibling same-name replacement coverage, additional credential-shaped environment names, and
explicit negative/non-finite reported-cost handling that remains sticky across later usage snapshots.
Preserve exact credential-value and sensitive-key
backstops; heuristic expansion must not be presented as general secret detection. Exact identity grammar
may fail closed on unsupported authoring forms, but every rejected supported-provider identity needs an
explicit reviewed spelling rather than an automatic relaxation.

The implemented controls also hash each pinned config before provider spawn and bind the Fable reader's
mounted minimal settings file by exact SHA-256 in smoke provenance. Container support rows can green only
when bounded inspection of the configured immutable image returns exact adapter and agent-CLI labels.
Missing/malformed labels, an unreadable settings file, or multiple declarations for the same settings
destination remain visible `WARN` provenance and therefore cannot satisfy the pinned drift check. The
reader candidate is built under a unique non-operator tag at
immutable id `sha256:b154aefda301a59a11857700debe826a282dc6e07b76a0ebb46dd6a8e55a03f1`;
the running operator image/tag/process were not changed.

Run from the candidate release binary and exact image id. Compare versioned artifacts to
`compatibility/baselines/pinned.json`; any provenance, capability, auth, phase, terminal, or diagnostic
change is a visible diff requiring review. Per-case execution, runner/not-run code, drift, and budget
violations plus aggregate success/cancellation/budget state are also visible. Terminal projection includes attempt/one-prompt/tool/permission/
cleanup evidence but excludes variable usage; diagnostic projection includes dropped counts, complete
failure metadata, and lifecycle order with only transition timestamps removed. Baseline updates happen
only in a promotion PR that also
updates `docs/compatibility.md` and the changelog when release-relevant.

## R3c — floating-current lane

- **Branch:** `agent/reliability-r3c-floating-lane`

- Resolve candidates without changing production pins.
- Capture exact resolved adapter, nested CLI/SDK, image/base, model catalog, and auth prerequisites.
- Run the same minimal case shape and compare to pinned.
- Classify results as candidate pass/fail/unknown; never claim support solely from catalog discovery.
- A floating success becomes production only through R4's reviewed promotion process.

Tests prove a floating result cannot mutate config, Containerfiles, lockfiles, baseline, or compatibility
docs.

## R3d — scheduling and evidence retention

- **Branch:** `agent/reliability-r3d-scheduled-canaries`

Scheduling is blocked until a runner/credential owner is named. GitHub-hosted runners do not inherit the
developer's subscription auth and must not receive copied personal state casually.

After that owner decision:

- before any unattended schedule, add a parent-owned hard process deadline and bounded TERM/KILL
  escalation around the child smoke. A stopped or internally wedged child must not park the scheduler
  indefinitely, and a later operator cancellation must not be swallowed after the first signal;
- daily: cheap minimal pinned and floating cases within a fixed total budget;
- weekly: one representative read-only review per supported provider lane, but only after R3d adds and
  deterministically validates a bounded bridge-workflow artifact adapter; never schedule R3a's explicit
  `representative_probe_not_implemented_in_r3a` rows as if they were evidence;
- change-triggered: adapter/protocol crate/agent CLI/Containerfile/auth/model-policy/release workflow;
- manual `workflow_dispatch` for promotion evidence.

Use least-privilege workflow permissions, no write token for canary jobs, concurrency that avoids
duplicate billable runs, and artifact retention with bounded sanitized JSON only. Quarantine requires an
owner, reason, expiry, cost cap, and retry cap zero; expired quarantine fails visibly.

## R3e — OpenRouter provider expansion

- **Branch:** `agent/reliability-r3e-openrouter`
- **Prerequisite:** R3a–R3d merged and deterministic provider-fake tests green

Add OpenRouter through the existing explicit `kind = "api"` / OpenAI-compatible boundary in its own PR.
Configuration records only the API-key environment-variable name; neither config, diagnostics, canary
artifacts, nor test fixtures may contain the value. Add bounded model discovery, `doctor`/provenance,
request/header, streaming/terminal, provider-error, timeout, and redaction coverage against a local fake
before one separately authorized minimal live smoke. OpenRouter is an explicit agent/provider selection,
never an automatic fallback after another provider may have accepted a prompt. Add both pinned and
floating cases through the R3 promotion rules; do not edit the running operator service until the branch
is merged, rebuilt, and swapped during a coordinated quiet period.

## R3f — OpenCode provider expansion

- **Branch:** `agent/reliability-r3f-opencode`
- **Prerequisite:** R3e merged; exact installed OpenCode CLI/protocol/version behavior grounded locally

First record the installed executable, version, non-secret environment-variable names, model catalog,
and supported automation protocol without sending a provider turn. Select the narrowest existing bridge
boundary that matches observed behavior; do not infer ACP, OpenAI compatibility, session semantics, or
tool/permission framing from the product name. Pin the exact adapter/CLI/protocol resolution, add corpus
and fake-process tests for initialization, model selection, prompt terminal/error behavior, cancellation,
and secret redaction, then run one separately authorized minimal smoke. Keep OpenCode explicit and
independent from OpenRouter so either provider can be promoted or rolled back without changing the other.
Add its pinned/floating rows only from artifact-exact evidence, and use the same coordinated operator
service promotion boundary as R3e.

## Required tests and controls

- manifest schema boundaries, duplicates, missing pins, secret-shaped fields, invalid budgets/timeouts;
- selection by lane/case without accidental all-case billing;
- one R2c call per case and zero automatic retries;
- aggregate artifact remains valid when setup, a case, final publication, or target identity fails;
- pinned support cannot green unless it actually completed and matched its expected status;
- Linux staged descriptors are object-validated and absent from the first provider descendant;
- pinned comparison reports provenance/capability/auth/phase/terminal drift independently;
- floating lane cannot update production state;
- cancellation/budget exhaustion stops before starting the next case;
- logs and artifacts contain no credential values;
- direct CLI, ACP, bridge, host, and container results remain distinct evidence rows;
- ignored/unrun cases are explicit, never omitted.

## Completion

R3 is complete when R3a–R3f are merged, at least one pinned and floating run artifact exists for every
claimed bridge provider path, every direct or historic control is explicitly dispositioned, the
runner/credential/cost owner is documented, and a deliberate baseline promotion has been exercised.
OpenRouter/OpenCode live turns remain separately authorized; deterministic green gates alone do not
manufacture a support claim.
Update the central roadmap's next action to R4.
