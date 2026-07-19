# R3 — Compatibility manifest and canary implementation plan

- **Status:** overall R3 **ACTIVE**; R3a **MERGED** at `3927df3f` by PR #31; R3b
  **MERGED** at `504c1e43` by PR #32; R3c **MERGED** at
  `983398427c9f04861a2f1da501a7650c4a1cdd80` by PR #33; R3d design is **APPROVED / MERGED**
  at exact design head `b54840a017b87521677f1f95c3f7be69de55361d` by PR #37, merge
  `6eeea6ce553b792dc92cef95ee45f2234f7afe4e`. R3d0 is
  **MERGED** by PR #38 at merge commit `c2d147fb1f0df275f3c6452cdd212e185c002d08`. R3d1 is
  **APPROVED FOR PR** on `agent/reliability-r3d1-supervisor`; initial exact candidate `01438c34` and first closure head
  `e81ebbb` each received Sol/xhigh `REVISE`. Second closure head `8feda4d` marked all four requested topology/cursor
  residuals `FIXED`, found one new post-anchor-retention `WRONG / High`, no new `SMELL`, and returned `REVISE`.
  Third closure head `7fafe79` marked that item `FIXED`, confirmed prior residuals closed, found two new `WRONG`
  (`High` and `Minor`), no new `SMELL`, and returned `REVISE`. Fourth closure head `b55c17d` marked both inherited
  retained-capability findings `FIXED`, found one new signal-capable anchor-lifecycle `WRONG / High`, no new
  `SMELL`, and returned `REVISE`. Exact fifth-remediation head `b511d6c` received Sol/xhigh implementation
  `APPROVE` with no new finding, then the single Fable/xhigh release/compatibility lens returned `APPROVE` with no
  `WRONG` and three nonblocking Minor `SMELL`s. Its
  candidate release binary is 26,574,640 bytes at SHA-256
  `7d74f85aeeb22d25e226e45457fccc4038b5e1de81a8c084c3d226ca0b9bd154`. Its focused restart plan is
  [`2026-07-19-r3d1-supervisor-signal-parity.md`](2026-07-19-r3d1-supervisor-signal-parity.md).
  The merged R3d0 implementation was
  `agent/reliability-r3d0-foundation`: the fourth closure review approved exact cursor
  `b6f5c9e7af2ffd0a1b022e3f07c2898a3d2c65c4`, and proof-only test commit
  `e771067f4a7e742ad813368f01018b011e86bbce` isolates its sole nonblocking `SMELL`; exact
  `c548dc0edcc1b21bfb14aa3e78736d633ce0fdc7` confirmed the proof `FIXED` and mechanism unchanged,
  then returned `REVISE` on one stale-handoff `WRONG` and one no-effect-wording `SMELL`; exact
  `e9d030f07d4c623ad2d00d0c918d02486d32fb7b` marked the wording `FIXED` and handoff `PARTIAL`
  only on conditional publication language, with no new finding; exact
  `1d2fb80a2804a53b6f4076f10f4d4aea61a48f21` marked that remainder `FIXED`, found no new
  `WRONG` or `SMELL`, and returned `R3D0 DOCS REMEDIATION: APPROVE`; exact
  `d61176ca0c248fe884cffd320f34b073738729d0` received the independent Opus/xhigh release/
  compatibility lens, found no `WRONG`, four nonblocking `SMELL`, required no pre-PR remediation, and
  returned `R3D0 RELEASE/COMPATIBILITY: APPROVE`. The post-review deterministic owner-host validator and
  release-artifact check reconciled S4's stale prompt evidence to the branch's documented hashes; S1-S3 are
  accepted intentional constraints.
  No timer, private authority issuance, live characterization, model discovery, credential
  access, container/runtime access, registry/image effect, compatibility provider turn, GitHub check mutation,
  or production-operator lifecycle action has occurred. Nine pinned rows are
  implemented. Exact
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
  R3b merged (`504c1e43`, PR #32); R3c merged (`98339842`, PR #33); R3d design merged
  (`6eeea6ce`, PR #37)
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
- **State:** **MERGED** at `983398427c9f04861a2f1da501a7650c4a1cdd80` by PR #33
- **Current code head:** `4bd63f3f129a08586742c3c3e946fecfa02839ba`; deterministic full-branch
  gates are green. Sol/xhigh review of exact
  docs head `9d9f713d1ba72763efc67243c77da9e4425a4893` adjudicated all 14 inherited findings **FIXED**, reported no
  `SMELL`, and returned `GATE: REVISE` on one new archive-metadata allocation `WRONG`; `4bd63f3` closes it.
  Fresh Sol/xhigh closure review of exact docs head `056738111075317d3e7bcb3784491975e138e771`
  adjudicated all 15 inherited findings **FIXED**, found no new `WRONG` or `SMELL`, and returned
  `GATE: APPROVE`. The separate Opus 4.8/xhigh release/compatibility lens of exact clean `6637c13`
  found no `WRONG` or `SMELL`, returned `READY`, and ended `GATE: APPROVE`.
- **Design evidence:** one bridge-mediated clean-room Sol/xhigh read-only design pass inspected exact
  `504c1e43`. Beyond that review turn, it ran no provider compatibility prompt, package resolution,
  container action, build, test, or nested agent. The design turn is architecture evidence, not
  compatibility evidence.

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

- create package metadata, `HOME`, npm cache/prefix/temp/user-config, downloaded archives, and the
  materialized tree only below the bundle capability;
- give npm only the fixed CONNECT proxy and only enough authority to produce an exact lock with
  `--package-lock-only`; npm never receives tree-write authority;
- parse only fixed npmjs HTTPS tarball URLs and exact SHA-512 integrity from that lock, reserve one shared
  download budget before every chunk write, and let the bridge download, verify, preflight, and unpack one
  exact archive at a time;
- raw-preflight GNU long-name/long-link and local/global PAX metadata records against a 1 MiB per-record cap
  before tar preprocessing in both planning and materialization; account the PAX-effective file size and
  reject plan/materialization size drift before creating the output file;
- strip provider and credential variables from the child environment;
- reject install hooks, git/file/path dependencies, missing or bad integrity, non-HTTPS/non-npmjs URLs,
  hardlinks, escaping symlinks, devices, sockets, duplicate paths, excessive files/bytes, and lock/tree
  mismatch before publication;
- reserve the full descriptor-relative tree's aggregate entries and declared regular-file bytes before the
  first package entry write; reopen ancestor directories on demand instead of retaining one descriptor per
  directory;
- apply the package's bounded `bin` map to the target file's executable mode, without creating `.bin`
  shims or guessing paths;
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

- a resolution binding with resolution id plus artifact, recipe, and production-manifest hashes;
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

Implementation cursor on 2026-07-17: all four implementation slices and seven adversarial-review rounds are
complete through the latest code fix. `1c1115cb` defines the strict recipe/resolution contracts and the five
recorded red-before-green boundaries; `0c25686c` adds the provider-free typed package/image/config
materializer and atomic private bundle; `e159915` adds exact pre-provider revalidation, same-session bounded catalog evidence,
floating pass/fail/unknown truth, and independent floating-to-pinned comparison while preserving old
smoke-v2 fallback compatibility. `356092f` adds the four-case recipe and stable operator runbook; `57e63a0`
is the lint-only original implementation head. The first Sol/xhigh review of exact `a5dfef8` returned nine
`WRONG`, no `SMELL`, and `GATE: REVISE`: npm could contact non-npmjs URLs before validation; resource bounds
were post-effect; infrastructure failure/cancellation could remain definitive; resolver cancellation could
orphan descendants; inventory omitted mode/owner; unrelated baselines were accepted; catalog-only drift
leaked into generic capability; aggregate cancellation/budget was omitted; and the cursor overstated the
reviewed head. `e3459a5` closes all nine. `d86e418` adds the missing catalog-only regression after an exact
old-code mutation proved the broader test was vacuous for that behavior. The closure review of exact
`646d61bada59146be3818c6fed816f8c969f5bda` adjudicated inherited findings 1 and 3-9 fixed, but left the
shared resource-bound finding unresolved because a 10 ms watcher plus per-file `RLIMIT_FSIZE` could not
prevent a transient aggregate overshoot. It also found a new process-group reuse `WRONG`: normal completion
reaped the group leader before later `kill(-PGID)`, so the numeric group could be reassigned. Exact
`f15ae88eae4e8ec3d0e8ed5d45c1b8a101f9dcc8` removes npm tree-write authority, verifies and unpacks exact
integrity-bound archives under bridge-owned pre-reserved hard aggregate limits, reopens descriptor-relative
ancestors instead of retaining unbounded directory descriptors, and keeps a trusted live process-group
leader until group termination and reap are complete. The next Sol/xhigh review inspected exact
`5facc9c7d8438afdec3ad2bb92358154e0a743dd`, adjudicated inherited findings 1 and 3-10 **FIXED**, but left
finding 2 **PARTIAL** because production still reserved and wrote each package before preflighting the next.
It found one new `WRONG`: an integrity-matching ordinary transitive archive could omit `package.json` or
claim another name/version because the lock entry's archive identity was discarded. It reported no
`SMELL` and returned `GATE: REVISE`. Exact `b3793e834056d8c9d58a1cae815e463aa124d72b` downloads, verifies,
and preflights every selected archive; accumulates and commits one complete selected-tree reservation before
the first package-entry write; then reopens, materializes, and removes one archive at a time. Each archive
must contain a bounded `package.json` whose name/version equals the lock entry, including an explicit npm
alias name when the install path differs from the archive identity. The next Sol/xhigh review inspected exact
`260e4a61765246fa33002e98778a50171f45e143`, adjudicated all 11 inherited findings **FIXED**, reported no
`SMELL`, and returned `GATE: REVISE` on two new `WRONG` items. A correct-identity ordinary transitive archive
could declare a missing bin target because only found targets were checked. Separately, byte-sensitive
archive/reservation sets allowed `Foo` and `foo` to pass preflight even though they collide on a
case-insensitive destination after the first write. Exact
`4621ab5cd01612db2f72fae8b9b9b467def5fb93` requires every declared bin target to name a planned regular
file. It also defines one fail-closed portable package-tree namespace: paths and symlink targets must be
UTF-8 ASCII, and archive entries, implicit/explicit directories, and leaves share case-insensitive keys
while preserving their original spelling for descriptor-relative writes. The next Sol/xhigh review
inspected exact `af698066456bc76f5f52a725221bbafb574fd154`, adjudicated all 13 inherited findings
**FIXED**, reported no `SMELL`, and returned `GATE: REVISE` on one new `WRONG`: a symlink target's lowercase
key was discarded, so a target spelling that resolved on case-insensitive macOS could be retained verbatim
and become dangling in the Linux reader image. Exact `dd99267ba4bd806b1cd33939cc2d0f16505d2f3f`
syntactically normalizes each in-package target and rejects it before writes when a portable-equivalent
planned path has different spelling. An exact-spelling target remains accepted. The next Sol/xhigh review
inspected exact docs head `9d9f713d1ba72763efc67243c77da9e4425a4893`, adjudicated all 14 inherited
findings **FIXED**, reported no `SMELL`, and returned `GATE: REVISE` on one new `WRONG`: non-raw
`tar::Archive::entries()` buffers GNU long-name/long-link and local PAX extension bodies before the bridge's
entry/file limits, so a highly compressed oversized extension could allocate outside the configured bounds.
Exact `4bd63f3f129a08586742c3c3e946fecfa02839ba` runs bounded raw metadata preflights before both non-raw
planning and materialization passes, caps all four GNU/PAX extension types at 1 MiB per record, accounts
PAX-effective file sizes in the plan, and rejects effective-size drift before creating an output file.

With the two closure regressions applied to the pre-fix code, the focused gate failed **47 / 2**: the first
package had already published `node_modules` when the later package exceeded the complete-tree limit, and a
wrong-identity archive was accepted. A separate removed-reservation mutation failed its focused hard-bound
test **0 / 1** because the over-limit materialization returned `Ok`; restoration passed **1 / 0**. Latest
focused resolution gates at `b3793e8` were **50 / 0**. With the three new red-first controls applied to
`260e4a6`, the focused gate failed **50 / 3**: missing bin, portable case collision, and non-ASCII namespace
acceptance each returned `Ok`; focused resolution gates at `4621ab5` passed **54 / 0**. The new
symlink-spelling regression failed **0 / 1** against `af69806` because planning returned `Ok`; focused
resolution gates at `dd99267` passed **55 / 0**. Against the exact reviewed production tree, the new
oversized-metadata regression failed **0 / 1** because it did not return the pre-parse bound error, and the
PAX-effective-size regression failed **0 / 1** because the plan retained the raw size **8** instead of **4**.
Exact-`4bd63f3` focused resolution gates pass **61 / 0**, including exact-edge and one-over controls for GNU
long-name/long-link and local/global PAX metadata plus materialization-before-write and effective-size
controls. The exact `646d61b` pre-hardening host workspace passed
**2,141 / 0 / 12 ignored** across **70** test/doc-test executables, with compatibility unit **54 / 0**,
compatibility CLI **20 / 0**, and ACP same-session catalog, smoke catalog, and additive fallback controls
**1 / 0** each. Its format/diff, workspace all-target check, warnings-denied all-target/all-feature Clippy,
locked release, hygiene **37/7**, pinned manifest, four-case recipe, protected-input identity, and dependency
policy gates were green. Exact-`4bd63f3` host gates pass **2,165 / 0 / 12 ignored** across **70** test and
doc-test executables. Format/diff, workspace all-target check, warnings-denied all-target Clippy,
locked release, hygiene **37/7**, pinned manifest **9 cases**, floating recipe **4 cases**, protected-input
identity, and dependency policy are green. During this fold, a grouped focused run again reported the
unrelated existing cancellation-descendant assertion failed **0 / 1** after **59** other tests passed; its
immediate isolated rerun passed **1 / 0**, and three subsequent full-workspace runs passed it. This recurring
timing-sensitive signal remains reported and unmodified rather than rebaselined. Fresh exact-head
Sol/xhigh closure review of exact `056738111075317d3e7bcb3784491975e138e771` adjudicated all 15 inherited
findings **FIXED**, found no new `WRONG` or `SMELL`, and returned `GATE: APPROVE`; the reviewer accepted the
supplied gates and ran no builds, tests, containers, registries, providers, or network services. A separate
clean-room Opus 4.8/xhigh release/compatibility review inspected exact clean
`6637c13b7e3f82dde4f59790c40d8e0eded47aa6`, independently challenged credentials/secrets, registry and
dependency authority, mutable images/tags, cost/admission, artifact atomicity/rollback/retention, and
promotion/compatibility truth, found no `WRONG` or `SMELL`, returned release determination `READY`, and ended
`GATE: APPROVE`. It accepted the supplied deterministic gates and ran no builds, tests, containers,
registries, provider smokes, model discovery, image resolution/build, or promotion action.

Three preceding Claude diagnostics are explicitly not reviews: operator Fable, fresh-isolated-process
Fable, and operator Opus each incorrectly supplied `a2a-bridge.mode=read-only` to the Tier 0 prompt-only
Claude agent. Mode application precedes model configuration, so each request failed as `agent crashed` in
about 0.5-0.6 seconds with no `acp.config_resolved`, review output, or usage. The fresh isolated failure
falsified stale warm-session state; a corrected Opus request that omitted only `mode` emitted both
`acp.config_resolved` records and completed normally. This confirms a controller-request mismatch rather
than Fable/Opus degradation. The corrected Opus turn is the one policy-limited second opinion after the
green Sol correctness review; no Opus re-review is required.
Prior removed-check mutations produce these exact reds: unauthorized CONNECT admission wedged the negative
proxy test until bounded termination; a per-proxy counter left shared budget **5** instead of **2**; removing
RLIMIT plus the watcher returned late `PackageTreeDrift` instead of immediate `NpmDownloadBudgetExceeded`;
authentication became `Fail` instead of `Unknown`; cancelled active evidence stayed `Pass` instead of
`Unknown`; a resolver descendant survived cancellation; erased security metadata rejected the sealed-tree
positive control; an unrelated pinned hash returned `Ok`; aggregate cancellation emitted no
`__aggregate__` change; and old catalog comparison emitted both `capability` and `catalog.current_model`
instead of only the latter. Each green test retains its exact-bound, valid-definitive, direct-child,
same-content, malformed-binding, identical-catalog, or unchanged-aggregate control as applicable.

New deterministic controls cover single- and multi-package aggregate entry/byte limits one below and at the
exact edge before any `node_modules` publication; exact-edge and one-byte-over streaming downloads; bad
integrity; archive deadline, hardlink, special-file, duplicate, and escaping-link rejection; missing,
mismatched, exact, and explicit-alias archive identities; npm alias versus ordinary semver; host-plus-Linux
package selection; owner normalization below Darwin `/private/tmp`; more sibling directories than the macOS
soft descriptor ceiling; present/missing package-bin targets and mode/path normalization without `.bin`;
portable-equivalent leaf/implicit-directory rejection; non-ASCII entry/symlink-target rejection;
portable-only symlink-target spelling rejection with an exact-spelling positive control; bounded raw GNU/PAX
metadata at the exact edge and one byte over before either non-raw pass; PAX-effective planning and
materialization; materialization rejection before output writes; and retention of the process-group anchor
through final cleanup. The process-group regression failed against the pre-fix
leader-reap ordering and passes at `f15ae88`; complete-tree and identity regressions pass at `b3793e8`;
bin/portable-namespace regressions pass at `4621ab5`; symlink-target spelling passes at `dd99267`; metadata
allocation and effective-size regressions pass at `4bd63f3`.

The earlier `57e63a0` Linux/Rust 1.94.0 run passes complete `a2a-bridge` package **508 / 0 / 11 ignored**
across **16** groups, including binary **434 / 0**, compatibility CLI **21 / 0**, smoke CLI **15 / 0**, plus
ACP catalog **1 / 0**. It is historical evidence, not an exact-`4bd63f3` rerun: cleanup removed the local
Rust image, Docker has no equivalent cached image, and no new image pull was authorized. The current
provider-unexercised release candidate is 24,673,456 bytes at SHA-256
`be83cb71834051c5ae2f5a9ce590377061de086187e5069f8c44001b2c71aa7c`; the recipe SHA-256 is
`11d8f50de5515b2f6703741c9a00980e1dc96f766e6370677fd654a0968f0160`. The pinned manifest/baseline,
production configs, Containerfiles, compatibility matrix, support matrix, and changelog remain byte-identical
to `504c1e43`; `f15ae88` deliberately changes the bridge crate dependencies and workspace lock for its
bridge-owned gzip/tar path. No compatibility/provider smoke turn, model discovery, compatibility aggregate,
image resolution or build, operator rebuild, or operator swap ran; the recorded review turns are review
evidence only.

Each commit must build and keep existing pinned behavior green. Focused tests run first; final closure runs
format/diff, workspace check, warnings-denied all-target Clippy, the full workspace suite with exact totals,
locked release build, hygiene, pinned-manifest validation, floating-recipe validation, and Linux/Rust 1.94
coverage for modes, descriptor publication, candidate execution, and strict parsing.

The fresh Sol/xhigh adversarial full-branch correctness requirement is satisfied at exact `0567381`. The
separate release/compatibility requirement is satisfied by the corrected Opus 4.8/xhigh review of exact
`6637c13`, with no `WRONG` or `SMELL`, release determination `READY`, and `GATE: APPROVE`. Both reviewers
adjudicated inherited findings first where applicable. No review turn is compatibility evidence.

### Live gates and restart handoff

No live prompt or image resolution is needed for deterministic implementation closure. Explicitly
authorized, provider-free host package-resolution diagnostics exercised the registry/materialization path
before `f15ae88`: the retained Codex bundle resolved codex-acp **1.1.4** plus Codex **0.144.5** and passed
doctor **10 ok / 0 warn / 0 fail**; the retained Claude bundle resolved claude-agent-acp **0.59.0**,
Claude Agent SDK **0.3.207**, and bundled Claude **2.1.207**, then passed doctor
**11 ok / 0 warn / 0 fail**. Both owner-private, sealed bundles include host and Linux/glibc arm64 package
variants and left no npm cache/temp/prefix/home state. The failed diagnostic attempts exposed alias/semver,
Darwin ownership, descriptor retention, published bin-mode, and safe-leading-`./` defects that the current
deterministic tests now cover; those failed bundles were removed after their evidence was folded.

Those diagnostics started no adapter/provider session, called no `models`, built or inspected no image,
produced no aggregate, and spent no provider turn. The two retained successful bundles predate all of
`f15ae88`, the complete-selected-tree reservation in `b3793e8`, the portable namespace/bin enforcement in
`4621ab5`, symlink-target spelling enforcement in `dd99267`, and archive-metadata bounds in `4bd63f3`, so
they are diagnostic evidence rather than exact-current compatibility or promotion evidence.

After deterministic gates and reviews:

1. Obtain explicit authorization for exact-current registry/image effects against one exact recipe SHA and
   case set.
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

**R3c closure:** R3c merged at `983398427c9f04861a2f1da501a7650c4a1cdd80` by PR #33.
`4bd63f3f129a08586742c3c3e946fecfa02839ba` is its final code head and exact
`6637c13b7e3f82dde4f59790c40d8e0eded47aa6` is the approved release-review boundary. Focused resolution
tests passed **61 / 0**; full host passed **2,165 / 0 / 12 ignored** across **70** groups, with release,
hygiene, manifest/recipe, protected-input, and dependency-policy gates green. Exact-`0567381` Sol/xhigh
closure review approved all 15 inherited findings with no new `WRONG` or `SMELL`; exact-`6637c13` Opus
4.8/xhigh release review found no `WRONG` or `SMELL`, returned `READY`, and ended `GATE: APPROVE`.
Linux/Rust 1.94 remains green only on historical `57e63a0` until an image pull is separately authorized.
Keep the pinned manifest/baseline and every protected production/support input unchanged. R3d does not
authorize an exact-current compatibility resolution, model discovery, container/image effect, provider
turn, or production-operator lifecycle action; each live gate below retains its own authority boundary.

## R3d — owner-bound scheduling and evidence retention

- **Branch:** `agent/reliability-r3d0-foundation`
- **Base:** merged R3d design main `6eeea6ce553b792dc92cef95ee45f2234f7afe4e` (PR #37)
- **Status:** design of record **APPROVED / MERGED** at exact design head
  `b54840a017b87521677f1f95c3f7be69de55361d`; R3d0 default-off policy/schema implementation is
  **RELEASE/COMPATIBILITY APPROVED / PR READY** at exact mechanism commit
  `5baeeb3f47183ea2a47d2cdc5ffce26f1df7dbfb`, approved cursor
  `b6f5c9e7af2ffd0a1b022e3f07c2898a3d2c65c4`, and proof-only test head
  `e771067f4a7e742ad813368f01018b011e86bbce`; exact proof-confirmation cursor
  `c548dc0edcc1b21bfb14aa3e78736d633ce0fdc7` found docs-only remediation, and exact second-
  confirmation cursor `e9d030f07d4c623ad2d00d0c918d02486d32fb7b` left only conditional publication
  wording; exact final Sol cursor `1d2fb80a2804a53b6f4076f10f4d4aea61a48f21` approved that docs
  remediation, and exact Opus/xhigh cursor `d61176ca0c248fe884cffd320f34b073738729d0` approved the
  release/compatibility lens. No timer, private
  authority issuance, live characterization, model discovery, credential access, container/runtime access,
  registry/image effect, compatibility execution turn, GitHub check mutation, or production-operator lifecycle
  action has occurred
- **Initial review:** one clean-room Fable/xhigh/plan review of exact base `98339842` returned six
  `WRONG`, thirteen `SMELL`, and `R3D DESIGN: REVISE`. Its retained local report is
  `/Users/wesleyjinks/.claude/plans/r3d-clean-room-adversarial-deep-hollerith.md`, mode `0644`, 40,180
  bytes, SHA-256 `9ac3c33300135e48f89c65b7f2076ccc24301137a5c09926044112bf39a19e4a`.
  The reviewer invoked an internal exploration helper despite the no-nested-agent charter; treat the
  report as adversarial input, not independent proof. The concrete findings were adjudicated against the
  repository and the owner decisions below. Do not spend a Fable re-review merely to relabel findings.
- **First Sol review:** one fresh bridge-mediated Sol/xhigh/read-only review of exact
  `a20db1992e16993061eda9cfc8d297791fdf1466` returned four `WRONG`, seven `SMELL`, and
  `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-review-a20db199.XvVxDw/review.md`, mode `0644`, 21,877 bytes,
  SHA-256 `30835e786959019c2a96c3f4d0c1eda1fa4e23612afafd89e820592b1e708aee`. It used one
  one-node provider turn, did not read the Fable report or invoke a nested reviewer, and left the
  production operator untouched. Its concrete mechanisms were folded in exact `d5041ee`.
- **First closure review:** one fresh bridge-mediated Sol/xhigh/read-only review of exact
  `d5041eedb8c95397600851db36de822182c9565a` adjudicated all eleven inherited findings `FIXED`,
  then returned three new `WRONG`, three new `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-d5041ee.FVPJeA/review.md`, mode `0644`, 18,966 bytes,
  SHA-256 `03e404d559d16b29701b1606653979c943b86affdca1f14b80c8aa3a9609b701`. Exact
  `1c3a7ce` folded its canonical execution fingerprint, legacy-entrypoint boundary, explicit owner-iCloud
  privacy contract, scheduled-case registry, ongoing cold verification, and consumed-evidence mapping.
- **Second closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `1c3a7ce0e2c52e0181ccd3bdfd81776d402b7565` marked five of the six inherited findings `FIXED`,
  the scheduled-registry item `PARTIAL`, and found no regression in the earlier eleven. It then returned two
  new `WRONG`, three new `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-1c3a7ce.8VATsb/review.md`, mode `0644`, 16,643 bytes,
  SHA-256 `f5e84d0c1ae800fc24255faa61302dd7d95d6922a54cc3cc8084f42dfb486588`. Exact
  `9414aa8` addressed its lane-wire, exact-base invalidation, handoff-cursor, expected/observed identity,
  independent storage-consent, and scan-before-iCloud findings.
- **Third closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `9414aa8f61f31b0b1286b88bc45f558f79f26d3b` marked four of six inherited items `FIXED`, two
  `PARTIAL`, and found no regression in the earlier fixed mechanisms. It then returned two new `WRONG`, no
  new `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-final-9414aa8.gTNxSa/review.md`, mode `0644`, 16,072 bytes, SHA-256
  `6cc83e07d3ac9d97e9ea0ee4d2f357278f24841e80bc3ff04c64519acb6b4f95`. This revision
  replaces watcher-timed base/head gating with an exact required current GitHub test-merge result, adds the
  strict
  scheduled-execution source, finishes authority terminology, and reconciles the merged R3c status.
- **Fourth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `6bc06fe6118c8b6e193be666e4b4961546916761` marked all four inherited items `FIXED`, found no
  regression in earlier fixed mechanisms, then returned one new `WRONG`, one new `SMELL`, and
  `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-6bc06fe.uJsgZS/review.md`, mode `0600`, 12,872 bytes,
  SHA-256 `1e8c35ce653ddbdb66c43ebe71a0cf49dc9eeed9decf1d4fcc87805a9e0b9317`. This revision
  makes a published success valid for its immutable SHA lifetime, removes the unenforceable same-SHA
  freshness promise, and adds action-time branch-rule/context/source revalidation.
- **Fifth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `a7db6e7037a3ad21e9db19ac529264977ffaaa10` marked both inherited items `FIXED`, found no
  regression in any earlier fixed mechanism and no new `WRONG`, then returned one new `SMELL` and
  `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-a7db6e7.NiCZ9g/review.md`, mode `0600`, 15,561 bytes,
  SHA-256 `b3293a39c1c0af10b83080b55fe9e45c266873b01d26784d96fa2cad9d31a796`. This revision adds
  a single-check-run GitHub publication outbox with exact-id reconciliation and no provider, consumption,
  or terminal-check replay.
- **Sixth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `c241087b02989e347a812c90b1ded1ff2f21aa01` marked the inherited outbox finding `FIXED` and found no
  new standalone `SMELL`, then found one transient-confirmation regression `WRONG` and returned
  `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-c241087.Z5Mfk3/review.md`, mode `0600`, 15,886 bytes,
  SHA-256 `b6f300b6598d38cebeab50d4b8ef9d4a45bad854e14bd53ab0344ab7f965d7a2`. This revision keeps a
  first transient failure `in_progress` and terminalizes the same check only on immutable failure or the
  separately authorized confirmation's pass/second identical failure.
- **Seventh closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `e0cc7dca4f5738d8f717921fcaba4eda86b6fd22` marked the transient-confirmation mechanism `PARTIAL`,
  found no new `WRONG` or other regression, then returned one multi-case convergence `SMELL` and
  `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-e0cc7dc.T4ns7v/review.md`, mode `0600`, 11,693 bytes,
  SHA-256 `de6fa68c1e893e4a02376581dcabb8b9302b2a36e3ed4ec01e3cfeacdd6936ea`. This revision adds the
  complete ordered due-set reducer and pass/pass, pass/fail, crash, and SHA-regeneration fixtures.
- **Eighth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `c50811f345718736f35411263b2351141db819c7` marked multi-case convergence `FIXED` and found no
  regression in earlier closed mechanisms, then returned one repeated-unknown suppression `WRONG`, no new
  `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-c50811f.UZux95/review.md`, mode `0600`, 10,591 bytes,
  SHA-256 `5b3be5b5ee762618a88a20bb5436c53057b4bd04d29bcf68c85eb6a6de0492a1`. This revision keeps repeated
  non-waste `candidate_unknown` in its existing lifecycle and reserves suppression for independently typed
  immutable waste or the second identical complete typed transient failure.
- **Ninth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `fb8a2f48a810c4975c386b93f6dd5db06ee6ede6` marked the repeated-non-waste-unknown finding `FIXED`,
  found no regression in earlier closed mechanisms, then returned one initial-characterization authority
  `WRONG`, no new `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-fb8a2f4.VP48YA/review.md`, mode `0600`, 12,032 bytes,
  SHA-256 `af6e8ce71696781f080a1592fa689575f5c9da84c8f6d14398220eb5a577b948`. This revision
  adds mutually exclusive one-shot characterization and post-characterization standing-grant admission arms,
  so bootstrap never fabricates an authority or characterization identity that does not yet exist.
- **Tenth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `ae9db39180e8e9f5ae55152ab160cd8cd32b8ae5` marked the bootstrap-authority finding `FIXED`, found no
  regression in earlier closed mechanisms, then returned one characterization-versus-execution identity
  `WRONG`, one one-shot-uniqueness `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-ae9db39.TfHxcx/review.md`, mode `0600`, 14,973 bytes,
  SHA-256 `4c443ee88d3ad50c669fe0ebe61eb3c755c5ac5b583a70656c610171ac480d42`. This revision
  separates stable effect-shape characterization from exact drift-execution identity and enforces one live
  characterization entry per profile across batches.
- **Eleventh closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `2eb242a5deb6db561878e721db0132d3e5c4606e` marked duplicate-entry handling `FIXED`, the identity split
  `PARTIAL`, and found no regression outside deduplication. It returned two residual identity-layer `WRONG`
  findings, one overview-wording `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-2eb242a.YhxiDC/review.md`, mode `0600`, 13,189 bytes,
  SHA-256 `602dd9259c25b9e70b7fb544e813aeb8e5b9336db53b2be5a843524b16fedf6d`. This revision
  replaces the standing grant's ambiguous exact-manifest binding with a stable named profile-policy bundle,
  keeps trigger/request/window/attempt identity admission-only, and reconciles the overview terminology.
- **Twelfth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `8dc60542eaff8579224c75112ca7a9a7f293cf3b` marked the stable-bundle repair and overview reconciliation
  `FIXED`, trigger-independent execution identity `PARTIAL`, and found no regression in the earlier closed
  mechanisms. It returned two new `WRONG`, one new `SMELL`, and `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-8dc6054.OvmshJ/review.md`, mode `0600`, 14,130 bytes,
  SHA-256 `f680c3517af959f086d3e2fa1445b0e1fb329bba269db901e30a76b36949fc8f`. This revision
  gives generic manual work a one-run admission identity, stages every D7-reachable claimed-support profile,
  carries R3c's non-reusable process-group anchor into R3d1, and revokes live one-shot entries on rollback.
- **Thirteenth closure review:** one fresh one-node bridge-mediated Sol/xhigh/read-only review of exact
  `cc01a52aaf8279106f095182989ce15667f0cce2` marked the support-profile inventory, process-group anchor,
  and rollback repairs `FIXED`, found no regression or new `WRONG`/`SMELL`, and left generic manual admission
  `PARTIAL` because an earlier unqualified transaction still required a persistent envelope arm. It returned
  `R3D DESIGN: REVISE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-cc01a52.Y6zfQO/review.md`, mode `0600`, 10,390 bytes,
  SHA-256 `a5f656fcaee56e3882c7bc809f796e41eaa9afd61311f26a5f19594f5edf83d0`. This revision
  splits final admission into mutually exclusive persistent-envelope and one-run generic-manual transactions
  under the same lock/order and adds the absent-arms positive plus mixed-arm negatives.
- **Fourteenth closure review and design approval:** one fresh one-node bridge-mediated Sol/xhigh/read-only
  review of exact `b54840a017b87521677f1f95c3f7be69de55361d` marked the final generic-manual transaction
  finding `FIXED`, found no regression and no new `WRONG` or `SMELL`, required no amendment, and returned
  `R3D DESIGN: APPROVE`. Its fixed output is
  `/private/tmp/a2a-bridge-r3d-sol-closure-b54840a.ObmMDa/review.md`, mode `0600`, 12,737 bytes,
  SHA-256 `4eba120a5cecce94c4afca10d5e9ecd4da4047136a8f638d927675aa4328c249`. This exact commit is
  the approved design-of-record boundary; the following status-only fold records that verdict without changing
  a mechanism, schema contract, slice, test obligation, or live gate.
- **Initial R3d0 implementation review:** one fresh bridge-mediated Sol/xhigh/read-only review of exact
  implementation commit `e7e5fa14da127511c080e802625afbcc455d94e1` returned eleven `WRONG`, two
  `SMELL`, and `R3D0 IMPLEMENTATION: REVISE`. Its retained output is
  `/private/tmp/a2a-bridge-r3d0-sol-review-e7e5fa1/review.md`, mode `0644`, 23,542 bytes, SHA-256
  `ad2c5207b654269b2599b360aa88067521ef83abc9e09843a88bee5e9de57de5`. Exact remediation
  commit `f4f242fdc48827cb56d511e1037fe2ec46d9d4fd` was the first remediation attempt against those
  thirteen findings.
- **First R3d0 implementation closure review:** one fresh bridge-mediated Sol/xhigh/read-only review of exact
  `f4f242fdc48827cb56d511e1037fe2ec46d9d4fd` marked six inherited items `FIXED`, seven `PARTIAL`,
  found two new `WRONG` and two new `SMELL`, and returned `R3D0 IMPLEMENTATION: REVISE`. Its retained
  output is `/private/tmp/a2a-bridge-r3d0-sol-closure-f4f242f/review.md`, mode `0644`, 25,322 bytes,
  SHA-256 `110b9d2841c4f077a0b96fac19d7ece5cf07bad850714bbd787597fa330ba90c`.
  Exact code/foundation-doc commit `e3321db5c052d7f8a9d549b23cea6aa9a7df3784` folds all seven required
  remediation families: repository-wide nonnull Git object format; structured secret and exact config/effect
  semantics; status/hold/evidence coherence; file-object generation identity; reviewed characterization
  provenance; immutable chained publication identity; and versioned hash domains with targeted regressions
  and reconciled docs.
  Focused gates are **6/0** foundation, **22/0** schema, and **21/0** R3d0 CLI; the serial workspace is
  **2,214/0/12 ignored** across **55** reported test binaries. Check, warnings-denied Clippy, locked release,
  dependency policy, hygiene **37/7**, pinned-manifest **9**, floating-recipe **4**, and schedule-foundation
  validation are green. That exact-head Sol/xhigh closure review is recorded immediately below; the one Opus
  release/compatibility lens remains gated on final Sol approval.
- **Second R3d0 implementation closure review:** one fresh bridge-mediated Sol/xhigh/read-only review of exact
  cursor head `ee57f4a2f7509dd5a4bd281be1a36b7f117d834b` marked Git identity, status/hold/evidence,
  reviewed-characterization reuse, and publication-outbox identity `FIXED`; marked strict secret handling,
  repeated-path file-generation checking, and cursor/proof reconciliation `PARTIAL`; found no new `WRONG`
  beyond those residuals, found two `SMELL`, and returned `R3D0 IMPLEMENTATION: REVISE`. Its retained output
  is `/private/tmp/a2a-bridge-r3d0-sol-closure-ee57f4a/review.md`, mode `0644`, 18,583 bytes, SHA-256
  `445191467e708fef46036dbe41548599ffbfedfa8f21a68a93e16879dd565f99`. Its working analysis also
  identified that host and reader rows accepted arbitrary absolute session cwds; independent external-CLI
  probes confirmed both scheduled-advisory and claimed-support traversal escapes before remediation.
  The current fold trims inside an opening quote before credential-value classification; scans decoded JSON
  object keys; binds policy, advisory rows, and claimed-support rows to the exact owner-approved trusted cwd
  root; compares digest and file identity for every repeated canonical capture; adds paired CLI valid/invalid
  probes for Git, status, holds, portable evidence, reviewed-characterization reuse, and publication outbox;
  corrects the stale dependency node; and specifies R3d3's required quarantine-opening dereference under the
  owner lock. The review's proposed rename/restore full-loader sequence is not a valid pre-change acceptance
  witness because rename/restore changes the ctime captured in `RegularFileIdentity`: chronological A-to-B
  replacement already fails the final snapshot check. A direct regression now proves that existing failure
  and separately proves that duplicate same-path captures with distinct identities fail before deduplication.
  Exact remediation commit `ca4c453e6f589295b2434abfb1e1c708a2cb1dd2` carries that fold. Focused
  gates are foundation units **8/0**, schema units **23/0**, and R3d0 CLI **28/0**. The full serial workspace
  is **2,224/0/12 ignored** across **55** reported test binaries. Format/diff, all-target workspace check,
  warnings-denied all-target Clippy, locked release build, dependency policy, hygiene **37/7**, pinned manifest
  **9**, floating recipes **4**, and schedule foundation **6/4** are green. The provider-unexercised release
  binary is 26,478,368 bytes at SHA-256
  `368e72192d4656dfa1ec88a699fb2308f540600871c41f9b7fd4d7436e84b633`. Red mutations prove the
  quoted-TOML, decoded-key, duplicate-capture, trusted-cwd, support-owner, and policy-root regressions fail on
  the removed mechanisms; the valid-JSON quoted case was already caught by the raw-JSON layer and remains an
  additional boundary regression. That exact-head re-review is recorded immediately below.
- **Third R3d0 implementation closure review:** one fresh bridge-mediated Sol/xhigh/read-only review of exact
  cursor head `be9d8a7a689b5f2c451f6059784903ce6d78f8b5` marked six inherited families `FIXED`,
  trusted-cwd containment `PARTIAL`, found one new `WRONG` for credential-shaped scheduled prerequisites,
  found no new `SMELL`, and returned `R3D0 IMPLEMENTATION: REVISE`. Its retained output is
  `/private/tmp/a2a-bridge-r3d0-sol-closure-be9d8a7/review.md`, mode `0644`, 16,562 bytes, SHA-256
  `c0510898b83f09372313785dd45d48c236fe144e93ca3938b4715f76ded8b041`. Pre-fix tests prove an
  in-root symlink to an outside directory returned `Ok(())` from cwd validation and prove both a credential-
  shaped prerequisite and `credential_env` duplication validate after re-pinning the copied inventory; an
  ordinary `PATH` prerequisite is the paired positive. Exact remediation commit
  `5baeeb3f47183ea2a47d2cdc5ffce26f1df7dbfb` resolves a mounted owner root and cwd to real
  contained directories, binds the resolved path into profile identity, preserves only static/no-authority
  validation when that owner root is absent, and shares the production credential-name exclusion with the
  scheduled registry. Focused gates are foundation **9/0**, schema **23/0**, and R3d0 CLI **31/0**; the full
  serial workspace is **2,228/0/12 ignored** across **55** reported binaries. Format/diff, all-target check,
  warnings-denied Clippy, locked release build, dependency policy, hygiene **37/7**, pinned manifest **9**,
  floating recipes **4**, and schedule foundation **6/4** are green. The unchanged profile-policy bundle is
  `aed0e9b224d84624220a6091e51601a677b13d254091a12bc3b1879e36bf5e81`; the provider-
  unexercised release binary is 26,480,544 bytes at SHA-256
  `f2869caa4ccdc5b8fc055a803e462a05a2354cd53f4fa5b5aeaed71ea64efd28`. The exact-head fourth
  closure review is recorded immediately below.
- **Fourth R3d0 implementation closure review:** one fresh bridge-mediated Sol/xhigh/read-only review of exact
  cursor head `b6f5c9e7af2ffd0a1b022e3f07c2898a3d2c65c4` marked both inherited remediation
  families `FIXED`, found no new `WRONG`, found one nonblocking `SMELL` because the public CLI fixture did
  not independently reach the explicit `credential_env`-equality branch, and returned
  `R3D0 IMPLEMENTATION: APPROVE`. Its retained output is
  `/private/tmp/a2a-bridge-r3d0-sol-closure-b6f5c9e/review.md`, mode `0644`, 12,224 bytes, SHA-256
  `aa7b1051b83b94d84dc36273cf302419ffe2ecc41d20282001cffb530898374a`. Exact proof-only
  commit `e771067f4a7e742ad813368f01018b011e86bbce` adds an aligned, ordinary-name
  `SERVICE_ENV` credential/config fixture and asserts the branch-specific `must not repeat credential_env`
  diagnostic. Removing only that guard made the public CLI test fail by accepting the fixture after inventory
  re-pin; restoring it passes. The full serial workspace remains **2,228/0/12 ignored** across **55**
  reported binaries. Format/diff, all-target check, warnings-denied Clippy, locked release build, dependency
  policy, hygiene **37/7**, pinned manifest **9**, floating recipes **4**, and schedule foundation **6/4**
  are green. The release binary and profile-policy bundle remain byte-identical. The first proof-fold
  confirmation is recorded immediately below; the Opus/xhigh release/compatibility lens remains gated on a
  green exact-head Sol confirmation.
- **First R3d0 proof-fold confirmation:** one fresh bridge-mediated Sol/xhigh/read-only review of exact cursor
  `c548dc0edcc1b21bfb14aa3e78736d633ce0fdc7` marked the inherited proof-isolation `SMELL`
  `FIXED`, confirmed the approved mechanism unchanged, found one `WRONG` for two stale live roadmap handoffs
  and one `SMELL` for incomplete no-effect inventories, and returned `R3D0 PROOF FOLD: REVISE`. Its retained
  output is `/private/tmp/a2a-bridge-r3d0-sol-confirm-c548dc0/review.md`, mode `0644`, 11,037 bytes,
  SHA-256 `5b45405e21118bf5b98cd0f1944e69e0bcb13815c5308864ca19abdad9d1a7f8`. The current
  docs-only fold replaces both obsolete handoffs with the proof-confirmation cursor and required sequence,
  and uses the same complete no-effect boundary in every current status surface. No production, schema,
  configuration, or test code changes. The second proof-fold confirmation is recorded immediately below.
- **Second R3d0 proof-fold confirmation:** one fresh bridge-mediated Sol/xhigh/read-only review of exact cursor
  `e9d030f07d4c623ad2d00d0c918d02486d32fb7b` marked the no-effect-inventory `SMELL` `FIXED`,
  marked the stale-handoff `WRONG` `PARTIAL` only because six surfaces did not state the same conditional
  publication tail, found no new `WRONG` or `SMELL`, and returned `R3D0 DOCS REMEDIATION: REVISE`. Its
  retained output is `/private/tmp/a2a-bridge-r3d0-sol-confirm-e9d030f/review.md`, mode `0644`, 8,750
  bytes, SHA-256 `aa24e4e8a307b12fe6c5cca57212b536cce0c26e58c7d66f25641a4d191a9daf`. This
  docs-only fold applied one literal conditional sequence everywhere; the final confirmation is recorded
  immediately below.
- **Third R3d0 proof-fold confirmation:** one fresh bridge-mediated Sol/xhigh/read-only review of exact cursor
  `1d2fb80a2804a53b6f4076f10f4d4aea61a48f21` marked the inherited publication-tail `WRONG`
  `FIXED`, found no new `WRONG` or `SMELL`, required no remediation, and returned
  `R3D0 DOCS REMEDIATION: APPROVE`. Its retained output is
  `/private/tmp/a2a-bridge-r3d0-sol-confirm-1d2fb80/review.md`, mode `0644`, 5,136 bytes, SHA-256
  `0bfe50a90056f2db8a14404ca02c526bc9e55be9d7f3772c098d9539f39f4fed`.
- **R3d0 release/compatibility lens:** one independently routed bridge-mediated Opus/xhigh/read-only review
  of exact cursor `d61176ca0c248fe884cffd320f34b073738729d0` inspected every changed path plus the
  release, upgrade, provider-drift, platform, security, default-off, and later-increment seams. It found no
  `WRONG`, four nonblocking `SMELL`, required no pre-PR remediation, and returned
  `R3D0 RELEASE/COMPATIBILITY: APPROVE`. Its retained output is
  `/private/tmp/a2a-bridge-r3d0-opus-lens/review.md`, mode `0644`, 9,836 bytes, SHA-256
  `f7a8e55f540ec9dd318b2f788c6d05f61f1641cff6b8f5851b271b64dafe0a64`. S1-S3 record the
  intentional owner-host, strict-authoring, and owner-pinned portability constraints and require no current
  change. S4 identified stale prompt hashes, not a branch defect: the post-review owner-host validator
  reproduced profile-policy bundle SHA-256
  `aed0e9b224d84624220a6091e51601a677b13d254091a12bc3b1879e36bf5e81`, and the local
  26,480,544-byte release artifact reproduced SHA-256
  `f2869caa4ccdc5b8fc055a803e462a05a2354cd53f4fa5b5aeaed71ea64efd28`. Publish the
  non-draft R3d0 PR; let GitHub gates run; do not merge until those gates are green and the owner directs the
  merge.

R3d makes the already-bounded pinned and floating compatibility machinery safe to invoke under a narrow
tagged effect authorization. It adds scheduling, supervision, admission, accounting, retention, visibility,
and a trusted pre-merge compatibility gate. It does not add arbitrary prompts, automatic provider
fallback, automatic promotion, or production-operator session management.

### Non-negotiable boundaries

- Scheduled and pre-merge canaries use fresh one-shot bridge processes. They never execute through the
  long-lived production `serve` endpoint and never stop, restart, drain, rotate, or close its sessions.
- GitHub never directly invokes credentialed provider work. A trusted local scheduler pulls immutable
  metadata, recomputes impact, and owns every local effect.
- The one-shot characterization authorization and provider-effect standing grant authorize only exact fixed
  compatibility case ids and fixed lifecycle probes. The first is consumable only by the explicit
  characterization command for a `characterization_required` profile and one exact characterization execution;
  the second is usable only after
  completed characterization and is the sole authority for unattended scheduling. Neither is authority for
  general `serve`, `submit`, `run-workflow`, `implement`, other repositories/controllers, arbitrary prompts,
  or automatic fallback.
- Every prompt-capable attempt remains one fixed prompt, zero retry, zero provider substitution, and zero
  replay after possible acceptance. A failure is evidence, never same-window retry permission. The sole
  later-window transient confirmation is a new independently admitted attempt authorized in advance by the
  bounded `confirmation_due` policy, provider-effect grant allowance, repeat nonce, and budget below.
- Candidate outcomes remain `candidate_pass`, `candidate_fail`, or `candidate_unknown`. Scheduling does
  not promote a pin, baseline, support claim, config, image, release, or production service.
- Raw operator evidence remains private to the named operator. Secret-scanned sealed archives may leave the
  Mac only through the owner-authorized iCloud cold tier bound by valid storage consent; they are never public
  compatibility or promotion artifacts. Only bounded state and an evidence-record hash may be committed or
  published to GitHub.
- Locking/accounting guarantees cover only R3d-aware entrypoints. While either R3d provider-effect authority
  arm can admit, the trusted operator must not invoke billable compatibility through a retained pre-R3d binary.
  Exact legacy process detection, conservative reconciliation, and the explicit enforcement limit are
  defined below; the already-running legacy `serve` process remains allowed because R3d never routes canaries
  through it.

### Approved owner decisions

| Decision | Approved contract |
|---|---|
| D1 — substrate | Hybrid: local launchd supplies time, a fresh one-shot bridge owns each canary, the long-lived operator remains interactive, and GitHub performs deterministic checks/change detection only. A later R2f-gated lane owns long-lived operator health. |
| D2 — authority | Checked-in non-authoritative policy plus a private owner-only effect-authority envelope with mutually exclusive one-shot-characterization and post-characterization standing-grant admission arms, and independently revocable cold-storage consent. Each one-shot entry is narrow to one characterization profile and one exact characterization execution; the standing grant is narrow to characterized profiles while every concrete run remains exact-execution-fingerprint bound. A direct generic manual run instead carries one non-reusable, one-run local-CLI acknowledgement identity and never inherits the envelope's standing authority. Neither persistent arm authorizes general bridge/server use; storage consent grants only the approved iCloud evidence lifecycle. |
| D3 — model/task routing | Compatibility uses the lowest-cost eligible model for the exact provider/adapter/capability. General dogfooding uses the task matrix below as advisory, auditable policy rather than automatic routing. |
| D4 — evidence | Two-tier owner storage: 10 GiB hot local evidence and 25 GiB cold iCloud-backed archives, with typed retention, pins, tombstones, quotas, and local status/notifications. |
| D5 — representative probe | Defer broad scheduled reviews. Design the fixed read-only fixture adapter with operator input in a separate increment before implementation. |
| D6 — holds/quarantine | Separate automatic safety holds, exact-execution-fingerprint waste suppression, and explicit operator quarantine. Characterize every profile that R3d may admit automatically or require for D7, including future scheduled advisory and exact claimed-support profiles, before eligibility; new exact drift identities under an unchanged profile execute once and never reuse evidence. |
| D7 — change gate | Compatibility-affecting PRs require a trusted local affected-case canary on the exact current GitHub test-merge result SHA (`merge_commit_sha` / `refs/pull/<n>/merge`) before merge. A dedicated required context exists only on that result, never the PR head; a coalesced main run remains the post-merge integration backstop. |
| D8 — freshness/GC | Fresh same-window discovery observation with content-addressed reuse; bounded post-tick GC plus weekly reconciliation. Never call stale bytes current. |
| D9 — budgets | Durable reserve-then-reconcile accounting with a UTC ledger, rolling guard, protected pre-merge pool, and conservative charge for ambiguous or missing usage. |
| D10 — operator ownership | R3d proves fresh one-shot compatibility only. R2f owns shared-session health, close/capacity, stagnation, and non-disruptive drain/rotation. |

### D1/D2 — execution owner and effect authority

The developer Mac identified by `environment_owner` owns credentials, launchd, the evidence root, the
local scheduler, and the least-privileged GitHub check credential. There is no self-hosted GitHub Actions
runner and no copied personal auth on a GitHub-hosted runner. Launchd owns only the timer: it invokes an
immutable installed `a2a-bridge compatibility schedule-tick` binary, and all policy, locking, preflight,
deadline, child, ledger, artifact, and status behavior lives in reviewed bridge code. A slept-through
window is missed; launchd does not cause a catch-up burst.

Launchd uses separate arguments/labels for the low-frequency test-merge-gate watcher and the daily compatibility
window, but both enter the same reviewed binary, provider-effect grant, admission, ledger, and supervisor
boundaries. The
test-merge watcher only pulls metadata and posts state until deterministic checks are green and the debounce expires;
its exact poll/debounce cadence is a private policy value. The daily timer has at most one UTC window id.
Neither accepts a webhook command or shell recipe.

R3d adds one checked-in policy and one private authority envelope:

1. A checked-in scheduling policy defines schema version, allowed trigger and case groups, impact rules,
   characterization requirements, provider families, capability requirements, hard maxima, required
   preflights, evidence classes, and notification transitions. R3d0 canonically derives a versioned
   `profile_policy_bundle_hash` over that policy, the characterization inventory's scheduled-registry and
   D7-reachable production-support semantic profile definitions, resolver/recipe constraints, config/prompt/
   artifact templates, allowed effect classes, and profile maxima. This
   stable authority artifact is not a production or generated execution manifest and contains no exact pin,
   resolved package/image/config, test target, or candidate identity. It is reviewable but grants no effects.
2. A private authority envelope below the operator home is a single-link regular file, directory mode
   `0700`, file mode `0600`, writable only through the local operator CLI. It contains three independently
   validatable and revocable record types:
   - a **one-shot characterization authorization** contains one or more exact, independently consumable
     `characterization_once` entries. Each entry binds its batch-authorization id/hash plus a unique entry id,
     generation, hash, and consumption nonce; operator and environment owner; exact
     `profile_policy_bundle_hash`, tagged profile-source kind plus exact scheduled-advisory or production-
     support row/hash, characterization-profile fingerprint and exact characterization-execution fingerprint;
     installed scheduler binary and normalized price/ranking-snapshot hashes,
     proposed expected-effective identity, provider family, allowed registry/image/provider effects, and known
     pre-R3d executable inventory; conservative token/cost/time/attempt caps, retry/fallback zero, the explicit
     characterization command,
     `not_before`/`expires_at`, and its own revocation generation. One reviewed batch may enumerate the full
     future schedule set, but an entry authorizes only its one profile/execution pair and cannot lend unused scope
     or budget to another entry;
   - the **provider-effect standing grant** binds a unique grant id; operator and environment owner; exact
     `profile_policy_bundle_hash`, installed scheduler binary hash, and normalized price/ranking-snapshot hash;
     exact trigger lanes, case ids, characterization-profile fingerprints, and completed-characterization ids/
     hashes;
     allowed registry/image/provider effects; provider families; per-case, per-run, per-trigger-pool,
     per-provider, UTC-day, and rolling limits; the exact allowed launchd-label set plus each installed
     plist's canonical hash and trigger lane; `not_before`, `expires_at`, and provider-grant revocation
     generation; and the known pre-R3d executable inventory; and
   - the **cold-storage consent** binds its own id/hash, operator/environment owner, evidence-class scope,
     independent `not_before`/`expires_at` and revocation generation, exact cold root, owner-approved
     `owner_icloud` replication mode, and opaque local FileProvider-domain identity.

Cold-storage consent may outlive or be revoked independently of either provider-effect authority and grants only
cold publication/upload/verification/cloud-retention/hot-eviction operations over already-completed,
locally sealed evidence. No provider-effect record's validity implies storage consent or vice versa. The
scheduler validates the selected provider-effect authority arm before credential access, registry/image/
provider effects, or candidate spawn; the archive publisher validates cold-storage consent before any iCloud-
root write.

At issuance under the authority-state lock, a durable profile index rejects more than one unconsumed, reserved,
or consumed-but-unreconciled one-shot entry for the same characterization profile across the proposed batch and
every other active batch. Entry state is `available -> consumed_unreconciled -> reconciled`; a proved pre-effect
refusal before durable consumption leaves the sole entry available. Consumption or ambiguity never restores it.
Envelope entries, the profile index, and consumption/reconciliation state commit through one crash-consistent
authority journal under that lock. Recovery rederives the index from immutable entries plus the consumption
ledger; missing, divergent, duplicated, or partially committed state fails closed before issuance or admission.
After terminal reconciliation, any same-profile reissue must be a new separately reviewed batch/entry that
names the prior entry and outcome plus an operator reason; it is never an automatic retry, pooled spare, or
continuation of the old nonce/budget. The old record remains immutable.

Missing, expired, revoked, consumed, wrong-owner, wrong-host, stale-hash, broadened-case, command/trigger
mismatch, or cap mismatch in the selected provider-effect authority refuses admission with a typed zero-effect
status. Policy or characterization-profile changes invalidate an old one-shot entry and any standing grant that
covered the old profile. A changed exact characterization execution invalidates that unconsumed one-shot entry;
an exact test-merge/candidate/resolution change under an already characterized unchanged profile does not
invalidate its standing grant. Neither the scheduler nor `serve` can mint, renew, widen, or repair either
authority. Exact
dates and numerical caps are operator rollout values; one-shot characterization uses reviewed conservative
caps, while standing-grant caps are derived from completed characterization. Neither auto-renews.

Private-authority mutation and both final-admission paths share a short-lived authority-state lock distinct
from the aggregate-wide admission lock. Under that lock exactly one path is valid:

- **Persistent-envelope admission** reopens the envelope, requires exactly one tagged
  `characterization_once` or `standing_grant` arm and canonical absence of every manual field, validates the
  arm, and proves `now + derived_terminal_deadline <= expires_at`. A `characterization_once` commit also
  consumes that exact entry/nonce; crash or ambiguity after commit may reconcile evidence and charge but can
  never restore or replay it. A `standing_grant` commit binds the exact completed characterization id/hash
  required by that arm.
- **Generic-manual admission** requires canonical absence of both persistent arms, derives and validates
  exactly one local `ManualAdmissionV1` plus its unique nonce from the direct CLI invocation, and proves the
  same terminal-deadline containment against its one-run expiry. It never opens an envelope arm or treats the
  manual record as persistent effect authority. Its reservation commit durably consumes the manual nonce.

Both paths atomically journal their complete tagged admission identity, authority-bound admission-attempt
fingerprint, equivalent-work reservation, and budget reservation. Mixed paths/fields, no path, or multiple
identities refuse before effects. Admission always acquires the aggregate-wide lock before the authority-state
lock; authority mutation takes only the latter, so no path reverses the order. That durable reservation commit
is the admission linearization point. Persistent-arm revocation, or either path's expiry, before that commit
refuses the attempt; revocation after a persistent reservation prevents later admissions but does not kill or
replay the already-admitted bounded attempt. The operator CLI increments each persistent record's revocation
generation under the same lock. A launchd invocation must use `standing_grant` and match one exact allowed
label and installed plist hash; the daily and test-merge-watcher labels are both bound explicitly rather than
inferred from a singular name.

Cold archive publication has its own action-time consent fence under the authority-state lock. It revalidates
the independently valid cold-storage-consent id/hash, owner/environment, time bounds, evidence class,
root/domain, and revocation generation; proves the remaining derived cold-publication bound ends no later
than consent expiry; and journals that consumption before writing into iCloud. Revocation or expiry before
this point keeps the artifact hot; revocation after publication blocks later archives/hot eviction but cannot
claim already-synchronized bytes were recalled or
delete them implicitly. Read-only integrity reconciliation of already-retained evidence remains allowed and
visible; removing cloud data is a separate explicit owner action.

The initial trust domain is one named operator account on one named Mac. Locks, ledgers, and active indexes
must reside on its local APFS filesystem, never iCloud or a network filesystem. The selected provider-effect
authority's host identity plus `environment_owner` excludes distributed execution; R3d does not implement a
distributed lease. A
hostile same-UID/root actor and malicious code from an operator-approved same-repository author remain
explicit non-goals. Candidate builds receive no credentials, and candidate runs receive only the one
provider's narrow required credential set; no other operator or GitHub secrets are inherited.

Manual compatibility commands invoked through the installed R3d-aware binary retain their explicit
acknowledgements and case selection. They share R3d's admission lock and accounting ledger but do not inherit
the provider-effect standing grant. That grant supersedes per-aggregate manual acknowledgement only for an
exact scheduled or trusted-pre-merge request that binds the provider-effect grant in its evidence.

At final admission, a direct local generic manual invocation's explicit `--acknowledge-billable` creates a
strict, non-renewable `ManualAdmissionV1` identity under the authority-state lock. It binds a scheduler-generated
unique request nonce; operator/environment owner and installed scheduler binary; exact input source and selected
case/profile/execution fingerprints; requested evidence purpose and freshness bucket; command, caps, retry/
fallback zero, allowed provider/registry/image effects, and a short one-run expiry. Its tagged
`manual_acknowledgement` hash is durably consumed with the reservation whether the request executes or reuses
eligible evidence. It cannot be supplied or replayed by a caller, accepted from `serve`/A2A/a timer/watcher,
stored as reusable/standing authority, or used by `ScheduledExecutionSourceV1`. Its immutable audit record
remains durable. Missing explicit acknowledgement,
source/case/cap/purpose mismatch, stale scheduler identity, expiry, duplicate nonce, or post-seal mutation
refuses before credential or provider-capable process access. This one-run local-CLI identity is admission
authority for generic manual accounting only; it is not a third arm in the private provider-effect envelope.

The explicit characterization command is the sole manual-shaped path that may consume persistent envelope
authority: it accepts only a sealed `ScheduledExecutionSourceV1` or
`ClaimedSupportCharacterizationSourceV1`
carrying the sole valid `characterization_once` entry for an exact
`characterization_required` profile and its bound characterization execution. It cannot accept
`standing_grant`, an already consumed entry, a completed characterization for that profile, or any scheduled/
test-merge/main trigger. Generic manual
compatibility acknowledgement cannot substitute for this entry.

A manual command may reference only the authority envelope's cold-storage-consent id/hash after an explicit
`archive-to-owner-icloud` acknowledgement for already-completed, secret-scanned evidence. That storage-only
use grants no provider/registry/image/scheduler authority. Without it, manual full evidence remains hot and
storage status may block later work rather than uploading implicitly.

Retained pre-R3d binaries cannot participate in the new cooperative lock/ledger. Issuing a one-shot
characterization batch or activating a standing grant therefore binds an inventory of their exact path,
device/inode, hash, and allowed non-compatibility process identities; documents their compatibility subcommands
as rollback-only/not authorized while any R3d provider-effect authority can admit; and requires a zero-live-
legacy-compatibility observation. Preflight and the final pre-spawn fence enumerate
processes by exact executable/start identity and argv shape: the retained production `serve` process is
allowed, but a legacy `compatibility` process or an ambiguous provider-capable legacy child creates a
safety hold. A discovered earlier legacy aggregate is imported into the ledger from validated evidence; an
ambiguous/no-artifact attempt retains the configured full conservative charge. Scheduling stays held until
reconciliation completes. Initial activation records an operator legacy-quiescence timestamp and evidence
inventory. If activity during the current UTC/rolling window cannot be ruled out or reconstructed, the
ledger begins with the full provider/global ceiling charged and admission waits for that window to age out;
absence of an observed process is never treated as proof of zero prior spend.

The delivery guarantee is intentionally not an OS security claim against the trusted same-UID operator
starting an uncooperative old executable after the final fence. Such direct use violates the active-grant
operator contract and is outside the R3d-aware concurrency guarantee; preventing it would require a future
version-independent credential/lease broker. Normal manual compatibility uses the R3d-aware binary, and no
legacy process is automatically killed or replayed.

### D3 — compatibility and dogfooding routing policy

Compatibility routing is enforced. "Cheapest" is never inferred from a name: the selected raw model must
be advertised, satisfy the adapter/capability under test, have an authoritative price or explicit operator
cost ranking, and be characterized in the exact environment. The pinned lane uses the exact last-approved
low-cost identity. The floating lane observes additions, removals, deprecations, and capability changes
through the bounded catalog captured by an admitted smoke session, and evaluates separately bound
price/ranking changes. It emits an advisory recommendation and never mutates a pin or grant. Deprecation
blocks visibly without silent fallback.

The checked-in policy names the price/ranking authority, normalization schema, and maximum age. Each selected
provider-effect authority binds the normalized snapshot hash, `observed_at`, and `valid_until`; stale, missing, ambiguous, or
currency-incompatible input blocks selection rather than choosing a model by name. Provider-reported quota
is only an additional veto and never replaces this accounting or ranking authority.

| Compatibility path | Initial low-cost choice to characterize |
|---|---|
| Claude basic provider/ACP path | Lowest eligible Haiku model at its lowest/default effort |
| Claude effort-setting path | Sonnet 4.6 at `low` |
| Codex provider/ACP path | Raw advertised `gpt-5.6-luna` family id at `low` |
| Kiro | `qwen3-coder-next` at its lowest supported setting; because Kiro is currently not model-configurable, use a dedicated exact config rather than a request override |
| Ollama | Smallest adequate local model, with no network billing |
| OpenRouter/OpenCode | Free or least-expensive eligible identity only after R3e/R3f integrate and characterize the provider |

Existing Fable/xhigh and Sol/xhigh support rows are not implicitly daily cases. R3d adds or selects explicit
provider-minimal case ids; their evidence proves the named provider/adapter/capability path, not support for
an unexercised expensive model. Catalog changes propose a new candidate for operator review and
recharacterization. This daily/provider-generic routing rule does not override D7: a PR that can affect a
model-, effort-, mode-, alias-, or capability-specific support claim runs that exact claimed identity.

### Canonical characterization-profile and case-execution identities

R3d defines two distinct versioned canonical SHA-256 identities. The
`characterization_profile_fingerprint` names the bounded provider-effect shape that the operator observes once.
It serializes:

- repository identity without a commit, PR, or test-merge identity; tagged profile-source registry/schema and
  semantic scheduled-advisory or production-support row shape; scheduling policy, recipe/resolution-policy,
  config-template, fixed-prompt, and artifact-policy hashes;
- case id, lane, classification, evidence purpose, execution mode, expected status, provider/agent/capability,
  adapter/SDK/CLI and image-family names plus their allowed resolution constraints, auth-path type, and
  non-secret prerequisite names/shapes;
- requested raw model/effort/mode and proposed-or-characterized expected-effective model/effort/mode;
  environment owner, OS/architecture, trusted session cwd (the resolved in-root directory path when the owner
  root is mounted; otherwise only the declared static path for no-effect offline validation), host/reader
  shape, redaction policy, retry/fallback zero, and maximum token/cost/time/attempt caps.

The profile deliberately excludes the changing immutable execution inputs the canary is meant to test: exact
PR/base/head/test-merge SHA/ref/tree/ordered parents or scheduled-main range; candidate binary bytes/build
provenance; exact run manifest/pin/resolution bundle, package versions/integrities, image/base-image digests,
and generated config values derived solely from those resolutions; and actual per-attempt caps at or below the
characterized maxima. Those execution values may be excluded only when the reviewed resolver, source generator,
and validator derive them from a profile-bound policy/recipe. Caller-supplied, widened, or out-of-constraint
values fail closed. Trigger source/kind, request id, window id, attempt id, and repeat nonce are a separate
admission-only class: they are absent from both reusable fingerprints and validated through authority/
admission. A newly selected raw model, effort/mode, provider/capability, prompt/
template/policy/recipe, environment/auth/execution shape, or larger maximum cap changes the profile and requires
new characterization.

Every concrete provider-capable attempt separately uses a canonical `case_execution_fingerprint`, never the
current binary-only `CandidateIdentity` or an informal subset. It is trigger-independent and includes the
profile fingerprint plus the exact execution target—PR/base/head/current test-merge SHA/ref/tree/ordered parents
or scheduled-main head/range—and the complete exact run manifest, registry row, generated config, pin/
resolution/package/image integrity, candidate hash/length/build provenance, requested/expected identity,
environment/prerequisites, actual caps, and canonical typed absences. It contains no trigger source/kind,
request, window, attempt, authorization, or repeat identity. Field order and normalization are fixed, and no
path/name may substitute for a required content hash.

Characterization lifecycle and one-shot-authority uniqueness bind the profile fingerprint. Outcome evidence,
holds/suppressions, equivalent-work reservation, ledger, consumption, and the schedule sidecar bind the exact
execution fingerprint and also carry the profile fingerprint. A profile-field change returns the case shape to
`characterization_required`. A change only to an excluded exact test-merge/candidate/resolved package or image
field keeps the completed profile characterization but creates a new execution fingerprint that must execute
once; it cannot reuse, consume, suppress from, or green itself with evidence from another exact fingerprint.
`claimed_support_gate` evidence never crosses an exact test-merge result SHA even when two results produce
byte-identical candidates.

Persistent effect-authority ids and a one-run manual-acknowledgement id are not fields in either reusable
identity: rotating an otherwise equivalent standing grant or invoking equivalent work manually must not force
recharacterization. Final admission instead derives an `admission_attempt_fingerprint` over both profile and
execution fingerprints plus exactly one complete tagged admission-authority identity:

- batch-authorization id/hash plus one-shot entry id/generation/hash/consumption nonce for
  `characterization_once`;
- standing-grant id/generation/hash plus characterization id/hash for `standing_grant`; or
- locally derived `ManualAdmissionV1` hash and unique consumed request nonce for `manual_acknowledgement`.

It also binds the trigger/window/attempt identity. Concretely that is trigger source/kind, request id, window
id, attempt id, and optional repeat nonce. The applicable sealed source or manual admission record,
equivalent-work and budget reservation, ledger, and consumption record bind all three fingerprints; the
schedule sidecar does so for characterization/scheduled/test-merge work. Authority rotation therefore
prevents stale admission/replay without relabeling unchanged case or execution evidence.

No pre-admission fingerprint contains a value learned only after admission. The result separately records the
observed effective model/effort/mode from the admitted session. That observation must equal the profile's
proposed-or-characterized expected-effective identity before evidence is eligible for reuse or consumption. A
mismatch is `candidate_unknown`, creates a safety hold for that execution fingerprint, and retains the original
reservation and conservative charge; it never re-keys the admitted ledger/equivalent-work record after
provider acceptance. For a first characterization, the exact `characterization_once` entry and reviewed row
bind both the proposed profile and the exact characterization execution fingerprint. The proposed expected
identity becomes the private characterized profile binding only after the observed result matches; mismatch
remains non-reusable `candidate_unknown` evidence.

R3d0 adds a separate strict versioned `compatibility/scheduled-cases.toml` registry for provider-minimal
advisory probes. A row declares the exact characterization-profile shape and strict dynamic-resolution
constraints above and starts at `characterization_required`; it is not a `support` claim and is never inserted
into the protected
production manifest/baseline. For an explicitly authorized characterization or eligible tick, the R3d-aware
scheduler derives a sealed schema-v1 execution manifest with lane `floating-current` and classification
`canary`, then records the registry plus profile/execution fingerprints in the schedule sidecar. Old R3
binaries never read the
separate registry and continue
to parse the unchanged aggregate; feeding the registry to an old manifest command fails explicitly. A
private profile-characterization record plus an exact provider-effect grant entry—not a checked-in status
edit—moves the probe into scheduled eligibility. R4 alone may promote support claims.

R3d0 also adds one checked-in strict `CharacterizationProfileInventoryV1`. Each entry is tagged as either a
`scheduled_advisory` reference to the exact semantic scheduled-registry row or a `claimed_support_gate`
reference to the canonical semantic profile projection of a production-manifest `classification = "support"`
row. The inventory must contain every profile that a timer, trusted main/test-merge trigger, or D7 classifier
can make due; startup/dry-run recomputes it from both source registries and fails closed on omission, duplicate,
stale hash, or a reachable support row that is not inventoried. The initial claimed-support inventory is
exactly `codex-host-bridge-gpt56-sol`, `codex-reader-bridge-gpt56-sol`,
`claude-host-acp-044-fable`, and `claude-reader-055-fable`. Their semantic profile projections—not the exact
generated run manifest, candidate, package/image resolution, or test target—join the stable
`profile_policy_bundle_hash`. A production support-row/profile change therefore requires an inventory/bundle
update and fresh characterization before D7 may admit that profile.

### Strict scheduled-execution source

R3c's existing `compatibility run --resolution` route deliberately requires every generated floating case's
model/effort/mode to equal its pinned support baseline. R3d must not weaken that validator or feed the separate
scheduled registry through it: Luna/Haiku provider-minimal probes intentionally differ from the Sol/Fable
support rows. Direct hand-authored `run --lane floating-current` remains
`floating_resolution_required`.

R3d0 therefore adds a third, strict, versioned `ScheduledExecutionSourceV1` owned only by the R3d-aware
scheduler. It is an owner-only, create-new/no-follow, content-hashed source under local evidence scratch that
binds:

- the scheduled-case-registry schema/hash, exact row id/hash, canonical `profile_policy_bundle_hash`, and
  characterization-profile fingerprint;
- the scheduling policy, complete case-execution fingerprint, authority-bound
  `admission_attempt_fingerprint`, and exactly one tagged `admission_authority` arm;
- candidate binary/build provenance; exact package-set, adapter/SDK/CLI, image/base digest, config-template,
  generated-config, prerequisite, auth-path, environment, prompt, and artifact-policy identities; and
- the row's exact requested raw and expected-effective model/effort/mode plus caps and retry zero.

For this scheduled source only, `admission_authority` is a strict tagged union with exactly two arms:

- `characterization_once` binds the batch-authorization id/hash and one-shot entry id, generation, hash, and
  consumption nonce; the exact `characterization_required` profile and characterization execution fingerprint;
  proposed expected-effective identity; reviewed conservative maxima and explicit characterization-command
  trigger; and typed absence of both a completed characterization and any applicable standing grant for that
  profile. It can be created only from an unconsumed private authorization entry and is durably consumed at
  admission.
- `standing_grant` binds the provider-effect-grant id, generation, and hash plus the completed private
  characterization id/hash for the same profile fingerprint. The exact execution fingerprint is independently
  derived for the current trigger/resolution and must remain within that profile. The arm carries typed absence
  of one-shot
  authorization fields and is the only arm accepted for scheduled, main, or trusted test-merge triggers.

Missing, unknown, or multiple arms; a mixed-arm payload; a stale/consumed one-shot entry; a one-shot entry for
an already characterized or different profile/execution; a standing grant without the exact completed profile
characterization; an execution value outside the profile's constraints; or any noncanonical absence marker
fails schema/validation before credential or provider-capable process access.
`manual_acknowledgement` and every `ManualAdmissionV1` field are schema-invalid here; generic manual work uses
the unchanged pinned/floating compatibility source plus its separate local admission record, never this source.

Its generated execution manifest is still strict schema v1 lane `floating-current`, classification `canary`.
Model/effort/mode may differ from a pinned support row only by equaling the exact scheduled registry row; no
other field can inherit, widen, or fall back from caller input. The scheduler rederives the source and config
from the checked-in row/templates plus verified package/image provenance, then journals its hash with the
equivalent-work/budget reservation. The source never enters the production manifest/baseline, cannot carry
`support`, cannot update pins, and is not promotion evidence.

R3d2 adds a distinct `RunSource::Scheduled` validator and internal `--scheduled-source` runner path. It
reopens and rehashes every bound object, rederives the complete source, requires the exact admitted authority
and all fingerprints, independently rederives the profile-policy bundle, verifies that its hash matches the
selected authority, and refuses before provider-capable spawn on any mismatch. Only `schedule-tick` or an
explicit one-run characterization command may create/admit that source; a direct compatibility invocation,
arbitrary manifest, production-support classification, unlisted model variance, caller-supplied package/config
drift, or
missing/revoked authority fails closed. The explicit characterization command requires `characterization_once`
and refuses every unattended trigger; `schedule-tick`, main, and test-merge paths require `standing_grant` and
refuse one-shot authority. A newly resolved exact package/image or test-merge/candidate identity is accepted
only when the source generator derived it under unchanged profile-bound constraints; it always produces a new
execution fingerprint and never evidence reuse. R3d4 uses the existing pinned production-manifest route
whenever a
test-merge impact requires an exact claimed-support identity; it may select `RunSource::Scheduled` only for
an explicitly classifier-proved provider-generic/advisory case. Daily advisory work and R3d5 scheduled-
advisory characterization use the scheduled source. Old R3 binaries never parse this separate source.

Claimed-support characterization does not pass a production support row through that advisory source. R3d0
adds a separate strict, owner-only, create-new/no-follow `ClaimedSupportCharacterizationSourceV1` for the
explicit one-run command. It binds the exact production-manifest schema/hash and
`classification = "support"` row/hash, stable bundle/profile plus exact characterization-execution and
admission fingerprints, installed candidate, full pinned config/package/image identities, exact requested/
expected model/effort/mode and caps, and only the matching `characterization_once` entry. `standing_grant`,
`manual_acknowledgement`, timer/watcher/main triggers, row substitution, and typed-arm mixing are invalid.
R3d2's internal `--claimed-support-characterization-source` validator reopens and rederives all bindings, then
invokes the unchanged strict pinned-manifest runner. Its evidence purpose remains `characterization`, never
support promotion, manifest mutation, or reusable proof for another exact execution. A later D7 gate uses the
ordinary pinned production route under `standing_grant`; R4 alone owns support promotion.

General dogfooding routing is documentation and audit policy in R3d, not an automatic controller. Each
turn records task class, provider, requested/observed effective model, effort, mode, override reason, and
whether it is a primary or independent lens.

| Task class | Primary | Effort | Escalation or second opinion |
|---|---|---|---|
| Bounded summary, docs, lightweight brainstorming | Luna, Haiku, or another inexpensive eligible model | low/medium | Sol when scope or consequences expand |
| Small, tightly specified implementation | Luna or Sonnet | medium/high | Sol for cross-cutting or difficult work |
| Normal implementation | Sol | high | Opus for a genuinely independent architecture lens |
| Spec/design/technical architecture authoring | Sol | high/xhigh | Opus 4.8 for assumptions, alternatives, gaps, and cross-cutting concerns |
| Clean-room spec/technical design | Same as authoring, without inherited conclusions | high/xhigh | Opus independent lens; no nested helpers unless explicitly authorized |
| Adversarial design or implementation review | Sol | xhigh | Opus xhigh for uncertain assumptions/gaps; Fable xhigh only for hard or complex cases |
| Release/compatibility review | Sol | xhigh | Opus or Fable release lens only after the primary review is green |
| Full-branch review | Sol | xhigh | Opus xhigh for assumptions/gaps; Fable only when complexity or risk justifies it |
| General brainstorming, requirements gathering, analysis, or grooming uncertainty | Sol | high/xhigh | Opus for alternative framing; Fable for hard/complex ambiguity, contradiction, or synthesis |
| Deadlock, data race, complex leak, transaction proof, critical algorithm proof, zero-downtime migration, or rare production failure | Sol | max | Fable adversarial lens when useful |

`max` is reserved for tightly connected evidence that benefits from depth over parallelism or after high/
xhigh fails. Fable is reserved for hard/complex work. Automatic routing of general repository work remains
deferred until an authenticated controller/task-authority design exists. R3d0 adds a concise checked-in
routing reference and links it from `AGENTS.md` and the operator skill without advertising unimplemented
scheduler commands.

### D6 — case characterization, holds, suppression, and quarantine

Every inventoried characterization profile intended for future unattended/advisory compatibility or a D7
claimed-support gate follows this lifecycle:

```text
proposed
  -> characterization_required
  -> characterized_green | characterized_known_issue | characterization_inconclusive
  -> scheduled_active | required_gate_active | operator_quarantined | deferred
  -> retired
```

Characterization is a separately authorized, operator-observed run of the exact profile plus one exact
representative execution: provider, requested raw model/effort/mode, proposed expected-effective identity,
environment/auth/execution shape, prompt/template, maximum caps, and the selected adapter/CLI/package/image/
candidate identities are all recorded; the observed effective identity must match. A profile-fingerprint
change returns the case shape to `characterization_required`. An execution-only test-merge, candidate, or
strictly resolved package/image change under the same profile does not. Uncharacterized is
not quarantine and records `not_run(characterization_required)`. Before R3d finalizes the automatic unsafe/
waste table or enables any timer/check admission, characterize every inventoried profile once. Scheduled
advisory profiles use the approved lower-cost models; D7 claimed-support profiles use their exact production
Sol/Fable model, effort, mode, adapter, capability, and host/reader shape. Some advisory profiles may remain
known issues, quarantined, or deferred; none should surprise the first unattended tick. A claimed-support
profile must be characterized and present in the standing grant before its required-check impact class can be
enabled. Obsolete historical and explicit non-goal rows are outside this matrix.

The initial scheduled-advisory set is Codex Luna-low host/reader, Claude Haiku host/reader, Claude Sonnet-low
effort control, Kiro/Qwen host/reader once deterministic support exists, Ollama, and later OpenRouter/OpenCode
after integration. The initial D7 set is the four exact Sol/Fable production-support profiles named above. A
floating case profile is characterized once. Each newly resolved
immutable execution fingerprint still runs once as advisory `candidate_pass|candidate_fail|candidate_unknown`
evidence; evidence, hold, suppression, and consumption state remain exact-execution scoped and never transfer
merely because the profile matches.

Three controls remain distinct:

- **Safety hold:** automatic and non-expiring when ownership or possible effects are uncertain. It requires
  explicit operator clearance. Triggers include possible prompt acceptance without terminal evidence;
  cleanup/release/retire failure; TERM/KILL without proved exit; unreaped process/container; artifact or
  ledger failure after admission; unreconciled accounting; identity drift after effects; duplicate
  billable processes; or cleanup/evidence worker panic/timeout.
- **Waste suppression:** automatic only for an exact complete execution fingerprint where repetition is
  predictably
  useless: a typed immutable model/config/protocol rejection, removed model, absent/mismatched immutable
  input, or the same typed transient failure with the same complete fingerprint twice. `candidate_fail`
  is only an outcome and never by itself proves permanence. A first clean transient or untyped provider
  failure enters `confirmation_due` and gets at most one later-window confirmation; only a second identical
  complete failure enters suppression. A pass records recovery. Material identity change clears suppression.
  Expired authority, quarantine, and exhausted current-window budget remain their own typed blocking states
  and are re-evaluated by their own lifecycle; they are not relabeled as waste.
- **Operator quarantine:** an explicit owner action that skips a case until another explicit action.
  Private mode-`0600` records follow a checked-in schema and can be mutated only by the local CLI, never
  `serve` or A2A. R3d0 validates a closed record's `opening_sha256` only as a syntactically valid digest;
  R3d3 must, under the owner lock, dereference the immutable opening record and verify that exact hash before
  accepting closure. Expiry fails closed and does not auto-resume.

Pre-prompt auth/model/runtime/owner/environment refusals, clean expected negatives, first clean transient
provider failure, missing cost telemetry with a retained conservative charge, and poor-but-terminal output
are not automatically unsafe. A quarantined row is `not_run(quarantined:<id>)`; characterization-required,
quarantined, held, or suppressed aggregate status is degraded and non-promotable, never green by omission.

### Non-billable preflights

Immediately before admission, the trusted scheduler revalidates host owner/architecture, the selected tagged
effect authority and policy hashes, exact candidate/config/manifest/recipe identities, quarantine/hold state,
ledger headroom, OAuth runway, required environment bindings, the bound pricing/ranking snapshot, local storage
headroom, and
the R3b container-start control for reader cases
using an already-present pinned image with no pull. It also validates the scheduled-case registry plus profile/
execution fingerprints and legacy executable/process inventory above. The `characterization_once` branch requires
`characterization_required`, the exact proposed raw/expected-effective identity, the sole live profile entry,
and canonical absence of a completed profile characterization/applicable standing grant; the `standing_grant`
branch requires the exact completed profile characterization's raw/expected-effective bindings and proves the
current exact execution was derived within that profile. A non-OK preflight records
`not_run(<typed-reason>)`, zero provider spend, status transition, and notification threshold progress. It
never calls `models` or starts an adapter merely to diagnose the preflight; the fresh effective catalog is
captured from the one already-authorized smoke session. Standing authority plus green preflights supersedes
per-run manual authorization only within the scheduled/pre-merge scope.

### Supervision and signal contract

`schedule-tick` captures a monotonic start at process entry, before reading metadata or policy, and owns one
hard absolute deadline from that instant. For each trigger it records and sums bounded maxima for metadata
fetch, checkout/candidate build, preflight, resolution/materialization, every selected case timeout, evidence
publication, cold-archive handoff, cleanup grace, and a fixed margin. A phase absent from that trigger
contributes zero, never an implicit unbounded allowance. Derivation time counts against the bound. It refuses
before credential/registry/image/provider effects when the schedule window, grant expiry, or time budget
cannot contain the complete bound. Elapsed derivation is rounded conservatively so the serialized remainder never
understates the executable remainder. The deadline is not reset between phases. Each phase is capped by the earlier
of its own maximum or the hard deadline after reserving every later phase, cleanup grace, and fixed margin; an
already-exhausted phase refuses before polling even an immediately-ready effect.

The compatibility runner handles SIGTERM exactly like SIGINT: stop admitting later cases, let the current
bounded child follow its cancel/release/retire path, publish its aggregate, and clean scratch. Before spawning
the runner or staged smoke, the parent creates a trusted inert bridge-owned group-leader anchor and places the
workload in that process group. It records anchor/runner/child pids, process groups, start identities, container
run labels, and phase before effects. The anchor remains live through TERM; the one terminal group-KILL step
keeps its exited leader unreaped until the supervisor journals that no later group signal is permitted, then
final wait/reap releases the identity. The retained, unreaped child handle is itself the group-signal capability:
PID/PGID reuse is impossible until reap, so a late process-observation error cannot suppress required cleanup.
TERM/KILL journals before the effect and then uses that retained capability directly; it does not gate the capability
on another fallible liveness observation. An absent or mismatched capability refuses without a numeric-group signal
and moves the already-journaled attempt to an ambiguous safety hold.
A non-hold supervisor record binds the scheduler, runner, and every anchored group to one exact session. A safety
hold retains at least one anchored-group record even when its runner identity is unavailable. `Prepared`, `Running`,
`TermGrace`, and `KillJournaled` require every anchor to remain `RetainedLive`. An anchor may become
`ReleasedReaped` only on entry to `Reaping` after later group signals are forbidden, or `Ambiguous` only on entry
to `SafetyHold` after later group signals are forbidden. Once the supervisor
has acquired a descendant-group anchor, it retains the exact live capability and serializable record before any
fallible workload observation. Registration then revalidates the runner and every workload; any session, ancestry,
liveness, or identity-observation failure appends that exact acquired group to the durable hold before disabling
later group signals. A dead or unobservable anchor is retained as ambiguous rather than silently dropped.
A crash that makes the signal/journal ordering ambiguous recovers to a
hold and never retries a numeric-group signal. R3d1
reuses or factors the already-proven R3c `CommandProcessGroupGuard` mechanism; it must not restore a numeric-
PGID-only signal path. A
descendant-created group in the same session may be group-signaled only after the supervisor revalidates the exact
runner, existing workload, incoming workload, and anchor identities and successfully
adds and retains its own anchor member; a new-session escape, vanished leader, failed anchor acquisition, or
ambiguous identity creates a safety hold and is never signaled through a stale numeric PGID. Escalation is:

1. TERM the runner and begin the bounded cleanup grace;
2. a second TERM/INT during grace escalates immediately;
3. KILL only exact recorded/enumerated runner and child groups whose bridge-owned anchor is still live, never a
   process name or unanchored/stale numeric PGID;
4. journal the terminal group-signal attempt, wait/reap direct workload children, prove no non-anchor member
   remains, then release/reap each bridge-owned anchor and prove group absence; repeated cancellation after
   this mark performs no group signal;
5. reap only containers with the exact run label;
6. prove exit, or retain the owner-wide admission lock/hold state and refuse successors;
7. publish a durable `killed_after_deadline` or `killed_after_cancellation` supervisor record according to the
   write-once KILL cause, joined to the runner artifact by run/window id, and retain the conservative budget charge.

Startup reconciles any incomplete supervisor record before new admission. An unproved or uninterruptible
survivor, missing/dead anchor before group absence is proved, or unreconciled anchor release creates a safety
hold rather than a duplicate attempt or stale signal. Recorded descendants that create another session/
process group remain in the enumeration and follow the anchor-or-hold rule; an unobserved host descendant that
escapes all recorded ancestry is an explicit macOS containment limit, not grounds for a success claim.
Deadline-kill tests use fake children only; a live provider turn is never killed merely to prove the mechanism.
Prepared recovery is resumable only when its retained anchor remains exact and its group has no possible workload;
the crash window after a workload spawn but before the Running journal therefore holds. Journal generations are
read through the retained directory descriptor with no-follow and before/after identity checks. Before terminal
success, the supervisor descriptor-pins and hashes the actual child join and optional aggregate bytes, validates
their run/window/hash bindings, parses the unchanged aggregate, and only then releases anchors.

### Admission and concurrency

R3d adds a non-blocking exclusive owner-wide compatibility admission lock inside the runner's billable
boundary, not only in launchd. Scheduled, pre-merge, and manual compatibility commands therefore contend on
the same local lock. The lock is acquired before reservation or provider-capable spawn, records holder and
trigger identity, and refuses rather than queues. Per-run liveness leases continue to identify cleanup; they
are not admission locks. `environment_owner` structurally excludes cross-machine execution.

The delivery claim is narrow: at most one R3d-aware billable compatibility aggregate per owner machine at an
instant, no per-attempt replay, and at-least-once evidence opportunities across distinct windows. It does not
claim to serialize unrelated interactive provider turns through `serve` or an uncooperative pre-R3d binary;
the active-grant operator contract, detection, hold, and reconciliation boundary above govern the latter. A
crash releases the OS lock, but the durable supervisor/ledger reconciliation must clear before a successor
can obtain effect authority.

Serialization is not deduplication. Before reservation, the same transaction computes a trigger-independent
`equivalent_work_key` from the complete canonical `case_execution_fingerprint`, evidence-purpose
contract, and required freshness bucket. Trigger source/kind, request/window/attempt/repeat ids, and provider-
effect authority identify the consumer/admission and exist only in the admission fingerprint/reservation record,
not this key. The freshness bucket is canonical policy output, never a trigger-specific timestamp. The policy
defines a closed evidence-purpose lattice: `claimed_support_gate` may satisfy
`provider_path_advisory` only when every exact identity, prompt, cap, and freshness requirement is at least
as strong, while `manual_diagnostic` is incomparable by default. `characterization` is also an execution-only
consumer purpose: preexisting evidence cannot satisfy the first characterization, although its reviewed
completed evidence may later satisfy `provider_path_advisory` under the ordinary exact-identity/freshness
rules. A live reservation for the key refuses
without queuing; valid completed evidence with an equal-or-stronger purpose is reused without another
provider call and gains a new consumption record binding the requesting trigger and authority to the
existing evidence hash. Because the canonical fingerprint includes exact test-merge/main identity and every
case/pin binding, reuse cannot cross a claimed-support test-merge result or material case change. A matching
characterization profile alone is never an equivalent-work match and grants no evidence reuse.

A deliberately repeated diagnostic requires a one-run manual repeat authorization. The automatic transient
confirmation requires an exact one-confirmation allowance in the provider-effect standing grant, applies
only to `confirmation_due` in the next eligible window, and has its own budget/repeat nonce. Neither repeat is
inferred from a different trigger.

### D9 — durable budget accounting

The local mode-`0600` ledger uses a canonical UTC calendar-day id for audit and idempotence plus a rolling
24-hour provider-attempt ceiling to prevent a midnight double burst. There is no catch-up, carryover, or
automatic borrowing. A case is charged to its admission window even when it crosses midnight.

Before spawn, admission durably reserves the case's declared attempt, token, observable-cost, and time caps.
A valid terminal artifact may reconcile token/cost/time downward. A proved pre-effect refusal releases the
reservation. Possible prompt acceptance, KILL/crash, missing or invalid telemetry, ambiguous spawn state, or
unreconciled evidence keeps the full charge. Subscription providers retain an attempt-count charge even
when USD is zero or absent. Derived USD uses a bound authoritative pricing snapshot; unknown currency or
price keeps the cap charge.

The attempt-idempotency key binds the complete `admission_attempt_fingerprint` plus the repeat nonce when
explicitly present; the separate equivalent-work key above prevents sequential
cross-trigger duplication. Journal and materialized snapshot updates are crash-consistent and replay
idempotently. Characterization, scheduled background, test-merge gates, and generic manual commands have
separate accounting classes beneath shared provider and global ceilings. A one-shot entry carries its own
conservative characterization reservation; no class borrows another class's protected allocation without new
matching effect authority. Manual work retains explicit acknowledgement, may use only otherwise-unallocated
headroom after protected scheduled/test-merge reserves, records `manual_used`, and cannot borrow either
protected pool; an intentional duplicate also requires the explicit repeat authorization above. Exhausted
test-merge capacity leaves the required check blocked, not green. Manual acknowledgement alone cannot
reallocate.
Provider-reported remaining quota, when available, is an additional veto and never accounting truth. This
ledger accounts for bridge compatibility activity only; it does not claim to measure interactive usage from
other repositories or agents.

One-shot entries establish separately reviewed conservative profile maxima before characterization evidence
exists. Standing-grant operational caps are then derived from that evidence plus a documented margin and must
be no greater than the characterized profile maxima; lowering the operational cap does not change the profile,
while raising a profile maximum requires recharacterization. Neither inherits the current broad `500000` token /
`$5` aggregate defaults or auto-increases.

### D4 — evidence store, retention, visibility, and privacy

The hot root is `~/Library/Application Support/a2a-bridge/operator/evidence/`, mode `0700`, with mode-`0600`
files and a 10 GiB hard cap:

| Hot allocation | Cap |
|---|---:|
| JSON/index/ledger/tombstones | 1 GiB |
| Active scratch | 4 GiB |
| Recent sealed bundles | 5 GiB |

The cold root is `~/Documents/a2a-bridge/evidence-archive/`, mode `0700`, with an initial 25 GiB cap.
This is an explicit owner-approved iCloud Drive replication/offload boundary, not local-only storage:
secret-scanned sealed evidence may be uploaded to and later offloaded by the owner's private iCloud account
when—and only when—cold-storage consent is currently valid and binds `owner_icloud`, the root, and the local
FileProvider domain. Provider-effect authority may be valid, expired, or revoked independently; it neither
grants nor withdraws cold-storage consent. No other cloud destination or public artifact publication is
authorized.

Completed directories are sealed as one compressed archive plus manifest rather than syncing package-tree
fanout. The trusted packer first constructs, seals, secret/redaction-scans, and hashes the archive plus
manifest in descriptor-held local-APFS scratch under the hot root. A scan failure creates no path in the
iCloud-backed cold root. Only after that immutable local object passes the scan and the action-time storage-
consent fence may the publisher create a cold `.partial` mode `0600` through descriptor-relative,
no-follow/create-new operations and copy the already-scanned bytes. It verifies copied length and SHA-256,
then atomically renames within the cold root. Before and after publication, each cold partial/archive/manifest
must be the same single-link regular file with the expected owner/device/inode. Any symlink, replacement,
hard-link, placeholder/unavailable-file, scan, copy, hash, or unexpected FileProvider-domain anomaly refuses
archival and degrades status without deleting the hot source.
Active databases, locks, ledgers, indexes, and runtime images stay local; R3d adds no application-level
encryption claim beyond the owner's iCloud account/platform protections.

Hot eviction has a second action-time fence: reopen the cold object no-follow, require it fully materialized,
single-link, readable, and hash-valid, and require the platform's uploaded/synchronized state for the bound
FileProvider domain. If status is unavailable or any check fails, retain hot evidence and degrade/block
rather than deleting it. After safe hot eviction, iCloud may offload the archive as intended. Weekly
reconciliation checks remote-presence metadata for every retained archive and rehydrates/hash-verifies a
bounded rotating batch so every full-evidence object is content-verified at least once per 30 days and before
consumption. Missing/corrupt/unavailable evidence creates a retention incident and blocks new hot eviction;
it never receives a fabricated tombstone or green status.

Sizing was approved from the 2026-07-17 observation that the operator directory was about 45 MiB, the two
retained R3c resolution directories were about 520 MiB and 604 MiB with package trees dominating, and the
Mac had about 498 GiB free. The 10/25 GiB limits are policy ceilings, not targets, and must not silently grow
when later bundle/image measurements change.

| Evidence class | Full evidence | Compact/audit record |
|---|---|---|
| Routine green scheduled | 30 days | 180 days |
| Preflight-blocked/not-run | 90 days | 180 days |
| Failed or `candidate_unknown` | 180 days | 1 year |
| Manual compatibility | 90 days | 1 year |
| Incident | Pinned until explicit unpin, then 180 days | Permanent tombstone |
| Promotion/release | Through supported release lifetime | Permanent release record |
| Authorization/budget audit and tombstones | — | 1 year minimum |

These clocks have one explicit precedence. A case/recipe `retention_days` is the minimum full-evidence
clock for that case; the evidence-class table may extend but never shorten it. Effective full retention is
`max(case_or_recipe_minimum, evidence_class_minimum, active_pin_or_release_lifetime)`, measured from
terminal publication. The sealed cold archive is the authoritative full-evidence object for that clock.
Compact records follow the table's independent later clock, while pins override both until explicit unpin.

Hot materialization is a cache, not a second retention authority: after a cold archive is verified and no
lease is open, routine green material stays hot at least 14 days and blocked, manual, failed, or unknown
material at least 30 days; pins may require longer. Quota GC may evict eligible hot bytes after those minima
without shortening the authoritative cold full-evidence clock. Reconstructible resolution/package bundles
and runtime images follow the cache/GC rules below, but their deletion cannot remove the sealed aggregate,
schedule sidecar, diagnostics, provenance inventory, or audit record required by the effective clock.

The two R3b live-attempt aggregates are migrated from `/private/tmp` into pinned incident records with their
known hashes during rollout. If a source is already missing, record that absence and the expected hash; never
fabricate migrated evidence.

`compatibility schedule status [--json]` reports last/next window, current policy/provider-grant hashes and
expiry, independent storage-consent hash/state/expiry,
case lifecycle, last outcome, holds/quarantines, ledger headroom, storage/archival state, and missed ticks.
Local macOS notifications fire only on transitions: green-to-red, recovery, auth expiry/block, missed tick,
quota/storage pressure, safety hold, or unreaped ownership. Raw evidence is never committed or published to
GitHub; the only upload exception is the storage-consent-bound private owner-iCloud cold tier above. Later GitHub
publication may expose a separately reviewed sanitized summary; the initial check contains only state, exact
SHA, and an evidence-record hash.

Drift-red, safety hold, duplicate billing, or unreaped ownership notifies immediately. The first auth block,
unknown result, storage block, or missed tick notifies once and deduplicates while the fingerprint is
unchanged. A repeated identical non-waste `candidate_unknown` only increments its count/last-seen audit and
remains in its existing unknown/hold/block lifecycle; it never enters confirmation or suppression merely by
repetition. An independently typed immutable waste condition suppresses immediately, while the second
identical complete typed transient failure changes suppression state under D6; each suppression transition
notifies. Recovery always notifies. Notification delivery failure is retained in status but never rewrites
the underlying canary outcome.

### D8 — floating freshness and garbage collection

Every scheduled or pre-merge floating execution obtains a same-window observation of registry versions and
integrities, base-image digests, relevant configuration, and the pricing/ranking input used for selection;
the one admitted smoke session supplies the same-window observed effective model/effort/mode and catalog
observation. The observed identity is result evidence and must match the pre-admission characterized
expected-effective identity as defined above. If the
provider-free immutable identity matches a verified retained object, reuse its content-addressed bytes and
record a new observation. Materialize/build only on identity change, missing bytes, or failed verification.
The low-cost smoke still runs once in an eligible scheduled window because unchanged packages do not exclude
provider-side or authentication drift. Failed discovery or an unsafe/missing same-session catalog is
`candidate_unknown`; yesterday's bundle remains historical and is never relabeled current.

Bounded cleanup runs after each tick and a full reconciliation runs weekly:

- routine-green bundles keep at most the latest three per provider/case, are not eligible before 14 days,
  and are not retained past 30 days solely to satisfy keep-last-three;
- failed/unknown reconstructible hot bundle caches retain 90 days; incident/promotion bundle caches follow
  their pins; bundle manifests and inventories retain 180 days. These cache clocks never shorten the sealed
  full-evidence clock above;
- runtime images remain local: keep current production, the last two successful candidates per provider,
  pinned images, and every digest referenced by any running or stopped container;
- a shared bundle lease excludes open readers from deletion. Image GC holds the relevant admission/GC lock,
  queries all container references by immutable digest, and treats a runtime race/refusal as a visible safe
  skip rather than forcing removal;
- tombstone is durable before unlink and crash recovery completes the same idempotent action;
- quota pressure deletes only eligible unpinned oldest evidence. If that cannot restore headroom, status is
  degraded and later large materialization is blocked; protected evidence is never force-deleted.

This explicitly supersedes R3c's operator-only cleanup rule only for objects indexed and leased by R3d. It
does not authorize broad Docker/Podman cleanup or removal of unrelated bridge/operator artifacts.

### D7 — deterministic impact classification and the pre-merge gate

GitHub PR/push CI remains non-billable. A trusted classifier produces a typed impact suggestion; the local
scheduler recomputes impact from the exact immutable diff and its own trusted classifier before using it.
The remote artifact is never authority.

| Changed surface | Cases made due |
|---|---|
| ACP adapter/protocol/runtime bridge | affected Codex/Claude host and reader cases |
| Containerfile, mount, credential-copy, or runtime integration | affected reader cases |
| Model catalog, alias, effort, mode, or capability policy | exact affected claimed-support cases; a low-cost substitute is insufficient |
| Authentication/provenance | authenticated cases for that provider/environment |
| Smoke, compatibility runner, supervisor, ledger, or scheduler | all eligible characterized cases |
| Tests only | deterministic tests only unless production impact is also classified |
| Documentation only | no provider case |
| New OpenRouter/OpenCode/provider path | `characterization_required`, never immediate scheduling |

The PR-head/base classification is advisory until GitHub creates the current synthetic test-merge result.
R3d's atomic guard is the required context on that result:

1. Before enforcement, the exact base-branch rule must require one new dedicated R3d context with strict
   up-to-date protection and its expected authenticated publisher source. That context is never posted to a
   PR head. If GitHub cannot bind the expected source for this repository, the gate remains disabled until an
   equivalent owner-approved source restriction exists. Missing, changed, unreadable, loose, bypassed, or
   head-populated protection leaves R3d pending and refuses enablement.
2. After ordinary deterministic PR checks pass and the debounce expires, the local watcher accepts only the
   configured repository/base, same-repository non-fork PR, operator-approved author, exact current base and
   head, `mergeable = true`, non-null API `merge_commit_sha`, and canonical `refs/pull/<n>/merge` ref. A
   webhook, workflow artifact, PR code, or caller-supplied SHA/ref is never authority.
3. The watcher fetches that exact ref; requires the fetched SHA to equal the API `merge_commit_sha`; proves a
   two-parent test merge whose ordered parents are the observed base and head; records its tree; recomputes
   impact on that exact merge result; and validates provider-effect grant/case characterization. Any mismatch,
   conflict, unknown mergeability, missing ref, or non-canonical parent relation stays pending.
4. The scheduler builds that exact test-merge SHA in a fresh isolated environment with provider and GitHub
   credentials withheld, then runs the verified candidate with only the narrow credentials required for the
   affected fixed cases. Model-, effort-, mode-, alias-, capability-, auth-, or provider-specific impact
   executes the exact currently claimed support identity, including Sol/Fable where applicable. A provider-
   minimal low-cost substitute is allowed only for a classifier-proven provider-generic seam whose mutation
   tests demonstrate that model-specific behavior is unreachable.
   If the PR introduces or changes a D7-reachable claimed-support profile, the check remains
   `in_progress(characterization_required)` and the watcher cannot characterize it. The operator may issue a
   separately reviewed one-shot entry bound to that profile and the then-current exact test-merge execution,
   invoke the explicit characterization command under a manual characterization trigger, and revise the
   standing grant after terminal reconciliation. Test-merge regeneration retains only the completed profile
   characterization; the required gate must still execute the new exact execution fingerprint. No impact class
   is enabled unless every profile it can select is inventoried, characterized, and present in the grant.
5. The publisher records evidence and posts the dedicated required conclusion only on that exact test-merge
   SHA. `success` requires an executed passing due set or locally proved `not_applicable` classification.
   `failure` requires a typed immutable/invalid-candidate failure or the second identical complete failure
   from the one authorized confirmation. A first clean transient/untyped `candidate_fail` records its attempt,
   evidence, charge, and `confirmation_due` state but leaves the single check run `in_progress`; it creates no
   terminal GitHub-consumption record. Unknown, blocked, expired, invalid-evidence, and due-but-not-run states
   likewise remain `in_progress/pending`. GitHub `neutral`/`skipped` never satisfies the contract. A consumed
   artifact counts only when it proves the same test-merge SHA/fingerprint and equal-or-stronger purpose:
   terminal pass may satisfy, terminal failure fails, and confirmation-due/unknown/blocked/expired/invalid-
   evidence stays pending. Publication durably creates the one terminal GitHub-consumption record under the
   then-current policy, authority, and freshness bucket before posting.

   Multiple due cases use one deterministic reducer over the complete ordered due-set vector. Success waits
   until every case is terminal pass. Any typed immutable or confirmed terminal case failure fails fast;
   remaining unstarted confirmations become `not_run(aggregate_terminal)` and consume no nonce, attempt, or
   budget. Otherwise the aggregate remains `in_progress`, including when one case has recovered but another
   remains `confirmation_due`, blocked, or unknown. The one terminal consumption binds the due-set vector,
   every attempt/evidence record present at the decision, and every explicit not-run reason.
6. Immediately before consumption and publication, the publisher re-fetches PR metadata and the canonical
   test-merge ref plus the exact active branch-protection/ruleset state. In one guarded observation it proves
   the SHA, base, head, ordered parents, and tree still match; strict mode remains enabled; the dedicated
   context and expected authenticated source remain required; and the context is absent from the PR head.
   It binds the active rule ids and canonical response hashes into the sidecar before posting. Absence or any
   identity/rule/source drift blocks publication; changed test-merge identity makes the artifact historical.
   A newly generated different SHA has no R3d context, so the required gate remains unsatisfied until new
   evidence exists.

R3d publishes only through the Checks API as the expected GitHub App; commit-status publication is not an
allowed implementation. Before any terminal conclusion, the publisher creates one `in_progress` check run
for the exact SHA/context/source with a deterministic `external_id`, then durably binds the returned
`check_run_id`. A create request whose response is lost enters `create_unknown`: recovery lists the exact
SHA/name/App and accepts only one matching `external_id`. Zero or multiple matches remain pending under a
safety hold and require explicit operator reconciliation; recovery never blindly creates a duplicate.

Terminal publication uses a crash-consistent outbox under the publisher lock. Its durable lifecycle is
`create_intent -> create_unknown|remote_pending -> prepared -> update_unknown -> remotely_observed ->
confirmed`; `create_unknown` can move to `remote_pending` only after the unique remote match above. The stable
outbox identity binds repository/PR/test-merge identity, check-run/external ids, and context/App source.
`prepared` atomically adds the terminal consumption id, desired conclusion, complete evidence set/hash, and
final guarded observation/rule hashes, so the terminal consumption and exact desired update are durable.
The publisher then PATCHes that already-bound check-run id; it never creates a terminal check. A successful
GET of that exact id showing the expected SHA/
name/App, external id, conclusion, and evidence details moves through `remotely_observed` to a durable local
`confirmed` record.

The confirmation lifecycle never replaces the remote check object. The first transient attempt leaves its
outbox at `remote_pending` and its check `in_progress`; crash recovery restores `confirmation_due` from the
durable attempt/evidence/ledger records without another provider call. A separately authorized later-window
confirmation has its own attempt id, repeat nonce, and charge. If it passes and every other due case passes,
the publisher prepares one terminal success consumption referencing the complete evidence set and PATCHes
the same check-run id. If the identical complete failure repeats, it prepares terminal failure and PATCHes
that id. A typed immutable failure may terminalize immediately. If the test-merge identity changes first,
the old confirmation state becomes historical and cannot authorize work for the replacement SHA.

With multiple confirmation-due cases, each confirmed pass is journaled without preparing terminal success
until the reducer sees all passes. Crash/restart resumes the remaining ordered set without duplicating the
resolved case. A terminal failure stops later admissions under the fail-fast rule above. Test-merge
regeneration historicalizes the entire partial vector and leaves the old check nonterminal; no unresolved
case or nonce transfers to the replacement SHA.

Recovery never replays provider work or creates another consumption. Before any retry it GETs the persisted
check-run id. An exact terminal match is confirmed without another write. An exact `in_progress` result may
be PATCHed again only after the complete final guard is rerun; this updates the same remote object. A missing,
multiple, wrong-source, wrong-SHA/context/external-id, conflicting-terminal, malformed, or unavailable result
stays pending and creates a safety hold rather than posting another check or conclusion. A lost PATCH response
therefore reconciles by observation first. GitHub may expose the exact terminal result before the local
`confirmed` journal write when a response is lost; that distributed gap is safe because the terminal
consumption and desired outbox entry were durable before PATCH. Local status remains `publication_unknown`
until the exact remote terminal is observed and journaled; the design does not claim an atomic local/remote
commit.

The required test-merge context—not watcher timing—is the atomic merge guard. GitHub documents that
`merge_commit_sha` names the current test merge before merge, `refs/pull/<n>/merge` represents what the
repository would look like if merged now, a status on the test merge must pass, and checks from a previous SHA
do not satisfy the latest commit. R3d deliberately never creates this context on a PR head, so GitHub's
head-status fallback cannot green a regenerated test merge. Strict up-to-date protection is defense in depth,
not the source of exact-result atomicity. The current public personal repository cannot use GitHub merge
queues; transferring to eligible organization infrastructure would be a separate owner decision, not a
hidden R3d prerequisite.

GitHub does not consult local policy, freshness, or provider authority again when an administrator later
merges a commit whose required context is already successful. R3d therefore makes the enforceable narrower
contract explicit: after the terminal consumption record and success are published, that success is valid
for the lifetime of the identical immutable test-merge SHA. If an absent ref reappears with the same SHA,
base, head, ordered parents, and tree, the existing success remains valid; later policy/freshness-bucket or
authority changes do not retroactively revoke it. They do block new execution, evidence reuse for another
identity/purpose, and publication on another SHA. If owner policy later requires action-time freshness at
the merge click, this gate must remain disabled until a GitHub-native or other action-time primitive can
enforce it; watcher timing cannot provide that guarantee.

An administrator can change or remove protection after the final guarded read or after publication. That is
an explicit owner administrative bypass outside R3d's atomic claim, not a scheduler success. The next watcher
observation records the drift locally, blocks later publication, and requires restoration or an explicit
operator waiver; R3d does not claim it can prevent a repository administrator from removing the guard.

If the test merge is regenerated before any possible provider acceptance, its reservation is released and
the new result may admit normally. If regeneration occurs after possible acceptance, the old attempt
completes only as historical evidence with its conservative charge; the replacement remains pending until an
explicit one-run reauthorization reserves its fresh fingerprint. Base/head churn never becomes automatic
billable replay merely because GitHub produced another test-merge SHA.

PR code, workflow output, and webhook callers cannot supply a prompt, command, path, case id, model, budget,
provider-effect grant, or local effect. The local GitHub credential is never inherited by the build or
candidate process.
Forks and unknown authors fail closed. Because an unavailable local scheduler leaves the check pending,
branch protection has one explicit audited operator bypass for emergencies; it never manufactures a pass.

After trusted merges, the next local scheduled window coalesces all commits since the last successful main
run and executes each affected eligible case at most once. That integration backstop records commit range and
classifier version/hash. It catches merge-commit or interacting-change differences but is not the primary PR
guard. An immediate post-merge run remains an explicit operator action.

### D5/D10 — deferred representative and long-lived-operator lanes

Broad scheduled repository reviews are not part of minimum R3d. Before implementation, a separate focused
design increment with operator input must choose fixtures, planted/expected findings, tolerance/scoring,
provider rotation, model/effort, costs, and evidence classification. Any later scheduled representative
probe is Tier 2 read-only only, targets an exact fixture commit, uses a fixed checked-in prompt, rejects
arbitrary repositories/prompts and any write-tool event, has hard deadline/output/token/cost caps, and never
retries/falls back. Review content is evaluation material, not compatibility or promotion evidence. At most
one representative fixture run occurs per week total while rotating providers; Fable/Opus are not scheduled
by default.

R3d records `fresh_one_shot_compatibility = pass|fail|unknown` and
`shared_operator_health = not_evaluated`. A green one-shot result cannot imply a healthy long-lived service.
R2f owns structured pre-prompt ACP failures, backend/session capacity, capability-gated `session/close`, dead
backend detection, phase-aware stagnation, and generation-based non-disruptive drain/rotation that preserves
running turns and warm sessions. After R2f exists, R3d may display its separately produced read-only health
summary but may never act on it.

### Scheduled artifact additions

The existing strict R3 aggregate and nested schema remain byte-compatible schema version 1; R3d does not add
fields to them or claim that old `deny_unknown_fields` readers ignore additions. Scheduling metadata lives
in a separate strict, independently versioned `ScheduleEvidenceRecord` JSON sidecar. It binds the aggregate
SHA-256 when one exists plus trigger kind; repository/PR/base/head, exact test-merge SHA/ref/tree and ordered
parents, the guarded observation id, or scheduled-main commit range; policy,
the exact tagged admission-authority kind and complete arm identity—batch-authorization id/hash plus one-shot
entry id/generation/hash/consumption nonce, or standing-grant id/generation/hash plus characterization id/hash—
with canonical typed absences, storage-consent, quarantine, and characterization
ids/hashes; tagged scheduled-advisory-registry or production-support-manifest source/row hashes and canonical
typed absences; canonical characterization-profile, case-execution, and
admission-attempt fingerprints/versions plus the canonical `profile_policy_bundle_hash`;
window, attempt-idempotency, equivalent-work, consumption, and optional repeat ids; classifier version/hash
and affected cases; complete deadline derivation; preflight results; admission lock holder; budget
reservations/reconciliation; supervisor anchor/process identities, anchor acquisition/release,
escalation/reap results; freshness observation;
requested and characterized expected-effective identities plus the separately observed effective result;
typed check scope (`test_merge_result`), required-rule/context/source hashes; publication-outbox id/state,
check-run/external ids, optional terminal-consumption id and desired conclusion with typed absence before
`prepared`, remote-observation hash/attempts, and status publication result; plus evidence-index id. The
dedicated context is invalid on a PR-head SHA by construction.

The parent publishes the sidecar even for a killed/setup-incomplete run and joins it to the runner artifact
by run/window id and optional aggregate hash. New schedule readers validate the sidecar and unchanged
aggregate separately and never invent schedule authority when the sidecar is absent. Old R2d/R3 binaries
continue reading/comparing the unchanged aggregate and are not expected to understand the sidecar. An
unknown sidecar version fails schedule-specific consumption without making the underlying aggregate invalid.

### Dependency-ordered implementation slices

1. **R3d0 — design/policy/schema foundation (non-billable).** Land this approved design; add checked-in
   scheduling, characterization-profile inventory, one-shot-characterization authorization, provider-effect-
   grant, tagged admission-authority, one-run manual-admission,
   storage-consent, characterization, hold/quarantine, typed
   failure/suppression, impact, ledger,
   canonical profile-policy bundle, characterization-profile, case-execution, and admission-attempt
   fingerprints, equivalent-work/
   consumption, scheduled-case registry, strict scheduled- and claimed-support-characterization sources,
   schedule-sidecar, publication-outbox, evidence-index, status, and routing
   schemas plus validators and docs. Scheduled prerequisites reuse the production credential-name exclusion,
   and mounted owner cwd paths resolve to real directories inside the trusted root; an offline static-path
   validation never authorizes execution. Add and review
   every exact provider-minimal advisory row/config intended for R3d5 characterization; keep it outside the
   protected support manifest. Add the initial four exact claimed-support profile references without changing
   that manifest. Do not execute either class. No timer, credential access, registry/runtime effect, or provider
   path.
2. **R3d1 — supervisor and signal parity (non-billable).** Add `schedule-tick` parent, SIGTERM parity,
   derived deadline, retained bridge-owned group-leader anchors through TERM/grace/KILL/reap, exact process-
   tree identity, anchor-or-hold handling for descendant-created groups, repeated-cancel, recovery, and joined
   parent/child artifact contract using fake processes only.
3. **R3d2 — authority, admission, preflights, and accounting (non-billable tests).** Add provider-effect-grant,
   one-shot-characterization, and storage-consent validation with independent revocation linearization, the
   mutually exclusive scheduled/claimed-support-characterization admission-authority reducers, scheduled- and
   claimed-support-characterization-source generators/validators with independent profile-policy-bundle
   rederivation, strict
   one-run local `ManualAdmissionV1` derivation/consumption, owner-wide lock, authority-bound attempt
   fingerprints, profile-indexed one-shot uniqueness,
   equivalent-work reuse/refusal, characterization state,
   three control types, durable reserve/reconcile ledger, UTC/rolling windows, scheduled/test-merge/manual
   accounting classes, legacy executable/process detection and conservative import, and automated zero-effect
   preflights. Immediately before admission, re-resolve the trusted owner root and requested cwd as real
   filesystem objects and fail closed unless the cwd remains contained; an R3d0 offline/static validation is
   never sufficient for that action-time check. Before wiring R3d1 to a production control implementation, make
   Darwin zero/error group enumeration errno-aware for absence proof, own cancellation before `Running`, supply an
   exact-runner-exit primitive, preserve whichever SIGINT/SIGTERM registration succeeds if the other fails, and
   either exclude `.` from externally derived supervisor record ids or give each record a private journal directory.
4. **R3d3 — evidence, status, and retention.** Add hot/cold stores, index, sealing, pins, tombstones, quotas,
   leases, bundle/image GC, local CLI/status/notifications, strict schedule-sidecar plus unchanged-aggregate
   compatibility, crash-consistent publication-outbox journal storage, explicit owner-iCloud upload/offload
   state plus rotating content verification, quarantine-opening dereference under the owner lock before
   closure, and incident migration.
5. **R3d4 — launchd and trusted test-merge/main triggers.** Add dry-run/preflight-only modes, fake-clock
   timer, impact classifier, exact `merge_commit_sha` / `refs/pull/<n>/merge` watcher, local GitHub check
   publisher App including its single-check-run crash-recovery outbox, consumed-evidence mapping, and
   changed-test-merge invalidation, coalescing,
   debounce, and the disabled-by-default launchd installation. Add a live validator for strict branch
   protection, the dedicated required context and expected source, canonical test-merge production, and the
   invariant that the context never exists on a PR head. No schedule or required check is enabled by merge.
6. **R3d5 — characterization and staged enablement (separate live authority).** Characterize every inventoried
   future scheduled-advisory profile with its lower-cost model and every D7-reachable claimed-support profile
   with its exact Sol/Fable production identity under exact single-use `characterization_once` entries. Finalize
   provisional hold/waste classifications and exact caps, exercise the rollout ladder, then issue and enable
   the first post-characterization provider-effect standing grant/timer and only the required-check impact
   classes whose complete profile set is characterized, after the deterministic and review gates are green.

Each code slice must be reviewable and default-off. It may merge independently only when its own complete
tests are green and no earlier invariant is weakened. The full R3d branch receives one Sol/xhigh adversarial
implementation review; because supervision/concurrency/accounting is hard and cross-cutting, one Fable/xhigh
adversarial implementation/release lens is justified only after Sol is green, with no Fable re-review loop.

### Required deterministic evidence

- Every new behavior has a regression that fails against its pre-change implementation plus a negative or
  edge case for every new path.
- Policy/effect-authority absence, expiry, revocation before reservation, stale hashes, owner/host mismatch, broadened
  cases, invalid caps, wrong label/plist hash, deadline past selected effect-authority expiry, and unauthorized trigger
  all refuse before effects. Revocation after durable reservation blocks successors without killing the
  bounded admitted attempt. Missing/revoked/wrong-root/wrong-domain cold consent or a cold-publication bound
  past consent expiry keeps scheduled or explicitly acknowledged manual evidence hot with zero iCloud write.
  Crossed-state tests prove expired/revoked provider authority plus valid storage consent can archive only
  already-completed evidence, while valid provider authority plus expired/revoked storage consent cannot
  create a cold entry. Consent revocation before its publication journal blocks the write; revocation after
  that linearization permits only the already-admitted bounded copy and blocks every later archive/eviction.
- Rollback state/crash tests revoke the standing grant and every `available` or `consumed_unreconciled`
  one-shot entry under the authority lock before labels/check admission are disabled. A crash at each journal
  point recovers fail-closed; an already reserved bounded attempt is not killed, reconciled history remains
  immutable, and no unused/revoked entry can admit or be replayed after rollback.
- Tagged-admission tests prove an exact `characterization_once` entry admits the explicit first profile
  characterization while both completed profile characterization and applicable standing grant are absent,
  and that `standing_grant` later admits a different exact execution under the same profile. Missing/unknown/
  both arms, mixed fields or noncanonical absences, stale/expired/revoked/already-consumed authorization, wrong
  row/profile/execution/proposed identity/caps/command, prior profile characterization, standing grant before
  characterization, stale/mismatched profile characterization, and characterization authority presented by
  `schedule-tick`, main, or test-merge all refuse before credential or provider-process access. Same-batch,
  cross-batch, concurrent-issuance, issuance-journal crash, and corrupt/divergent-index duplicate profile
  entries are rejected or recovered fail-closed. Crash immediately before
  durable consumption leaves the sole entry available and admits no effect; crash at or after consumption
  leaves it consumed/unreconciled, conservatively charged, non-replayable, and blocks same-profile reissue.
  After terminal reconciliation, only a fresh reviewed authorization naming the prior entry/outcome may reissue;
  it receives a new nonce/budget and no pre-pooled retry. One-shot and standing positives bind distinct
  authority-bound attempt fingerprints, reservations, ledger entries, and sidecars to the same profile and
  their different exact execution fingerprints.
- Generic-manual admission tests prove an explicit local `--acknowledge-billable` derives and consumes one
  `ManualAdmissionV1` identity without reading or inheriting `standing_grant`. Missing acknowledgement,
  caller-supplied/duplicate nonce, replay, stale binary/expiry, wrong source/case/profile/execution/purpose/
  freshness/cap/effect, `serve`/A2A/timer/watcher origin, post-seal mutation, use as characterization, and any
  manual field in `ScheduledExecutionSourceV1` refuse before effects. A direct state-machine positive proves a
  valid generic manual record with canonical absence of both persistent arms reaches the shared durable
  reservation. Mixing either persistent arm/field into that record, or a manual field into persistent-envelope
  admission, refuses before effects. The valid one-run record binds its own admission fingerprint, reservation,
  ledger, consumption, and manual artifact.
- Fake children cover ignored TERM, SIGSTOP, exited runner with surviving descendant group, publication
  wedge, repeated cancellation, unproved exit, startup recovery, and unrelated-process survival. A direct
  recycled-PGID red mutation releases/reaps the anchor before final group signaling, reassigns the old numeric
  group to an unrelated fake group, and demonstrates the stale signal; the production path retains the anchor,
  prevents that reassignment until final reap, and leaves the unrelated group alive. Forced anchor-acquisition
  or anchor-liveness failure and crashes immediately before/after the terminal-signal journal instead hold with
  no stale group-signal retry.
- Scheduled versus manual and test-merge-watcher versus daily trigger sources targeting the same exact execution,
  same evidence purpose, and same canonical freshness bucket, plus concurrent/sequential duplicates and two-
  process races, prove one execution fingerprint/equivalent-work key and exactly one billable attempt. Trigger
  source/kind and request/window/attempt/repeat or manual-versus-standing authority mutations change only
  admission fingerprints; changing the exact target SHA/range changes the execution fingerprint and is
  intentionally non-equivalent. A separate `manual_diagnostic` purpose remains incomparable with scheduled
  advisory work and therefore does not reuse it. Completed equal-or-stronger evidence is reused through a
  consumption record; explicit repeat/confirmation uses a distinct authorization and budget. Crash/restart
  never replays or double-charges.
- Canonical-fingerprint mutation tests change every profile field independently: repository, tagged semantic
  scheduled-advisory or production-support row, policy/recipe/resolution constraint, template/prompt/artifact
  policy, case/purpose/execution shape, provider/agent/capability/family, auth/prerequisite/environment,
  requested or expected-effective
  model/effort/mode, and maximum cap. Each changes the profile and returns characterization to required;
  absent-versus-empty and ordering/normalization collisions are rejected. Separate exact-execution mutations
  change PR/test-merge SHA/ref/tree/base/head/ordered parents or main range, candidate bytes, exact manifest/
  pin/resolution/package integrity, image/base digest, derived config, or actual cap within the characterized
  maximum. Each prevents evidence reuse but retains the same profile and admits exactly one new canary under
  the unchanged `profile_policy_bundle_hash`-bound `standing_grant`; the grant never binds the exact run manifest.
  An out-of-constraint resolution or cap above the maximum refuses. Byte-identical candidates in two test
  merges never share `claimed_support_gate` evidence, and new floating resolutions never consume prior-version
  evidence.
  Observed-effective equality passes, while a mismatch returns `candidate_unknown`, holds the original
  fingerprint, forbids reuse/consumption, and never mutates the admitted reservation key.
- Profile-policy-bundle mutation tests independently change each bound policy, semantic scheduled-advisory or
  D7-reachable production-support row, characterization-inventory entry, resolver/recipe constraint, template,
  allowed effect class, and profile-maximum input. The canonical hash
  changes and every one-shot entry or standing grant carrying the old/wrong hash refuses before effects; omitted,
  reordered, broadened, and noncanonical inputs also refuse. Exact test target, candidate, run-manifest, pin,
  resolved package/image/config, or within-maximum actual-cap drift leaves the bundle hash and standing grant
  unchanged while still changing the exact execution fingerprint as specified above.
- Legacy-boundary tests allow the exact retained `serve` process, safety-hold a pre-R3d
  `compatibility run` or ambiguous child at both preflight fences, import a validated legacy aggregate,
  retain a full charge for an ambiguous attempt or unknown initial rolling window, and state-test the
  trusted-operator post-fence limit.
- Ledger crash points cover reserve-before-spawn, spawn ambiguity, prompt acceptance, terminal reconcile,
  test-merge regeneration before versus after possible acceptance, midnight crossing, rolling guard,
  missing usage/cost, protected scheduled/test-merge reserves, manual unallocated headroom, and idempotent
  restart. Pre-acceptance regeneration may release; post-acceptance regeneration charges and requires explicit
  replacement-result reauthorization rather than automatic replay.
- Preflight fixtures cover OAuth runway, provider/model removal, config drift, storage pressure, runtime
  start degradation, stale price/ranking snapshot, container cleanup failure, and
  characterization/quarantine/hold states with zero provider calls. Characterization-inventory fixtures cover
  the four initial production-support profiles plus all scheduled-advisory rows; omitted, duplicate, stale, or
  newly D7-reachable uncharacterized profiles keep their impact class/check disabled or pending.
- Failure-disposition tests prove typed immutable failure suppresses and terminalizes immediately. A first
  transient/untyped `candidate_fail` becomes `confirmation_due`, leaves the same remote check `in_progress`,
  and creates no terminal GitHub consumption; crash/restart preserves that state without replay. One
  separately authorized confirmation pass terminalizes the same check as success, while a second identical
  complete failure suppresses and terminalizes it as failure. Authority/quarantine/budget states never enter
  the waste machine or terminalize the check.
- Repeated-unknown negatives feed two identical unavailable-catalog, publication-ambiguous, invalid-evidence,
  and infrastructure `candidate_unknown` results through status/notification handling. They increment and
  deduplicate audit/notification state but never create `confirmation_due` or suppression. A matched positive
  control proves the second identical complete typed transient failure does suppress and notify, while an
  independently typed immutable waste condition suppresses on its first observation.
- Multi-case confirmation fakes put two ordered cases in `confirmation_due`. Pass/pass resolves in both
  orders; a crash after the first pass preserves `in_progress`, no terminal consumption, the resolved case,
  and the one still-unused nonce/charge, then the second pass creates exactly one success consumption binding
  all four attempts. Pass/identical-second-failure resolves in both orders: pass-first binds all four attempts
  at the later failure, while failure-first terminalizes immediately and proves the other confirmation is
  `not_run(aggregate_terminal)` with zero second nonce/charge. Regeneration after one pass historicalizes the
  entire old vector, leaves its check nonterminal, and transfers no case/nonce to the new SHA. Member/order,
  crash, duplicate-attempt, and incomplete-vector mutations never prepare success or lose a pending case.
- Fake-clock tests cover DST, sleep/missed window, no catch-up, duplicate tick, debounce, coalesced commits,
  test-merge-ref creation/deletion/regeneration, changed base or head, the contained-base/stable-head
  change-then-revert construction, unavailable local scheduler, required-check timeout, and a complete
  deadline whose
  metadata/build/preflight/resolve/case/publication/archive/cleanup terms cannot be omitted or reset.
- Impact tests cover every path class, exact claimed-model/effort/mode/alias/capability regressions,
  classifier mutations that attempt an unsafe low-cost substitution, forks/unknown authors, PR-supplied data
  that attempts to widen scope, and exact GitHub `success|failure|in_progress` mapping. Only proven
  no-impact may succeed without a provider case; every due-but-not-run fixture remains blocking. Consumed
  exact-test-merge terminal-pass/typed-or-confirmed-terminal-fail/confirmation-due-or-unknown fixtures map to
  success/failure/pending respectively; a check on a PR head or superseded test-merge result never counts.
  Test-merge fixtures prove a base advance from `B0` to
  contained commit `C` with stable revert head `H` produces a distinct required result, refuse missing/
  unreadable/noncanonical refs and non-two-parent merges, and reject ref/API/base/head/parent/tree/observation
  races at final publication. A delete/recreate fixture begins with an already-published success and proves
  an identical immutable result keeps that success for its SHA lifetime without inventing a GitHub-invisible
  generation or a second local consumption. Policy/freshness/authority changes after publication do not
  retroactively revoke it, while a changed SHA has no context and requires new authority/evidence. Protection
  fixtures prove strict mode, the unique required context and expected
  source, and absence of that context from every current/historical PR-head status/check before enablement.
  A regenerated test-merge SHA starts without the context; a result from the prior SHA never counts. Finding
  the context on a head, a loose/missing rule, or an unexpected source prevents enablement instead of
  normalizing timeout to green. A rule/strict/context/source/head-status mutation between evidence completion
  and final publication also refuses. Post-publication administrative removal is classified as an explicit
  owner bypass, never as a canary success.
- Publication-outbox fakes crash after durable create intent, after binding the remote pending check id,
  after terminal consumption/`prepared`, after GitHub accepts the exact PATCH but before its response, and
  after remote observation but before local confirmation. Lost-create recovery accepts exactly one matching
  App/SHA/name/`external_id` and never blindly POSTs again. Lost-update recovery GETs the exact persisted id:
  matching terminal confirms without a write; matching `in_progress` reruns the full final guard before
  PATCHing that same id; absence, duplicates, wrong source/identity, conflicting terminal, malformed payload,
  or unavailable reads stay pending with a safety hold. Tests prove no recovery-triggered provider replay or
  duplicate attempt consumption, no more than one terminal GitHub consumption, no second terminal check,
  no conflicting conclusion, and no local confirmed claim before exact remote observation. A first transient plus
  pass and first transient plus second identical failure both use one remote check across two explicitly
  authorized attempts and one accepted terminal transition, with any lost-response retry targeting only the
  same observed-`in_progress` check id.
  Crash/restart preserves the immutable-SHA success contract when the remote terminal preceded local
  confirmation.
- GC tests cover open-reader/exclusive deletion, started-between-query-and-remove, stopped-container image
  reference, pin survival, manifest/class/pin retention precedence, hot-cache versus cold-full clocks,
  keep/age ordering, tombstone-before-unlink crash, iCloud symlink/hard-link/replacement/placeholder,
  wrong FileProvider domain, not-uploaded/offloaded action-time state, periodic rehydrate/hash corruption,
  local secret-scan failure with no cold entry, partial/copy/hash failure, and quota pressure that blocks
  rather than deletes protected evidence.
- The strict schedule sidecar round-trips through the new parser; the old-parser compatibility fixture reads
  the byte-unchanged aggregate while ignoring the separate file, and unknown sidecar versions fail only
  schedule-specific consumption. Scheduled-case registry tests prove an uncharacterized row is not a support
  claim, cannot run unattended, derives only an advisory lane `floating-current`, classification `canary`
  execution manifest under explicit authority, round-trips that exact wire spelling through the current strict
  parser, and is never parsed as the production manifest by an old binary. Scheduled-source tests are pre-
  change red for Luna/Haiku against the support-baseline resolver, then prove exact registry-bound sources for
  both a new test-merge identity and a newly resolved package/image execute through fake providers under one
  unchanged characterized profile without relaxing `--resolution` or reusing evidence. Direct/hand-authored
  sources, `support` classification, unlisted model/effort/mode, caller-supplied or post-seal package/config/
  image changes, resolution outside profile-bound constraints, missing/revoked authority, old-reader parsing,
  and an attempt to green an exact claimed-support impact with the advisory source all refuse before provider-
  capable spawn. Claimed-support-characterization-source tests exercise all four initial production-support
  profiles through fake providers. The source must preserve the exact protected
  support row and pinned-run validator while binding one-shot/profile/execution/admission identities; wrong or
  non-support row, row/profile drift, standing/manual authority, unattended trigger, replay, an attempt to
  mutate the support manifest/baseline, or an attempt to treat characterization evidence as promotion refuses.
  A newly introduced D7 profile keeps its check pending until that explicit one-shot path completes and the
  revised standing grant covers it. A static/behavioral test
  proves scheduler invocation contains no `serve` endpoint or production-operator lifecycle action.
- Run format/diff checks, workspace all-target check, warnings-denied Clippy, locked release build,
  repository hygiene, manifest/recipe/policy validation, all scheduler CLI tests, and the full serial
  workspace suite. Report exact passed/failed/ignored totals and every live/unexercised boundary.

### Live rollout gates

No compatibility, model-discovery, or live-smoke provider turn is authorized by this design or its reviews;
the recorded design reviews are review evidence only. After all deterministic gates and the required reviews
are green:

1. Install code with timer and GitHub publisher disabled. Run one local `schedule-tick --dry-run`; prove the
   exact would-run plan, canonical fingerprints, derived deadline, hashes, locks, budgets, legacy inventory,
   owner-iCloud domain binding, preflights, and zero effects.
2. Run preflight-only controls including expired OAuth and degraded container start; prove typed not-run
   evidence, status/notification transitions, and zero spend. A fake/exact-fixture legacy compatibility
   process must hold admission while the retained production `serve` identity remains allowed.
3. Have the operator issue one reviewed no-retry/no-fallback `CharacterizationAuthorizationV1` batch whose
   entries enumerate the complete initial `CharacterizationProfileInventoryV1` exactly once: every future
   scheduled-advisory profile plus the four D7-reachable production-support profiles. Invoke the explicit
   characterization command once per entry using the exact approved low-cost identity for advisory rows and
   the exact pinned Sol/Fable identity for support rows, each with its exact environment/config and
   characterization-execution fingerprint. Each scheduled or claimed-support-characterization source binds and
   consumes its own `characterization_once` entry; the batch grants no pooled retries or scope. It omits
   obsolete historical, non-goal, or not-yet-integrated rows.
4. Review the aggregate once, classify known issues/inconclusive cases, set holds/suppressions/quarantines,
   derive numerical caps, record legacy-manual quiescence, explicitly authorize the private owner-iCloud cold
   boundary, and only then have the operator issue the first `standing_grant` provider-effect authority plus
   independent cold-storage consent. Do not rerun failures merely to obtain green.
5. Keep calendar firing disabled, load the reviewed daily job for on-demand use, and invoke its exact label
   once through `launchctl kickstart` under that `standing_grant`. Verify label/plist binding,
   reservation/reconciliation, index, status, retention, and exact evidence binding, then unload it.
6. Let one timer tick fire while the operator observes it. Deadline-kill remains fake-process-only.
7. Run PR classification in shadow mode on one exact trusted PR base/head pair; it remains advisory and is
   never compatibility evidence.
8. After proving every impact class maps only to inventoried, completed characterizations present in the
   standing grant, bootstrap a fresh unique R3d context in shadow mode on an exact test-merge SHA only, then
   make one explicit audited GitHub change that requires that context from the expected source and strict up-
   to-date protection.
   First exercise a no-impact test merge and test-merge regeneration; then, with separate live authority,
   exercise one affected exact test-merge SHA. Verify API/ref/base/head/tree/ordered-parent binding, that the
   context is absent from every PR head, current-result required-check wait, prior-result refusal, the
   contained-base/stable-head construction, pre/post-acceptance churn accounting, publication, missing or
   unmergeable-ref blocking, final branch-rule/context/source hash binding and drift refusal, immutable-SHA
   success lifetime, the single remote check-run/external-id and confirmed outbox record, timeout, and
   emergency/administrator bypass before leaving the rule enforced. Publication crash points remain
   fake-API-only during rollout; do not manufacture an ambiguous live GitHub write.
9. Enable the low-use daily window. First post-enable red/unknown/blocked transitions are reviewed before
   the schedule is left unattended.

### Rollback and restart handoff

Rollback first takes the authority-state lock and increments revocation generations for the provider-effect
standing grant and every nonterminal one-shot entry, including `available` and `consumed_unreconciled`; it
preserves immutable consumed/reconciled audit records. An attempt already past durable reservation remains
bounded and is not killed, but no revoked entry can admit, replay, reissue, or lend scope. Rollback then unloads/
disables both launchd labels and disables the required test-merge check or explicitly bypasses it through the
audited owner path. It leaves cold-storage consent
unchanged so already-completed evidence can satisfy retention unless the operator separately revokes that consent; a
storage-consent revocation blocks new cloud writes/hot evictions but does not claim to recall existing bytes.
Rollback does not revert evidence, erase ledger charges, delete pins/tombstones, restart the long-lived
operator, or run missed windows on re-enable. The last reviewed immutable scheduler binary remains the code
rollback target. Any code revert is a normal reviewed PR.

**Restart point:** continue R3d1 from `/private/tmp/a2a-bridge-r3d1-supervisor` on branch
`agent/reliability-r3d1-supervisor`, based on merged R3d0 main
`c2d147fb1f0df275f3c6452cdd212e185c002d08`. The initial exact-base Fable review plus
exact-`a20db199`, exact-`d5041ee`, exact-`1c3a7ce`,
exact-`9414aa8`, exact-`6bc06fe`, exact-`a7db6e7`, exact-`c241087`, exact-`e0cc7dc`, exact-`c50811f`, and
exact-`fb8a2f4`, exact-`ae9db39`, exact-`2eb242a`, exact-`8dc6054`, exact-`cc01a52`, and exact-`b54840a`
Sol reviews are retained at the paths/hashes above.
The Fable six `WRONG`/thirteen `SMELL`; first-Sol four `WRONG`/seven `SMELL`; first-closure three
`WRONG`/three `SMELL`; second-closure two `WRONG`/three `SMELL`; third-closure two `WRONG`/zero new
`SMELL`; fourth-closure one `WRONG`/one `SMELL`; fifth-closure zero `WRONG`/one `SMELL`; sixth-closure
one `WRONG`/zero new `SMELL`; seventh-closure zero new `WRONG`/one `SMELL`; and eighth-closure one
`WRONG`/zero new `SMELL`; ninth-closure one `WRONG`/zero new `SMELL`; tenth-closure one `WRONG`/one
`SMELL`; eleventh-closure two residual `WRONG`/one `SMELL`; twelfth-closure two new `WRONG`/one new
`SMELL`; thirteenth-closure one inherited `PARTIAL`/zero new findings; and fourteenth-closure one inherited
`FIXED`/zero new findings are folded into D1-D10 and the slices/gates above. The fourteenth closure found no
regression, required no amendment, and returned `R3D DESIGN: APPROVE` at exact `b54840a`. All D1-D10 owner
decisions were approved on 2026-07-17. PR #37 merged the approved design at `6eeea6ce`. R3d0 delivered the
checked-in non-authoritative policy, complete profile inventory, proposed advisory configs, canonical identity,
inert record schemas/validators, and routing/foundation docs; PR #38 merged it at `c2d147fb`. Exact implementation
commit
`e7e5fa1` received a bridge-mediated Sol/xhigh/read-only review with eleven `WRONG`, two `SMELL`, and
`R3D0 IMPLEMENTATION: REVISE`; its retained report is
`/private/tmp/a2a-bridge-r3d0-sol-review-e7e5fa1/review.md`, SHA-256
`ad2c5207b654269b2599b360aa88067521ef83abc9e09843a88bee5e9de57de5`. Exact-`f4f242f` closure
review marked six inherited items `FIXED`, seven `PARTIAL`, found two new `WRONG` and two `SMELL`, and
returned `REVISE`; its retained report is `/private/tmp/a2a-bridge-r3d0-sol-closure-f4f242f/review.md`,
SHA-256 `110b9d2841c4f077a0b96fac19d7ece5cf07bad850714bbd787597fa330ba90c`. Exact
code/foundation-doc commit `e3321db5c052d7f8a9d549b23cea6aa9a7df3784` folds all seven required
remediation families and passes foundation units **6/0**, schema units **22/0**, R3d0 CLI integration
**21/0**, and the full serial workspace **2,214/0/12 ignored** across **55** reported test binaries;
workspace check, warnings-denied Clippy, locked release build, dependency policy, repository hygiene, and
manifest/recipe/schedule-foundation validators are green. Exact cursor `ee57f4a2f7509dd5a4bd281be1a36b7f117d834b`
then received the second R3d0 implementation closure review retained at
`/private/tmp/a2a-bridge-r3d0-sol-closure-ee57f4a/review.md`, SHA-256
`445191467e708fef46036dbe41548599ffbfedfa8f21a68a93e16879dd565f99`; it returned four
inherited `FIXED`, three `PARTIAL`, no new `WRONG`, two `SMELL`, and `R3D0 IMPLEMENTATION: REVISE`.
Exact remediation commit `ca4c453e6f589295b2434abfb1e1c708a2cb1dd2` closes its five requested
items plus independently reproduced trusted-cwd and decoded-key failures. Its focused gates are foundation
**8/0**, schema **23/0**, and R3d0 CLI **28/0**; the full serial workspace is **2,224/0/12 ignored** across
**55** reported test binaries; all complete deterministic release/validator gates are green. The current
profile-policy bundle SHA-256 is
`aed0e9b224d84624220a6091e51601a677b13d254091a12bc3b1879e36bf5e81`. No timer, private authority
issuance, live characterization, model discovery, credential access, container/runtime access, registry/image
effect, compatibility provider turn, GitHub check mutation, or production-operator lifecycle action has
occurred. Exact cursor
`be9d8a7a689b5f2c451f6059784903ce6d78f8b5` then received the third Sol/xhigh closure review
retained at `/private/tmp/a2a-bridge-r3d0-sol-closure-be9d8a7/review.md`, SHA-256
`c0510898b83f09372313785dd45d48c236fe144e93ca3938b4715f76ded8b041`; it returned six inherited
`FIXED`, trusted cwd `PARTIAL`, one new `WRONG`, no new `SMELL`, and `R3D0 IMPLEMENTATION: REVISE`.
Exact fourth remediation `5baeeb3f47183ea2a47d2cdc5ffce26f1df7dbfb` closes both accepted states.
Exact cursor `b6f5c9e7af2ffd0a1b022e3f07c2898a3d2c65c4` then received the fourth Sol/xhigh
closure review retained at `/private/tmp/a2a-bridge-r3d0-sol-closure-b6f5c9e/review.md`, SHA-256
`aa7b1051b83b94d84dc36273cf302419ffe2ecc41d20282001cffb530898374a`; it marked both inherited
families `FIXED`, found no new `WRONG`, found one nonblocking proof-isolation `SMELL`, and returned
`R3D0 IMPLEMENTATION: APPROVE`. Exact proof-only commit
`e771067f4a7e742ad813368f01018b011e86bbce` makes that branch mutation-isolated: removing only
the equality guard causes the aligned ordinary-name CLI fixture to be accepted after inventory re-pin.
Focused gates remain **9/0**, **23/0**, and **31/0**, and full serial workspace is **2,228/0/12 ignored**
across **55** binaries; all complete deterministic release/validator gates are green. Exact cursor
`c548dc0edcc1b21bfb14aa3e78736d633ce0fdc7` then received the first proof-fold confirmation
retained at `/private/tmp/a2a-bridge-r3d0-sol-confirm-c548dc0/review.md`, SHA-256
`5b45405e21118bf5b98cd0f1944e69e0bcb13815c5308864ca19abdad9d1a7f8`; it marked the proof
`SMELL` `FIXED` and mechanism unchanged, found one stale-handoff `WRONG` and one no-effect-wording `SMELL`,
and returned `R3D0 PROOF FOLD: REVISE`. Exact cursor
`e9d030f07d4c623ad2d00d0c918d02486d32fb7b` then marked the no-effect `SMELL` `FIXED`, the
handoff `WRONG` `PARTIAL` only on conditional publication wording, found no new item, and returned
`R3D0 DOCS REMEDIATION: REVISE`; its retained report is
`/private/tmp/a2a-bridge-r3d0-sol-confirm-e9d030f/review.md`, SHA-256
`aa24e4e8a307b12fe6c5cca57212b536cce0c26e58c7d66f25641a4d191a9daf`. This docs-only fold
closed that remainder. Exact cursor `1d2fb80a2804a53b6f4076f10f4d4aea61a48f21` then marked the
publication-tail item `FIXED`, found no new `WRONG` or `SMELL`, required no remediation, and returned
`R3D0 DOCS REMEDIATION: APPROVE`; its retained report is
`/private/tmp/a2a-bridge-r3d0-sol-confirm-1d2fb80/review.md`, SHA-256
`0bfe50a90056f2db8a14404ca02c526bc9e55be9d7f3772c098d9539f39f4fed`. Exact cursor
`d61176ca0c248fe884cffd320f34b073738729d0` then received the Opus/xhigh release/compatibility
lens retained at `/private/tmp/a2a-bridge-r3d0-opus-lens/review.md`, SHA-256
`f7a8e55f540ec9dd318b2f788c6d05f61f1641cff6b8f5851b271b64dafe0a64`; it found no `WRONG`, four
nonblocking `SMELL`, required no pre-PR remediation, and returned `R3D0 RELEASE/COMPATIBILITY: APPROVE`.
The deterministic owner-host validator and release-artifact check reconciled S4's stale prompt hashes to the
branch's documented values; S1-S3 remain accepted intentional constraints. R3d1 now implements only the
default-off exact-identity supervisor/signal mechanism and typed no-effects parent boundary. Initial exact candidate
`01438c34f2c17d3c4632583222b57748201e291b` received a bridge-mediated Sol/xhigh/read-only review with
eight `WRONG`, two `SMELL`, and `R3D1 IMPLEMENTATION: REVISE`; its retained report is
`/private/tmp/a2a-bridge-r3d1-sol-review-01438c3/review.md`, 6,290 bytes, SHA-256
`5515c25a33170a9ffa176a116e88ced44dac7754ddbdc10017b6683b94d3334b`. First closure review of exact
`e81ebbb388ab6ca38b6a0f4c20c4dd54f1690df3` marked nine inherited items `FIXED`, topology and stale-cursor
items `PARTIAL`, found no new `WRONG` or `SMELL`, and returned `R3D1 IMPLEMENTATION: REVISE`; its retained
report is `/private/tmp/a2a-bridge-r3d1-sol-closure-e81ebbb/review.md`, 10,258 bytes, SHA-256
`fa6b12a67e65df7438cb00ab953792e307b0e0b3748a5c9c37e170d96c088a24`. The second remediation rejects
topology-free holds and cross-session operational snapshots and durably inventories an already-acquired group
before a session, ancestry, liveness, or identity-observation hold. All three failures were observed red on
`e81ebbb`. Focused gates are **6/0**, **1/0**, **31/0**, **33/0**, **4/0**, **21/0**, and **2/0**; the complete
binary suite is **543/0/0**, and full serial workspace is **2,279/0/12 ignored** across **56** binaries. Format and
diff checks, workspace check, warnings-denied Clippy, locked release, dependency policy, hygiene **37/7**, manifest **9**,
recipes **4**, and foundation **6/4** are green. The candidate release binary is **26,574,640 bytes**, SHA-256
`7d74f85aeeb22d25e226e45457fccc4038b5e1de81a8c084c3d226ca0b9bd154`.
Second closure review of exact `8feda4d93c22ebe2c5e8867d46e006af50b8899f` marked all four requested
topology/cursor residuals `FIXED`, found one new `WRONG / High`, no new `SMELL`, and returned
`R3D1 IMPLEMENTATION: REVISE`; its retained report is
`/private/tmp/a2a-bridge-r3d1-sol-closure-8feda4d/review.md`, 5,777 bytes, SHA-256
`d9042da3d20c30955c52ffb86e78df06cd055acfdc9f873bac888cc7f1a67799`. The third remediation performs no
fallible observation after exact descendant-anchor acquisition and before retaining the capability and record.
Registration owns workload revalidation and can journal that exact group into `SafetyHold`. Its real two-workload
regression failed at the pre-retention error on `8feda4d` and now proves the stale workload holds while the other
workload remains live and durably inventoried. Third closure review of exact
`7fafe7933faca56842c64773011040be670cb2dc` marked that inherited item `FIXED`, confirmed the prior four residuals
remain closed, found two new `WRONG` (`High` and `Minor`), no new `SMELL`, and returned
`R3D1 IMPLEMENTATION: REVISE`; its retained report is
`/private/tmp/a2a-bridge-r3d1-sol-closure-7fafe79/review.md`, 6,176 bytes, SHA-256
`aabaae00bb2a4eca44db018a7a434b71f56ff6fed63cb90de94b2bd76bfa14b6`. The fourth remediation removes the
fallible liveness preflight only from TERM/KILL, uses the retained capability after durable journal publication, and
makes an actually missing/recycled capability fail closed into `SignalJournalAmbiguous` without a numeric signal.
It also corrects the focused status generation. The observation-error TERM/KILL and recycled-capability negative
tests both failed on `7fafe79` before the fix. Fourth closure review of exact
`b55c17d390861b5afa86a5f812b7727f38f630a0` marked both inherited findings `FIXED`, confirmed the earlier
mechanisms remain closed, found one new `WRONG / High`, no new `SMELL`, and returned
`R3D1 IMPLEMENTATION: REVISE`; its retained report is
`/private/tmp/a2a-bridge-r3d1-sol-closure-b55c17d/review.md`, 5,866 bytes, SHA-256
`3472273ff438cb58b1ceb8eeba69bc3ed6ee0dbd2fb5faaddaf471292489c634`. The fifth remediation requires retained
anchors through every signal-capable phase, permits release or ambiguity only in the corresponding no-later-signal
phase, and rejects a non-retained `start_running`. The schema, start, and transition tests all failed on `b55c17d`
before the fix. Exact fifth-remediation head `b511d6ce490590e54aae87dccad57e99fbe59a5a` received Sol/xhigh
`R3D1 IMPLEMENTATION: APPROVE` with no new finding; its retained report is
`/private/tmp/a2a-bridge-r3d1-sol-closure-b511d6c/review.md`, 5,023 bytes, SHA-256
`1bf7bf1873c224b0da0067e53a440295c02be7ec677f82553525e5d808840b6d`. The single Fable/xhigh adversarial
implementation and release/compatibility lens then found no `WRONG`, three nonblocking Minor `SMELL`s, and returned
`R3D1 RELEASE/COMPATIBILITY: APPROVE`; its retained report is
`/private/tmp/a2a-bridge-r3d1-fable-lens-review/review.md`, 7,837 bytes, SHA-256
`088676af7e11beb4d33f1c4410dcf5bfc4a0e55dc1eaa689288934a04de01bed`. No post-approval mechanism change
occurred; this fold only records review evidence and carries the three integration-hardening smells into R3d2.
Run exact-final deterministic gates and publish a non-draft R3d1 PR. Preserve R3c/R4 inputs, keep R2f operator
lifecycle work out of R3d, and never touch the long-lived operator lifecycle from this slice.

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

R3 is complete when R3a–R3f are merged, at least one authorized pinned and floating run artifact exists for
every claimed bridge provider path, every direct or historic control is explicitly dispositioned, and the
runner/credential/cost owner plus scheduling status is documented. R3 produces promotion-ready evidence but
does not exercise or write a baseline promotion; that deliberate policy/action remains R4. OpenRouter/
OpenCode live turns remain separately authorized, and deterministic green gates alone do not manufacture a
support claim. Then update the central roadmap's next action to R4.
