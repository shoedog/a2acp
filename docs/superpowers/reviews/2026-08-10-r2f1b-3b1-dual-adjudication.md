# 3b1 dual-lens adjudication — repair round R1–R7 (declared cap: ONE round)

Artifact: `feat/r2f1b-3b1-process-authority` @ `45a72f01` (base `0a3c2434`).
Lenses: opus senior-lead REVISE (2 WRONG, 8 SMELL — `2026-08-10-r2f1b-3b1-opus-lens.md`)
vs sol/max REJECT (6 WRONG-BLOCKER, 1 SMELL — `2026-08-10-r2f1b-3b1-sol-lens.md`).
Adjudicator: orchestrator, at source on the s3b1 worktree (frozen identity verified).

## Verdicts (every mechanism re-verified at source; anchors on `45a72f01`)

| Finding | Verdict | Source evidence |
|---|---|---|
| sol-1 census omission → vacuous `Complete` | **REAL-WRONG** | `process.rs:1443-1464` acceptance = registry-identity ∪ parent-link closure; a reparented same-PGID member (never previously accepted, parent dead) is omitted; `:1515-1548` empty before/after sets ⇒ `all_stopped` vacuously true ⇒ `stable=Some([])` ⇒ zero SIGKILL ⇒ `settle_dispatch` `Complete`. Distinct from the tested fork-during-sweep case (parent accepted there). |
| sol-2 journal capacity not secured pre-spawn | **REAL-WRONG** (merges opus-W1) | `retained_resource_flight.rs:320-348` `needed=len+1+slots_after>cap→Full`; terminal headroom reserved only at `journal_intent` (`:1103-1126`, post-spawn); `process.rs:1976` V2 cap 512 hard-coded; `:1998-2023` reserve+attach precede spawn but reserve no lifecycle capacity; `acp_backend.rs` `session_entry` attaches an owner per new `SessionId` unconditionally (no V2 gate), `attach_owner`/`detach_owner` append one row each, no eviction. Opus's ~509-session / ~254-cycle scenario arithmetic checks out. |
| sol-3 macOS census: PATH `ps`, unbounded, trusted | **REAL-WRONG** | `process.rs:1026-1028` `Command::new("ps")` (PATH-resolved), `.output()` no deadline; rows' ppid/pgid trusted (`:1038-1053`, only start identity kernel-probed); empty output passes `status.success()` ⇒ live root vanishes from census ⇒ sol-1's vacuous-Complete arm; blocked `ps` hangs driver and all joiners (violates no-eternal-hang). |
| sol-4 reap timeout settles `Complete` | **REAL-WRONG** | V2 `:1629-1637` and V3 `:1717-1725` reap loops `Ok(None) => break` at deadline, nothing recorded; `settle_dispatch:1556-1567` fails only on `rc==-1&&errno!=ESRCH` ⇒ kill rc=0 + unreaped child (D-state/NFS) ⇒ `Complete` while the process exists. |
| sol-5 concurrent joiner refused during Driving | **REAL-WRONG** (narrowed) | `reject_recycled_pid_if_present:1320-1344` has the Finished carve-out (`:1330`) but NotFound-while-**Driving** ⇒ `IdentityUnavailable`; called pre-claim at `terminate_blocking_inner:1661` and at `join_blocking:1295`; `settle_identity_refusal:1738-1739` Join/Finished arms return `Err` instead of joining. Exactly the Driving window; Finished path is already correct. |
| sol-6 authoritative terminal discarded at projection | **REAL-WRONG** | `settle:1274-1284` returns `Result<(),_>`, discarding `SettlementV1.result` (adopted terminal `:1225-1231`, `AlreadySettled:1250-1259`); `settle_dispatch:1572-1575` returns its LOCAL proposal; ACP `process_action_failed` classifies only `Failed` as failed ⇒ adopted `Unknown`/`Partial` reads clean. |
| opus-W1 V2 journal exhaustion | **REAL-WRONG** | = V2 half of sol-2 (independent convergence by both lenses). |
| opus-W2 refusal leaves tree SIGSTOPped; zombie freezes closure | **REAL-WRONG** | `grep SIGCONT crates bin` = zero matches; stopped-probes return `false` for zombies (Linux `state=='T'` only `:1065-1071`; darwin `pbi_status==SSTOP` only `:1108-1111`); zombie's parent is itself stopped ⇒ unreapable ⇒ 16 passes ⇒ `ContainmentUnstable` ⇒ no SIGKILL, no resume — tree frozen and unkillable through the bridge. |
| opus ledger clarification (darwin red) | **ACCEPTED** | Root cause = test spawns `/bin/true`, absent on macOS (only `/usr/bin/true`); bind fails at `Spawn`, never reaches `ImmutableStart`; production `:2044-2066` passes `Some(pid)` correctly. Repair = test binary path, not production. Supersedes the ledger's "row lacks pid Some(_)" description. |
| sol SMELL wire goldens / opus S8 slots drift | **CARRY** | Exact goldens for the new event shapes + unstable-closure zero-SIGKILL test ride the repair. |
| opus S4 driver-panic strands joiners | **CARRY** (bounded) | `claim_action:1268` → panic before `finish_action:1283` leaves `Driving` forever; Condvar wait `:1303-1307` never wakes; reachable from V3 `AcpBackend::drop`. |
| opus S7 V2 drop conditional on poisoned claim | **CARRY** (bounded) | `legacy_v2_drop:1760` poisoned action lock ⇒ `Err`, no group SIGKILL; historical Drop was unconditional ⇒ descendant leak on the parity path. |
| opus S3 kill-switch notify delayed behind terminate | **CARRY IF TRIVIAL** | Notify precedes the awaited terminate only if reordering provably cannot drop the escalation; otherwise ledger. |
| opus S1/S2/S5/S6, sol severity framing on V3-dormant items | **LEDGER / fold into tests** | S2 folds into R4's seam test; S1 real-host closure coverage ledgered to 3b2 (needs real descendants harness); S5 dead state — wire or delete in-round if one-line, else ledger; S6 coverage note only. |

Dedup: sol-2 ⊇ opus-W1; sol-1 and sol-3 share the vacuous-empty-census arm; opus-W2 and sol-1
are the two failure directions (freeze vs escape) of the same closure mechanism.

## Repair directives (dispatched to sol/xhigh via bridge implement; base = branch tip)

- **R1 census integrity**: anchor-based admission — while any registry member's immutable
  identity verifies live, admit every kernel-confirmed same-PGID member regardless of current
  parent (setpgid/session semantics make same-PGID-while-anchor-live sound); after anchor
  loss, nonempty unknown same-PGID membership ⇒ `ContainmentUnstable`, never vacuous
  `Complete`; a live root absent from its own census invalidates the census (error, not empty).
  darwin census: absolute `/bin/ps` as enumerator ONLY, bounded by a deadline, every row's
  pgid/ppid re-validated against kernel probes (`libc::getpgid` + existing `proc_pidinfo`)
  before use — no PATH resolution, no trusted rows, no unbounded wait.
- **R2 capacity-before-spawn**: V3 — pre-spawn admission reserves the full protective
  lifecycle (binding-failure row, capture, intent, signal batch, terminal) before
  `Command::spawn`; insufficient cap refuses with ZERO spawn calls (cap-2/cap-7 tests). V2 —
  owner attach/detach evidence becomes non-failing for legacy behavior (Full on the
  compatibility journal must not fail the attach or the session; owner set still updates;
  512-cap churn test proves sessions + retirement stay usable past 253 cycles).
- **R3 refusal-state safety**: exited/zombie members count as contained in `all_stopped`
  (they cannot fork); every refusal exit from `close_and_kill` SIGCONTs each member its
  volley stopped; 16-pass `ContainmentUnstable` test asserts zero SIGKILL AND resumed members.
- **R4 reap truthfulness**: deadline expiry with `try_wait()==Ok(None)` ⇒ typed timeout
  recorded, disposition `Failed` — never `Complete` — in BOTH V2 and V3 arms (signal
  sequence unchanged; V2 parity is signal-shape, not disposition); injectable child-wait
  seam + persistent-`Ok(None)` test (this also makes the forced-kill disposition assertion
  discriminating — opus S2).
- **R5 join convergence**: NotFound-while-Driving/Finished joins the in-flight result
  (never authorizes another signal, never refuses); different-live-identity still refuses;
  `settle` returns the authoritative `SettlementV1.result` and `settle_dispatch` projects
  IT to every caller; ACP teardown consumers classify any non-`Complete` disposition as
  failed; barriered two-caller test asserts identical terminal results.
- **R6 darwin red**: fix the test's spawn path (`/usr/bin/true` or runtime-resolved);
  assert the ImmutableStart leg actually executes on darwin (bridge-core host back to green).
- **R7 small items**: finish_action drop/unwind guard (driver panic settles joiners with a
  typed error); poison-tolerant V2 drop group SIGKILL; exact serialization goldens for
  `OwnerDetached`/`ProcessBindingFailed`/`ProcessSignalsObserved` + strict-reader negatives;
  S3 notify-reorder only if provably safe; S5 wire-or-delete only if one-line.

## Cap and convergence

Declared cap: ONE targeted repair round on the existing artifact (this document is the
declaration). Findings are closed-enumerable (each names state, wrong result, bounded fix).
If the round's review surfaces open-class findings, park and escalate per steering.
D3 residual (fingerprint-not-owner-id) stays DEFERRED with ledger — leak-guard posture.
V3 remains route-unarmed; arming stays slice-4 scope.
