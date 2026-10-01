# Handoff — Docker auto-removal cleanup race repair (bounded prerequisite to ADR-0041 2B3b)

**Written:** 2026-09-30 (round 3 repair; rounds 1–2 are preserved below as receipts) · **By:** Sonnet 5.5 implementor (quarantine clone) · **Provider:** claude
**Workspace:** `/Users/wesleyjinks/code/.a2a-implement/impl-35818-c2c16khq` · `implement/impl-35818-c2c16khq` ·
**Measured state:** `[MEASURED]` base `a25ddb01a9322f5994604f51c05c0378efe64965` · HEAD
`07d5b32125ff92804d5c320176ab7424c9ed18b1` (the round-2 commit) + a staged round-3 repair, uncommitted, not committed
by this session · Probe `git rev-parse HEAD` / `git status --short` / `git diff --cached --stat` · Output:
`reaper.rs` + this handoff only.
**Predecessor:** controller handoff `2026-09-29-adr0041-slice2b3b-controller-handoff.md` (§1 step 2, §4 row 2).
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** `[MEASURED]` claims in **§9** were probed by the round-3 session and are bound to the hashes in §9.7.
Every `[MEASURED]` tag in **§0–§8** that is not explicitly marked "(round 3)" was measured by the round-1/2 session
against the bytes named in §6 and is **`[INHERITED]` here**: round 3 re-ran none of it, and its raw evidence did not
survive (§0 b). Those totals and matrices are receipts bound to their old hashes and are not new local runs.
`[INHERITED]` also covers the task text, the reviews, and earlier handoffs.

**Task (authoritative):** `.git/A2A_TASK.md` in the clone, SHA-256
`a7dc9843aefb0843335c41958639444bb2b7889686150143e4c7d42fbd328e0e`; task/resume ID `impl-35818-c2c16khq`.
**Review state:** Sol round 1 of the original cap returned REJECT (`wrong=3 smell=0 blockers=3`) against `607a9a15`;
§8 records each finding and its round-2 repair. `[INHERITED]` Sol's review of `07d5b321` found one bounded parser
blocker plus a stale commit-message SMELL, and the controller disclosed one converging extension, round 3/3. §9
records that repair. This handoff claims no approval; a separate independent Sol round-3 receipt will bind the final
controller commit.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` (round 3) this clone is the sole writer; nothing was pushed, merged,
committed, or switched by round 3, and only the two output paths are staged. The controller creates a new scoped
commit; the frozen automatic checkpoint remains Rejected — RESOLVED.
**(b) Custody exposure** — the exposure this item flagged in round 2 **was realised**: `[MEASURED]` (round 3)
`/tmp/a2a-evidence/` does not exist in the round-3 container. Controller correction: round-1 raw evidence survives
in `/private/tmp/adr0041-2b3b-20260929/cleanup-evidence`; round-2 raw logs were lost, leaving the embedded
receipts bound to their old hashes. Round 3's evidence is under
`.git/a2a-evidence/cleanup-round3/` in the mounted quarantine (index in §9.7) — RESOLVED for round 3, and the
controller should snapshot it before the lane ends.
**(c) In flight / irreversible** — `[MEASURED]` (controller) the authorized Sonnet implementation turn completed;
bridge teardown records cleanup complete (668 ms). No additional provider smoke or replay ran. Test runtimes
were shell fixtures; cargo used `--locked --offline` and read-only `/cargo`, with proxy variables unset — RESOLVED.
**(d) Model identity** — `[MEASURED]` (controller) every assistant model in the saved continuation transcript
`54a56deb-49d2-4129-8295-023593788e97.jsonl` is `claude-sonnet-5-5`. Receipt:
`/private/tmp/adr0041-2b3b-20260929/cleanup-round3-model-provenance.json`, including transcript SHA-256.
The implementor itself had only a self-report; the controller resolved the provenance gate. Earlier session
`abc03096-64d8-44f2-81ca-c7390acee053` stopped unchanged for missing `/cargo` — RESOLVED.
**(e) Operational cost of finding 1 — an owner-visible decision** — `[MEASURED]` the review required confirming
absence after a **zero** exit too. Every successful removal now depends on one inventory, and the task fixed that
inventory as the existing unfiltered `ps --all --no-trunc` capped at 64 KiB. On a host whose inventory exceeds the
cap (roughly 700 or more containers), every removal, successful or not, returns `IdentityUnavailable`. That is the
`Unknown` disposition for a protected v3 flight, though the container was removed. An ID-scoped `--filter id=`
inventory would remove that exposure. The controller retains the task's existing bounded fail-closed contract
and defers command/filter expansion; no new owner permission is required. Current host measurement: 993 bytes,
11 rows. The operational limitation remains disclosed — DEFERRED.
**(f) Authorization granted but not exercised** — `[INHERITED]` owner: "Repair and review cleanup before
implementation." Not exercised: any 2B3b Git-plane code, any provider smoke, any roadmap/spec/AGENTS/manifest edit.
**(g) Frozen checkpoint** — `.git/a2a-bridge/implement-checkpoint.json`, SHA-256
`39492f7ccc865fec327a4a27fd8cf63c486454dd1acbdd1c4cac4d2cd05930c0`, terminal Rejected. `[INHERITED]` that is the hash
the blocked first round-3 session recorded before any edit, and `[MEASURED]` (round 3) it is byte-identical when
re-hashed at the end (`FINAL-RECEIPT.txt`). It was not modified, restarted, or forged — RESOLVED.
**(h) Empty `NAMES` field** — `[INHERITED]` the controller clarified that an entirely empty names field is valid for
a non-empty identity (no name alias matches; the exact ID still decides), while empty elements inside a non-empty
list and bare-slash names are rejected. The round-3 harness candidate had rejected empty names; the repair follows the
clarification and §9.5 P5 proves the superseded behavior would fail — RESOLVED.
**(i) Container suite reliability** — `[MEASURED]` OPEN, see §9.6. Two of four all-targets runs on the final bytes
failed one or more timing-sensitive tests (three different tests in three crates, none of which reaches the changed
parser), and two passed clean; the default-plus-doctest run passed. The three failures did not reproduce in
isolation or in the standalone repeats counted in §9.6, and the same-environment controls did not reproduce them.
The controller's own host rerun on the final commit is the deciding evidence.

## 1. Resume order

1. Verify the §9.7 hashes against the staged tree, and confirm the model identity from the provider transcript
   (§0 d).
2. Commit the staged tree with `.git/A2A_COMMIT_MSG` (written by round 3 to describe the final behavior, not round 1).
3. Run the controller's host gates on that final stable commit with no overlapping work. `[INHERITED]` the earlier
   controller macOS suite overlapped round-2 mutations and is inadmissible for attribution. Give §9.6 to whoever
   reads a red timing test.
4. Send the commit to the independent Sol round-3 review and give the reviewer §7 and §9.8.
5. Preserve §0 (e) as the disclosed retained inventory bound; filter expansion is deferred.
6. On approval, land through the scoped PR path in the controller handoff §1 step 3, and only then continue to 2B3b
   (controller handoff §1 step 4).

**STOP conditions:** open-class findings at the round cap (this is round 3/3 — no further extension is disclosed);
a failing gate without a same-environment control; any evidence that the exact-ID `rm` runs twice.

## 2. State ledger

Rows without an `R3` prefix are rounds 1–2, `[INHERITED]` receipts bound to the bytes named in each row and to §6;
round 3 did not re-run them. The `R3` rows are round 3's own `[MEASURED]` results, detailed in §9.

| Item | State | Evidence / correction |
|---|---|---|
| R3 hypothesis (parser) | confirmed | `[MEASURED]` §9.3: the parser, not the caller, reported a malformed inventory as absent |
| R3 RED (round-2 production bytes, tests only) | done | `[MEASURED]` 7 of 56 `reaper::tests` failed behaviorally, source `66732216…` (§9.3) |
| R3 repair | done | `[MEASURED]` `parse_container_inventory_contains` only; no API change (§9.2) |
| R3 GREEN | done | `[MEASURED]` `reaper::tests` 56 pass on `30860ae0…`, 5 runs (§9.5) |
| R3 mutation matrix | done | `[MEASURED]` 14 of 14 mutants flipped; the old oversized fixture is proven non-discriminating (§9.5) |
| R3 fmt / clippy `-D warnings` / hygiene / `git diff --check` | green | `[MEASURED]` on the final `reaper.rs` bytes (§9.6) |
| R3 full workspace | **2 of 4 all-targets runs red** | `[MEASURED]` all-targets 4,866 / 0 / 13 / 90 twice, and 4,864 / 2 and 4,865 / 1 on the other two; default+doctests 4,877 / 0 / 13 / 106; base control 4,838 / 0 / 13 / 90 (§9.6) |
| R3 commit message | done | `.git/A2A_COMMIT_MSG` describes the final behavior; not committed by this session |
| Hypothesis | confirmed | `[MEASURED]` RED below; the failure is in `remove_container_id`, not the identity probe |
| RED attempt 1 (base production code) | done | `[MEASURED]` 7 of 10 tests failed behaviorally, source `edb6d3c1…` (§2.2) |
| RED round 1 (attempt-1 production code) | done | `[MEASURED]` 7 of 44 tests failed behaviorally, source `d3b81551…` (§8.2) |
| Repair | done | `[MEASURED]` `remove_container_id` + `confirm_exact_id_absent_after_removal` + `runtime_inventory_contains_until` |
| GREEN | done | `[MEASURED]` `reaper::tests` 45 pass, stable across 8 repeat runs (§8.3) |
| Mutation matrix | done | `[MEASURED]` 14 of 14 admissible mutants flipped; 1 equivalent mutant (§4) |
| fmt / clippy `-D warnings` / hygiene / `git diff --check` | green | `[MEASURED]` on the final bytes (§5) |
| Full workspace | green with one unattributed flake | `[MEASURED]` `--all-targets` 4,855 / 0 / 13 / 90; default rerun 4,866 / 0 / 13 / 106; first default run failed one unrelated flock test (§5) |
| Independent review | round 3 pending | this handoff claims no approval; the separate Sol round-3 receipt binds the final controller commit |

### 2.1 Hypothesis, stated before any edit

- **Cause:** `run --rm` auto-removal races the bridge's `rm -f <id>`; the runtime answers nonzero because removal is
  already in progress, and `remove_container_id` mapped every nonzero exit to `NonZeroExit` without asking whether
  the exact captured ID was already absent.
- **Confirming observation:** a fake runtime whose `rm` exits nonzero and whose inventory does not list the ID gives
  `Err(NonZeroExit)` on unchanged code, after the `rm` call is recorded once. `Ok(())` after the repair.
- **Falsifying observation:** unchanged code returning `Ok`, or failing as `Spawn`/`Timeout`/`IdentityUnavailable`/
  `IdentityChanged` — the failure would then come from fixture setup or another path.
- **Alternative mechanism:** the identity probe (`inspect` fails while the inventory still lists the container →
  `IdentityUnavailable`). Ruled out: the recorded failure was `container.reap.nonzero_exit`, which only
  `remove_container_id` produces.

### 2.2 RED attempt 1 — base production code, tests only

`[MEASURED]` `reaper.rs` SHA-256 `edb6d3c15f47159054ce851ab53114d6c103660272606157237ff89b41a07f5b`; the base file is
`a08228a6656c8a8d487e8e122f5ecbe47494ac8a1b3f824a901da4647d765f47` and the only diff hunk against it is inside
`mod tests`. Command: `cargo test -p bridge-core --lib reaper::tests -- --test-threads=1`.

```text
test result: FAILED. 31 passed; 7 failed; 0 ignored; 0 measured; 1068 filtered out
exact_id_absent            left: Err(NonZeroExit)  right: Ok(())
delayed_absence            left: Err(NonZeroExit)  right: Ok(())
unrelated_and_recycled     left: Err(NonZeroExit)  right: Ok(())
still_present (window)     "a present object must be observed repeatedly within the window, saw 0"
present_at_deadline        "the object must have been observed"
unusable_inventory         "failing inventory: an ambiguous observation fails closed without retrying"  left: 0 right: 1
hung_inventory (deadline)  "the inventory must have started"  left: 0 right: 1
```

That RED was behavioral: every test compiled against the unchanged API, the `rm` call was recorded once before each
result assertion, and the failure was `Err(NonZeroExit)` where `Ok(())` was required. Its `rm`-succeeds and
`rm`-hangs preservation tests passed before and after.

## 3. What was built (rounds 1–2; the parser contract is amended by round 3, §9.2)

`crates/bridge-core/src/reaper.rs`:

- `remove_container_id` computes **one** absolute `tokio::time::Instant` deadline on entry (`checked_add`; overflow
  refuses as `Timeout` before any spawn) and bounds the `rm -f <id>` child with `timeout_at(deadline, …)`. It then
  calls `confirm_exact_id_absent_after_removal` after **either exit status**: neither is evidence, because `run --rm`
  can win the race behind a nonzero exit and a runtime wrapper can exit zero without removing anything.
- `confirm_exact_id_absent_after_removal` runs the existing parser through `runtime_inventory_contains_until`, an
  absolute-bound form of the existing inventory command. Every inventory child (spawn, output, exit) is bounded by one
  `observe_until` = the earlier of the original deadline and a 2 s window. It never runs `rm` again.
- `runtime_inventory_contains` (used by the identity probe) keeps its `Duration` signature and delegates to the
  absolute-bound form. Its 2 s bound now starts just before the spawn, an immaterial change.

| Outcome after the one `rm -f <id>` (any exit status) | Result |
|---|---|
| ID absent from a complete, in-bounds, parseable inventory, answered inside the bound | `Ok(())` |
| ID present, re-observed every 50 ms until the bound | `NonZeroExit` |
| Bound reached, or child interrupted by it, after a present sighting | `NonZeroExit` |
| Bound reached, or child interrupted by it, with no sighting (incl. a slow inventory) | `Timeout` |
| Absence first seen after the bound (stale answer) | refused: same rule as the two rows above |
| Inventory spawn failure | `Spawn` (propagated, no retry) |
| Inventory nonzero exit, malformed, or oversized | `IdentityUnavailable` (propagated, no retry) |
| No time left before the first observation | `Timeout`, no child spawned |
| `rm` itself hangs past the deadline | `Timeout`, no observation |

Typed causes matter downstream: for a protected v3 flight `bridge-container` maps `IdentityUnavailable` to the
`Unknown` disposition and every other failure to `Retained`, and the flight journals `failure_code`.
Constants: `CONTAINER_REMOVAL_OBSERVATION_WINDOW = 2 s`, `CONTAINER_REMOVAL_OBSERVATION_INTERVAL = 50 ms`. Identity,
ownership-label, and custody checks (`production_with_timeout`, `drive_managed`) are untouched and still run first.

### 3.1 New tests (17) — `reaper::tests` went from 28 to 45 (round 3 adds 11 more and replaces one fixture: 56, §9.4)

| Test | Proves |
|---|---|
| `nonzero_rm_with_the_exact_id_absent_settles_cleanup_successfully` | repaired path, one `rm`, one inventory |
| `nonzero_rm_with_delayed_absence_settles_within_the_observation_window` | delayed absence; 3 inventories, still one `rm` |
| `nonzero_rm_ignores_unrelated_and_recycled_names_when_proving_absence` | a same-name successor with a new ID is not the captured ID; only `rm -f <captured id>` ran |
| `nonzero_rm_with_a_still_present_object_never_greens_after_the_window` | present → `NonZeroExit`; ended by the 2 s window, not the 5 s deadline |
| `nonzero_rm_with_a_present_object_at_the_deadline_never_greens` | ended by the deadline → `NonZeroExit` |
| `nonzero_rm_with_a_hung_inventory_times_out_at_the_shared_deadline` | `rm` uses 1 s of 1.5 s; total under 2.2 s |
| `hung_rm_times_out_without_observing_the_inventory` | `rm` timeout preserved, no inventory |
| `exhausted_deadline_never_starts_an_absence_observation` | no child after the deadline |
| `successful_rm_completes_only_after_the_exact_id_is_confirmed_absent` | zero exit is confirmed by one inventory |
| `successful_rm_with_a_still_present_object_never_greens` | zero exit + present → `NonZeroExit`, window-bounded |
| `an_unusable_inventory_never_greens_and_keeps_its_typed_cause` | failing / malformed / oversized → `IdentityUnavailable`, one inventory; also after a zero exit. Round 3 replaced its oversized fixture (70,000 `a` bytes with no tab, which the parser refused even without the size limit) with valid rows that only the limit can refuse (§9.4–§9.5) |
| `an_unspawnable_inventory_never_greens_and_keeps_its_typed_cause` | → `Spawn`, after either exit status |
| `an_inventory_that_reports_absence_after_the_window_never_greens` | 5 s deadline, inventory answers absent at 3 s → `Timeout` under 2.8 s |
| `absence_first_observed_after_the_bound_is_never_accepted` | a stale absence polled after the bound → `Timeout` |
| `a_present_sighting_followed_by_an_interrupted_inventory_never_greens` | sighting then a hung inventory → `NonZeroExit` |
| `a_managed_flight_journals_the_typed_cause_of_an_unconfirmable_removal` | returned **and journaled** `identity_unavailable` / `spawn_failed`, flight settles `Failed` |
| `a_managed_flight_journals_a_confirmed_absent_removal_as_complete` | `removed = true`, no failure code, `Complete` |

Every fixture test acquires `production_runtime_fixture_permit()` and warms up its runtime through
`warm_up_runtime`. The shared `RemovalRaceRuntime` shell runtime records each `rm` argument vector and `ps` call.

## 4. The mutation matrix (round-2 bytes `02e19bbe…`; `[INHERITED]`, not re-run in round 3 — round 3's matrix is §9.5)

`[MEASURED]` one foreground pass on `02e19bbe…`; each mutant applied, `reaper::tests` run with `--test-threads=1`,
the file restored (`cmp` confirmed). Script `r2-mutate.py`, log `r2-mutation-matrix.log`. Attempt 1's 13-row matrix
ran on superseded bytes and is not reproduced here.

| # | Mutant | Verdict | Failing tests (short names) |
|---|---|---|---|
| N1 | inventory cause collapsed to `NonZeroExit` | FLIPPED | typed-cause ×2, journaled typed cause |
| N2 | interrupted inventory ignores a prior sighting | FLIPPED | present-sighting-then-interrupted |
| N3 | stale absence after the bound accepted | FLIPPED | absence-first-observed-after-the-bound |
| N4 | inventory child bounded by the deadline, not the window | FLIPPED | absence-after-the-window |
| N5 | window ignored, poll to the deadline | FLIPPED | absence-after-the-window, both still-present tests |
| N6 | present at the end of the window greens | FLIPPED | both still-present tests, present-at-deadline |
| N7 | a zero exit trusted as proof of absence | FLIPPED | confirmed-absent, still-present (zero exit), typed-cause ×2 |
| N8 | a second `rm` after absence | FLIPPED | five tests incl. journaled complete |
| N9 | unsettled always `Timeout` | FLIPPED | present-sighting-then-interrupted |
| N10 | unsettled always `NonZeroExit` | FLIPPED | four tests incl. absence-after-the-bound, exhausted-deadline |
| N11 | no pre-observation bound check | FLIPPED | exhausted-deadline |
| N12 | single observation, no polling | FLIPPED | four tests incl. delayed-absence |
| N13 | inventory gets a fresh 30 s bound | FLIPPED | absence-after-the-window, hung-inventory |
| N14 | inventory child bounded by nothing near the deadline | FLIPPED | absence-after-the-window, hung-inventory |
| N15 | `rm` bounded by a fresh `timeout`, not the shared `deadline` | NOT-FLIPPED | none — **equivalent mutant** |

N15 is equivalent: `deadline` is computed immediately before `spawn`, so the two bounds differ only by spawn latency.

## 5. Verification totals (round-2 bytes `02e19bbe…`; `[INHERITED]`, not re-run in round 3 — round 3's totals are §9.6)

**Environment:** `[MEASURED]` Linux `7.0.14-orbstack` aarch64, uid 0, rustc/cargo 1.94.0, Git 2.54.0. Every cargo
command ran with `CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/tmp/target CARGO_INCREMENTAL=0` and the
four proxy variables unset.

| Gate | Result on `02e19bbe…` |
|---|---|
| `cargo fmt --all -- --check` | green |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | green, 0 warnings |
| `cargo run -p a2a-bridge -- validate --repo-hygiene` | green — 41 tracked artifacts, 9 example configs |
| `git diff --check` | green |
| `cargo test --workspace --locked --no-fail-fast --all-targets` | **4,855 passed, 0 failed, 13 ignored, 90 groups** |
| `cargo test --workspace --locked --no-fail-fast` (with doctests), run 1 | 4,865 passed, **1 failed**, 13 ignored, 106 groups |
| same command, rerun on the same bytes | **4,866 passed, 0 failed, 13 ignored, 106 groups** |

- **The one failure is unattributed.** Run 1 failed `liveness::tests::crashed_lease_file_persists_with_free_lock_probes_dead`
  (`liveness.rs:331`, `left: Some(false) right: Some(true)`). It does not reproduce: it passed in the `--all-targets`
  run, in the default rerun, in attempt 1's runs, and in 20 alternating `bridge-core` lib runs (10 on the candidate,
  10 on an exported base, none failed anything). I did not run a valid outcome control for the workspace. **Unproven
  hypothesis:** a `flock` held by a duplicated descriptor in a concurrently forked child, which the lock-release
  assertion cannot tell from a live holder. My new polling tests add process spawns to the same test binary, so they
  may raise the odds. I have no measurement that says they do.
- **Exclusions applied in my runs: none** — no `--exclude`, no `--skip`.
- **Verifier-profile exclusions, named.** `examples/a2a-bridge.containerized.toml` runs `cargo test --workspace --locked
  --no-fail-fast --exclude bridge-container -- --skip process::tests::terminate_reaps_child_no_zombie --skip
  process::tests::term_ignoring_loop_forces_group_sigkill --skip process::tests::drop_group_kills_descendants`. The
  skips are substring matches, so they also skip the current `…_host_signal_semantics` names. Here `bridge-container`
  and all three process tests ran and passed unexcluded.
- **Not run:** `cargo deny` (not installed), the controller's macOS lane, CI's native-ext4 lane.
- **No other crate depended on an unconfirmed successful `rm`.** Requiring an inventory after a zero exit broke no
  test outside `reaper.rs`.
- **Inherited macOS failure did not reproduce.** The task's host baseline (`b199` code: 4,831 / 1 / 13 / 90) had the
  expired-OAuth smoke test returning `acp.initialize.transport`; here it passed.
- **Test-count delta.** Against the base, the all-targets test names differ by exactly the 17 new `reaper::tests::…`
  names, none removed; `bridge-core` lib is 1,096 → 1,113. The base names come from an export whose outcomes are
  **inadmissible** (24 failures: those tests require the checkout under `/Users/wesleyjinks/code` in a real repo, and a
  bare `/tmp` export is neither), so I used it only to enumerate names and attributed nothing from it. The totals are
  not comparable with macOS's 4,832 because the platforms differ.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Base HEAD | `a25ddb01a9322f5994604f51c05c0378efe64965` (docs-only vs. the host baseline `b199bfd0`: 6 docs files) |
| Review head (attempt 1) | `607a9a1595a8303a62da2f163f8f27487e69754e` |
| Branch | `implement/impl-35818-c2c16khq` |
| Base `reaper.rs` SHA-256 | `a08228a6656c8a8d487e8e122f5ecbe47494ac8a1b3f824a901da4647d765f47` |
| Attempt-1 `reaper.rs` SHA-256 (the reviewed bytes) | `842056f9f57f6b0b9bf52b78a2623b6b5f410a9f7373cb626a2495f46c6a1645` |
| Round-1 RED, tests only, `reaper.rs` SHA-256 | `d3b81551468bd324d00e4e52534151a319d9ef2501653ea20dcd70bf7520bc8d` |
| Round-2 final `reaper.rs` SHA-256 (= `07d5b321` blob, the bytes §2–§5 are bound to) | `02e19bbe4d7f23efd4d5f611ebe6be823e345cb6f4c43e9d7858975f57e34730` |
| Round-2 HEAD (the artifact round 3 repairs) | `07d5b32125ff92804d5c320176ab7424c9ed18b1` |
| Round-2 handoff SHA-256 (this file before round 3) | `7635eb933637b5d1a90111973e6c9ae203f1fa43b4db409b0e7f05d602130acd` |
| Round-3 RED, tests only, `reaper.rs` SHA-256 | `66732216ed6543f98c11a8ca40358538906d5dfe7bc18927bdd07dc0627c3fa3` (lines 1–1301 byte-identical to `07d5b321`) |
| **Round-3 final `reaper.rs` SHA-256 (staged)** | `30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d` |
| Task SHA-256 | `a7dc9843aefb0843335c41958639444bb2b7889686150143e4c7d42fbd328e0e` |
| Task / resume ID | `impl-35818-c2c16khq` |
| Frozen checkpoint SHA-256 (terminal Rejected; untouched) | `39492f7ccc865fec327a4a27fd8cf63c486454dd1acbdd1c4cac4d2cd05930c0` |
| Staged paths | `crates/bridge-core/src/reaper.rs`, this file |
| This file's own SHA-256 | not embeddable; recorded in `.git/a2a-evidence/cleanup-round3/r3/FINAL-RECEIPT.txt` |
| Commit message for the controller | `.git/A2A_COMMIT_MSG` |
| Model | `claude-sonnet-5-5` (self-reported, unproven — §0 d) |
| Evidence, rounds 1–2 (container-local, **LOST** — §0 b) | had been `/tmp/a2a-evidence/`: `r2-red-reaper-tests.log`, `r2-reaper.red-tests.rs`, `r2-green-reaper-tests.log`, `r2-reaper.candidate.rs`, `r2-mutate.py`, `r2-mutation-matrix.log`, `r2-clippy.log`, `r2-hygiene.log`, `r2-workspace-all-targets.log`, `r2-workspace-default.log` (the failing run), `r2-workspace-default-rerun.log`, `flake-loop.log`, `env.sh`; attempt 1's files carried no `r2-` prefix |
| Evidence, round 3 (host-mounted, survives teardown) | `.git/a2a-evidence/cleanup-round3/` — index in §9.7 |

## 7. Interpretations the reviewer should check, and what remains

**Judgement calls — please challenge them:**

1. **Unsettled at the bound is `NonZeroExit` after a present sighting, `Timeout` without one.** This applies whether
   the bound stops the loop, interrupts an inventory child, or a stale absence is refused. Without the sighting rule
   the outcome would depend on whether a poll was in flight at the bound, which my own fixture exposed as a race.
2. **A zero exit with the object still present is `NonZeroExit`.** The code name reads wrongly for an exit status of
   zero. I added no variant because the `container.reap.*` registry is documented outside this task's Files
   (`docs/containerized-agents.md`, the operator skill) and `bridge-container` matches on it. It is `Retained`, not
   `Unknown`, downstream.
3. **An interrupted inventory is `Timeout`, not `IdentityUnavailable`.** It is the inventory's own typed cause, and
   `Timeout` is `Retained` for a protected v3 flight while `IdentityUnavailable` is `Unknown`.
4. **2 s window / 50 ms interval** are my numbers, not measured Docker timings; always clamped to the original
   deadline (10 s in production).
5. **Stale absence is refused.** `timeout_at` polls its inner future first, so an answer can arrive after the bound.
   The refusal is covered by a test that holds the only runtime thread past the bound.
6. **Deadline overflow refuses as `Timeout` before spawning.** Unreachable for the current constant callers.

**Residual risks:**

- **Inventory size, now on every removal (§0 e).** Unfiltered `ps --all --no-trunc` over 64 KiB is oversized and
  returns `IdentityUnavailable`, even after a successful `rm`. Before this change a successful removal never read the
  inventory. Mitigation not applied: an ID-scoped filter, which changes the shared inventory command.
- `parse_container_inventory_contains` also matches a *name* equal to the ID string. That can only over-report
  presence, so it never greens wrongly.
- When absence is confirmed, the managed flight journals `removed = true` with no failure code and cannot tell a
  removal by `rm` from one by `--rm`. Both are `Complete`.
- Every successful removal now costs one extra `container ps` call.
- The doc comment on `crate::sandbox::reap_argv` ("`rm -f` of a gone ID is a harmless error the caller ignores") is
  stale. I left it because `sandbox.rs` is outside the task's Files.

**Next action for the controller (round 3):** commit the staged tree with `.git/A2A_COMMIT_MSG`; snapshot
`.git/a2a-evidence/cleanup-round3/`; rerun the host gates on the final stable commit with nothing overlapping
(§0 i, §9.6); confirm the model identity; then send the commit to the independent Sol round-3 review with §9.8. Owner
decision resolved by the controller: retain the specified fail-closed 64 KiB bound and defer filter expansion (§0 e).

**§2c verdict:** UNREVIEWED — claim: "after the one exact-ID `rm -f`, cleanup settles successfully only when the
captured ID is confirmed absent from a fully validated runtime inventory inside one bound; every other outcome keeps a
typed, journaled failure, and a malformed inventory can neither settle absence nor be hidden behind a matching row" ·
pass: SELF (implementor) · evidence tier: EXECUTED (fixture runtimes, no provider or Docker daemon). Independent
confirmation is the next step; this handoff claims no approval.

## 8. Repair round 1 (Sol implementation review, round 1 — REJECT, 3 blockers)

### 8.1 The findings and the response

| # | Finding (paraphrased) | Response |
|---|---|---|
| 1 | A zero exit from `rm` is still trusted as proof of removal; a wrapper that exits zero and leaves the ID present yields `Complete`. | **Accepted.** Attempt 1 read "do not treat a successful exit code as proof of absence" as being about the inventory. The review's reading is the plainer one. Absence is now confirmed after either exit status (§3), with zero-exit/absent and zero-exit/present tests. This makes every removal depend on the inventory, which is §0 (e). |
| 2 | The 2 s window did not bound an inventory child: it got the full deadline remainder and `Ok(false)` was accepted without checking the window; a slow spawn could green after the absolute deadline. | **Accepted.** The child is bounded by an absolute `observe_until`, expiry is checked before absence is accepted, and the spawn is inside the bound. Tests: absence answered at 3 s under a 5 s deadline, and a stale answer polled after the bound. |
| 3 | Every non-timeout inventory error was collapsed to `NonZeroExit`, losing `Spawn` / `IdentityUnavailable` in the caller and the journal. | **Accepted.** Errors propagate with their own cause. I had judged the collapse safe; that was wrong, since `bridge-container` maps `IdentityUnavailable` to `Unknown` and the rest to `Retained`. Tests assert the returned code and the journaled `failure_code`. |

### 8.2 RED — attempt-1 production code, tests only

`[MEASURED]` `reaper.rs` SHA-256 `d3b81551468bd324d00e4e52534151a319d9ef2501653ea20dcd70bf7520bc8d`; the only diff
hunk against `607a9a15` starts in `mod tests` (`@@ -1900,0 +1901,4 @@`). Every assertion on the `rm` call count ran and
passed before the failing assertion, so each production call demonstrably started.

```text
test result: FAILED. 37 passed; 7 failed; 0 ignored; 0 measured; 1068 filtered out
a_managed_flight_journals_the_typed_cause_of_an_unconfirmable_removal   left: Err(NonZeroExit)  right: Err(IdentityUnavailable)
absence_first_observed_after_the_bound_is_never_accepted                left: Ok(())            right: Err(Timeout)
an_inventory_that_reports_absence_after_the_window_never_greens         left: Ok(())            right: Err(Timeout)
an_unspawnable_inventory_never_greens_and_keeps_its_typed_cause         left: Err(NonZeroExit)  right: Err(Spawn)
an_unusable_inventory_never_greens_and_keeps_its_typed_cause            left: Err(NonZeroExit)  right: Err(IdentityUnavailable)
successful_rm_completes_only_after_the_exact_id_is_confirmed_absent     left: 0                 right: 1   (ps calls)
successful_rm_with_a_still_present_object_never_greens                  left: Ok(())            right: Err(NonZeroExit)
```

Finding 1 is the last two rows, finding 2 the second and third, finding 3 the first, fourth, and fifth.
`a_managed_flight_journals_a_confirmed_absent_removal_as_complete` passed before and after (preservation).

### 8.3 GREEN, and one race the repair exposed

`[MEASURED]` `reaper::tests` 45 passed on the final bytes, and 8 further runs (5 parallel, 3 with
`--test-threads=1`) were all green.

- My first repair returned `Timeout` whenever an inventory child was cut off by the bound. That made
  `nonzero_rm_with_a_present_object_at_the_deadline_never_greens` racy: it failed once with `left: Err(Timeout)
  right: Err(NonZeroExit)`, depending on whether a poll was in flight at the bound. I added
  `a_present_sighting_followed_by_an_interrupted_inventory_never_greens` first and watched it fail on that
  intermediate code (`left: Err(Timeout) right: Err(NonZeroExit)`), then made the outcome depend on whether the
  object was ever seen present (§7, call 1). The log of the failing intermediate run was overwritten, so this
  paragraph is the only record of it.
- The final gates and totals are in §5 (round-2 bytes; round 3's are §9.6).

## 9. Repair round 3 (Sol review of `07d5b321` — one parser blocker, one SMELL; disclosed converging extension, 3/3)

Everything in §9 is `[MEASURED]` by the round-3 continuation unless tagged `[INHERITED]`. Every cargo command ran with
`CARGO_HOME=/cargo CARGO_NET_OFFLINE=true CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/target-r3 --locked --offline`, the
proxy variables unset, on Linux `7.0.14-orbstack` aarch64, uid 0, rustc/cargo 1.94.0, Git 2.54.0. `/cargo` is the
read-only warm cache; no dependency was fetched.

### 9.1 The findings and the response

| # | Finding (paraphrased from the round-3 task input — `[INHERITED]`, I did not read the review text) | Response |
|---|---|---|
| 1 | **BLOCKER.** `parse_container_inventory_contains` split each row with `split_once('\t')`, so a row with an extra tab read as a valid row with no match. It also returned `Ok(true)` on the first matching row, before later rows were parsed. A malformed inventory could therefore settle absence and `Complete`. | **Accepted.** Confirmed by RED (§9.3), repaired in the parser only (§9.2). |
| 2 | **SMELL.** The commit message described round 1. | **Accepted.** `.git/A2A_COMMIT_MSG` describes the final behavior. This session did not commit. |

### 9.2 The repair

`crates/bridge-core/src/reaper.rs`, `parse_container_inventory_contains` only (same signature, same public API, no
other runtime file touched). The contract, also written in its doc comment:

- Every non-empty row is exactly two tab-separated fields, `ID<TAB>NAMES`. A missing or extra field is refused.
- The ID, after stripping an optional `sha256:` prefix, is non-empty.
- `NAMES` is either empty or a comma-separated list. An empty `NAMES` field is valid (`[INHERITED]` controller
  clarification, §0 h): a runtime can release an object's names before the object leaves the inventory. No name alias
  matches it, and the exact ID still decides presence. An empty element, or a bare `/`, inside a non-empty list is
  refused.
- **The whole inventory is validated before any answer**: presence is accumulated with `|=`, so a matching row can
  never hide a malformed row before or after it, and a malformed row can never let the selector read as absent.
- Unchanged: invalid UTF-8 refuses the whole inventory; blank lines are skipped; CRLF and a leading `/` on a name are
  accepted; ID and name matching semantics for valid rows, including the `sha256:` normalization.
- Not in the parser, and unchanged: the output limit (`take(64 KiB + 1)` plus a length check in
  `runtime_inventory_contains_until`), the exit-status check, the deadline/window, one `rm`, and the typed causes.

The complete malformed population at the parser seam, with the round-2 parser's answer taken from the RED run (§9.3),
for a selector that is otherwise present or absent:

| Class (test row) | Round-2 parser, alone | Round-2 parser, beside a match | Round-3 parser |
|---|---|---|---|
| missing tab `other` | refused | **`Ok(true)` when after the match** (refused before it) | refused |
| extra field `other⇥name⇥extra`; trailing tab `other⇥name⇥`; empty middle field `other⇥⇥name`; many fields; empty names + trailing tab `other⇥⇥` | **`Ok(false)`** | **`Ok(true)`** | refused |
| empty ID `⇥name`; empty ID and names `⇥`; bare `sha256:` ID | **`Ok(false)`** | **`Ok(true)`** | refused |
| empty list element `a,,b`; leading `,a`; trailing `a,`; only `,` | **`Ok(false)`** | **`Ok(true)`** | refused |
| bare slash `/`; bare slash after a name `a,/` | **`Ok(false)`** | **`Ok(true)`** | refused |
| invalid UTF-8, anywhere in the stream | refused | refused | refused (unchanged) |
| empty `NAMES` with a non-empty ID (valid) | accepted | accepted | accepted, ID decides; **an empty selector no longer matches it** (round-2: `Ok(true)`) |
| over-limit output | refused by the caller | refused by the caller | refused by the caller (unchanged) |

**Effects on the parser's other callers (fail-closed, no code change outside `reaper.rs`):**

- The identity probe after a failed `inspect` (`observe_container_identity`): a malformed inventory used to read as
  `AlreadyGone`, and `reap_observed` then returned `Ok(())` without removing anything. It is now `IdentityUnavailable`.
  Covered by `a_malformed_inventory_keeps_missing_selector_classification_unknown`, which failed on all 14
  refusable classes at RED.
- `operator_observe_container_identity` (`bin/a2a-bridge/src/main.rs:6982`): `Err(_)` already mapped to `Unknown`, so a
  malformed inventory changes from `Absent` to `Unknown`. That file is outside this task's Files. **No dedicated test
  covers that mapping;** it is exercised only by the full-workspace runs.

### 9.3 Hypothesis, and the behavioral RED

**Stated before any probe.** Cause: `split_once('\t')` folds any extra tab into `NAMES`, and the first match returns
before later rows are parsed, so a malformed inventory can read as absence or as presence. Expected on `07d5b321`:
the parser tests return `Ok(false)` / `Ok(true)` where a refusal is required, and `remove_container_id` returns
`Ok(())` after one inventory with the `rm` recorded once. Falsifying observation: the old parser already refusing
these inputs, or `remove_container_id` failing for another reason. Alternate mechanism: the caller's read, size, or
exit handling.

**RED.** `reaper.rs` SHA-256 `66732216ed6543f98c11a8ca40358538906d5dfe7bc18927bdd07dc0627c3fa3`. Its production region
(lines 1–1301) is byte-identical to `07d5b321` (`diff` clean); every change is inside `mod tests`. Command:
`cargo test -p bridge-core --lib --locked --offline reaper::tests -- --test-threads=1`. Log `red-reaper-tests.log`.

```text
test result: FAILED. 49 passed; 7 failed; 0 ignored; 0 measured; 1068 filtered out
a_malformed_inventory_never_settles_absence_after_either_exit_status   28 rows: Ok(()) after 1 inventories
                                                                       (14 classes x nonzero and zero exit)
a_malformed_inventory_keeps_missing_selector_classification_unknown    14 classes: Ok(()) (already gone)
a_managed_flight_journals_a_malformed_inventory_as_unavailable_never_complete
                                                                       3 of 4 rows: returned Ok(()), journaled removed=true,
                                                                       settled Complete; 1 row: NonZeroExit, not IdentityUnavailable
a_matching_row_cannot_hide_a_malformed_row_from_the_removal            3 rows: Err(NonZeroExit) after 35 inventories
a_present_sighting_followed_by_a_malformed_inventory_never_greens      left: Ok(())  right: Err(IdentityUnavailable)
runtime_inventory_parser_refuses_every_malformed_row_wherever_it_appears   every class listed in §9.2 accepted
runtime_inventory_parser_allows_an_empty_names_field_that_only_the_id_can_match  "": left Ok(true) right Ok(false)
```

The RED is behavioral and admissible: every `remove_container_id` and managed-flight test asserts
`rm_calls().len() == 1` before its result assertion (all passed, so the production call demonstrably started), and the
identity-probe test asserts that both the `inspect` and the `ps` fixture calls were logged. The same fixtures and exit
statuses pass the preservation tests, and the failing value is the false absence or the hidden malformed row the
hypothesis predicts. The falsifier did not fire and the caller's read/size/exit handling is ruled out, because the
same inventories produced `ps_calls() == 1` and exit 0. The 49 passing tests include the preservation tests: valid presence, valid absence,
invalid UTF-8, an empty `NAMES` field observed as present through the production path, and the 45 round-2 tests.

### 9.4 Tests added (11) and changed (1) — `reaper::tests` went from 45 to 56

| Test | Proves |
|---|---|
| `runtime_inventory_parser_refuses_every_malformed_row_wherever_it_appears` | 15 classes × 6 positions (alone, after an unrelated row, before / after the ID match, after the name match, last with no newline) |
| `runtime_inventory_parser_refuses_invalid_utf8_anywhere` | 4 streams × 3 selectors |
| `runtime_inventory_parser_keeps_valid_presence_and_absence` | ID with and without `sha256:`, every name, blank lines, CRLF, leading `/`, empty inventories |
| `runtime_inventory_parser_allows_an_empty_names_field_that_only_the_id_can_match` | the clarification, including that an empty selector matches nothing |
| `a_malformed_inventory_never_settles_absence_after_either_exit_status` | `remove_container_id`, 14 refusable classes × nonzero and zero `rm`: `IdentityUnavailable` after exactly one inventory |
| `a_matching_row_cannot_hide_a_malformed_row_from_the_removal` | malformed row after / before the captured ID, and after an empty-names row |
| `a_present_sighting_followed_by_a_malformed_inventory_never_greens` | sighting, then a malformed poll: `IdentityUnavailable` after 2 inventories |
| `an_empty_names_field_still_observes_the_exact_id_as_present` | two sightings by ID alone, then absence: `Ok(())`, one `rm`, 3 inventories |
| `an_empty_names_field_with_the_id_present_never_greens` | empty names, ID present to the window end: `NonZeroExit` |
| `a_malformed_inventory_keeps_missing_selector_classification_unknown` | identity probe after a failed `inspect`: `IdentityUnavailable`, no `rm` |
| `a_managed_flight_journals_a_malformed_inventory_as_unavailable_never_complete` | returned **and journaled** `identity_unavailable`, no `removed=true`, settles `Failed`, never `Complete` |
| *(changed)* `an_unusable_inventory_never_greens_and_keeps_its_typed_cause` | its oversized case is now 3,500 whole 20-byte valid rows that never name the captured ID |

The populations use one shared `MALFORMED_INVENTORY_ROWS` table and collect every mismatch before asserting, so the
RED and any regression show the whole population, not the first failing row. No test file, config, or other source was
touched. Every fixture test takes `production_runtime_fixture_permit()` and warms its runtime up first, as before.

### 9.5 GREEN and the mutation matrix

**GREEN.** `reaper.rs` SHA-256 `30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d`. The same command:
**56 passed, 0 failed**. Four further runs (three at default parallelism, one more serialized) were also 56 / 0. The
diff from the RED source is the parser hunk only.

**Mutation matrix.** One foreground pass, one mutant at a time; the file was restored from the candidate snapshot in a
`finally` and hash-checked after every mutant (`cmp` clean at the end). Script `mutate.py`, log `mutation-matrix.log`.

| # | Mutant | Verdict | Tests that failed |
|---|---|---|---|
| P1 | `split_once`: an extra tab folds into `NAMES` again | FLIPPED | 6: the parser population, all four production/identity/managed regressions, sighting-then-malformed |
| P2 | the first match returns `Ok(true)` before later rows | FLIPPED | parser population, matching-row-cannot-hide, managed flight |
| P3 | an empty ID accepted | FLIPPED | 5 |
| P4 | an empty list element skipped | FLIPPED | 3 |
| P5 | an empty `NAMES` field refused (the superseded harness candidate) | FLIPPED | 3: both empty-names production tests and the empty-names parser test |
| P6 | an empty `NAMES` field matches an empty selector | FLIPPED | the empty-names parser test |
| P7 | last row wins, not any row | FLIPPED | 4 |
| P8 | `sha256:` no longer stripped from the row ID | FLIPPED | 6 |
| P9 | leading `/` no longer stripped from names | FLIPPED | 6 |
| P10 | blank lines are rows | FLIPPED | the valid-inventory test |
| P11 | invalid UTF-8 replaced lossily | FLIPPED | the invalid-UTF-8 test |
| P12 | the inventory output limit removed | FLIPPED | **only** `an_unusable_inventory_never_greens_and_keeps_its_typed_cause`, on `oversized inventory of valid rows` |
| P13 | the inventory exit status ignored | FLIPPED | 3 |
| P14 | a malformed inventory collapsed to `NonZeroExit` in the confirmation loop | FLIPPED | 6 |

**Control for P12.** With the limit removed and the *old* 70,000-`a`-byte fixture put back, that test passes (`1
passed`, `control-old-oversized-fixture.log`). The old fixture never guarded the output limit, because the parser
refused its tab-less bytes anyway. The valid-row fixture does: the bounded read stops 17 bytes into row 3,277, after its
tab, so without the limit the parser accepts the truncated tail as a valid absence.

**Not re-run:** the round-2 matrix N1–N15 (§4). Its script and logs were container-local and are gone, and round 3 did
not touch the code those mutants exercise. It stays an `[INHERITED]` receipt bound to `02e19bbe…`.

### 9.6 Gates on the final bytes, and the flaky tests

All on `reaper.rs` `30860ae0…` except the base control, which is labelled. The hash was recorded at the start and end
of the gate script and around every attribution run, nothing else was running, and nothing was excluded or skipped
(`--exclude` / `--skip` unused).

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | green |
| `cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings` | green, 0 warnings |
| `cargo run -p a2a-bridge --locked --offline -- validate --repo-hygiene` | green — 41 tracked artifacts, 9 example configs (re-run on the final staged tree: `FINAL-RECEIPT.txt`) |
| `git diff --check` | green |
| `cargo test --workspace --locked --offline --no-fail-fast --all-targets`, run 1 | **4,864 passed, 2 failed**, 13 ignored, 90 groups |
| same, run 2 | **4,865 passed, 1 failed**, 13 ignored, 90 groups |
| same, run 3 | 4,866 passed, 0 failed, 13 ignored, 90 groups |
| same, run 4 | 4,866 passed, 0 failed, 13 ignored, 90 groups |
| `cargo test --workspace --locked --offline --no-fail-fast` (default + doctests) | 4,877 passed, 0 failed, 13 ignored, 106 groups |
| **Base control**: the same tree with `reaper.rs` swapped for `a25ddb01`'s (`a08228a6…`), all-targets | 4,838 passed, 0 failed, 13 ignored, 90 groups |

Totals reconcile: 4,866 all-targets tests = 4,838 base + 28 new `reaper::tests` (28 → 56, none removed); 4,877 −
4,866 = 11 doctests. The round-2 receipt (§5: 4,855 all-targets, 4,866 default) is the same 11-test delta.

**Three failures in the counted runs, three different tests — none reaches the changed parser, none reproduces standalone:**

| Run | Failed test | Signature |
|---|---|---|
| all-targets 1 | `bridge-api` `backend::tests::settlement_refusal_does_not_mask_the_provider_failure` | `provider request count was not reached: Elapsed(())` |
| all-targets 1 | `a2a-bridge` `cli_tests::production_history_routes_cover_selected_and_configured_branches` | `killed route child was not reaped: Timeout` (a 15 s wait, then 5 s) |
| all-targets 2 | `bridge-core` `reaper::tests::nonzero_rm_with_a_hung_inventory_times_out_at_the_shared_deadline` | `the inventory must have started`, `left: 0 right: 1` |

- **`reaper` failure.** The test is a round-2 test, unchanged by round 3. Its `rm` fixture sleeps 1 s of a 1.5 s
  deadline, so the inventory child must spawn and log inside the last 0.5 s; `ps_calls() == 0` means the inventory never
  started, before any inventory byte was parsed. I did not reproduce it, so the **mechanism is unproven**. What was
  measured: 10/10 passes in isolation; 0/6 under 100 busy-loop processes on 15 cores (the pure CPU-starvation
  hypothesis did not reproduce it, and a stall or spawn delay of about 0.5 s is only consistent with the signature, not
  proven); 0 failures in 23 undisturbed full parallel `bridge-core` lib runs with a diagnostic variant of only that test
  (assertion text and timestamps); and **0 failures in 22 full parallel lib runs on the round-2 bytes `07d5b321`**
  (the fair control: it has this test and none of round 3). On the exact candidate bytes the full `bridge-core` lib
  ran 5 times inside the workspace runs (all-targets runs 1–4 and the default run): run 2 failed, the other 4 passed.
  With the 23 diagnostic-variant runs that is 1 failure in 28 full lib runs against 0 in 22 for the control, a
  difference that is not significant at that rate.
- **`bridge-api` and `a2a-bridge` failures.** They live in crates whose tests never call the parser. Standalone they
  passed 10/10 in isolation and 30/30 (`bridge-api` lib) and 8/8 (`a2a-bridge` bin) as whole-binary repeats on the
  candidate bytes.
- **A disturbed run I caused, excluded.** Run 24 of the diagnostic loop failed two `sandbox::tests`
  (`credential_destinations_require_the_declared_source_type`, `missing_ordinary_extra_bind_is_mount_not_credentials`,
  class `Unknown` where a class was expected). I had launched the control script while that loop was still running, so
  two cargo jobs and two writers on `reaper.rs` overlapped it. I killed the control script and its orphaned cargo child
  and test binary (my kill command also matched and ended my own shell, which was harmless), then confirmed that no
  cargo or test process remained and that `reaper.rs` was back to the candidate bytes (`cmp` clean). That run is
  excluded from every count above and is not evidence in either direction; the aborted control output is kept,
  labelled inadmissible, in `flake/ABORTED-controls-raced-with-diag-INADMISSIBLE/`, and the controls were re-run
  cleanly behind a guard that refuses to start unless the file is the candidate.
- **What this supports and does not.** The final bytes have two clean all-targets passes and a clean default-plus-doctest
  pass, and the two red runs failed only tests that do not reach the parser. I have **no proof** those tests are flaky
  rather than sensitive to something this container does under a long workspace run, and the base control passing once
  is not proof either. This is the same class as round 2's unattributed `flock` failure (§5). Treat the controller's
  host rerun on the final commit as the deciding evidence, and treat a red timing test there as unattributed until it has
  a same-environment control.
- `[INHERITED]` unchanged verifier-profile exclusions and the "Not run" list from §5 (cargo deny, the controller's macOS
  lane, CI's ext4 lane) still apply. No provider smoke, replay, or billable turn ran.

### 9.7 Identifiers and evidence

| Item | Verbatim |
|---|---|
| Base | `a25ddb01a9322f5994604f51c05c0378efe64965`, base `reaper.rs` `a08228a6656c8a8d487e8e122f5ecbe47494ac8a1b3f824a901da4647d765f47` |
| HEAD repaired | `07d5b32125ff92804d5c320176ab7424c9ed18b1`, `reaper.rs` `02e19bbe4d7f23efd4d5f611ebe6be823e345cb6f4c43e9d7858975f57e34730` |
| RED source | `66732216ed6543f98c11a8ca40358538906d5dfe7bc18927bdd07dc0627c3fa3` |
| **Final `reaper.rs`** | `30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d` |
| Handoff before round 3 | `7635eb933637b5d1a90111973e6c9ae203f1fa43b4db409b0e7f05d602130acd` |
| Task / checkpoint | `a7dc9843…0e` (task `impl-35818-c2c16khq`) · `39492f7c…0c0` (terminal Rejected, untouched) |
| Model | `claude-sonnet-5-5`, self-reported, unproven (§0 d) |
| Staged | `crates/bridge-core/src/reaper.rs` and this file only; nothing committed, pushed, or merged |
| Commit message | `.git/A2A_COMMIT_MSG` |

Evidence, all under `.git/a2a-evidence/cleanup-round3/` (host-mounted): `ROUND3-STATUS.md`, `blocker-diagnostics.log`,
`parser-harness/`, `parser-fix.patch` (the earlier blocked session's harness, superseded by §9.2 — the patch rejected an
empty `NAMES`); and under `r3/`: `env.sh`, `reaper.base-07d5b321.rs`, `reaper.base-a25ddb01.rs`,
`reaper.red-tests.rs`, `reaper.candidate.rs` (+ `.sha256`), `red-reaper-tests.log`, `RED-SUMMARY.txt`,
`green-reaper-tests.log`, `green-repeat-*.log`, `mutate.py`, `mutation-matrix.log`, `mutants/P1..P14.log`,
`mutants/CONTROL-P12-old-fixture.log`, `control-old-oversized-fixture.log`, `gates.sh`, `gates/` (`SUMMARY.txt`,
`00-env.txt`, fmt, clippy, workspace all-targets, default, hygiene), `attribution.sh`, `attribution2.sh`,
`attribution/` (runs 2–4, the base control, isolation runs), `flake/` (`stress.sh`, `diag.py`, the diagnostic loop,
`controls.sh`, `controls/`, the quarantined `ABORTED-…INADMISSIBLE/`), and `FINAL-RECEIPT.txt`.

### 9.8 Judgement calls the reviewer should check, and what remains

1. **One malformed row anywhere makes the whole inventory unusable**, even a row unrelated to the selector. The price
   is fail-closed: any unexpected stdout line, header, or warning from a runtime now yields `IdentityUnavailable` (an
   `Unknown` disposition for a protected v3 flight) where the old parser might have produced a wrong `Complete`. Docker
   and Podman container names cannot contain a tab or a comma, and IDs are hex, so a well-formed inventory never
   trips it. I have no live Docker or Podman output to confirm that.
2. **Empty `NAMES` is accepted** per the controller's clarification. I recall, unverified, that a runtime can release
   a name before the row leaves `ps --all`; I had no daemon to measure it.
3. **No character whitelist** on IDs or names. The runtime defines the charset, and matching stays exact. A row whose
   ID field is formatted unusually can only under-match; that property is unchanged from round 2.
4. **The parser has no size limit.** The bound stays at the two callers that read a bounded stream. The operator
   observer reads whole stdout, so a very large but complete, valid inventory still answers correctly there; only a
   truncated one could give a false absence, and truncation happens only behind the caller's limit.
5. **`str::lines` semantics are unchanged:** a `\r` before `\n` is stripped, a lone `\r` inside a field is not
   rejected.
6. **Flaky timing test left as written.** `nonzero_rm_with_a_hung_inventory_times_out_at_the_shared_deadline` has a
   0.5 s margin. Round 3's scope is the parser, the failure signature is upstream of it, and I could not reproduce it
   to write a RED, so I did not change it. An unapplied option: a 3 s deadline with a 3.6 s bound.
7. **Residual from §7 stays open:** the 64 KiB inventory exposure on every removal (§0 e), and the stale
   `reap_argv` doc comment in `sandbox.rs`.
8. **Process incident, disclosed:** the overlapped launch in §9.6 is mine; it invalidated one loop run and one control
   attempt, both excluded and both kept in the evidence directory.

**Next action for the controller:** see §7 "Next action (round 3)". **§2c verdict:** UNREVIEWED — SELF (implementor),
EXECUTED tier (fixture runtimes, no provider or Docker daemon); no approval is claimed.


## 10. Controller custody checkpoint

The authorized worker completed before controller edits. Production `reaper.rs` remains SHA-256
`30860ae070746c5de9b9a85469f8e07dfd67e94f9da7183c5469c98f281aab2d`. The controller only reconciled
provenance and orchestration statements in this handoff. A second complete round-3 evidence copy now exists
at `/private/tmp/adr0041-2b3b-20260929/cleanup-round3-evidence-final`.

Controller closure supersedes the earlier pending-review state: independent Sol approved clean source
`434905a231b662a7c152eaf27bf5d2b0ce64dbdd`, wrong=0/smell=0/blockers=0, round3/3. Receipt:
`2026-09-29-container-auto-removal-review-round3.md`. The original automatic checkpoint stays Rejected.

Controller macOS gates executed outside the sandbox on that stable source: all targets **4860/0/13**
(90 groups), default/doctests **4871/0/13** (106 groups), fmt, warnings-denied clippy, hygiene all passed.
The source hash and HEAD were unchanged after the complete run. No tests were excluded; direct ambient
OAuth token and proxy variables were unset. Actual executed reaper population: 56 tests.

The first host attempt had 251 sandbox-refused failures per suite and is inadmissible for source behavior.
The first base control reused the candidate test artifact through the shared Cargo target directory;
its 4860/0/13 result is also inadmissible as base evidence. The fresh real clone at exact a25ddb01 then
rebuilt bridge-core and passed **4832/0/13**, with the original 28 reaper tests and no candidate-only names.
Receipts: `cleanup-host-candidate-approved/summary.json`, `cleanup-host-fresh-base/summary.json`, and
invalidation notes under `cleanup-host-final` / `cleanup-host-base-approved`, all beneath the scratch root.

The controller imported the exact reviewed two-file delta and published PR128 at `25e63468`, with the
exact production hash above. CI and landing remain pending; 2B3b Git-plane work has not started. Named parallel Linux timing failures remain
unattributed receipts; serial host success does not prove their cause. Inventory/filter expansion and
the out-of-scope stale sandbox comment remain deferred.
