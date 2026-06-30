# E8a — Named Prompt Registry — RESUME HANDOFF (post-reboot / new session)

> **Read this first when resuming.** Work was interrupted mid-implementation by an environmental macOS
> `syspolicyd` (Gatekeeper) CPU storm that hangs the final `a2a_bridge` codegen/link — NOT a code problem.
> Everything is committed to git; resume is fast.

**Date:** 2026-06-28
**Branch:** `feat/e8a-named-prompt-registry` (off `main` `cfb5431`, after E7)
**Cwd:** `/Users/wesleyjinks/code/a2a-bridge`

---

## 0. The blocker that stopped us — DIAGNOSED (root cause + remediation ladder)

**Symptom:** `rustc` freezes FOREVER at the codegen→link boundary of the FINAL `a2a_bridge` unit
(`rustc` cputime crawls then sticks at a fixed value = **0% CPU = blocked on a syscall**, NOT computing;
build sticks at `303/304`, test at `319/326`). `syspolicyd` (PID-of-the-boot) spikes to **30–40% CPU**
during the build and has burned **156 min cumulative CPU** this boot.

**ROOT CAUSE (confirmed 2026-06-29):** the macOS **exec-assessment database is wedged.**
`/var/db/SystemPolicyConfiguration/ExecPolicy` is a SQLite db (proof: `ExecPolicy-wal` / `ExecPolicy-shm`
sidecars) that syspolicyd hits on the **first execution of every newly-built executable**. Its **WAL is
runaway and never checkpoints** — observed growing **4.1 MB → 5.9 MB with NO build running** (stuck at /
past SQLite's 1000-page auto-checkpoint threshold; a long-lived reader is blocking the checkpoint). Every
`cargo build` spawns dozens of fresh executables (build scripts, proc-macro dylibs, the **linker**) → each
first-exec thrashes the wedged db → the linker `execve` blocks → rustc freezes.
- **Survives reboot** because the WAL is on-disk and isn't cleanly checkpointed at shutdown (user rebooted
  twice — did NOT clear it). This is why "just reboot" is WRONG.
- **NOT an XProtect issue:** XProtect is current + healthy (bundle `5347`, installed 2026-06-03, scans
  enabled). Ruled out.
- **Was fine earlier same session** (288 tests ran ~50 min before) → the WAL wedged mid-session.

**Why the obvious fix is blocked:** the targeted fix is to move the `ExecPolicy*` trio aside so syspolicyd
rebuilds a fresh db — but `/var/db/SystemPolicyConfiguration/` is **SIP-protected** (`restricted` flag +
`csrutil status: enabled`), so **`sudo mv` fails with `Operation not permitted`.** Can't touch it while
SIP is on.

**FIX IN PROGRESS (2026-06-29):** installing **macOS 26.5.2** (was on 26.5.1 `25F80`; `26.5.2-25F84` was
pending). The OS updater rebuilds SIP-protected security dbs on a blessed path + reinitializes the security
subsystem on a fresh boot → should clear the wedged WAL. **If this handoff is being read, the update may NOT
have cleared it — work the ladder below.**

### Remediation ladder (do in order; stop when a build links without freezing)

1. **Re-confirm state after the update.** A build is unblocked iff syspolicyd stays low during compile AND
   the WAL is small:
   ```
   ls -laO /var/db/SystemPolicyConfiguration/ExecPolicy*      # ExecPolicy-wal should be SMALL or absent
   ps aux | grep '[s]yspolicyd' | awk '{print $3"%", $10}'    # cumulative TIME reset on fresh boot
   ```
   Then try the build (§2). If it links → blocker gone, proceed with E8a. If it freezes again at the final
   `a2a_bridge` unit (rustc cputime sticks at a fixed value, 0% CPU) → the WAL is still wedged; continue.

2. **Recovery-mode reset (guaranteed, targeted — needs the user; ~4 reboots).** This removes the SIP block:
   - Shut down. On Apple Silicon: hold the **power button** until "Loading startup options" → **Options** →
     Continue → pick admin user + password.
   - **Utilities → Terminal:** `csrutil disable` → `reboot`.
   - Back in macOS, real terminal:
     ```
     sudo mv /var/db/SystemPolicyConfiguration/ExecPolicy{,.bak}
     sudo mv /var/db/SystemPolicyConfiguration/ExecPolicy-wal{,.bak}
     sudo mv /var/db/SystemPolicyConfiguration/ExecPolicy-shm{,.bak}
     sudo reboot
     ```
     (move ALL THREE together — leaving an orphan `-wal` corrupts the rebuild; moving aside is reversible.)
   - syspolicyd rebuilds a fresh empty `ExecPolicy` on boot. Verify build links.
   - Re-secure: boot to Recovery again → Terminal → `csrutil enable` → `reboot`. (Leaving SIP off is a
     security regression — re-enable once builds are confirmed healthy.)

3. **Last-resort to SHIP TODAY (NON-durable, uncertain).** `sudo spctl --master-disable` (on macOS 26 it
   also needs System Settings → Privacy & Security → "Allow applications from: **Anywhere**", which only
   appears after running the command). **Caveat:** `spctl` governs **quarantine-based Gatekeeper**, which is
   a DIFFERENT mechanism than the ExecPolicy exec-assessment that's actually wedged — it may NOT unblock the
   build. Try it only if Recovery isn't available; `sudo spctl --master-enable` immediately after.

**The deeper "why now":** likely a background security-data update mid-session left a long-lived reader on
the ExecPolicy db, blocking WAL checkpoint; the WAL then grew unbounded and every exec-assessment degraded.

**Separately — this machine OOM-stalls parallel `cargo`:** always build/test/clippy the bin crate with
**`-j 1`** (one rustc ≈ 800 MB; parallel rustc exhaust swap → all stall at 0% CPU in `S` state — a DIFFERENT
failure than the syspolicyd freeze above, same 0%-CPU symptom). See E7's gotcha. Reserve `-j 2` for small
crates only.

---

## 1. EXACT STATE — what's committed, what's WIP

Branch commits (newest first):
- `74606bf` **WIP(e8a): T6 + T7-T9 prompt CLI — UNVALIDATED** ← the code that never got to compile/test
- `0202a02` feat(config): T2-T5 — `[[prompts]]` registry + node resolution at the `load_workflows` seam ✅ green
- `7a608ab` feat(core): T1 — `PromptId` newtype (permissive `/ _ - .` + `Ord`) ✅ green
- `36090b5` plan v3 · `717cfec` plan v2 · `451a185` plan v1 · `bbe4cd3` spec v3 · `2794dc2` spec v2 · `e6c82eb` spec v1

**Validated + committed:** T1 (bridge-core), T2–T5 (config.rs) — all tests green; **288 bin tests passed
(back-compat confirmed: `prompt_file: String→Option` broke nothing).**

**WIP / UNVALIDATED (in `74606bf`):**
- **T6** `config::parse_prompts_only` + test `prompt_only_parse_ignores_unrelated_sections`.
- **T7** `prompt_list_lines` + test `prompt_list_sorts_ids_no_file_io`.
- **T8** `prompt_show_text` + test `prompt_show_resolves_one_and_errors_on_unknown`.
- **T9** `prompt_cmd` + `PROMPT_USAGE` + dispatch wiring (`TopSubcommand::Prompt`, `parse_top_subcommand`,
  the dispatch arm, `TOP_USAGE` line, the unknown-subcommand list) + test
  `prompt_cmd_dispatch_help_unknown_sub_and_strict_args`.

The T6–T9 code is written carefully per plan v3 but was NEVER compiled. Treat it as "should compile" — verify.

---

## 2. RESUME STEPS (in order)

1. **Confirm the storm is gone:** `ps aux | grep '[s]yspolicyd' | awk '{print $3}'` → should be ~0% at idle.
2. **Build (memory-safe):** `cargo build -p a2a-bridge -j 1` → expect `Finished`. (If it hangs at the final
   `a2a_bridge` unit, do the `spctl --master-disable` dance from §0.)
3. **Validate T6–T9** (target cached after step 2 — run each filter individually; libtest multi-filter ORs
   poorly, and `cargo test -p a2a-bridge <a> <b>` errors on the 2nd positional):
   ```
   for t in prompt_only_parse_ignores prompt_list_sorts_ids prompt_show_resolves_one prompt_cmd_dispatch; do
     cargo test -p a2a-bridge -j 1 "$t" 2>&1 | grep 'test result:'
   done
   ```
   - All green → the WIP `74606bf` stands (optionally `git commit --amend`/reword to drop "UNVALIDATED", or
     leave it and let the final squash/merge message carry the truth).
   - Any failure → fix the code, re-run, `git commit --amend` (or a fixup commit).
4. **Then implement T10 + T11** from the plan (`docs/superpowers/plans/2026-06-28-e8a-named-prompt-registry.md`).

---

## 3. REMAINING E8a TASKS (from plan v3)

- **T10 — migration + golden** (`config.rs` golden test + `examples/*.toml`):
  - Golden test `migrated_named_graph_byte_identical_to_prompt_file_for_file_backed` (synthetic old/new pair;
    GREEN guard, not a red test — it just confirms the T5 seam).
  - Migrate **`examples/a2a-bridge.workflows.toml`** (3 workflows: code-review/spec-review/plan-review — NO
    `design`; `[[prompts]]` `file=` per node).
  - Migrate **`examples/a2a-bridge.containerized.toml` AND `.podman.toml` IDENTICALLY** (MANDATORY — a parity
    test `~main.rs:5615` asserts docker≡podman structurally). **MINIMAL set:** register `review-implement`
    once → `prompt="review-implement"` on its 5 nodes + one inline `text=` for the single-line `smoke-reply`;
    **every other node stays `prompt_file=`** (back-compat permits mixed; a follow-up slice finishes the sweep).
  - Verify: `for c in workflows containerized containerized.podman; do ./target/debug/a2a-bridge prompt list
    --config examples/a2a-bridge.$c.toml; done` + the parity test stays green.
- **T11 — `init` scaffold** emits `[[prompts]]` + `prompt="<id>"` nodes (`init_cmd`, `main.rs:~4070`,
  path-injectable via `--dir`; test mirrors `main.rs:~5593`). NO invented `init_cmd_at`.
- **Final gate (controller, clean host, `-j 1`):** `build --all-targets` clean; `clippy -p bridge-core -p
  a2a-bridge --all-targets` 0 warnings (`-D warnings` — note `ResolvedPrompt.source`/`description` carry
  `#[allow(dead_code)]` for this); `fmt --all`; full `cargo test -p bridge-core -p a2a-bridge`.
- **Whole-branch dual review** (codex correctness + claude architecture, via the bridge — see §5) → fold →
  **live-gate** (run the migrated `code-review` with real agents; confirm named-prompt output == `prompt_file`
  behavior) → **merge `--no-ff` to main** → push → **memory** (`e8a-...-shipped.md` + MEMORY.md + this HANDOFF).

---

## 4. NEXT SLICES (after E8a ships)

- **E8b — Composition / `{{> partial}}` includes** (the other half of E8): `{{> partial}}` resolved at
  config-load (inside `resolve_prompt_registry`), BEFORE the runtime `{{var}}` pass; a partial IS a
  `[[prompts]]` entry (referenced by id); transitive expansion + **cycle detection** + depth cap;
  `prompt show --resolved` renders the expanded form (raw stays default). Collapses the 66+ duplicated review
  scaffolds into shared partials (`_preamble/review-readonly`, `_contract/bounded-stop`, …). The E8a
  `BTreeMap<PromptId, ResolvedPrompt>` substrate + the permissive `PromptId` grammar (admits `/`) already
  support it with NO breaking change. Spec §7 + the v3 fold sketch it.
- **Follow-up cleanup slice (optional):** finish migrating the remaining `containerized.toml` nodes (~30) +
  the per-slice scratch configs from `prompt_file` to named prompts (deferred from E8a's minimal set).
- **Roadmap status:** the core orchestration roadmap (Slices 0–10 + cancel-tokens + E1/E6/E3/E7) is COMPLETE
  and merged. **E8 (= E8a + E8b) is the LAST roadmap tail item.** After E8b, only documented deferrals remain
  (push-visibility · detached-node interactive permit · per-agent Defer · A3 auto-heuristic). See
  `docs/superpowers/2026-06-17-orchestration-HANDOFF.md`.

---

## 5. PROCESS REMINDERS (binding, standing)

- **The loop:** architect→spec→dual-review→[re-review]→plan→dual-review→[re-review]→TDD-per-task→
  controller-verify(clean host)→whole-branch-review→fold→live-gate→merge→push→memory. E8a is mid
  **TDD-per-task** (T1–T5 done, T6–T9 written, T10–T11 pending).
- **Role matrix:** codex gpt-5.5 HIGH implements (write, no commits); codex gpt-5.5 XHIGH + claude review
  (read-only); **Opus (controller) architects/verifies-in-clean-host/commits/live-gates.** For E8a the
  controller is implementing inline (plan code is fully specified + dual-reviewed) — this was an explicit
  call; codex can take over the keystrokes if preferred.
- **Review tooling (self-hosted on the bridge):** the E8a spec/plan reviews ran via
  `a2a-bridge run-workflow e8a-<phase>-review --input <freeform-taskspec> --config
  examples/a2a-bridge.e8a-<phase>-review-{codex,claude}.toml` (codex=correctness, claude=architecture; both
  read the spec/plan/code read-only, end with a bounded STOP + verdict). Those `examples/a2a-bridge.e8a-*.toml`
  + `prompts/e8a-*.md` are **untracked review scratch — do NOT commit them with the feature.** Re-use the
  pattern for the whole-branch review. **GOTCHA:** post-E7, `run-workflow --input` is GATED — the review input
  must be a valid task-spec (use `---\ntask-type: freeform\n---\n<focus>`).
- **STAGING DISCIPLINE:** stage ONLY each task's named files. The worktree has a pre-existing
  `M examples/a2a-bridge.slicing-analysis.toml` and MANY untracked `examples/*.toml` / `prompts/*.md` — NEVER
  fold them.
- **Binding docs:** spec `docs/superpowers/specs/2026-06-28-e8a-named-prompt-registry.md` (`## v2`/`## v3`
  supersede v1 §3–§6); plan `docs/superpowers/plans/2026-06-28-e8a-named-prompt-registry.md`
  (`## Plan v2`/v3 fold the PR-FIX/PRR-FIX). Anchors may drift ±3 lines — trust NAMES.
