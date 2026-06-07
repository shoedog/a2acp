# Verify-integrity hardening — design (DEFERRED)

**Status:** DEFERRED 2026-06-07. Captured for later; not scheduled. The focused "clean-flags" fix was found
inadequate by a codex design review (below); the robust fix is bigger and the realistic risk is low (verify
is **advisory** — the operator is the real merge gate — and the agents in use, codex/claude doing tasks,
are not adversarial). Re-trigger if the threat model changes (untrusted/adversarial agents) or verify ever
becomes a hard gate.

## Problem

`a2a-bridge implement` runs a bridge-deterministic VERIFY (cargo fmt/clippy/build/test, each in its own
`docker run`, reading the unforgeable container exit code) on the **committed** clone tree, to gate
agent-written commits. Before each verify, `reset_worktree_to_head` runs `git reset --hard HEAD` +
`git clean -fdq`. An agent can defeat the verify gate by planting config that the clean doesn't remove,
making fmt/clippy falsely PASS.

## What we ruled out / what clean flags can fix

- **NOT a bug:** untracked scratch is already removed by `-fdq`; `cargo fmt --all -- --check` correctly
  flags a mis-formatted freshly-added module file.
- **Gitignored configs survive `-fdq`** (confirmed): commit a `.gitignore` rule + plant a gitignored
  `rustfmt.toml`/`clippy.toml`/`.cargo/config.toml` → it survives → neuters the linter. `-x` removes it.
- **`-e target` is unsafe** (codex): preserving `target/` lets an agent `include!`/`build.rs`/path-dep a
  planted file under `target/` → verify builds non-committed input. Don't preserve `target`.
- **One `-f` misses nested git repos** (codex): `git clean -fdxq` won't remove an untracked nested repo
  (needs `-ff`); an agent could point a path-dep/include at one.
- → the **pragmatic interim** that closes the *realistic* (lazy/non-adversarial) vectors: `git clean
  -ffdxq` (no `target` exception) in `reset_worktree_to_head`. Documents that `-x` can break repos that
  depend on ignored generated files / local `.env` / untracked vendored dirs / uninitialized submodules
  ("committed-tree-only" semantics — acceptable, must be documented).

## What clean flags CANNOT fix (why the focused fix is inadequate)

- **Live-writer race:** the warm `:rw` implement container stays mounted RW through verify/review
  (`main.rs` — retired only after the loop). A background process the agent started can **re-plant files
  after the reset, before/during the verify container**. No `git clean` flag closes this.
- **`.git` metadata tampering:** an agent can rewrite `.git/config` (`core.worktree`, sparse-checkout, …);
  `-x` doesn't touch `.git` state.

## Recommended robust design (codex)

**Verify from a fresh `git archive HEAD` export, isolated from the writer:**

1. After the commit, `git -C <clone> archive --format=tar HEAD | tar -x -C <export_dir>` where `<export_dir>`
   is a fresh temp dir **outside** `allowed_cwd_root` (so the agent's `:rw` mount cannot reach it).
2. Mount `<export_dir>` **`:ro`** into the verify container (instead of the live clone); keep
   `CARGO_HOME`/`CARGO_TARGET_DIR` on the separate cache volume.
3. Run the `[[verify.commands]]` there.

This makes verify independent of ignored files, nested repos, `.git` metadata, **and** the live-writer race
(the export is immutable + outside the agent's reach + mounted `:ro`). It also lets the agent clone keep its
`target/` for fix-turn build speed. Limitation (acceptable + honest): submodule contents aren't in
`git archive HEAD` → a build needing submodule code fails — that's "committed-tree-only" behaviour.

**Touch points:** `compose_verify` (mount the export dir instead of the `allowed_cwd_root` mount + `cd` it),
`run_verify_step`/the implement flow (create + clean up the export dir per verify), and a temp-dir lifecycle
(reap the export). Full cadence (spec → dual-review → plan → build → gate) when picked up.

## Review provenance

a2a-local codex-review (gpt-5.5), 2026-06-07: 4 BLOCKERs (the two clean-flag fixes above + the live-writer
race + `.git` tampering), recommending the archive-export design. The focused clean-flags fix was rejected
as insufficient.
