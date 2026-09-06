# Handoff - R2f1b slice 5A fresh V3 admission authority

**Written:** 2026-09-06
**Workspace:** `/Users/wesleyjinks/code/.a2a-implement/impl-12447-99q2jzxs`
**Exact base:** `34ced0f93526f981a835c237c56d7ba580e989f4` (PR #100), tree `192ba759a98eccbc9c5f21c187beddf8996c4463`
**Authority:** retained-artifact repair continuation from candidate `7d5cd283`; 120 added nonblank formatted Rust lines versus that candidate; 656 cumulative added nonblank formatted Rust lines versus PR #100 base; three Rust paths in this continuation; no production wiring; no live/provider/operator effects; no GitHub publication.

## 0. Gating Facts

R2f1b 4J is merged as PR #100 at `34ced0f93526f981a835c237c56d7ba580e989f4`. Slice 5A is a retained-artifact repair continuation in progress and intentionally production-unwired. The continuation authority is limited to `crates/bridge-workflow/src/admission.rs`, `crates/bridge-workflow/src/executor.rs`, the already-staged `crates/bridge-workflow/tests/r2f1a_bound_executor.rs`, this handoff, the slice-5 decomposition, the slice-5A task doc, and the roadmap; the prior admission-test artifact remains unchanged.

## 1. Exact Identities

| Item | Value |
|---|---|
| Base commit | `34ced0f93526f981a835c237c56d7ba580e989f4` |
| Base tree | `192ba759a98eccbc9c5f21c187beddf8996c4463` |
| Retained repair predecessor | `7d5cd2839a034410b0b289e7972065d1d5850c17` |
| Required commit subject | `feat: add R2f1b fresh V3 admission authority` |
| Rust authority | Three continuation paths: admission, executor binder, and already-staged bound-executor tests. |
| Rust count | `656` cumulative added nonblank formatted Rust lines versus `34ced0f93526f981a835c237c56d7ba580e989f4`; continuation delta `120` lines versus `7d5cd283`. |
| Production caller census | Real construction sites remain explicit `r2f1b: None`; `freeze_fresh_v3` remains absent from production callers. |

## 2. Bound Implementation

`WorkflowAdmissionV1::freeze_fresh_v3` validates a root attempt identity and absent caller contract before freezing. It requires the resolved deadline activation to be `AutomaticR2f1b`, delegates to the existing single-pass admission freezer, derives an automatic `FrozenR2f1bContractV1`, and returns both `AdmittedWorkflowRunV1` and `WorkflowSnapshotV3` for the same delivery spec.

The custody builder walks the frozen provider-attempt matrix, ignores direct checkouts, mints `WorktreeCustodyIdV1::mint()` once for each unique worktree checkout digest, binds the exact digest and target cwd, and refuses a repeated digest with a different target. The snapshot validates canonically before return. Existing `freeze` and `admit_r2f1b_contract_v1` behavior remains unchanged, including explicit/manual contract behavior.

The final repair is in the fresh proof and canonical executor binder. The proof now retains the exact admitted `Arc<WorkflowRunSpecV1>` and exact admitted `Arc<R2f1bAdmissionV1>` minted from the same successful `freeze_fresh_v3` result. The binder matches `r2f1b` and `fresh_r2f1b_admission` together: `(None, None)` remains V2, `(Some(contract), None)` remains the explicit/manual path, `(Some(contract), Some(proof))` takes the fresh path only when both private retained Arcs match by exact `Arc::ptr_eq`, `(None, Some(proof))` returns `ConfigInvalid` for a fresh proof without its admitted contract, and a mixed run spec returns stable reason `fresh R2f1b admission proof does not match the admitted run specification` before custody binding.

## 3. RED Evidence

Prior behavioral RED evidence from exact PR #100 base showed the supplied-contract automatic admission path refused under armed production readiness, preserving the explicit-contract boundary while the owned builder supplies the new admitted path.

The prior missing-contract regression remains retained provenance. The final hard-read-only review of candidate `7d5cd283` was host Codex `gpt-5.6-sol`/`xhigh` hard-read-only execution `exec-52e6aa6c7b9d1ca997c3dc204f2b52eb`, attempt `attempt-77929e6e9065ae97803dec6906bfc058`, result `/private/tmp/r2f1b-slice5a-final-rereview-result-20260906.md`, SHA-256 `725bcccb87643371377f106c1310918d4b1aa73d9e049e2d8273234239d344a6`. It found one `WRONG` blocker: a proof from one genuine same-attempt fresh V3 admission could be paired with another genuine admission's public run spec because the proof retained only the contract Arc. It also returned one deferred evidence-provenance `SMELL`; this repair closes only the blocker.

The controller captured an admissible same-host RED on unchanged production candidate `7d5cd283`: the exact selector compiled, selected one test, constructed two genuine same-attempt fresh V3 admissions with distinct direct workflow specs, and failed 0 passed / 1 failed / 22 filtered because the binder accepted the mixed admission. The retained log SHA-256 is `b183db1d5e90912f2c9a5970c43054c6321eeb14da2729a5f6316e57e955e618`.

## 4. GREEN Evidence

Focused predecessor coverage includes worktree and direct-only graphs, one checkout-planner pass, same-digest deduplication, conflicting target refusal, canonical V3 round trip, admitted/snapshot equality, byte-equal delivery specs, automatic workload identity binding, successor and mismatched identity refusal, caller-supplied contract refusal, and disarmed/manual-test refusal before effects.

Final binder coverage now includes the fresh positive binder, equal-but-separately allocated automatic-contract substitution refusal by exact identity proof, missing-contract refusal, run-spec-from-another-genuine-admission refusal, manual binder preservation, and V2 negative controls. Controller focused GREEN after repair is recorded for the exact mixed-admission selector 1 passed / 0 failed / 22 filtered, complete bound-executor binary 23 passed / 0 failed, and fresh-V3 admission selectors 2 passed / 0 failed / 12 filtered. The corresponding log SHA-256 values are `dbfbe06f065b14dff4598bd6c6513de62131a810172199c0d4e8dadfd12f5174`, `a4fdebf973d8f07cfeb7137b3c7b9b791d50a4be2336dcba9aa8c27df1a99841`, and `02041b0c968bf369f81c61d28c3ddb3a0430438e46fa2d56ef811fe711964525`.

The final post-doc controller gate passed with diff-check, Cargo fmt, workspace all-target check, workspace all-target/all-feature Clippy with `-D warnings`, workspace all-target tests, doctests, workspace all-target/all-feature build, release bridge build, and repository hygiene all exiting zero. All-target totals were 86 binaries / 4,397 passed / 0 failed / 13 ignored with log SHA-256 `494ceb97163ac52e2668b2fac24a5f2da8925c6cef226dceab29a79dd8d0fbc4`; the 13 ignored tests are explicit live-provider/auth lanes. Doctests were 16 crates / 2 passed / 0 failed / 0 ignored with log SHA-256 `5fb7ab0a7c0343c698613e270ef23fbe1514fe9617369c39620a13775458329f`. Hygiene reported 41 tracked artifacts / 9 configs with log SHA-256 `6e14f1b773562dcb92caa6c245532dce4fb238fe72f06c7745d9121dae232125`.

## 5. Remaining Exclusions

No production caller is wired to the fresh V3 builder. No V3 task/history reservation, detached terminal CAS, served coordinator/A2A/MCP path, batch/offline/implement path, resume claim exchange, boot recovery, backend behavior, worktree custody behavior, compatibility row, live smoke, production/operator action, push, PR, merge, release, deployment, or running-operator mutation is included in 5A.

## 6. Verification State

The prior 497-line candidate and the previous missing-contract repair evidence are superseded provenance. The repaired exact-identity mechanism is present locally, and the final post-doc controller gate passed. One final Sol/xhigh hard-read-only rereview of the repaired candidate remains pending. No approval, publication, merge, deployment, operator restart, live smoke, or production wiring is claimed.

## 7. Next Action

Run the one final Sol/xhigh hard-read-only rereview for this repaired 5A candidate. If it passes, the next implementation slice is 5B: V3 task/history reservation plus detached terminal CAS before any real served or offline surface wiring.
