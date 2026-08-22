# T3b plan — two independent authors, four operator claims refuted, one safety disagreement

Both authors worked the same brief against `cafeae13`. Both produced five slices with
the destructive one last, both derived caps at the lane's 1.48x worst-observed ratio,
and both refused to compress to the brief's ~2,000-line estimate. **Opus is the base.**

## Four operator claims refuted — all verified against the tree

Mine, in the authoring brief. Each was checked on `origin/main` before acceptance.

| Claim in the brief | Verified result |
|---|---|
| The frozen transition table is in `custody_writer.rs` | **FALSE.** `LEGAL_CUSTODY_TRANSITIONS_V1` is in `custody.rs`; `custody_writer.rs` only enforces it. A spec would have sent an implementer to the wrong file. |
| Only scope item (d) requires no table edge | **FALSE.** `LEGAL_CUSTODY_TRANSITIONS_V1[0]` **is** `(ProtectionPrepared, UnusedSettled)` — frozen since slice 2a. Neither (c) nor (d) may add an edge. |
| B18's async/trait recovery seam is T3b work | **STALE.** T3a's 3A-1 port made `ExactAbsenceProbeV1` sync (`Send + Sync`, plain `fn`). What remains is the *executor* seam — five async boot callers invoking a sync sweep. |
| T3b settles the populations T3a admits | **FALSE BY OMISSION.** See below. |

## The safety disagreement, and how it resolves

T3a's `admit_custody_population` admits **two** populations to the probe:
`ProtectionPrepared` + claim, and `PreservationUnknown { MaterializationInFlight }` +
claim. The two authors disagreed on what T3b may do with the second.

- **Opus:** it is not settleable *and not marker-retirable*. `PreservationUnknown`
  appears in the frozen table **only as a destination** — it has no outgoing edge — and
  its `claim_presence()` is `Required`. T3b must refuse it by construction, with a named
  test. It reads scope item (d)'s "both populations" as **legacy sidecar + V3 custody
  record**.
- **Sol:** it "receives the same state-agnostic marker-removal authority but no
  transition-table edge" — i.e. retire its marker without transitioning.

**Opus is correct, on the code's own words.** `custody.rs` states the rule directly:

> The three preserving states are `Required`: **a preservation with no artifact leaves
> R2f2 nothing to dispose of.**

`claim: Option<PreservedWorktreeClaimV1>` is a field **inside** `WorktreeCustodyRecordV1`.
The marker *is* the record file. Retiring it unlinks the file and destroys the claim —
which is precisely R2f2's disposal artifact. `PreservationUnknown` is one of the three
preserving states.

`[MEASURED]` `PreservationUnknown` appears twice in the table, both times as the second
element: `(Materializing, PreservationUnknown)` and
`(PreservationPrepared, PreservationUnknown)`. Zero outgoing edges.

**Ruling: T3b settles exactly one population — `ProtectionPrepared` with a claim — and
must refuse `PreservationUnknown { MaterializationInFlight }` by construction, with a
named test.** Sol's reading would have authorized destroying a preservation artifact on
the slice that deletes things.

Opus's reading of "both populations" is also the one consistent with the code:
`remove_worktree_if_safe` today handles the **legacy sidecar** and refuses whenever a V3
custody record coexists, so "both" naturally means the two marker kinds.

## What else Opus supplied that sol did not

- **B20's same-object proof at unlink time.** `unlinkat` has no "only if inode X" flag,
  so the proof comes from making the name private: capture by `renameat2(RENAME_NOREPLACE)`
  into a reserved namespace, re-`fstat` the capture and require identity equality, then
  unlink *the capture name*. The public record name is never the unlink target.
- **A named residual with a bound.** An `UnusedSettled` record can never be re-proved
  from a cold start: the schema carries no `source`, and `UnusedSettled` **forbids** the
  claim that would carry one. So a crash between transition and retirement leaves a
  marker no later sweep can authorize removing — correct and fail-closed, but a real
  bounded leak that needs an operator-visible category.
- **A `btime` portability risk** in `required_object_identity_v2`, which returns
  `Unsupported` without a birthtime. This lane has already lost a round to a
  three-filesystem split.

## Sizing — both authors, same method, same conclusion

| Author | Projected | Caps |
|---|---:|---:|
| Opus | 2,625 | 3,880 |
| Sol | 2,220 | 3,310 |

Both exceed the brief's ~2,000 and both say so as a finding rather than compressing.
Opus's is the more conservative and prices production at 1,070 of 2,625 — the excess is
the mandated battery at this lane's measured 35-60 lines per test.

## Slice 1 is non-destructive by construction

The refusing settlement window: two-phase open (publication cell → pin → read → custody
cell → re-read → byte-identity), refusing acquirers only, guards dropped in reverse. It
adds no transition, rename, unlink, or provider edge. Its frozen artifact is a
**single-mutation control against its own head** rather than a behavioural red against the
base — Opus's reasoning being that on `cafeae13` no symbol it touches exists, so any
base-relative control is a compile error, and this lane has already root-caused
compile-error "reds" as non-evidence.
