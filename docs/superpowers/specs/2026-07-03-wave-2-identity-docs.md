# Wave 2 — Identity & Docs (spec, v1)

**Status:** Draft, pending codex xhigh spec-review.
**Source:** `docs/2026-07-03-strategic-analysis.md` next-steps #3, #4, #5 + the identity ruling ("personal tool, published well" — reference implementation, maintained-not-supported).
**Branch:** `feat/wave-2-identity-docs`. Docs/config only — zero Rust changes.

## W2-A: README rewrite

**File:** `README.md` (full rewrite of stale sections; keep what is accurate).

**Requirements:**
1. Fix the actively-wrong claims: the store is durable SQLite (WAL as of wave 1), NOT
   "in-memory only" (current :430-432); MCP (`a2a-bridge mcp`), container isolation,
   workflows, worktrees are SHIPPED, not "Deferred" (:417-421); drop the "Increment 3b"
   framing (:9-14) — describe the tool as it is.
2. Crate table covers all 15 crates + bin (currently 8; `bridge-a2a-outbound` described
   as a stub — it is a real HTTP/SSE client now). One line each.
3. Surface the flagship commands a new reader never learns exist: `run-workflow`,
   `implement`, `task-spec`, `mcp`, `run-batch`, `session`/`task`, `init`, `validate`;
   `[sandbox]`/`[worktrees]`/`[[prompts]]`/`[[languages]]` config blocks get a pointer
   table (NOT full docs — link AGENTS.md / docs/onboarding.md / docs/containerized-agents.md).
4. Add `## Troubleshooting`: agent not on PATH / not in `allowed_cmds`; agent auth
   expired (codex/claude/kiro one-liners); `--session-cwd` omitted (agents act in launch
   cwd); port in use; `-j 1` for `--all-targets` on memory-constrained machines;
   containerized MCP env trap (link `docs/containerized-mcp-env-trap.md`).
5. Add a short "What a review run looks like" sample-output excerpt (~15 lines, real
   shape: run-workflow invocation + a synth verdict tail) so users can gut-check success.
6. Keep: hexagonal architecture section (accurate per the strategic analysis — but fix
   the typestate framing: the runtime lifecycle is SessionManager's claim states; the
   typestate is a compile-time spec artifact, say so in one honest line); protocol
   bindings section (update ACP pin text to 1.0.1 — verify against Cargo.toml); the
   per-request metadata table; License section (already AGPL, leave).
7. Identity stance near the top: reference implementation bridging A2A↔ACP;
   maintained-not-supported pre-1.0; link CONTRIBUTING.md.
8. Every claim in the new README must be verifiable in-repo (command exists in
   `TOP_USAGE`, config key exists in config.rs) — no aspirational text.

## W2-B: one-shot artifact purge (move, not delete)

**Files:** `examples/*`, `prompts/*`, `.github/workflow-artifact-allowlist.txt`,
new `docs/history/` tree.

**Rule (mechanical, not a hand-list):**
- MOVE (`git mv`) to `docs/history/examples/` and `docs/history/prompts/`: every file
  matching the one-shot dev-process patterns — `slice-*`, `slice[0-9]*`, `e1-*`, `e3-*`,
  `e6-*`, `e7-*`, `cancel-tokens-*`, `arch-review-*`, `orchestration-*`, `warm-sessions-*`,
  `1d-plan-review*`, `jsts-*`, `python-spec-review*`, `lsp-go-*`, `*livegate*`,
  `*-impl-codex*`, `*-impl.md`, `impl-smoke*`, `smoke-*`, `slicing-*` (the slicing-repo
  one-offs belong to that repo, parked in history here), plus any file the KEEP rule
  below does not cover after pattern application.
- KEEP in place: everything referenced by (a) `README.md`/`AGENTS.md`/`docs/onboarding.md`
  /`docs/containerized-agents.md` after W2-A, (b) the `init` scaffold embeds
  (`bin/a2a-bridge/src/main.rs` init tables — grep the embedded prompt/workflow ids),
  (c) the shipped product configs: `a2a-bridge.workflows.toml`, `a2a-bridge.multi-agent.toml`,
  `a2a-bridge.containerized.toml`, `a2a-bridge.containerized.podman.toml`,
  `a2a-bridge.panel.toml`, `sample-input.md`, and every `prompts/*.md` those configs
  reference (trace `prompt_file`/`[[prompts]] file=` refs mechanically), including
  `c2b-nav.md` (referenced by the containerized config) and the `design-*`/`review-*`/
  `spec-review-*`/`plan-review-*`/`panel-*`/`implement-*` canonical prompt families.
- A file matching BOTH rules stays (KEEP wins); the implementor lists any such conflicts
  in the commit message.
- Add `docs/history/README.md` (5 lines): what this tree is (frozen one-shot dev-process
  artifacts, machine-specific paths, never rerun), why it exists (hygiene ratio), and
  that git history is authoritative.
- Rewrite `.github/workflow-artifact-allowlist.txt` to exactly the kept set; moved files
  leave the `examples/`/`prompts/` hygiene scope automatically.

**Gates:** `./target/release/a2a-bridge validate --repo-hygiene` passes with the shrunk
allowlist; `a2a-bridge validate --config examples/<each kept .toml>` still passes for
kept configs whose agents exist as commands is NOT required (validate parses; agent
binaries need not run) — run `validate --config` on each kept config and report; grep
proves README/AGENTS/onboarding/init reference no moved path. `docs/superpowers/**`
references to old paths are historical documents and stay untouched.

## W2-C: sandbox tier ADR + presets

**Files:** `docs/adr/0032-sandbox-tier-model.md` (new), `examples/a2a-bridge.tiers.toml`
(new, small, allowlisted).

**ADR content (decision record, ~2 pages):**
1. Context: the container stack (ADR-0013/0016-0021/0030) is calibrated for untrusted
   content; the repo's own development ran host-side (`danger-full-access` impl configs,
   host RO reviews) — a de-facto second tier the ADRs never formalized; the strategic
   analysis + container-posture review found the real risk is UNDER-enforcement
   (opt-in `[sandbox]`, prompt-level-only constraints), not over-protection.
2. Decision — the four named tiers, each with: what enforces it (kernel / agent-native /
   prompt), what content classes it is approved for, and its known gaps:
   - **Tier 0 tools-off**: prompt-contract only; inlined-context review of any content.
   - **Tier 1 host + agent-native sandbox**: codex `sandbox_mode=read-only` (agent-honored,
     unaudited by us); claude has NO equivalent (Tier 1 ceiling for claude = Tier 0);
     approved ONLY for read-only work on trusted own-repo content.
   - **Tier 2 container `:ro` + default-deny egress**: kernel-enforced; required for any
     content an adversary could author (third-party PRs, deps, issues).
   - **Tier 3 container `:rw` + quarantine clone + verify creds-XOR-egress**: required
     for all write-capable/implement work regardless of trust.
   - Never-relax list (verify split, cred discipline, symlink canonicalization,
     hook neutralization) + the residual model-endpoint exfil channel documented as
     accepted risk.
3. Consequences: host-run `danger-full-access` implement configs are RETIRED from the
   sanctioned set for repos with any untrusted content and remain a documented Tier-1
   escape hatch for this repo only; future `[sandbox]`-less agent entries should state
   their tier in a comment.
**Presets file:** one `[[agents]]` example per tier with a `# Tier N — approved for: …`
comment header, agent ids `tier0-review`/`tier1-codex-ro`/`tier2-reader`/`tier3-impl`,
mirroring the real flags from the existing shipped configs (copy from
`a2a-bridge.workflows.toml` and `a2a-bridge.containerized.toml`, don't invent).

## Definition of done (wave)

1. Three tasks on the branch, one commit each; W2-A and W2-C are independent; W2-B runs
   AFTER W2-A lands (its KEEP rule reads the rewritten README).
2. Gates: `validate --repo-hygiene` green; kept-config `validate --config` results
   reported; no-moved-references grep green; `cargo test -p a2a-bridge -j 1` green (the
   config parse tests cover example loading — verify none hardcode moved paths).
3. Whole-branch dual review (opus 4.8 + codex xhigh) — docs waves still get the gate;
   reviewers fact-check README claims against the code and the purge keep/move lists.
4. Merge to `main`, push.

## Risks

- W2-B pattern rules could move a file a kept config references (broken `prompt_file`) —
  the trace-refs step + `validate --config` gate catches it.
- README claims drifting from code — requirement 8 + reviewer fact-check.
- Allowlist rewrite is order-sensitive with the hygiene guard — run the gate locally.
