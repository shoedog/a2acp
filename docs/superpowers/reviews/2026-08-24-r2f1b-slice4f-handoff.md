# R2f1b slice 4F handoff: durable fixed-grace timer (gated)

## What changed

- `resolve_execution_policy_with_readiness_v1` now refuses `FixedGrace` when the frozen
  `deadline_activation` is `ManualOnlyR2f1a` and admits it only when that frozen value is
  `AutomaticR2f1b`. The zero and effective-work-cutoff bounds remain after the gate and therefore
  apply to the newly admitted path.
- `bridge_core::fixed_grace_timer` adds a pure, clock-fed durable state machine with private
  `Unarmed`, `Armed`, and `Fired` storage. `arm` and `observe_elapsed` accept supplied monotonic
  offsets; the module reads no clock and starts no timer.
- A second arm is refused while armed, firing moves the durable state to `Fired`, and both a second
  expiry observation and re-arming after fire are refused. The refusal is explicit rather than a
  typestate compile-time prohibition.
- Expiry returns the existing `PolicyTriggerV1` vocabulary with the caller-supplied
  `ControlEventIdV1`, the frozen node reference, `FanOutPolicyNameV1::FixedGrace`, and the exact
  grace value. Two independently supplied trigger ids remain distinct.
- The timer carries the sibling's recorded node deadline unchanged through both durable states.
  Expiry returns only the policy trigger; it has no deadline mutation surface.
- The public deserializer validates every durable state before construction: current schema,
  nonzero/non-overflowing grace, a valid fixed-grace trigger, and a fired timestamp at or after the
  immutable expiry. Invalid persisted bytes therefore cannot bypass `arm` and emit a malformed
  trigger.
- Armed and fired canonical bytes plus every rejected state fixture are hand-written literals; no
  fixture or expectation generator was used.

## Required regression matrix

- `fixed_grace_admission_and_shipped_refusal_are_gated_by_frozen_activation` covers unchanged
  `(Disarmed, Production)` refusal, `AutomaticR2f1b` admission, and the standing production refusal
  gate through the public resolver. The same test covers both `grace_ms == 0` and
  `grace_ms > effective_work_cutoff_ms()` on the admitted path.
- `timer_arms_once_and_fires_once_without_renewal` covers the refused second arm, pending edge,
  inclusive expiry, refused second fire, refused post-fire re-arm, existing trigger vocabulary,
  exact grace, caller-owned non-colliding ids, and the exact unchanged `7_200_000` ms deadline.
- `timer_state_accepts_canonical_and_rejects_invalid_literal_wire_bytes` covers literal Armed and
  Fired canonical bytes, reconstruction of the Fired one-shot state, and literal rejections for a
  non-current schema, zero grace, overflowing grace, an inconsistent Fired trigger, and a Fired
  timestamp before expiry.
- The existing R2f1a policy regression now also asserts that a manual-only frozen activation
  refuses fixed grace rather than bypassing the frozen activation gate.

## Red-first evidence

The focused test target was added before the production module or export. With the prescribed
offline cache environment, this exact target exited 101 because
`bridge_core::fixed_grace_timer` was an unresolved import:

```text
cargo test -p bridge-core --test r2f1b_slice4f_fixed_grace --locked
```

The required named tests therefore could not execute on the pre-change production tree. This is the
same target-level RED-first form used by slices 4D and 4E. The final compact test file was also
overlaid onto a fresh disposable archive of the exact base and compiled from that archive with its
own target directory; the compiler again failed on the absent module before any named test ran.
After implementation the target passes
3 / 0 / 0.

## Frozen single-mutation control

- Patch: `docs/superpowers/reviews/2026-08-24-r2f1b-slice4f-mutation-control.patch`
- SHA-256: `bbae605d6e0955612d22ff613b631bbcd42c0e01c751d342a70993f28a44f7d8`
- Production mutation: change exactly the arming-state guard so it refuses only `Armed`, allowing a
  `Fired` timer to become `Armed` again.
- The patch applied cleanly to the candidate, passed
  `cargo clippy --all-targets --all-features --locked -- -D warnings`, reversed cleanly, and still
  applies cleanly after reversal.

The complete `cargo test --workspace --all-targets --all-features` candidate suite had an empty
failure set. The otherwise identical mutant suite ran with `--no-fail-fast` so every target
completed. Its actual mutant-minus-candidate red population is exactly one test in one target:

- `bridge-core --test r2f1b_slice4f_fixed_grace`:
  `timer_arms_once_and_fires_once_without_renewal`

Reversal restored the focused candidate to 3 / 0 / 0.

## Verification

All valid evidence runs used `CARGO_HOME=/cargo`, `CARGO_NET_OFFLINE=true`,
`CARGO_INCREMENTAL=0`, explicit
`RUSTDOC=/usr/local/rustup/toolchains/1.94.0-aarch64-unknown-linux-gnu/bin/rustdoc`, and localhost in
both proxy-exclusion variables.

- `cargo fmt --all -- --check`: green.
- `cargo check --workspace --locked`: green.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: green for both candidate and
  mutant.
- `cargo build --locked`: green.
- `cargo test --workspace --all-targets --all-features`: green on the candidate; the no-fail-fast
  mutant run completed with only the named one-shot regression red.
- `cargo run -p a2a-bridge --locked -- validate --repo-hygiene`: green; 40 tracked artifacts and 8
  validated example configs.
- `git diff --check`: green.

Excluded diagnostic attempts:

- The first RED-first command used the default Cargo home, attempted registry access, and failed on
  the injected proxy with a 403 before compilation. It is not RED-first evidence.
- An offline retry included an unnecessary `/cargo/git` directory precondition and exited before
  Cargo because that directory is absent. It is not test evidence.
- Two early full-suite attempts used the short command wrapper and ended without terminal test or
  doc-test status. Their partial logs are not gate evidence; the persistent-session candidate and
  mutant runs above replace them.

## Deliberate exclusions and stop boundaries

- `crates/bridge-workflow/src/executor.rs` is byte-identical to the base, including the bare
  `inflight.next().await` path. The timer is not wired into scheduler arbitration or executor code.
- Production readiness still ships `Disarmed`, so the public production resolver still freezes
  `ManualOnlyR2f1a` and refuses fixed grace exactly as before.
- No production caller arms the timer. No clock read, spawn, sleep, `select!`, cancellation token,
  cancellation effect, provider effect, or deadline rewrite was introduced.
- Progress epochs and warnings, eight-arm integration, issue #22 closure, and production arming
  remain slices 4G through 4J.
- `MAX_WORKTREE_CONFIGURES_IN_FLIGHT`, all manifests, and `Cargo.lock` are untouched.

## Counted size

The complete formatted change adds **300 nonblank physical Rust lines**: 14 additions in tracked
Rust files and 286 across the new timer module and regression target. This is exactly the 300-line
stop boundary. This handoff and the frozen mutation patch are non-Rust and
excluded from the count.
