# Containerized Agents — Slice B2b-1 Design: the `implement` clone+edit+commit foundation

**Date:** 2026-06-05
**Status:** Draft (pre review)
**Builds on:** B2a (per-turn `:rw` `ContainerRwBackend`, merged 68ae7ba, ADR-0018). First sub-slice of B2b
(the write-capable `implement` workflow). B2b-2 = build+test verify (toolchain image); B2b-3 = review-the-
diff + APPROVE/REJECT.

## Goal

Ship the **foundation of the `implement` loop**: an `a2a-bridge implement` subcommand that creates a
per-task **quarantined git clone**, has the `impl` (`ContainerRw`) agent **edit + stage** a change inside
it, the **bridge deterministically commits** the agent-staged index on a task branch, and the clone is left
for the **operator to review/merge/reap** — human-approval entirely outside the bridge. Validated by a real
`implement/<task-id>` branch with the agent's staged change, the source repo untouched, the turn contained.
**No** verify/build-test (B2b-2), **no** review-the-diff/approval primitive (B2b-3), reader image only.

## Decisions locked (from the design dialogue)

1. **New `implement` subcommand** owns the clone lifecycle (the run-context) — NOT the executor/backend. It
   reuses the shipped `ContainerRw` `impl` agent + the executor (a 1-node `implement-edit` workflow) + B2a's
   `--session-cwd` threading. It is essentially `run-workflow` wrapped with **clone-before** + **commit-and-
   hand-off-after**.
2. **Host-commits, "soft gate":** the agent owns the commit **content** (what to `git add` — judgment, incl.
   new files; and the commit **message**); the bridge owns the commit **action** (the deterministic per-turn
   checkpoint). The agent does NOT run `git commit`. The bridge does the mechanical commit, saving agent
   tokens. (Works because the identical-path `:rw` mount means the agent's in-container `git add` writes the
   clone's `.git/index` on the host fs, and the host's `git commit` snapshots that same index — paths agree.)
3. **`--no-hardlinks` committed-only clone** (CopyMode=committed for B2b-1; dirty-tree deferred). Fully
   independent object store; the source repo is never touched.
4. **Bot identity pre-merge, operator identity at merge.** The bridge commits with a clear bot identity
   (`a2a-implement <implement@a2a-bridge.local>`) — pre-merge tracking, freely rewritable. At merge the
   operator re-authors as **themselves** (contributors need the responsible human); the hand-off prints the
   command.
5. **`--no-verify` on the host commit** — the agent has `:rw` and could plant `.git/hooks/pre-commit`; the
   host must not execute a clone hook on the host machine. (Hooks aren't cloned/tracked, so a merge never
   carries them; they die with `rm -rf <clone>`.)

## Architecture

### Entry point
```
a2a-bridge implement <task> --repo <path> [--base-ref <ref>] [--config <path>] [--workflow <id>]
```
- `<task>`: the task description (positional, templated as `{{input}}` into the edit prompt).
- `--repo`: the source repo to clone (any host path — `git clone` runs on the host; only the *clone* must
  land under `allowed_cwd_root`, see below).
- `--base-ref`: branch/ref to clone from (default: the source repo's current HEAD).
- `--config`: the bridge config (defines the `impl` `ContainerRw` agent + the `implement-edit` workflow).
- `--workflow`: the edit workflow id (default `implement-edit`).
- Dispatched in `main.rs` alongside `serve|run-workflow|submit|task|init`.

### Clone lifecycle (run-context owns it)
On invoke, the subcommand:
1. Resolves `allowed_cwd_root` from the config (parse layer) and a `task-id` (`impl-<pid>-<nonce>`).
2. `git clone --no-hardlinks <repo> <allowed_cwd_root>/.a2a-implement/<task-id>` → `git -C <clone> checkout
   <base-ref>` (if given) → `git -C <clone> checkout -b implement/<task-id>`.
   - The clone dir is **under `allowed_cwd_root`** so it's `is_under` the `ContainerRw` mount anchor
     (B2a's `resolve_rw_target` gate). It is a **sibling of**, not inside, the source repo, so it's not in
     the source's git.
3. The clone **persists** after the run (NOT auto-reaped) — the operator reaps it on merge.

### Edit turn (reuse the executor)
A 1-node `implement-edit` workflow (`agent="impl"`, `prompt_file="../prompts/implement-edit.md"`,
`inputs=[]`). The subcommand runs it via `executor.run_with_context(graph, input=<task>, …, ctx{
session_cwd: Some(<clone>) })` — exactly the B2a run-workflow path, with the clone as the session cwd. The
node's `configure_session(SessionSpec{cwd: clone})` → the `ContainerRwBackend` mints a `:rw` container on
the clone; the agent (in-container, identical-path):
- edits files; uses `git diff` / `git stash` as working tools;
- **stages** its change (`git add <paths>`, including new files — judgment about what belongs);
- ends its reply with the commit message in a single fenced block:
  ````
  ```commit
  <subject>

  <optional body>
  ```
  ````
- does **NOT** run `git commit`.

The subcommand collects the workflow's terminal output (the node's final text, same as run-workflow).

### Commit (bridge, deterministic soft gate)
After the turn, the bridge:
1. **Parses** the LAST ```` ```commit ```` fenced block from the node output → the message.
2. **Stage check:** `git -C <clone> diff --cached --quiet`.
   - Index has staged changes → commit it (the agent's judgment).
   - Index empty but working tree dirty (`git -C <clone> diff --quiet` fails) → **flag loudly** +
     `git add -A` fallback + commit (don't lose work; log "agent didn't stage").
   - Nothing changed at all → **no commit**, report "implement made no changes."
3. **Commit:** `git -C <clone> -c user.name=a2a-implement -c user.email=implement@a2a-bridge.local commit
   --no-verify -m "<message>"`.
   - **Message safety gate:** no parseable ```` ```commit ```` block → fall back to a task-derived message
     (`implement: <first line of task, truncated>`) + flag. (Re-asking the agent for a message is a
     **warm-slice** enhancement — cheap to re-prompt a warm session; for per-turn B2b-1 the task-name
     fallback is the deterministic floor.)

### Hand-off (human-approval = outside the bridge)
The subcommand prints:
- the clone path, the branch `implement/<task-id>`, the commit sha + subject;
- the **operator re-author + merge** command, e.g.
  `git -C <repo> fetch <clone> implement/<task-id> && git -C <repo> cherry-pick --reset-author FETCH_HEAD`
  (re-authors as the operator) — or a squash-merge authored by the operator;
- `rm -rf <clone>` to reap.

No in-bridge APPROVE/REJECT primitive (B2b-3). The operator reviews the diff (and confirms no unexpected
`.git/hooks`/lifecycle changes in tracked files) and merges as themselves.

## Component / file boundaries

| Concern | Home | Note |
|---|---|---|
| `implement` subcommand dispatch + orchestration | `bin/a2a-bridge/src/main.rs` | `Some("implement") => implement_cmd(…)`; clone → run workflow → commit → hand-off |
| **Pure helpers** (git argv builders, `commit`-block parser + fallback, task-id, hand-off message) | **`bin/a2a-bridge/src/implement.rs`** (new module) | Docker-free + git-free unit-testable |
| `implement-edit` workflow + prompt | `examples/a2a-bridge.containerized.toml` + `prompts/implement-edit.md` | 1 node, `agent="impl"`; the edit+stage+message contract |
| reuse | `bridge-workflow::executor` (run_with_context), the B2a `ContainerRw` `impl` agent | no new backend / crate / image |

The pure helpers (argv builders for clone/checkout/commit/stage-check, the fenced-block parser, the
task-derived fallback, the hand-off text) are pure functions in `implement.rs`; the impure steps (running
git, running the executor) are the orchestration, validated by the live gate.

## Testing
- **Unit (Docker-free, git-free) in `implement.rs`:** clone/checkout/commit/stage-check argv builders
  (shape incl. `--no-hardlinks`, `--no-verify`, the bot `-c user.*`); the ```` ```commit ```` parser
  (extracts the last block; multi-block; ignores prose; missing-block → fallback); the task-derived
  fallback message; the hand-off message (paths/branch/sha/re-author command); task-id shape.
- **Live gate (Docker, operator-run):** `a2a-bridge implement "create a file FOO.md containing BAR, stage
  it, and return a commit message" --repo <throwaway clone of this repo> --config
  examples/a2a-bridge.containerized.toml` → assert: a real `implement/<id>` branch in the clone with the
  agent's staged change committed (the bot identity) + the agent's message; the **source repo is untouched**
  (`git -C <repo> status` clean, no new branch); `docker events` shows the contained `a2a-rw-*` turn; the
  empty-staging + unparseable-message fallbacks behave (a second run with a prompt that omits the block →
  task-derived message + flag).
- Coverage after `cargo llvm-cov clean --workspace` (floors workspace 85, bridge-core 90, bridge-workflow 90).

## Security (B2b-1 surface)
- The clone is a quarantine (independent object store; source untouched). The agent runs git **in the
  container** (contained); a clone hook it plants fires only in-container, and the **host commit uses
  `--no-verify`** so it can't execute on the host.
- The host `git commit` runs with the bridge's environment (no agent influence); the container env is the
  controlled `docker run` env (B2a).
- Merge carries only tracked changes (hooks aren't tracked); the operator reviews the diff before merging.

## Deferred (later sub-slices / follow-ons)
- **B2b-2:** build+test **verify** node (a Rust **toolchain image** + cargo-under-egress-lockdown:
  allowlist `crates.io`/`index.crates.io`/`static.crates.io` + github, or a `:ro` cargo cache).
- **B2b-3:** **review-the-diff** lenses (the existing review workflow on `git diff base..HEAD`) → synth
  verdict → **APPROVE/REJECT**; the review→tweak loop (multi-turn, per-turn host commits = checkpoints).
- **Warm re-ask** of a missing commit message (cheap on a warm session — the warm-pool slice).
- **Detachable `implement`** (serve/TaskStore) + formal crash-resume (`reset --hard HEAD` + re-run the
  pending node); CopyMode=dirty-working-tree; gVisor `--runtime=runsc` for write agents on Linux.

## Firewall
Designed from the bridge's own ports (the `implement` subcommand site, `executor.run_with_context` +
`WorkflowRunContext.session_cwd`, the B2a `ContainerRw` backend + `resolve_rw_target` gate, the existing
review-workflow `{{input}}` convention). `tsk-tsk` (dtormoen) READ as prior art for the repo-copy/CopyMode
shape only — NOT adopted (it owns scheduling + its own task store, colliding with our `WorkflowExecutor`/
`TaskStore`). Dual review = containerized dogfood (the B1-hardened `[sandbox]` agents) PRIMARY + a2a-local
`codex-review` (gpt-5.5) backstop. Once `implement` ships it should build its own subsequent changes (the
ultimate dogfood).
