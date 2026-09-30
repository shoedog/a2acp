# Handoff — ADR-0041 Slice 2B3b spec and Sonnet 5.5 orchestration

**Written:** 2026-09-30T00:25:00Z · **By:** Codex controller · **Provider:** codex
**Workspace:** `/Users/wesleyjinks/code/a2a-bridge` · `work/adr0041-2b3b-20260929` ·
**Measured state:** `[MEASURED]` HEAD `5d2a82c457e626b2a108063e1de6b13b94e97f92` · Tree DIRTY (docs only) ·
Probe `git status --short --branch` · Output: branch above; draft/spec/roadmap/handoff edits.
**Predecessor:** Claude handoff supplied by owner: PR #126 open; next 2B3b spec, Sol review, Opus implementation.
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the controller. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` this controller owns 2B3b. `pgrep -af adr0041-2b3b-20260929` found no
in-flight lane process after daemon recovery. The shared operator is separate, PID 87677 — RESOLVED.
**(b) Custody exposure** — `[MEASURED]` draft docs pending local snapshot commit. Scratch evidence is under
`/private/tmp/adr0041-2b3b-20260929`; do not delete it — OPEN until snapshot.
**(c) In flight / irreversible** — `[MEASURED]` implementation-container models probe session 74287; no
implementation dispatched and no spec review dispatched at this checkpoint — OPEN (probe only).
**(d) Authorization granted but not exercised** — owner: "Merge 2B3a and proceed to orchestrate spec and
implementation of 2B3b." Later: "use sonnet5-5 for implementation. You may need ti update the acp for the
bridge. update for claude, codex if so". Owner explicitly selected isolated bootstrap Git directory.

## 1. Resume order

1. Inspect `/private/tmp/adr0041-2b3b-20260929/container-models.json` and container doctor before dispatch.
2. Snapshot the draft, then run the validated host `spec-review` workflow with the round-1 brief; cap two.
3. Fold bounded findings on this artifact, rereview if rejected, and commit the approved task before implement.
4. Invoke `a2a-bridge implement` with the approved spec commit as exact base and private `implement.toml`.
5. Complete controller macOS verification, Sol implementation review and CI before approved per-child merge.

**STOP conditions:** possibly accepted provider failure (no automatic replay); scope/contract disagreement;
unproven Sonnet 5.5 identity; open-class findings at cap; failing gate without same-environment base control.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| PR #126 | done | `[MEASURED]` merged 2026-09-29T21:52:15Z at `5d2a82c4`; `gh pr view 126` confirmed |
| 2B3a code | done | `[MEASURED]` current checkout contains reader; PR #125 merge `b7c85aee` |
| Bootstrap refinement | done | `[MEASURED]` owner selected isolated bootstrap; design updated this turn |
| 2B3b draft | next | `[MEASURED]` task revision 1 on disk; independent Sol review pending |
| Private ACP updates | done | `[MEASURED]` Claude ACP 0.84.0 / SDK 0.3.284 / bundled CLI 2.1.284; Codex ACP 2.0.1 / Codex 0.159.1 |
| Host Sonnet smoke | done | `[MEASURED]` exact PONG, success true; transcript `message.model=claude-sonnet-5-5`; artifact `claude55-host-smoke.json` |
| Host Sol smoke | done | `[MEASURED]` completed session 13668; inspect `sol-host-smoke.json` before relying on its outcome |
| Container catalog/smoke | pending | `[MEASURED]` doctor completed; models in flight; smoke not yet sent |
| Implementation | pending | `[MEASURED]` not dispatched |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| Parent design | InitBare in final .git, then create-new HEAD/config | `[MEASURED]` updated to owner-approved isolated bootstrap and object copying |
| Parent process | Opus 5.5 for this child | `[MEASURED]` updated to owner's Sonnet 5.5 selection for 2B3b |
| Roadmap | PR #126 open; 2B3b not drafted | `[MEASURED]` merged docs cursor and draft/review status updated |
| Memory | Older 2B3a boundary evidence | `[INHERITED]` used only to locate and rebind current retained-pin contract; no memory write requested |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | Spec review | next | private host config, spec-review workflow, round-1 typed brief | snapshot/brief | task revision 1 |
| 2 | Container Sonnet identity | pending | inspect doctor/catalog, one bounded smoke then transcript model | models probe | image `fe3a644f…` |
| 3 | Implement/verify/review | pending | approved task commit and private implement config | Sol spec approval | two-attempt cap |

## 5. Invariants and traps — do not do these

- Never reopen staged names; retained descriptors are the reader's consumer contract.
- Never overwrite Git-generated HEAD/config; bootstrap stays under `.restore-work/` and only objects are copied.
- Never reset or replace the single restore ledger; charge both object-store copies.
- Never activate archived behavior, write the final record in G, or advance into 2B3c/2B3d.
- Never mutate shared operator packages/tags during this lane; PID 87677 can spawn sessions from them.
- Claude catalog aliases alone do not prove Sonnet 5.5. Isolated settings bind `ANTHROPIC_DEFAULT_SONNET_MODEL`;
  provider transcript proved the host smoke's concrete model.
- Initial private config validation used unsupported timeout fields and omitted server; those probes were
  inadmissible schema errors and were corrected before any successful model/turn evidence.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Base | `5d2a82c457e626b2a108063e1de6b13b94e97f92` |
| Task | `docs/superpowers/plans/2026-09-29-adr0041-slice2b3b-git-plane-task.md` |
| Scratch | `/private/tmp/adr0041-2b3b-20260929` |
| Review config | `/private/tmp/adr0041-2b3b-20260929/host.toml` |
| Implementation config | `/private/tmp/adr0041-2b3b-20260929/implement.toml` |
| Candidate image | `sha256:fe3a644f07b68e5ad8d513b64dfa5eb4e454e1e67f49964f23388b4c370a6ef5` |
| Original toolchain | `sha256:3390cbe8a6388615645ca9fb462173c16da7423f3ef3013893a8c44b8eecb875` (shared tag unchanged) |
| Model settings | `/private/tmp/adr0041-2b3b-20260929/claude-profile/settings.json` |
| Host transcript | `/private/tmp/adr0041-2b3b-20260929/claude-profile/projects/-Users-wesleyjinks-code-a2a-bridge/9d6927c3-4c25-4c10-9d32-a37d939cc3b4.jsonl` |
| Bridge | `/Users/wesleyjinks/code/a2a-bridge/target/release/a2a-bridge` built from base this turn |

## 7. Refutation verdict and owner questions

**§2c verdict:** NOT RUN — independent spec review pending · claim: "2B3b draft is implementable within its
owned seams" · pass: INDEPENDENT · evidence tier: STATIC-ONLY · record: planned Sol round 1.

**Questions the owner owes an answer to:** None; bootstrap choice was answered.
