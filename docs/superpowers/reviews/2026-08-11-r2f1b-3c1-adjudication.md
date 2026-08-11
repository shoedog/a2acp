# R2f1b 3c1 dual-lens adjudication — container authority (`f397ee5f..dc6b9031`)

Operator adjudication of the 3c1 dual-lens round. Lens records adjacent:
`2026-08-11-r2f1b-3c1-opus-lens.md` (opus senior-lead, VERDICT: REVISE — 3 WRONG /
10 SMELL) and `2026-08-11-r2f1b-3c1-solmax-lens.md` (gpt-5.6-sol at max via bridge
`run-workflow code-review`, VERDICT: REJECT — 5 WRONG / 2 SMELL). This was the FIRST
external review of the diff (the slice's internal implement-review never ran —
reviewer auth deaths, since remedied by owner re-login; the solmax lens itself ran
clean through the bridge post-reseed, which is also the closing evidence on the
auth-investigation recurrence).

Every WRONG from both lenses was operator-verified real at source before this
adjudication; none was refuted, none was acceptance-literalism. Notable structure:
the lenses **independently converged** on the refusal-path cancel defect (opus W3 ≡
sol W1), and each found real WRONGs the other missed — opus's live-probe `\t`
template discovery (sol read the same function and called the gate "exact") and
sol's projection/authority tracing (opus's "no name-based removal" spot-check was
argv-level only). Neither lens alone was sufficient; dual-lens mandatory for
destructive surfaces stands re-validated.

## Merged WRONG population → repair mandate R1–R7 (all operator-verified)

- **R1** (opus W-1, BLOCKER): identity-probe Go template emits literal `\t` (Rust
  `"\\t"`, reaper.rs:703) while the parser splits on a real TAB (:729) — every
  production container identity observation fails ⇒ containerized agents and all
  container reaping are disabled in this build. Live byte-level docker probe on this
  host confirmed (`5c 74` emitted; control with a real TAB parses). One-character fix
  + the S8 condition below.
- **R2** (opus W-2, BLOCKER): probe-before-`rm` inverts docker's `rm -f` idempotency:
  an already-gone container (`run --rm` self-removal — the common warm-exit case) or
  one transient spawn failure yields a non-`Complete` disposition, and
  `finish_reap`/`reserve_generation` then fence the session PERMANENTLY (no clear
  path exists off `Ok(Complete)`). Base recovered on identical inputs — regression.
  Fix: "selector resolves to nothing" = removal-complete; plus an exit for
  `RefusedUnknown` generations proven never-published.
- **R3** (opus W-3 ≡ sol W-1, convergent, BLOCKER): all `drive_managed` refusal arms
  return before the subordinate ACP cancel closure is even constructed, and the
  retained `ReapOwner` transitively pins the inner `Arc<dyn AgentBackend>` (stream
  `Arc`s survive too) — on any non-`Complete` disposition the agent is neither
  cancelled nor dropped, ever. Fix: run the subordinate unconditionally inside the
  flight (gate only removal on identity/labels); make the subordinate one-shot and
  released at settlement.
- **R4** (sol W-2, BLOCKER): unit-returning destructive wrappers collapse protective
  dispositions into success — cold `cancel` discards `Ok(Unknown/Retained)` → `Ok(())`
  (lib.rs:1646), `retire` records only `Err` (:1838-1841) — so the registry's
  join-or-refuse boundary sees success where destructive authority was refused. Fix:
  only `Complete` is success for unit wrappers; protective values become a typed
  refusal; invert the pre-ID cancel/retire tests accordingly.
- **R5** (sol W-3, BLOCKER; the 3b2 sol-1 class one layer up): protective
  dispositions are erased before projection — `container.teardown.reaped` recorded
  for every `Ok(_)` (lib.rs:1430), executor `.map(|_| ())` + trackers classify any
  non-error as `Complete`, durable projection says `"complete"` while the resource
  owner says `Unknown`. Fix: thread `BackendCleanupDispositionV1` through
  tracker/aggregation/diagnostics; stores already accept the vocabulary (3b2 R1).
- **R6** (sol W-4 + opus S7, BLOCKER): name authority survives on the live `:ro`
  path (`ReapController::production(runtime, name)` resolves the NAME at teardown,
  acp_backend.rs:1132 — a recycled name removes the successor) and the operator
  `containers reap` path (name re-resolved across EVERY configured runtime — a dead
  Docker record can remove a live same-name Podman container; prints `reaped` even
  when nothing ran). Fix: capture immutable ID + ownership labels + runtime at
  spawn/discovery, revalidate before exact-ID removal, report success only from
  successful removal status.
- **R7** (sol W-5 + opus S1/S2, repair-now despite V3-unarmed): `drive_managed`
  discards the durable terminal CAS winner from `settle()` and returns its local
  result (reaper.rs:584) — the exact 3b1 "split projection" class, repaired there,
  reintroduced here; the process driver is the in-repo control (returns the adopted
  value). Fix: return/project the adopted result; settle `Failed` on the
  pre-dispatch refusal path instead of leaving the flight non-terminal; record
  removal success independently of journal-append failure; thread real
  `duration_ms`.

Repair conditions (fold into the same round):
- **C1** (opus S8, condition not option): the slice's only new production I/O
  (`observe_container_identity` + `production_*` wiring, six call sites) has zero
  test coverage — extract a pure parse seam, pin a byte-exact golden against real
  docker output, add a docker-gated round-trip integration test. This is why 3,944
  green tests said nothing about R1.
- **C2** (opus S3): `validate_observed` requires the `a2a.*` namespace to contain
  NOTHING beyond the canonical set, but `{{json .Config.Labels}}` merges image
  labels — any base image carrying an `a2a.*` label refuses every spawn and reap.
  Require canonical keys present-and-equal; tolerate non-canonical `a2a.*` keys the
  bridge did not stamp (journaled, not refused).
- **C3** (sol SMELL-1): strengthen the `dc6b9031` test — assert
  `!release_task.is_finished()` before the notify and bound the initial `entered`
  wait.
- **C4** (sol SMELL-2): the runtime-timeout regression fixture keys on `$3`, which
  the new `inspect --format` argv makes `--format` — blind test; fix the index or
  branch per invocation.

## Rulings

- **Lattice BINDING row: RE-ARGUE ACCEPTED by both lenses** (opus with mechanism
  checks, sol "narrowly"). Production ContainerRw is unwrapped-outermost
  (main.rs:1642) and production is unconditionally V2 (`resource_flight_attempt_v3:
  None`, main.rs:1158) so the `Retained` precondition is unreachable. **BINDING
  CARRY-FORWARD** (both lenses, recorded): the two-field inner/outer split — or an
  equivalent non-lossy composite disposition — is REQUIRED in the same slice as the
  first of (i) any production constructor arming `resource_flight_attempt_v3`, or
  (ii) any composition placing ContainerRw inside a preservation-owning decorator.
  Opus S9 (single field already lossy: `SubordinateCleanup` ⇒ container REMOVED yet
  reported `Retained`) is evidence on that row.
- **retire()/inner.cancel substitution**: sound on the success path (relocated into
  the flight, exactly-once, linearised — strictly stronger than base) and UNSOUND on
  the refusal path — which is R3. Both lenses agree.
- **`dc6b9031` gate repair**: SOUND, discriminating power preserved (both lenses);
  sol's two tiny strengthenings are C3.
- **S2 session_reaps retention**: mechanism verified by both lenses as implemented;
  the permanent-fence defect riding on it is R2, the Arc-pinning is R3.

## Verdict adjudication

Opus REVISE vs sol REJECT is not a factual conflict — identical repair population,
different severity framing. Operative outcome: **targeted repair on the existing
artifact, ONE repair round (declared cap), then host gates + operator verification.**
Restart not warranted (findings closed and enumerable; architecture endorsed by both
lenses).

## Ledger (not in the repair round)

- opus S4: cold `cancel` now joins a full composite teardown (slow/failable
  `tasks/cancel`) — accepted semantic under join-or-refuse; watch operator UX.
- opus S5: `retire()` first-error propagation vs wrapper composition — revisit when
  ContainerRw is ever wrapped.
- opus S10: over-long inspect output degrades to `Timeout` rather than a
  distinguishable error — cosmetic.
- Process note (opus, endorsed): three green gate runs carried zero information
  about R1 because the gate was aimed entirely at the injected seam — C1 is the
  structural answer; keep "new production I/O ⇒ seam test in the same slice" as a
  standing review row.
- Auth ledger: the solmax lens ran clean through the bridge minutes after owner
  re-login; combined with the pre-reseed dispatch death (xdg-open ENOENT, ~5s),
  the single-token-family mechanism is now confirmed in both directions. The
  dedicated a2a-creds login session remains the standing fix.
