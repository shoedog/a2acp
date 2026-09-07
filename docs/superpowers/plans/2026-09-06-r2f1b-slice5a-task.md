---
task-type: implement
---

# R2f1b slice 5A - fresh automatic V3 admission authority

## Authority and exact base

Implement from exact merged `origin/main` commit `34ced0f93526f981a835c237c56d7ba580e989f4` (PR #100, R2f1b 4J), tree `192ba759a98eccbc9c5f21c187beddf8996c4463`.

This sub-slice continues the retained artifact at exact candidate `7d5cd2839a034410b0b289e7972065d1d5850c17`. The repaired continuation adds 120 nonblank formatted Rust lines versus that candidate, and the cumulative 5A Rust artifact measures 656 added nonblank formatted Rust lines versus `34ced0f93526f981a835c237c56d7ba580e989f4`. It is production-unwired: every real construction site remains explicit `r2f1b: None`, `freeze_fresh_v3` remains absent from production callers, and no coordinator, A2A, MCP, batch, offline/CLI, implement, boot/resume, task/history store, backend, worktree custody, or operator path is modified. The only newly authorized seam outside the original admission builder is the canonical executor binder that consumes the admitted proof/run-spec/contract identity.

No live smoke, compatibility case, provider session beyond authorized bridge implement/review turns, registry/image mutation, release, deployment, served restart, running-operator change, or GitHub publication is authorized.

## Required behavior

`WorkflowAdmissionV1` exposes one public fresh-V3 admission entrypoint. It accepts an initial `AttemptIdentity` plus normal frozen admission inputs with no caller-supplied R2f1b contract, performs exactly one checkout-planning pass, and returns both `AdmittedWorkflowRunV1` carrying the owned `R2f1bAdmissionV1` and a canonical root `WorkflowSnapshotV3` with the same byte-identical delivery spec, the same contract, and no predecessor digest.

The entrypoint owns custody authority. For each unique frozen worktree checkout digest it mints one nonzero CSPRNG `WorktreeCustodyIdV1`, binds that digest and target cwd, deduplicates identical checkout identities, and refuses a repeated digest with a different target. Direct checkouts produce no custody plan.

Fresh identity is mandatory. Successor attempts, parent attempts, mismatched request attempts, and caller-supplied contracts refuse before checkout planning. Automatic contracts are admitted only when readiness and policy activation resolve to `AutomaticR2f1b`; disarmed and manual-test controls refuse before checkout planning. Existing explicit/manual contract admission remains unchanged. The canonical binder must match `r2f1b` and `fresh_r2f1b_admission` together: `(None, None)` routes V2, `(Some(contract), None)` preserves the manual path, `(Some(contract), Some(proof))` takes the fresh path only when private retained `Arc<WorkflowRunSpecV1>` and `Arc<R2f1bAdmissionV1>` values both match by exact `Arc::ptr_eq`, `(None, Some(proof))` returns a stable `ConfigInvalid` refusal identifying a fresh proof without its admitted contract, and a mixed run spec returns stable reason `fresh R2f1b admission proof does not match the admitted run specification`.

## Tests and evidence

Retained behavioral RED evidence from the prior candidate includes the supplied-contract automatic admission test on PR #100 base: armed production readiness still refused a caller-supplied automatic contract through real admission. The previous missing-contract binder RED remains retained provenance.

The final hard-read-only review of candidate `7d5cd283` was host Codex `gpt-5.6-sol`/`xhigh` execution `exec-52e6aa6c7b9d1ca997c3dc204f2b52eb`, attempt `attempt-77929e6e9065ae97803dec6906bfc058`, result `/private/tmp/r2f1b-slice5a-final-rereview-result-20260906.md`, SHA-256 `725bcccb87643371377f106c1310918d4b1aa73d9e049e2d8273234239d344a6`. It found one `WRONG` blocker: the fresh proof authenticated the exact retained contract Arc but not the exact admitted run-spec Arc. It also returned one deferred evidence-provenance `SMELL`; this repair closes only the blocker. The controller then captured an admissible same-host RED on unchanged production candidate `7d5cd283`: the exact selector compiled, selected one test, constructed two genuine same-attempt fresh V3 admissions with distinct direct workflow specs, and failed 0 passed / 1 failed / 22 filtered because the binder accepted the mixed admission; retained log SHA-256 `b183db1d5e90912f2c9a5970c43054c6321eeb14da2729a5f6316e57e955e618`.

The repaired mechanism retains the exact admitted `Arc<WorkflowRunSpecV1>` and exact admitted `Arc<R2f1bAdmissionV1>` in `FreshR2f1bAdmissionProofV1`, both minted from the same successful `freeze_fresh_v3` result. The canonical binder checks the run-spec pointer first, returns stable typed `ConfigInvalid` reason `fresh R2f1b admission proof does not match the admitted run specification` on mismatch, then preserves the existing exact contract pointer check and reason before admitting the fresh automatic contract. Controller focused GREEN after repair is recorded for the exact mixed-admission selector 1 passed / 0 failed / 22 filtered, complete bound-executor binary 23 passed / 0 failed, and fresh-V3 admission selectors 2 passed / 0 failed / 12 filtered; corresponding log SHA-256 values are `dbfbe06f065b14dff4598bd6c6513de62131a810172199c0d4e8dadfd12f5174`, `a4fdebf973d8f07cfeb7137b3c7b9b791d50a4be2336dcba9aa8c27df1a99841`, and `02041b0c968bf369f81c61d28c3ddb3a0430438e46fa2d56ef811fe711964525`. The final post-doc controller gate passed with diff-check, Cargo fmt, workspace all-target check, workspace all-target/all-feature Clippy with `-D warnings`, workspace all-target tests, doctests, workspace all-target/all-feature build, release bridge build, and repository hygiene all exiting zero. All-target totals were 86 binaries / 4,397 passed / 0 failed / 13 ignored with log SHA-256 `494ceb97163ac52e2668b2fac24a5f2da8925c6cef226dceab29a79dd8d0fbc4`; the 13 ignored tests are explicit live-provider/auth lanes. Doctests were 16 crates / 2 passed / 0 failed / 0 ignored with log SHA-256 `5fb7ab0a7c0343c698613e270ef23fbe1514fe9617369c39620a13775458329f`. Hygiene reported 41 tracked artifacts / 9 configs with log SHA-256 `6e14f1b773562dcb92caa6c245532dce4fb238fe72f06c7745d9121dae232125`. One final Sol/xhigh hard-read-only rereview of the repaired candidate remains pending; no approval, publication, merge, deployment, operator restart, live smoke, or production wiring is claimed.

## Acceptance criteria

- Fresh root V3 admission is built in one checkout-planning pass and returns matching admitted authority plus canonical snapshot.
- Custody plans are internally minted, deduped by checkout digest, and rejected on target conflict; direct-only graphs have no custody plans.
- Readiness, manual-test, watchdog, successor, mismatched attempt, and supplied-contract refusals happen before prohibited effects.
- Existing V1/V2 encoding and explicit/manual contract behavior remain unchanged.
- Rust additions are 656 nonblank formatted lines versus the PR #100 base, with a 120-line continuation versus `7d5cd283`.
- Durable decomposition, task, handoff, and roadmap records match the implementation state.
