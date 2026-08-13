I'll start by reading the authoritative rulings and the preserved artifact.I have the full picture. Writing the design.Read-only pass complete. I inspected the source directly (`retained_resource_flight.rs`, `process.rs`, `backend.rs`, `liveness.rs`, `fs_custody.rs`, `ports.rs`, and the coordinator/container consumers), not just the review maps. Full design written to `/Users/wesleyjinks/.claude/plans/act-as-an-independent-toasty-dolphin.md`.

One custody note first: the adjudication, the Sol/xhigh lens, and the dual-adjudication are **not in this worktree** — they exist only at `/Users/wesleyjinks/code/a2a-bridge/docs/superpowers/reviews/` on the planning branch. I read them there. The 3c2 handoff and the slice-3 brief are present here.

---

## 1. Invariants and state machines

**Four authorities are conflated today; the design names one owner each.**

| Authority | Owner | Excluded from |
|---|---|---|
| A1 attempt recovery | `DurableProcessFlightAttemptV3` (once, at start, under lease) | any live request |
| A2 request admission | `ResourceFlightRegistryV1::reserve` (under `flights`) | recovery |
| A3 flight lifecycle | `RetainedResourceFlight` (under `transition`) | other flights |
| A4 cleanup observation | `ApiBackend` session map | flight mutation |

A1 currently runs *inside* A2 — `bind_remote_request` calls `recover_remote_request_reservations` (`process.rs:954`) with no lock spanning both and no consultation of `flights`. That single inversion **is** T1.

**Declared total lock order:** attempt lease (flock, handle lifetime) → `registry.flights` → journal operation lock (`append_lock` → flock on retained fd) → `flight.transition`. No callback under any of them; publication strictly outside all four. This is already the order `reserve` takes (`:2176`); the design only makes it explicit and makes recovery obey it.

**Invariants:** I1 deadness proof (lease held + `flights` held + empty); I2 admission exclusion; I3 total prefix function; I4 proof-only `Complete`; I5 no waiter outlives its declared deadline; I6 durable evidence survives until ack; I7 population O(in-flight), not O(lifetime); I8 immutable root object; I9 generation flights / schema v1 / slot accounting untouched (every change discriminated on `ResourceFlightKeyV1::DedicatedRemoteRequest`); I10 production unarmed throughout.

**Durable request state machine** — rows unchanged, slot accounting unchanged (`FlightReserved` reserves 4; four following rows consume one each):

```
∅ → RESERVED₀ → RESERVED → IDENTIFIED → INTENT → DISPATCHED → SETTLED → SETTLED+PUBLISHED → ∅
```

**Crash-cut recovery table (total — this is I3):**

| Cut | Provider effect possible | Action | Recipients |
|---|---|---|---|
| `RESERVED₀` (zero rows) | no | rollback reservation | — |
| `RESERVED` | no | CAS `Failed`, publish, retire | none |
| `IDENTIFIED` | no | CAS `Failed`, publish, retire | owner from `RemoteRequestIdentityCaptured` |
| `INTENT` | no | CAS `Failed`, publish, retire | `IntentJournaled.owner_snapshot` |
| `DISPATCHED` | **yes** | CAS `Unknown`, publish, retire | snapshot ∪ collateral |
| `SETTLED`, no marker | — | publish, mark, retire | derived from records |
| `SETTLED` + marker | — | retire only | — |
| anything else | — | **refuse** `Accounting` | — |

`INTENT → Failed` is provable: `begin_journaled_dispatch` (`:1469`) fsyncs `DispatchStarted` before `RequestScope::begin_dispatch` returns, i.e. before the POST future is built (`backend.rs:862-877`). That's owner decision **D2**.

**Cleanup observation machine** replaces the `Option<RemoteRequestSettlementV1>`:

`NoRequest → Complete` · `LegacyActive → Unknown` (**D1**) · `AdmissionInFlight → Unknown` · `Settleable` → `Complete` only from a durable `Complete`, else `Unknown` · `RetainedDebt → Unknown`.

Deliberate non-invariant, taken straight from the adjudication's refinement: a **proven** durable `Partial`/`Failed`/`Unknown` does *not* taint a later independent cleanup. Only an **unproven** terminal (settlement returned `Err`, or a drop with no durable result) creates debt. A red test pins both directions.

**Publication contract, weakened explicitly.** Exactly-once *delivery* across a crash cut is unachievable without a transactional consumer. Preserved: at-least-once, plus a durable idempotence key `(resource_flight_id, owner)` — both fields already exist on `NodeCleanupAggregationV1`, so no wire change. Removed: "the publisher is called exactly once"; the window is a crash strictly between the publish call and the durable marker. Binding on slice 5: its `NodeCleanupRecordV2.collateral` writer must be idempotent on that key (**D4**). Without this ruling T7 cannot be closed, only relocated.

## 2. Salvage map (21 rulings; exact seams in the plan file)

**KEEP** — `DedicatedRemoteRequestIdV1` and its refusals (schema-v1 `"req-1"` golden byte-identical); the typed request key; `RemoteRequestIdentityCaptured` + slot accounting; the `(turn_epoch, identity)` cancel/clear fence (`backend.rs:317-362`) — this is the forget/recreate ABA protection and the stale-round fence, and it is mutation-sensitive; `TurnScope` epoch-only clearing; the monotonic acceptance barrier and acceptance-aware `request_flight_failure`; the unarmed production route; `settle_request_scope`'s one-shot `Option<RequestScope>`; `join_blocking` unmodified (must stay Tokio-free for `terminate_blocking`'s bare thread); the Wiremock corpus.

**REVISE** — recovery *caller and locking* (`:2092`, relocate + gate); `recover_journaled_intent_as_unknown` (`:1761`, wrap in a total prefix classifier for request keys only); `RequestScope::drop`/`settle` slot clearing (`:380-413`, `clear_exact_with_outcome`); `cleanup_session_checked` (`:727-761`); terminal CAS → void publish (marker protocol); `reserve_flight` population bound + retirement; `bind_remote_request`'s discarded settlement error (`process.rs:1019`).

**REPLACE — exactly one mechanism:** `FileResourceFlightJournal`'s root/lock authority (`:572-632`). `metadata()`-then-create-capable-lock recreates a removed root, and every operation re-resolves `root`/`lock_path` **by path**, so a rename-replace redirects live handles. No local patch binds the directory *object*. Replaced by `PinnedDirectoryV1` + a retained lock fd.

**Discovered, not among the eight:** `ResourceFlightRegistryV1::flights` never removes an entry — one `Arc<RetainedResourceFlight>` leaks per lifetime request for the attempt's life. It is the in-memory twin of T6 and is folded into the same task.

**Reused rather than rebuilt** — `PinnedDirectoryV1`, `validated_child_name`, `open_child_no_follow`, `rename_child_no_replace`, `rename_child_replacing`, `pinned_root_unchanged`, `FailureCountdownV1` all already exist in `crates/bridge-core/src/fs_custody.rs` and are the workspace's sanctioned immutable-custody vocabulary. Only three primitives are genuinely missing (create-or-open child, unlink child, `fdopendir` listing).

## 3. Rejected alternatives

Fresh restart (no mechanism is unsalvageable; discards mutation-sensitive coverage). One big journal lock spanning recovery+admission (holds a file lock across a POST-preceding sequence, inverts the declared order; quiescence removes the need entirely). Consult `flights` during recovery but keep it per-bind (closes the same-process race only, and leaves recovery on the hot path). Journal rows for publication state (`append` refuses after `Settled` and `outstanding()` forbids it — would move schema-v1 goldens). Outbox **before** the terminal CAS (a lost CAS leaves a stale outbox needing rewrite under the same exposure; marker-after-terminal has one strictly smaller window). Claiming exactly-once with a retry loop (not achievable; claiming it *is* T7). Async settlement notification (imports a runtime into a type used from a bare `std::thread`). A longer `tokio::time::timeout` (any handle timeout leaves the worker parked — that is T4). Keep reservations, raise the cap (moves the brick). `Retained` instead of `Unknown` for Legacy (`Retained` asserts a known retained resource; the API backend holds none). Extending retirement to generation keys now (same class, far-away bound, re-creates the big-bang defect).

## 4. Task sequence — seven tasks, green after each

Each: one commit, focused gates (`fmt`, `diff --check`, `clippy -D warnings`, named tests), full-suite boundary before commit, **stop/split threshold 500 net implementation lines or 900 total**, production unarmed.

- **A — descriptor-relative journal root (T8).** `fs_custody` gains three `openat`-based primitives + `fdopendir` listing; journal holds `PinnedDirectoryV1` + retained lock fd; `hard_link` reservation trick becomes `rename_child_no_replace`; non-Unix `open()` refuses rather than degrading. All `FileResourceFlightJournal::open` call sites are tests — no production caller exists. Windows CI builds only `bridge-store` (+ deps), so `bridge-core` must still compile but runs no test here.
- **B — bounded lifecycle (T5, T6, + the registry leak).** Population counter enforced *before* reservation creation with an injectable limit; `RequestPrefixClassV1` total classifier; `retire_settled_flight`; unified census bound (the file/in-memory `==` vs `>` off-by-one). Retirement order `jsonl → marker → reservation` makes every crash window self-heal through the existing zero-row rollback.
- **C — durable publication marker (T7).** Marker child + re-publish-on-reopen; recipients derived from durable records **for request keys only** (generation keys keep the in-memory derivation, because legacy V2 attach is deliberately unjournaled and journal-derived recipients would be empty).
- **D — quiescent attempt-start recovery (T1).** `open_recovered` takes an attempt lease (a second live process refuses rather than terminalizing the first's requests), runs recovery once with `flights` **held and asserted empty**, sets `recovered`; `bind_remote_request` refuses if not recovered and no longer calls recovery. The census function takes the held guard as a parameter so exclusion is type-enforced, not commented.
- **E — deadline-bounded observation (T4).** `join_until` on `Condvar::wait_timeout` against the injected clock; `cleanup_session_checked` drops `tokio::time::timeout` entirely — the worker bounds itself.
- **F — cleanup states + drop custody (T2, T3).** `bridge-api` only: admission ticket with a `Drop` guard (every exit path, including the `?` on `flight.settle`), retained debt with the acceptance flag, `RequestScope::drop` settles *first* then transfers the outcome, and `forget/release_session_observed` overrides record the acceptance-aware `Persistence`/fatal diagnostic through the supplied observer.
- **G — reconciliation + carry-forward guards.** Includes `container_rw_cleanup_composition_is_untouched`: the API path stays single-field and neither adopts nor erases the 3c1 two-field inner/outer split.

A–E are strictly internal to an unarmed component. The single production-visible change (D1) lands second-to-last, alone, with its own red test.

## 5. Adversarial tests and schedules

Instruments: a barrier journal decorator (the pattern exists at `reaper.rs:1262` and `backend.rs::BreakJournalBetweenRounds`); crash-cut construction by driving to the cut and reopening (established by the 3a recovery tests); armed fault countdown (`FailureCountdownV1` shape); manual `MonotonicClock`; a worker-exit witness `AtomicBool`.

Load-bearing schedules: **C1/C2** park request A at the zero-row and journaled-intent cuts, drive B to completion, prove A stays live and settles `Complete` — both fail today (B rolls A back / terminal-CASes A `Unknown`, so a *successful* provider request reports `Unknown` and publishes nothing). **K1–K4** every pre-intent prefix terminalizes with the exact recipient set and repeat reopen is a no-op. **K5** stop after terminal CAS before marker → exactly one publication total. **K6** stop after publish before marker → second delivery with an identical idempotence key, pinning the *declared* weakening so a later reader cannot mistake it for a defect. **P1/P2** over-cap refuses before reservation creation; retirement restores admission. **F2** root renamed aside and replaced → every operation refuses and the replacement is untouched. **L1–L5** the four cleanup states plus the over-tainting guard, and the exact R3 regression (accepted drop + terminal refusal → `Unknown` + `persistence`/fatal/`accepted=true`). **T1w** unpolled scope + short deadline → `Unknown` **and** the worker observably exits.

Each task's commit message names its mutation sensitivity (remove the `flights`-held requirement → C1/C2 re-red; remove the marker → K5; revert to `tokio::time::timeout` → T1w; revert to path root → F2).

## 6. Owner decisions and residual risk

**D1** — `LegacyActive → Unknown` (recommend yes). The *only* production-observable change in the design: a `kind=api` warm session force-released mid-turn reports `unknown` instead of `complete` in detached-cleanup projection and terminal evidence. I verified it does not wedge session lifecycle — `session_manager.rs:4403-4451` shows a non-`Complete` disposition still removes the session; only `Err` creates a retry tombstone, and `require_complete_cleanup` is on the container path, not the API path. **D2** — pre-dispatch recovery `Failed` vs uniform `Unknown` (recommend `Failed`; affects node collateral rollup). **D3** — cleanup deadline: dedicated constant rather than `cfg.request_timeout` (120 s of blocking-pool thread per cleanup). **D4** — accept the publication weakening and bind slice 5's writer to the idempotence key. **D5** — retirement deletes the flight journal (forensic loss). **D6** — generation keys keep unbounded reservations for now; ledger it.

Residual SMELL: the bounded waiter still holds a blocking thread until the deadline; retired ids lose `IdentityCollision` detection (cryptographically irrelevant, stated not defended); the O(n) census now runs once per attempt instead of per request, which also retires risk R3-3; `parse`'s legacy opaque shape means a non-minted id doesn't inherit the exposure guarantee and nothing enforces that at the diagnostic boundary; the V3 file journal becomes Unix-only, which is a slice-4 arming precondition. 3d stays blocked.

**Unsalvageability criteria — none hold.** (1) No schema/golden change is required: no new rows, publication state lives in a sibling marker child. (2) No landed generation-flight consumer contract moves: every change is key-discriminated, and recipient derivation is specialized *precisely because* legacy V2 attach is deliberately unjournaled. (3) The eight WRONGs fall into five disjoint ownership regions with one strict dependency chain. (4) The largest task is ~500 implementation lines. (5) Not one of the eight touches request identity, the turn epoch, the ABA fence, or the acceptance barrier — the cancellation core is sound. The real defect was slice sizing: ~2,700 implementation lines against a 1,500 estimate and a 2,250 stop threshold.

DESIGN LENS: READY
