# Handoff — provider-refresh deterministic hybrid isolation

**Written:** 2026-09-05T12:09:23Z · **By:** Codex `/root` · **Provider:** codex
**Workspace:** `/private/tmp/a2a-provider-refresh-hybrid-isolation-20260905` · `fix/provider-refresh-hybrid-isolation-20260905` · **Measured state:** `[MEASURED]` HEAD `936534d8cffb225249a5eeccd5874552dc97e961` · Tree DIRTY · Probe `git status --short` · Output the four source/docs paths listed in §6
**Predecessor:** prior provider-refresh operator session, reconstructed from the current conversation and `/private/tmp/a2a-provider-refresh-secret-scan-20260904/docs/superpowers/reviews/2026-09-04-provider-refresh-secret-scan-handoff.md`
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` this branch remains isolated in the named worktree; the byte-identical trusted-root verification worktree is detached at the same base — **RESOLVED 2026-09-05T12:09:23Z**
**(b) Custody exposure** — `[MEASURED]` four uncommitted files exist only in this isolated worktree; no commit or push exists yet — **OPEN until committed**
**(c) In flight / irreversible** — `[MEASURED]` every verification command started by this worker has exited. No registry, image, provider, production-promotion, or service-lifecycle effect was started by this lane. The separate served operator still had three owned Sol/high submissions when its Astra/medium on-disk default was staged, so restart was correctly withheld — **RESOLVED for this lane; OPEN for operator reload**
**(d) Authorization granted but not exercised** — owner authorized the bounded provider-free hybrid-isolation implementation and local verification, then separately directed Astra/medium defaults for the standalone CLI and served operator. The two validated config files were promoted with backups, but the bridge was not restarted. No authorization from this lane grants registry/image resolution, a provider prompt, push, PR, or merge.

## 1. Resume order

1. Commit the four paths in §6 for durable local custody; no push authority is implied.
2. Preserve the trusted-root full-suite log at `/private/tmp/a2a-astra-medium.VqBjiG/provider-refresh-full-test-trusted.log` until the commit is reviewed. Its 85 Cargo target summaries total 4,386 passed, 0 failed, and 13 ignored; one nested subprocess summary (1 passed / 714 filtered) is excluded from those totals.
3. Ask separately before push, PR, registry/image resolution, provider verification, production promotion, or operator restart.
4. Before restarting the served operator for its Astra/medium default, repeat queue, turn, workflow, task, socket, and controller-ownership checks; do not stop PID 93630 while any of the three observed submissions remains connected.

**STOP conditions:** any provider/session prompt, registry or image effect, shared tag/config mutation, production promotion, service lifecycle action, open-class review finding, or need to change comparison semantics is outside this authority.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| Frozen custody | done | `[MEASURED]` branch/worktree and base SHA in §6; active checkout was not switched or cleaned |
| RED contract | done | `[MEASURED]` exact recipe test failed 0/1 because TOML rejected unknown `agent_cli_selector`; the earlier compile-error attempt was inadmissible and was corrected before belief update |
| Exact selector/base implementation | done | `[MEASURED]` resolver edge suite passed 70/70 before the final independent-axis test refinement; the two refined tests each subsequently passed 1/1 |
| Execution binding | done | `[MEASURED]` exact bound-execution test passed 1/1 and rejects CLI-selector recipe drift |
| Workspace check | done | `[MEASURED]` `cargo check --workspace --all-targets` completed green |
| Warnings-denied Clippy | done | `[MEASURED]` `cargo clippy --workspace --all-targets -- -D warnings` completed green |
| Full aggregate | done | `[MEASURED]` byte-identical trusted-root candidate: 85 Cargo target summaries, 4,386 passed / 0 failed / 13 ignored; the first sandbox run was inadmissible for regression attribution, and the `/private/tmp` host run's sole candidate-only fingerprint failure was fixed without resealing the legacy inventory |
| Release/hygiene gates | done | `[MEASURED]` format, workspace check, warnings-denied Clippy, release build, `git diff --check`, and repository hygiene all passed |
| Review | done | Round 1: 0 WRONG, 0 SMELL; round 2 at the declared cap: 0 WRONG, 0 SMELL |
| Provider/image resolution and live verification | parked | Separate effect and billable authorities are required |
| Production promotion | parked | Separate promotion authority is required after separately authorized verification |
| Astra/medium local defaults | partial | `[MEASURED]` CLI and served-operator files were validated, backed up, and promoted on disk; operator restart is withheld because three owned submissions remained connected |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| `docs/provider-refresh-runbook.md` | Manual nested-CLI pinning was described, but the deterministic resolver inputs and one-axis procedure were not | `[MEASURED]` updated with exact adapter, exact nested CLI override, exact Node 24 slim base, new-output/unique-tag rules, and separate verification/promotion authority |
| checked-in memory | None identified as false | No memory write was requested or performed |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | Durable commit | pending | Stage the four paths in §6 and create one local commit | None | no push authority |
| 2 | Diagnostic resolution | parked | Prepare private one-axis recipes, then request exact registry/image effect authorization | Owner authorization | no effect started |
| 3 | Served Astra/medium reload | parked | Reprove an empty ownership ledger, then perform a controlled restart and read-plane verification | Three observed owned submissions must drain | config SHA-256 `27a46727481f6080494f55d70dbdb2a61b4cee28578f96c79a494b772af840f4` |

## 5. Invariants and traps — do not do these

- Never treat `latest`, resolution success, or exact lock output as compatibility evidence — only separately authorized verification can establish behavior.
- Never combine adapter, nested CLI/SDK, and Node-base changes in an isolation case — change one axis from the same-environment passing control.
- Never reuse an output directory or owned image tag — resolution publication and image ownership are create-only.
- Never relax duplicate baseline mappings to pack all hybrids into one comparison — compare separate resolutions independently unless a separately reviewed comparison design changes.
- Never update `compatibility/floating-current.toml`, production config, shared tags, or the running operator in this slice.
- A malformed `cargo test` invocation and a zero-selected exact filter occurred; both were explicitly classified inadmissible and rerun with full module-qualified names.
- The first full suite was sandbox-denied across 19 targets. The unsandboxed `/private/tmp` run isolated 23 base-reproduced trust-root failures plus one candidate-only characterization mismatch. The fix omits only the legacy-default CLI selector from recipe serialization; exact selectors remain fingerprinted. A byte-identical worktree under the approved trusted root then passed the full suite.
- `cargo clean` removed only 4.8 GiB of generated output from the test-only trusted-root worktree after it filled the volume; source and retained logs were not deleted.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Base / current HEAD | `936534d8cffb225249a5eeccd5874552dc97e961` |
| Branch | `fix/provider-refresh-hybrid-isolation-20260905` |
| Worktree | `/private/tmp/a2a-provider-refresh-hybrid-isolation-20260905` |
| Modified source | `bin/a2a-bridge/src/compatibility_resolution.rs` |
| Modified binding | `bin/a2a-bridge/src/compatibility.rs` |
| Modified runbook | `docs/provider-refresh-runbook.md` |
| This handoff | `docs/superpowers/reviews/2026-09-05-provider-refresh-hybrid-isolation-handoff.md` |
| Floating request default | `adapter_selector = "latest"`; omitted CLI selector means `adapter-declared`; base `docker.io/library/node:24-slim` |
| Exact diagnostic forms | complete adapter semantic version; complete nested CLI semantic version; `docker.io/library/node:24.x.y-slim` |

## 7. Refutation verdict and owner questions

**§2c verdict:** PASS · claim: "The resolver can independently pin adapter, nested CLI/SDK, and Node 24 slim base while preserving legacy floating behavior and separate verification/promotion authority" · pass: SELF-PASS (NOT INDEPENDENT) · evidence tier: TEST-BACKED · record: this handoff

**Questions the owner owes an answer to:** None before durable local commit. Push/PR, registry/image resolution, provider verification, and production promotion each require a later explicit decision. The Astra/medium operator restart may proceed only after the observed submissions drain and custody is remeasured.
