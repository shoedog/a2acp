# LSP-over-MCP semantic nav (L3, Slice A) + cross-agent skills library — design

**Date:** 2026-06-13
**Status:** Approved (brainstorming) — ready for plan
**Scope:** Slice A — Rust-first, host-side reviewers. (Slice B = in-container implementor, Slice C = multi-language — both deferred, see Non-goals.)

## Goal

Give the bridge's **host-side reviewer agents** (claude + codex) *type-resolved* semantic code navigation — go-to-definition, find-references, hover, implementations, call-hierarchy — over the repo under review, delivered through the existing `[[agents.mcp]]` seam as a small **`lsp-mcp` shim** that wraps `rust-analyzer`. This complements prism (structural graph) with semantics prism cannot provide (trait/generic resolution).

Pair it with a **playbook skill** so the agents know *when and how* to use the nav tools, and document a reusable, source-controlled **cross-agent skills library** (`~/knowledge-ref/skills/`) as the home for that skill and future ones.

## Context

The bridge already delivers MCP servers to agents via `[[agents.mcp]]` (ADR-0028), with three delivery channels (`McpDelivery::{Acp, CodexNative, KiroNative}`) and `{cwd}` substitution — all SDK-free in `bridge-core::mcp`. prism is wired to the **host-side** claude + codex reviewers today:

```toml
[[agents.mcp]]
name = "prism"
command = "/Users/wesleyjinks/code/slicing/target/release/prism-mcp"
args = ["--repo", "{cwd}", "--cache-dir", "…/prism-cache-host"]
```

L3 plugs into that *same* seam with **zero `bridge-core` changes** (one small `render_kiro_agent_config` tweak for the skill path only — see the *Cross-agent skills library* section). prism gives a structural call/dep graph; LSP gives type-resolved answers (references that resolve through generics and trait impls, the actual type at a position, who implements a trait). They are complementary, both wired.

## Empirical findings (spikes)

The design is grounded in measurements run on 2026-06-13 (host: 24 GB, OrbStack 29.4.0, `rust-analyzer 1.94.0`). These are load-bearing — they reshaped the architecture away from a "warm daemon."

### Warm-index / cold-start (`rust-analyzer analysis-stats`, time-to-usable-index = "Database loaded")

| Scenario | Usable index | proc-macro/build step | Peak RSS |
|---|---|---|---|
| Cold — fresh clone, empty target | 8.89 s | 5.04 s | 2.91 GB |
| **Warm-reuse — different clone, shared `CARGO_TARGET_DIR` warmed by a sibling** | **0.72 s** | 0.115 s | 2.94 GB |
| Warm same-path (99 GB target) | 11.81 s | 0.12 s | 2.91 GB |
| slicing (110 k LLOC) cold | 6.59 s | 5.58 s | 1.89 GB |

**Conclusions:**
1. **Clone-reuse works and is dramatic** — a fresh clone pointed at a `CARGO_TARGET_DIR` another clone already warmed reaches a usable index in **0.72 s** (proc-macro/build compile collapses 5040 ms → 115 ms). The lever is a **shared on-disk cache**, not a long-lived in-memory daemon.
2. **Cold-start is seconds, not the rumored ~106 s**, because `~/.cargo` is warm and these repos are small/medium. Cold cost (and thus warm-reuse payoff) scales with repo size.
3. **The binding constraint is memory** (~2–3 GB resident per warm server, varying with *dependency* weight, not LLOC), not time.

### The other unknowns

- **Q1 — one LSP for many consumers:** Yes, *if same checkout*. `rust-analyzer` answers concurrent in-flight requests (2 fired, both returned). One server + one shim fans out N MCP consumers — so the review fan-out (codex + claude on the *same* clone) can share one warm server. Different clone paths → separate servers.
- **Q2 — one LSP for many projects:** Yes (LSP multi-root), but memory = **sum** of per-repo indexes. ~3–5 warm repos fit in 24 GB beside the trading workloads. Capability is fine; memory bounds it.
- **Q3 — one daemon outside the containers, reachable host- and container-side:** **No, not on macOS/OrbStack.** A host-created UNIX socket bind-mounted into a container shows up as a socket *file* but `connect()` is **refused** (the listener is a macOS process; the container is in the Linux VM — AF_UNIX does not cross virtiofs). TCP is blocked by the `--internal` egress-locked network. Host→host connect works. ⇒ **No single shared daemon.** Host-side reviewers run a host-side shim (Slice A); the in-container implementor (Slice B) gets a co-located shim baked into the image.
- **Q4 — incremental reindex as the implementor edits:** Yes, **~50 ms**. Writing a new symbol to disk + a `workspace/didChangeWatchedFiles` notification → the symbol is indexed in 0.05 s. (Relevant to Slice B; the shim forwards `didChangeWatchedFiles` on edits, or relies on rust-analyzer's own fs watcher.)
- **Q5 — hibernate to free RAM, reactivate fast from a stored index:** Yes, ~free. `rust-analyzer` persists no in-memory DB; killing it frees the full ~2.9 GB; restart against the warm on-disk `CARGO_TARGET_DIR` is **0.72 s**. The on-disk *build cache* is the "stored index" — no custom index store needed.

## Architecture

A new focused binary **`lsp-mcp`** (a sibling tool to prism — see Build-vs-adopt), wrapping `rust-analyzer` and exposing nav tools over MCP. Wired via `[[agents.mcp]]` exactly like prism.

```toml
[[agents.mcp]]
name = "lsp"
command = "/path/to/lsp-mcp"
args = ["--repo", "{cwd}", "--lang", "rust", "--target-cache", "…/lsp-target-cache"]
```

**Lifecycle = the agent's MCP session.** On session start the shim spawns one `rust-analyzer` rooted at `{cwd}`, completes the LSP handshake, and holds it **warm across every tool call** in that review; it exits when the session ends. No cross-session daemon, no idle-evict (deferred — the on-disk cache already makes a fresh per-session spawn sub-second). This matches the Q3 finding: there is no shared daemon, only a per-session server made cheap by the shared cache.

**Warm-reuse via a per-repo shared `CARGO_TARGET_DIR`.** The shim sets the child `rust-analyzer`'s `CARGO_TARGET_DIR` to a host cache directory **keyed by repo identity** (git `remote.origin.url`, falling back to a hash of the canonical source path), *not* a single global dir — so clones of the same repo reuse each other's build artifacts (the 0.72 s path) without cross-repo interleave. First build on a cold key pays the ~5 s proc-macro compile and briefly holds cargo's target lock; concurrent same-repo reviewers (codex + claude) serialize on that lock once, then run warm. The cache is read-mostly.

**The cache key is a *reuse boundary*, not a re-index trigger.** Re-indexing is content-driven and independent of the key: a `.rs` edit → rust-analyzer's salsa engine incrementally re-analyzes that file + dependents (~50 ms); a `Cargo.toml`/`Cargo.lock` change → cargo reloads and rebuilds only the *changed* deps/build-scripts/proc-macros, reusing the rest. Both fire the same way regardless of the key. The key affects *only* the cost of a fresh clone's **first** index (warm 0.72 s vs cold ~6–9 s). This is why `origin.url` beats a `Cargo.lock`-hash key: a lockfile-hash mints a new *cold* dir on every dependency bump (full proc-macro/dep rebuild) and fragments the cache across branches, whereas `origin.url` keeps one warm dir that cargo's own fingerprinting (package-id + features + rustc version) evolves correctly across commits, branches, and lockfile changes — the normal "one target dir for an evolving checkout" case. It never touches the user's separate `…/target`.

**Name-addressing (not a passthrough).** LSP is position-addressed (`file, line, char`); an LLM rarely knows coordinates. The shim accepts symbol *names* and resolves them to positions internally (via `workspace/symbol` / `documentSymbol`), so the agent calls `references(name: "EffectiveConfig")` rather than supplying line/column. This resolution layer is the shim's core value and is why MCP (not "use the LSP directly") is correct — the agents speak MCP, not LSP, and the shim is where the warm session, name-resolution, and (later) `didChangeWatchedFiles` forwarding live.

## Tool surface (~7)

Selection rule: **expose a tool only if it answers a reviewer question that prism's *structural* graph cannot — i.e., it needs type resolution.**

**Discovery (name → position):**
1. `workspace_symbol(query)` — find a symbol by name across the repo (the entry tool).
2. `document_symbols(file)` — file outline.

**Type-resolved:**
3. `definition(name | file+pos)` — resolves through re-exports, trait methods, generics.
4. `references(name | file+pos, include_declaration?)` — true blast radius; resolves generic/trait instantiations ripgrep and prism's structural callers miss.
5. `hover(name | file+pos)` — resolved type + signature + docs at a point.
6. `implementations(name | file+pos)` — trait impls / who implements a trait. Purely semantic.
7. `call_hierarchy(name | file+pos, direction: incoming|outgoing)` — type-resolved caller/callee graph.

**Deliberately out:** `rename`, `formatting`, `code_actions`, `completion`, `signature_help`, `inlay_hints` (write-/IDE-ergonomics, irrelevant to a read-only reviewer); **diagnostics** (the bridge's `verify` step already runs `cargo check`/`clippy` deterministically and unforgeably — RA diagnostics would duplicate it less reliably). `type_definition`/`type_hierarchy` fold into the above; add later only if a real need appears.

Each tool returns compact, agent-friendly results: a path, a 1-based line, the enclosing item's signature, and a short surrounding snippet — never a raw LSP `Location` blob.

## Cross-agent skills library (`~/knowledge-ref/skills/`)

The skill mechanism validated below is **general infrastructure**, not specific to lsp-nav. This slice establishes the pattern and the library; lsp-nav is its first inhabitant.

### Why a library

All three process agents converged on the **same "Agent Skills" standard** — a skill is a directory containing `SKILL.md` with YAML frontmatter (`name`, `description`) plus an optional `references/` dir, auto-activated by semantic match on `description` (and explicitly via a `/name` or `$name` command). Spike evidence:

| Agent | Discovery paths | Format | Activation | Source |
|---|---|---|---|---|
| **claude** (claude-agent-acp 0.44.0) | `~/.claude/skills/` (user), `.claude/skills/` (project) | `SKILL.md` + frontmatter | auto (settingSources `user`/`project`/`local` + `claude_code` preset) + `/name` | `acp-agent.js:1948` |
| **codex** (codex-acp) | `~/.agents/skills/` (user), `.agents/skills/` (repo + parents), `/etc/codex/skills` | `SKILL.md` + frontmatter, optional `agents/openai.yaml` | auto (`description`) + `$name` / `/skills`; **symlinked skill folders supported**; init list capped ~8 KB | developers.openai.com/codex/skills |
| **kiro** (kiro-cli) | `~/.kiro/skills/` (global), `.kiro/skills/` (workspace) | `SKILL.md` + frontmatter (`name` ≤64 chars, `description` ≤1024) | auto (`description`) + `/name` | kiro.dev/docs/cli/skills |

Because the format is identical, **one `SKILL.md` serves all three** — no per-agent content. The only divergence is the discovery *path*, and all three follow symlinks.

### Library layout

`~/knowledge-ref/` is a user-owned git repo (source-controlled separately from a2a-bridge). Skills live under `skills/`:

```
~/knowledge-ref/
  skills/
    lsp-nav/
      SKILL.md
      references/            # optional deeper detail (loaded on demand)
    # future (documented intent, NOT built this slice):
    # prism-nav/ rust/ code-review/ architecture/ software-development/
  install-skills.sh          # idempotent symlink fan-out (below)
  REFERENCES.md              # the skill standard + authoring best-practices (below)
  README.md
```

### Standard & references (`~/knowledge-ref/REFERENCES.md`)

The library stores the authoritative sources for the skill standard and authoring guidance, so the format and best practices travel with the repo rather than living in memory:

- **Agent Skills standard** — <https://agentskills.io/home>
- **Agent Skills — skill-creation best practices** — <https://agentskills.io/skill-creation/best-practices>
- **Anthropic — Agent Skills overview** — <https://docs.claude.com/en/docs/agents-and-tools/agent-skills/overview>
- **Anthropic — Claude Code skills** (invocation control, subagent execution, dynamic context injection on top of the standard) — <https://docs.anthropic.com/en/docs/claude-code/skills>
- **Anthropic — skill authoring best practices** — <https://docs.claude.com/en/docs/agents-and-tools/agent-skills/best-practices>
- **OpenAI Codex — skills** — <https://developers.openai.com/codex/skills>
- **Kiro CLI — skills** — <https://kiro.dev/docs/cli/skills/>

Key authoring principles to carry into every skill (from the above): a **`description` packed with concrete triggers** (Claude under-triggers — be "pushy"); **progressive disclosure** (frontmatter always loaded, `SKILL.md` body on activation, `references/` on demand) so keep `SKILL.md` concise; and **trust** — only install skills authored here or from a trusted source.

### Install mechanism — symlink fan-out

`install-skills.sh` symlinks **each** `skills/<name>/` directory into each agent's **user-level** discovery path (per-skill symlinks, so existing skills in those dirs are never clobbered):

```sh
for s in ~/knowledge-ref/skills/*/; do
  name=$(basename "$s")
  for dest in ~/.claude/skills ~/.agents/skills ~/.kiro/skills; do
    mkdir -p "$dest"
    ln -sfn "$s" "$dest/$name"
  done
done
```

User-level install means the **host-side reviewers cover the entire stable repo set**, not only repos that vendor the skill. The script is idempotent and safe to re-run after adding a skill or pulling the repo.

The library is **already bootstrapped** (`~/knowledge-ref`, its own git repo) with `install-skills.sh`, `REFERENCES.md`, `README.md`, and its first skill **`prism-nav`** (structural code navigation via the prism MCP tools — the structural counterpart to lsp-nav). The lsp-nav skill from this slice is added alongside it.

### a2a-bridge integration

- The bridge does **not** vendor skills. The lsp-nav skill content lives in `~/knowledge-ref/skills/lsp-nav/`. The a2a-bridge `init`/setup docs gain a step: "clone `~/knowledge-ref`, run `install-skills.sh`."
- **kiro custom-agent wiring (the only bridge-code change in the skill path):** the bridge writes *custom* kiro agents (`a2a-mcp-<id>`), which — unlike default agents — must opt into skills via a `resources` field. `render_kiro_agent_config` gains:
  ```json
  "resources": ["skill://~/.kiro/skills/*/SKILL.md", "skill://.kiro/skills/*/SKILL.md"]
  ```
  claude and codex (host-side, Slice A) auto-discover with no code change. (kiro is a `:ro` reader not yet in the review workflows, so this is wired-but-forward-looking; it costs nothing now and is ready when kiro joins reviews.)
- **Always-on fallback:** the bridge review prompt inlines a one-line pointer to the lsp-nav workflow, because auto-activation is `description`-dependent and a review *will* navigate.

## The lsp-nav skill

`~/knowledge-ref/skills/lsp-nav/SKILL.md`:

- **`name`:** `lsp-nav`
- **`description`** (the cross-agent trigger — engineered to fire on review/navigation tasks): e.g. *"Use when reviewing a code change, tracing how a symbol is used, assessing the blast radius of an edit, finding what implements a trait, or understanding an unfamiliar type — gives type-resolved go-to-definition, find-references, hover, implementations, and call-hierarchy via the `lsp` MCP tools."*
- **Body (the playbook):**
  - **Tool division of labor:** prism for structural/whole-graph questions (repo map, module deps, cheap); **lsp** for type-resolved point queries (definition/references/hover/implementations); read the file for actual logic. Don't use `references` where a repo-map read suffices.
  - **The chain:** `workspace_symbol(name)` → pick the hit → `document_symbols`/`hover` for context → `references`/`implementations`/`call_hierarchy`.
  - **Review heuristics:** for every changed `pub` item → `references` (blast radius); for a changed trait or impl → `implementations`; for an unfamiliar type at a call site → `hover`; for a changed function's contract → `call_hierarchy(incoming)`.
  - **Budget discipline:** targeted queries on the *diff's* symbols; don't spider the whole graph.
- **`references/`:** a longer "LSP vs prism vs grep" decision guide and worked examples, loaded on demand.

## Build vs adopt

Build a focused `lsp-mcp` shim, mirroring prism (custom, in `~/code/slicing`). Rationale: the shim's value is the *name-addressing + warm-session + curated 7-tool surface + per-repo cache keying* — opinionated glue a generic LSP-MCP bridge would not provide, and prism sets the precedent for a small purpose-built MCP binary. The plan should still spend ~30 min surveying existing LSP-over-MCP projects to lift transport/handshake code where licensing allows, but the integration shape is ours.

## Non-goals (this slice)

- **Slice B — in-container implementor nav.** Requires baking `lsp-mcp` + `rust-analyzer` into `a2a-toolchain`, `CodexNative` delivery inside the container, a shared-target *volume*, and `didChangeWatchedFiles`-on-edit wiring (Q4). Separate spec.
- **Slice C — multi-language** (gopls/pyright/tsserver). The shim's `--lang` flag and tool surface are designed to generalize, but only `rust` ships now. Non-Rust repos are smaller and pyright/tsserver re-analyze anyway (less warm-index payoff).
- **Cross-session idle-evict / hibernate daemon.** The disk cache makes per-session restart sub-second; a persistent server is unjustified until proven needed.
- **Gateway-container / shared daemon.** Killed by the Q3 macOS boundary; not worth the complexity while the bridge runs ~one implementor at a time.
- **Building the future skills** (prism-nav, rust, code-review, architecture). The library and pattern are established here; those skills are authored later.

## Error handling & edge cases

- **rust-analyzer fails to spawn / crashes mid-session:** the shim returns a structured tool error ("indexer unavailable: <reason>"); the reviewer degrades to prism + reading files (it already has both). Never blocks the turn.
- **Not-yet-indexed query:** `workspace/symbol` before "Database loaded" returns empty; the shim waits for the project-loaded signal (bounded, ~10 s cold / <1 s warm) before answering, and reports "indexing…" past the bound rather than a false empty.
- **Cold-cache first build:** logged; the per-repo target lock serializes concurrent same-repo reviewers once.
- **Non-Rust / non-cargo repo passed to the rust shim:** detect "no Cargo.toml" and return a clear error rather than hanging.
- **Skill does not auto-activate:** the inlined prompt pointer is the always-on fallback; the tools work regardless of whether the skill fired.
- **Memory pressure:** one warm server is ~2–3 GB; the review fan-out shares one server per clone (Q1). Document that concurrent reviews of *different* repos sum memory.

## Testing & DoD

- **Pure units** (shim, where it has a Rust core): name→position resolution, result shaping, per-repo cache-key derivation, project-loaded gating.
- **Live dogfood gate** (the bridge validating itself, as every slice has):
  1. Wire `lsp` into the host-side claude + codex reviewers; install the lsp-nav skill via the library.
  2. Run a real `implement-review` on an a2a-bridge change.
  3. **DoD:** a reviewer demonstrably issues a nav tool call (e.g., `references` on a changed `pub fn`) during the review — confirmed in the bridge logs — and the skill is shown to activate (or the inlined fallback drives the same usage). The macOS UDS boundary (Q3) is documented so Slice B starts from the right assumption.
- **Cross-agent skill check:** confirm the lsp-nav skill is discovered by claude (settingSources) and codex (`~/.agents/skills`) host-side; kiro discovery via the `resources` field is verified when kiro next runs.

## Open questions / risks

- **Skill auto-activation reliability across agents** — mitigated by the inlined prompt fallback; the live gate measures real activation.
- **Per-repo cache-key choice** — **resolved: `origin.url` with a path-hash fallback** (rationale in Architecture: the key is a reuse boundary, not a re-index trigger; a `Cargo.lock`-hash key would force cold rebuilds on every dep bump). Revisit only if clones without a remote *and* with colliding source paths appear.
- **`~/knowledge-ref` path** — adopted as proposed; the bridge install step treats it as configurable.
