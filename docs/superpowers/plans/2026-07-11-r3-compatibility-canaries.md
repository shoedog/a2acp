# R3 — Compatibility manifest and canary implementation plan

- **Status:** overall R3 **IN REVIEW**; R3a **MERGED** at `3927df3f` by PR #31; R3b
  **MERGED** at `504c1e43` by PR #32; R3c **IN PROGRESS** on
  `agent/reliability-r3c-floating-lane`. Nine pinned rows are implemented. Exact
  `c458045cf3d0923457519e253d22dd545363f98d` Sol/xhigh review approved the pre-incident deterministic
  tree. Authorized attempt 1 remains non-promotable stale-auth evidence; authorized attempt 2 passed both
  host paths and failed both reader paths before prompt acceptance when their runtime objects never started.
  The post-incident classification/cleanup fold passes binary **395 / 0 / 0**, affected bridge-core/ACP
  **514 / 0**, and the full serial workspace **2,085 / 0 / 12 ignored** across **70** test/doc-test
  executables. Format/diff, check, Clippy, locked release, hygiene **37/7**, manifest, and dependency-policy
  gates are green. The provider-unexercised release binary is 22,984,800 bytes at SHA-256
  `7c6cf5407fecb114c51ff211d8526df96c084d07217dc03f2913583c2481093d`; the bound manifest SHA-256 is
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. The earlier Linux/Rust 1.94
  binary **396 / 0 / 0** and Linux smoke CLI **15 / 0** gates predate this fold and were not rerun while
  local new-container starts remained degraded. Exact `a1641d0` Sol/xhigh review returned `REVISE` on
  pre-settlement cancellation ownership plus lifecycle-negative and legacy-compatibility coverage. Exact
  `d0be430` closure review fixed the live-runtime ownership and legacy findings, but returned `REVISE` on
  runtime-shutdown cleanup plus partial repeated-`Unknown` coverage. Exact `87c8f4e` closure review
  adjudicated both inherited items `FIXED`, found no new `WRONG`, and returned `APPROVE`; two
  resource/pathological-OS fault-boundary `SMELL`s remain accepted/nonblocking and unverified. The stale
  next-action wording it identified is fixed. The one clean-room Fable/xhigh/plan review of exact `a0c2c4c`
  independently found no `WRONG`, reported five nonblocking `SMELL`s, returned release verdict `READY`, and
  ended `GATE: APPROVE`. This docs fold closes its non-USD cost wording gap; all other verification/fault
  boundaries remain accepted/nonblocking. No Fable re-review will run.
- **Prerequisite:** R2c/R2d merged (`a6fec94c`, PR #29); R3a merged (`3927df3f`, PR #31);
  R3b merged (`504c1e43`, PR #32)
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
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. The current post-incident fold
  passes binary **395 / 0 / 0**, affected bridge-core/ACP **514 / 0**, and the full serial workspace
  **2,085 / 0 / 12 ignored** across **70** test/doc-test executables. The provider-unexercised release
  binary is 22,984,800 bytes at SHA-256
  `7c6cf5407fecb114c51ff211d8526df96c084d07217dc03f2913583c2481093d`; the bound manifest SHA-256 is
  `5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. All
  format/check/Clippy/release/hygiene/manifest/dependency-policy gates are green. The earlier Linux/Rust
  1.94 binary **396 / 0 / 0** and Linux smoke CLI **15 / 0** gates apply only to the pre-incident tree.
  An intermediate Linux run was **393/1** only because the container could not resolve this worktree's
  host-absolute `.git` pointer; the exact Git pointer/common-directory mounts restored repo identity and
  the unchanged test passed in the **394/0** rerun.
  The pinned baseline remains empty pending a future all-green, separately authorized aggregate. Fresh
  Sol/xhigh closure review of
  exact `c38978a` returned `APPROVE` with no `WRONG`; its sole nonblocking test-coverage `SMELL` is
  closed. A later exact-`f9f3e68` review returned `REVISE` on the pre-recovery deadline gap plus direct
  parser/comment smells. That fold had full host/Linux/merge-policy gates green. Sol subsequently approved
  exact `427d2ed` with all inherited items fixed and no findings.
  A post-review audit then demonstrated immediate-expiry inner-future polling and exact pinned third-party
  provider false-blocks. Review of exact `4574cbb` adjudicated both fixed, then returned `REVISE` because
  configure/prompt/drain retained inner-first deadline polling, the active gate inventories differed, and
  two operator surfaces allowed a rerun after only one green Claude doctor. Its provider-selector oracle
  `SMELL` is closed by an independent exact five-name assertion. The `9d10e6f` fold uses one deadline-first
  primitive through resolution/configure/prompt/drain, aligns the literal gate and two-doctor /
  one-new-four-case-aggregate contracts, and is full-gate green. Fresh Sol/xhigh review of exact `9d10e6f`
  adjudicated deadline ordering, two-doctor wording, and the provider-list oracle fixed; kept the release
  inventory partial and resolver fixture accepted/nonblocking; and returned `REVISE` because unpolled
  expired stages still serialized configure/prompt calls plus false acceptance and because active
  inventories omitted full binary/manifest bindings. The current fold tracks first poll before counting a
  stage, preserves exact timeout phase/last-completed evidence, repeats the full candidate binding on every
  active inventory surface, and is full-gate green. Fresh Sol/xhigh closure review of exact
  `c458045cf3d0923457519e253d22dd545363f98d` adjudicated both inherited `WRONG` findings fixed, retained
  the resolver fixture as accepted/nonblocking, found no new `WRONG` or `SMELL`, and returned `APPROVE`.
  The prompt-call
  assertion fails **0 / 1** on exact `9d10e6f` with actual `1` versus expected `0` and passes **1 / 0** on
  the current fold. The
  partially-progressed-resolver cleanup mutation `SMELL` remains accepted and nonblocking because the
  inspected ownership/drop, invalidation, and run-scoped backstop are not a direct acquisition fixture.

Fresh Sol/xhigh review of exact `a1641d063cc8564514bfc641e91f9f1ba323aa60` returned `REVISE` with one
`WRONG` and two `SMELL`s. The `WRONG` demonstrated that a registry initializer canceled after positive
`NotStarted` evidence but before timeout dropped `Supervised` without ever starting the exact named reap,
then allowed one successor initializer. The regression failed **0 / 1** on that reviewed tree by timing out
with zero reaps. The current fold establishes an unpublished-spawn guard immediately after process creation;
normal success transfers both owners, ordinary error joins cleanup, and cancellation at any pre-publication
await starts terminate-then-reap. The corrected regression uses Tokio `OnceCell`, observes exact client
exit before one reap, and permits one clean successor. The lifecycle-coverage `SMELL` is closed by direct
`container.runtime.start_failed`, repeated-`Unknown`, and synchronous/asynchronous probe-panic controls.
The compatibility `SMELL` is closed by proving a blocking conforming legacy `ReapFn` still runs detached and
does not delay the original spawn error. That fold then required exact-candidate Sol closure re-review.

Fresh Sol/xhigh closure review of exact `d0be43075e2ba9792bf9e47e5e3631ecf0d22b8b` marked the inherited
live-runtime cancellation `WRONG` and legacy compatibility `SMELL` fixed. It kept the repeated-`Unknown`
control `PARTIAL` because the 100 ms deadline permitted at most one observation, then returned `REVISE`
with one new `WRONG`: guard Drop could submit cleanup to the current Tokio runtime during shutdown, where
the new task need never be polled, leaving the exact named container behind. That shutdown regression failed
**0 / 1** on the reviewed tree with zero reaps. A second control then proved the same **0 / 1** zero-reap
result when shutdown occurred after ordinary-error settlement had already transferred both resources into
its runtime-bound task. The current fold terminates and reaps the exact client without Tokio, starts the
shared named reaper on a fresh OS thread/runtime, and retains one RAII join handle across the async
ordinary-error wait. The two corrected regressions prove client exit before one exact reap whether source
shutdown occurs before classification or during settlement; the strengthened unknown-state control counts
at least two observations before preserving the initialize timeout. A combined affected run also exposed one fixture-only 200 ms
exact-status timeout under post-compile load (**259 / 1** in bridge-core); default and all-feature isolated
controls passed, so the ordinary-status fixture now uses the production 1 s bound while the independent
20 ms hung-command cancellation control remains unchanged.

Fresh Sol/xhigh closure review of exact `87c8f4e096fbcd255bf97664cf6605cfb14c9e77` adjudicated the
runtime-shutdown `WRONG` and repeated-`Unknown` `SMELL` `FIXED`, confirmed the previously closed live-runtime
and legacy schedules remained fixed, found no new `WRONG`, and returned `APPROVE`. It accepted two
nonblocking fault-boundary `SMELL`s: OS-thread/fresh-runtime creation failure was not fault-injected, and the
five-second post-SIGKILL `try_wait` ceiling cannot prove exit under a pathological OS state. Its third
`SMELL` was the roadmap's stale “commit the fold” next action, fixed in this approval-recording docs fold.
The reviewer accepted the supplied gates without rerunning them and performed no container/provider action.

The one clean-room Fable/xhigh/plan adversarial implementation plus release/compatibility review of exact
`a0c2c4c5a526f99603702f826d5401aa39864d4d` independently inspected the full 29-path branch, found no
`WRONG`, returned release verdict `READY`, and ended `GATE: APPROVE`. It independently recomputed the
manifest, settings, and nine config hashes; verified the empty baseline; and rechecked both retained
aggregate files' mode, size, and SHA-256 without running a compatibility case. Its five nonblocking `SMELL`
groups are dispositioned as follows:

- the docs' negative/non-finite cost wording omitted the fail-closed non-USD-against-USD-cap case; fixed in
  this approval-recording fold;
- the exact external-provider truthiness agreement remains based on recorded pinned-SDK inspection rather
  than an in-tree oracle; accepted/nonblocking;
- OS-thread/fresh-runtime creation failure and the pathological post-SIGKILL ceiling remain the same
  explicitly unverified, accepted/nonblocking fault boundaries from the Sol review;
- intentionally broad fail-closed credential/Fable spelling rules and real-Podman label coverage remain
  accepted/nonblocking policy/coverage edges.

This was the only Fable review turn for the fold. It is review evidence, not compatibility evidence, and no
Fable re-review will run.

The initial fresh one-shot Sol/xhigh review of exact `57f3ee8` returned `REVISE` with two `WRONG`
findings and three `SMELL`s. The branch now keeps negative, non-finite, or non-USD cost history sticky across
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
recovery, preventing accepted runway from aging behind a fresh timeout; one deadline-first primitive refuses
without polling resolution, configure, prompt, or drain once expired. Truthy
Bedrock/Vertex/Foundry/Anthropic-AWS/Mantle selectors use
external host authentication and bypass first-party file OAuth, while false-like/unknown values and mounted
reader credentials do not. The original spawned regression
failed pre-change **1 passed / 1 failed** because the expired case reached the fake adapter. The newer
config-directory and delayed-recovery regressions also fail pre-change. The pre-attempt-2 post-`9d10e6f`
closure fold passed binary **395 / 0 / 0**, full serial workspace **2,071 / 0 / 12 ignored** across **70**
groups, Linux/Rust 1.94 binary **396 / 0 / 0**, and Linux smoke CLI **15 / 0**. Focused gates passed OAuth
doctor **5 / 0**, external-provider auth **1 / 0**, resolve-deadline **1 / 0**, execute-stage deadline
**1 / 0**, delayed-recovery CLI **1 / 0**, and expired/fresh CLI controls **2 / 0**. The provider-unexercised
release binary was 22,966,384 bytes at SHA-256
`323b4e219130480c9f0cafe90fe7c36d0a64ec17467707876698a82ef574a079`; the bound manifest SHA-256 is
`5d18cefef00972ead51dd7ad60da6e99cdc7d1c97a9b2f23cc17a5f5c235d828`. A fresh host login, post-login
sync, two green Claude doctors, and separate authorization then admitted one new four-case aggregate.
Attempt 1 must never be replayed or promoted.

That precondition was satisfied and the operator separately authorized attempt 2; it was a new aggregate,
not a replay. Candidate SHA-256 `323b4e21...a079` and the same exact manifest ran once with zero
retry/fallback. Codex/Fable host passed exact `PONG` in 6.853 s / 7.024 s. Both readers failed before
prompt acceptance in 30.430 s / 30.541 s: local spawn completed, ACP initialize timed out, and each exact
named container remained only `created` with a zero start timestamp. The aggregate ended non-cancelled in
74.853 s with 54,210 observed tokens, USD 0.227602 observed cost, no drift/budget violation, no promotion,
and all five controls unrun. It is retained owner-only at
`/private/tmp/a2a-bridge-r3b-live2.mbOljW/pinned-aggregate.json`, SHA-256 `319b3cf4...a9b3e`; it must never
be retried or promoted.

Both host passes, healthy proxy/network metadata, 247 GiB free host storage, and identical failures before
reader ACP traffic falsified provider/auth/egress/disk-specific causes. A no-network
`alpine:latest /bin/true` start also timed out before and after the two A2A objects were removed, while
runtime `info`, image listing, and exact-container inspection remained responsive. The settled boundary is
a local OrbStack/Docker new-container lifecycle stall; its initiating internal cause is unknown. The two
never-started objects were later removed exactly. OrbStack, running operator/user containers, turns, and
warm sessions were not restarted or killed.

The post-attempt deterministic fold keeps doctor read-only and adds the active startability boundary at the
actual production container spawn. A bounded exact-name runtime observer holds Spawn open while a positively
observed object remains pre-start; deadline then emits `container.runtime.start_timeout` as
`ContainerRuntime / ContainerFallbackCandidate`, with no Initialize transition and false prompt acceptance.
Started state preserves the ordinary Initialize path, while unknown observations preserve the prior diagnosis
rather than inventing container evidence. A bridge-owned production guard takes exact-client/controller
ownership immediately after spawn, transfers it only with a complete backend, and retains one independent
joined thread/runtime flight across ordinary error, cancellation, and source-runtime shutdown.
Public legacy callbacks retain detached fire-and-forget behavior. Typed reap failure is retained as a
bounded cause on the new start failure. The
classification, cleanup order, pre/post-settlement cancellation, one-successor `OnceCell`, `start_failed`,
started/unknown lifecycle, sync/async panic, deadline-first, parser, bounded-output/timeout, and legacy
regressions are deterministic. The pre-settlement cancellation and both source-runtime-shutdown regressions
each failed **0 / 1** with zero reaps; the deadline-first regression failed **0 / 1** with two runtime probes;
the original three mutations also fail under their exact pre-fix behavior. The full host suite passes
**2,085 / 0 / 12 ignored** across **70** test/doc-test executables; affected core/ACP tests pass **514 / 0**,
and binary tests pass **395 / 0**. No additional compatibility/model smoke ran; three source-only Sol reviews
ran. Exact `a1641d0` and `d0be430` reviews returned `REVISE`; exact `87c8f4e` returned `APPROVE` with no new
`WRONG` and two accepted nonblocking fault-boundary `SMELL`s. The one clean-room Fable/xhigh/plan review of
exact `a0c2c4c` independently found no `WRONG`, returned `READY`, and ended `GATE: APPROVE`; its nonblocking
findings are recorded above. R3b is **MERGED** at `504c1e43` by PR #32, with no Fable re-review.

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
final-sibling same-name replacement coverage, additional credential-shaped environment names, and explicit
negative, non-finite, or non-USD reported-cost handling that remains sticky across later usage snapshots and
fails closed against the USD-denominated cap.
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
- **State:** **IN PROGRESS** from clean base
  `504c1e434fd5845bc6745e0b0a0aae95427afbdd`
- **Design evidence:** one bridge-mediated clean-room Sol/xhigh read-only design pass inspected exact
  `504c1e43`. It ran no provider turn, package resolution, container action, build, test, or nested agent.
  The design turn is architecture evidence, not compatibility evidence.

### Core decision

R3c has three deliberately separate evidence levels:

1. A checked-in floating recipe manifest describes what “current” means.
2. `compatibility resolve` turns explicitly selected recipes into a private, exact, provider-free
   candidate bundle.
3. `compatibility run --resolution …` performs the existing one-prompt smoke only after a separate
   billable acknowledgement and records the actual session catalog and outcome.

A selector such as npm `latest` is a request, never resolved evidence. A successful floating run is only
`candidate_pass`; it is not support, promotion, a baseline update, or a production pin. Resolve and run
must remain separate commands: registry/image effects do not authorize a prompt, and a completed
resolution does not authorize all resolved cases.

Initial scope is exactly the four R3b supported bridge-smoke shapes: Codex host, Codex reader, Claude host,
and Claude reader. Historic direct CLI/ACP controls, Kiro, representative workflows, OpenRouter, and
OpenCode remain outside R3c.

### Authority and mutation boundaries

| Owner | May do | Must not do |
|---|---|---|
| Recipe author | Select reviewed package/config/image template enums, map one floating case to one pinned support case, and request a bounded selector | Supply commands, scripts, registry URLs, output paths, credentials, resolved identities, or support state |
| Resolver operator | Authorize registry traffic, private bundle writes, and uniquely named disposable image construction | Authorize a prompt, provider session, production mutation, shared-tag replacement, or operator-service action |
| Resolver | Write only below one retained output-directory capability plus runtime-owned unique image/cache state | Spawn an adapter, call a provider, read/copy credentials, use global npm state, execute lifecycle scripts, or write in a repository |
| Smoke operator | Authorize one exact resolution id, candidate binary, case set, owner, and budget | Implicitly authorize retries, fallback, re-resolution, promotion, or another candidate |
| Smoke runner | Revalidate the bundle and invoke the existing fixed-PONG smoke once per selected case | Repair drift, rebuild images, rewrite configs, open a second catalog session, or mutate production state |
| R4 promoter | Review and deliberately create production pins/baselines later | Be called by any R3c command |

Keep all of these byte-for-byte outside R3c's write authority:

- `compatibility/manifest.toml` and `compatibility/baselines/pinned.json`;
- `compatibility/configs/**`;
- `Cargo.toml` and `Cargo.lock`;
- `deploy/containers/*.Containerfile`;
- `docs/compatibility.md` and the support matrix/changelog;
- any selected running-operator config, tag, image, process, session, or service.

The resolver records and rechecks protected-input identities/hashes before publication. Any difference is
`protected_state_changed`. This is defense in depth: the primary proof is that no writable API or command
receives a protected path. Tests use sentinel files and injected escape attempts to prove the boundary.

### Checked-in recipe ownership

Add `compatibility/floating-current.toml`. It owns only:

- stable floating ids and one-to-one `baseline_case` mappings;
- closed npm package-set, config-template, and image-template enums;
- deliberately floating source selectors;
- fixed registry/runtime resource limits;
- artifact retention and strict-redaction policy.

It must not contain resolved versions/digests, generated paths, package inventories, model catalogs,
credentials, results, pins, or support claims. Validation rejects unknown fields, duplicate mappings,
unsupported or non-support baselines, direct/representative paths, writable execution, arbitrary registry
or path selectors, unknown templates, dependency cycles, and any command/script fragment.

The initial recipe set is:

- `codex-current`: `@agentclientprotocol/codex-acp@latest` plus its exact resolved
  `@openai/codex` dependency;
- `claude-current`: `@agentclientprotocol/claude-agent-acp@latest` plus its exact resolved
  `@anthropic-ai/claude-agent-sdk` dependency;
- one reviewed `node-acp-reader-v1` image template with a requested Node 24 slim base;
- four floating cases mapped to the four R3b support cases.

### CLI contract

Existing pinned commands retain their current behavior.

```text
a2a-bridge compatibility validate
    [--manifest <pinned-manifest> | --recipes <floating-recipes>]

a2a-bridge compatibility resolve
    [--recipes <path>]
    (--case <id>... | --all)
    --environment-owner <id>
    --runtime docker|podman
    --acknowledge-resolution-effects
    --out <new-directory>

a2a-bridge compatibility run
    --resolution <resolution.json>
    (--case <id>... | --all-resolved)
    --environment-owner <id>
    --acknowledge-billable
    --out <new-aggregate.json>

a2a-bridge compatibility compare
    --current <aggregate.json>
    [--baseline <pinned.json>]
    [--mode pinned|floating-to-pinned]
```

Admission rules:

- `resolve` is non-billable but requires effect acknowledgement before recipe access, output creation,
  registry/runtime calls, or scratch effects.
- `run` requires billing acknowledgement before resolution access, output creation, config inspection,
  runtime probing, adapter resolution, or provider spawn.
- Both commands require explicit selection. `--all` / `--all-resolved` are explicit opt-ins.
- Output must not exist, must be outside normal or bare Git repositories, and must have a
  descriptor-pinnable parent.
- Direct `run --lane floating-current` is rejected as `floating_resolution_required`; a hand-authored
  floating row cannot bypass resolution.
- `--manifest` and `--resolution` are mutually exclusive.
- `floating-to-pinned` accepts only an all-floating aggregate with a valid completed resolution binding.

### Resolution bundle

A successful owner-private bundle contains:

```text
<out>/
  resolution.json
  execution-manifest.toml
  configs/<case>.toml
  packages/<set>/package.json
  packages/<set>/package-lock.json
  packages/<set>/inventory.json
  packages/<set>/tree/...
  prerequisites/fable-settings.json
```

`resolution.json` schema version 1 records:

- `state = setup_incomplete | complete | failed` and a unique `resolution_id`;
- recipe and pinned-manifest canonical identities, schema versions, and SHA-256 values;
- candidate bridge canonical identity, SHA-256, and byte length;
- owner, OS, architecture, runtime executable identity, and fixed resolution limits;
- requested selectors separately from exact resolved adapter/nested CLI names, versions, integrity values,
  lock hash, sorted inventory hash, and bounded tree Merkle hash;
- requested base tag separately from exact registry/index and platform-manifest digests, generated template
  hash, final immutable image id, unique resolution-owned tag, and exact package labels;
- each generated config path/hash, baseline mapping, raw model/effort/mode copied from the pinned case,
  non-secret auth prerequisite names/destinations, and exact resolved bindings;
- `model_catalog.state = deferred_to_authorized_smoke`;
- protected-state before/after proof and a typed failure plus partial owned-resource inventory on failure.

Every resolved field rejects `latest`, ranges, aliases, mutable tags, missing integrity, or incomplete
versions. Generated execution cases remain `lane = "floating-current"`, use
`classification = "canary"`, and carry a `resolved` table. `pins` remains forbidden on floating cases;
`resolved` is forbidden on pinned cases. This keeps candidate evidence structurally distinct from
production policy.

The generated case copies its pinned counterpart's evidence path, probe, model, effort, mode, cwd, auth
path, prerequisites, budgets, and artifact policy. Only lane/classification, generated agent/config
materialization, exact package bindings, and image identity may differ.

### Provider-free resolution

Use a closed internal command enum, direct argv, bounded output, process-group timeout/cleanup, and a fake
executor for tests. Recipe text never becomes an executable, shell, Dockerfile fragment, registry URL, or
runtime subcommand.

For npm:

- create package metadata, `HOME`, npm cache/prefix/temp/user-config, and the materialized tree only below
  the bundle capability;
- use only the fixed npmjs registry;
- resolve with `--package-lock-only`, then materialize with
  `npm ci --ignore-scripts --no-audit --no-fund`;
- strip provider and credential variables from the child environment;
- reject install hooks, git/file/path dependencies, missing integrity, escaping symlinks, devices,
  sockets, duplicates, excessive files/bytes, and lock/tree mismatch;
- find the adapter executable through the package's bounded `bin` map, not guessed `.bin` paths;
- reuse a typed package-identity helper extracted from doctor provenance, never parse human
  `CheckResult.detail` strings.

For images:

- resolve the reviewed base tag to exact index/platform digests and generate the fixed
  `node-acp-reader-v1` build file internally;
- build from the digest and copy only the disposable package tree/lock plus non-secret settings;
- mount no credentials and pass no provider variables during build;
- use one resolution-unique tag and exact provenance labels;
- inspect and bind the immutable final id; generated configs reference only that id;
- never stop/restart/remove a container, replace a shared tag, or address the operator service.

The engine's pull/build cache is an acknowledged resolver effect. Successful unique tags and bundles are
retained as explicitly owned disposable resources; R3c does not implement broad or automatic cleanup.

### Same-session model catalog evidence

`resolve` must not call `models` or `describe_options`: ACP discovery starts an adapter/session and is not
provider-free. Instead:

- add a source-compatible default `AgentBackend::session_catalog(&SessionId) -> Option<AgentCaps>`;
- retain the initial bounded session/new model/effort/mode surface in `AcpBackend`'s live session;
- after lazy session minting and before smoke cleanup removes the session, add that catalog to the
  smoke-v2 target evidence;
- preserve one session and one prompt; never create a catalog-only session;
- preserve raw advertised order/current selections, but bound counts, field sizes, cumulative size,
  controls, and secret-shaped/exact-secret values before serialization;
- emit only a static rejection code for unsafe/missing catalog data and classify the floating result
  `candidate_unknown`;
- keep old smoke-v2 artifacts accepted; the R2d fallback parser accepts and ignores the additive field.

Catalog discovery alone is never a pass. A pass also requires exact PONG, clean terminal/cleanup evidence,
resolved provenance/config/image agreement, and no budget violation.

### Run binding, state machine, and outcomes

```text
RecipeValidated
  -> ResolutionAuthorized
  -> PackagesAndBaseResolved
  -> TreeAndImageMaterialized
  -> ConfigsRendered
  -> ProtectedStateRechecked
  -> Resolved

Resolved
  -> BillableAuthorizationRequired
  -> ResolutionRevalidated
  -> CandidateStaged
  -> CaseAdmitted
  -> OneSmokeRunning
  -> Observed
  -> ComparedToPinned
```

Immediately before any provider process, revalidate candidate binary, resolution/recipe identities,
generated manifest/config hashes, package inventory/tree/executable ownership, immutable image id/labels,
owner/platform/prerequisites, and aggregate time/token/cost headroom. Never re-resolve or repair. Drift is
`candidate_unknown`.

Aggregate schema additions are optional/backward-compatible:

- a resolution binding with resolution id plus artifact/recipe hashes;
- per-floating-case `baseline_case_id` and `candidate_outcome`;
- a summary count of `candidate_pass`, `candidate_fail`, and `candidate_unknown`.

Outcome truth table:

- `candidate_pass`: completed valid smoke, exact PONG, valid same-session catalog, all resolved bindings
  match, no drift, and no budget violation;
- `candidate_fail`: a valid smoke completed and definitively reported failure;
- `candidate_unknown`: unrun, runner/infrastructure/publication failure, invalid or missing evidence,
  catalog unavailable, binding drift, cancellation, or exhausted budget.

Floating failures and unknowns make the floating aggregate unsuccessful. They do not change the pinned
baseline comparison path or release support state. `floating-to-pinned` reports adapter, nested CLI, base,
image, catalog additions/removals/current selection, auth, capability, phase, terminal, and diagnostic
differences independently.

### Code ownership

- New `bin/a2a-bridge/src/compatibility_resolution.rs`: recipe/resolution DTOs, validators, typed executor,
  package inventory/tree hashing, config/image templates, protected-state guard, and atomic bundle
  publisher.
- `compatibility.rs`: CLI parsing, `Classification::Canary`, resolved bindings, run admission/revalidation,
  candidate outcomes, floating summary/comparison, and unchanged pinned behavior.
- `doctor.rs`: extract typed installed-package identity/provenance helpers for disposable trees.
- `bridge-core/src/ports.rs` and `bridge-acp/src/acp_backend.rs`: source-compatible same-session catalog
  access and teardown.
- `smoke.rs`: optional bounded catalog evidence captured before cleanup.
- `fallback_plan.rs`: old schema remains valid; accept/ignore the additive catalog.
- `bin/a2a-bridge/tests/compatibility_cli.rs` plus focused unit tests: CLI, effect barriers, atomicity,
  exact binding, classification, and no-mutation proof.
- `compatibility/floating-current.toml`: four recipes only.

### Atomicity, redaction, and failures

- Bundle directory mode `0700`; evidence/config/lock files `0600`; materialized package files become
  non-writable and executable entries owner-executable only.
- Publish valid `setup_incomplete` evidence first. Create children descriptor-relative with one link,
  reject symlink/name replacements, sync resources, and atomically replace with `complete` or `failed`.
- On post-rename directory-sync failure, restore the separately synced blocking setup copy, matching R3a.
- Preserve partial owned-resource inventory on failure; never emit raw npm/runtime stdout or stderr.
- Copy no credential values/files into the bundle or image. Record names and mount destinations only, and
  scan artifacts against exact present credential values plus the existing secret-shape backstop.

Static failure families include `recipe_invalid`, `resolution_ack_missing`, `npm_timeout`,
`base_digest_unavailable`, `package_identity_mismatch`, `package_tree_drift`, `image_label_mismatch`,
`protected_state_changed`, `write_scope_escape`, publication rollback failures,
`catalog_unavailable`, existing budget/cancellation reasons, and valid smoke failure. Dynamic subprocess
text is excluded.

### Red-before-green and mutation gates

Record these five exact pre-change reds on `504c1e43` before green implementation:

1. `compatibility resolve` is unknown.
2. A hand-authored floating row can reach `run` without a resolution.
3. A floating case has no mandatory generated-config hash.
4. A completed floating smoke failure can leave aggregate `success = true`.
5. Smoke does not retain the actual session catalog.

Recorded on unmodified `504c1e43` before the green implementation:

- `cargo run -q -p a2a-bridge -- compatibility resolve` exited 1 with unknown subcommand
  `"resolve"`.
- `cargo test -p a2a-bridge --test compatibility_cli
  validate_is_non_billable_and_accepts_the_versioned_manifest` passed **1 / 0** while its floating row
  had neither a resolution binding nor generated-config hash.
- `cargo test -p a2a-bridge --test compatibility_cli
  acknowledged_run_calls_the_smoke_contract_once_and_keeps_failure_evidence` passed **1 / 0** while
  invoking `run --lane floating-current` directly and returning command success for a completed FAIL.
- `cargo test -p a2a-bridge
  compatibility::tests::selected_cases_invoke_once_each_and_failures_are_not_retried` passed **1 / 0**
  while explicitly asserting aggregate success for one floating FAIL plus one floating PASS.
- `rg -n 'model_catalog|session_catalog' bin/a2a-bridge/src crates/bridge-core/src crates/bridge-acp/src`
  returned no match. There was no backend/session catalog contract or smoke evidence field.

These were deterministic contract probes only: no provider, registry, runtime, container, or output
materialization effect ran.

Every new path needs a pre-change-failing regression or exact removed-check mutation plus a negative/edge
control. Focused coverage must include:

- recipe duplicates/cycles/unknown templates/unsupported baselines/writable targets/path or registry
  escape;
- requested-selector versus exact-resolved grammar;
- direct-argv/no-shell safety and metacharacter controls;
- npm lifecycle-script suppression, isolated environment, credential stripping, integrity, tree bounds,
  and escaping symlinks;
- image digest/platform/label/shared-tag/id drift and runtime timeout/nonzero/oversized-output cases;
- byte-identical protected sentinels and rejected injected external writes;
- setup/resource/final/rename/directory-sync/rollback-name atomic failures;
- candidate, resolution, config, tree, executable, image, owner, and prerequisite drift before fake spawn;
- both acknowledgement barriers with zero reads/effects/calls when absent;
- one ACP session/new and one prompt, exact catalog order/current/options, and unsafe/missing/post-cleanup
  catalog controls;
- old/additive smoke-v2 fallback compatibility;
- complete pass/fail/unknown truth table and independent floating comparison dimensions;
- one smoke per selected case, zero retry/fallback, and stop-before-next cancellation/runner-failure paths.

### Implementation slices and commit order

1. **Contract and red tests:** recipe/resolution DTOs and validators, CLI skeleton, unresolved-floating
   refusal, candidate-outcome truth table, and the five pre-change reds. No external resolver effects.
2. **Provider-free resolution:** typed executor, isolated npm resolution/materialization, exact inventory
   and tree evidence, internal image/config rendering, protected-state guard, and atomic bundle. All package,
   image, effect, and failure-injection tests stay provider-free.
3. **Bound execution and observation:** `run --resolution` revalidation, same-session catalog capture,
   floating classification/summary/comparison, and R2d additive-schema compatibility.
4. **Checked-in recipes and operator docs:** four recipes, stable CLI/help/runbook, final handoff and gate
   evidence. Do not change the pinned manifest/baseline, production configs/locks/Containerfiles, support
   matrix, or changelog.

Implementation cursor on 2026-07-16: slice 1 is implemented on this branch. It adds strict recipe,
resolution, resolved-binding, canary-outcome, acknowledgement, and unresolved-floating admission
contracts, but deliberately has no registry/runtime/materialization executor. Focused gates are **7 / 0**
resolution-contract tests, **17 / 0** compatibility CLI tests, and **49 / 0** compatibility unit tests;
the complete `a2a-bridge` package gate is **470 passed / 0 failed / 11 ignored** across 16 groups, and
warnings-denied all-target Clippy is green. The ignored tests are the pre-existing explicitly live Kiro and
multi-bridge cases. No provider, registry, runtime, container, or output-materialization effect ran.

Each commit must build and keep existing pinned behavior green. Focused tests run first; final closure runs
format/diff, workspace check, warnings-denied all-target Clippy, the full workspace suite with exact totals,
locked release build, hygiene, pinned-manifest validation, floating-recipe validation, and Linux/Rust 1.94
coverage for modes, descriptor publication, candidate execution, and strict parsing.

Before merge require one fresh Sol/xhigh adversarial full-branch correctness review and, only after that is
green, one release/compatibility review focused on credentials, registry authority, mutable tags, cost,
mutation proof, artifact rollback, and non-promotion. Tag every finding `WRONG` or `SMELL` and adjudicate
inherited findings first. No review turn is compatibility evidence.

### Live gates and restart handoff

No live prompt or real registry/image resolution is needed for deterministic implementation closure.
Neither is authorized by the current session.

After deterministic gates and reviews:

1. Obtain explicit authorization for registry/image effects against one exact recipe SHA and case set.
2. Run one `compatibility resolve` and inspect the owner-private artifact/configs/package/base/image proof.
3. Run non-billable validation and doctor on generated configs. Do not call `models`.
4. Obtain separate authorization for one exact resolution id, candidate binary, case set, and budget.
5. Run one `compatibility run --resolution … --acknowledge-billable` with no retry/fallback.
6. Preserve failures as evidence and compare `floating-to-pinned` without updating the baseline or support
   docs.

Reader/Fable live cases also require recovered container starts and fresh green Claude credential doctors.
Those are environment prerequisites, not reasons to change R3c code.

Rollback is a normal revert of the R3c commits. Retain exact successful bundles and unique image tags until
operator-reviewed cleanup proves no running container uses them. Automated retention/disposal, parent-owned
scheduler deadlines, termination escalation, quarantine, and concurrency remain R3d. Promotion, production
pins/baselines, support wording, release integration, and rollback exercises remain R4.

**Restart point:** continue from the latest commit on `agent/reliability-r3c-floating-lane`; slice 1 is the
effect-free contract commit and its gates are recorded above. Implement slice 2 next with only fake
executors and provider-free package/image/materialization tests. Keep the pinned manifest and baseline
unchanged, and do not run a real registry, runtime, container, or provider operation without the separate
authorization described under live gates.

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
