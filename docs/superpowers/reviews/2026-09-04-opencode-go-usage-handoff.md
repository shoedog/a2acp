# Handoff — OpenCode Go routing docs and failed provider-free resolution

**Written:** 2026-09-04T23:08:40-0600 · **By:** `/root` Codex session · **Provider:** codex
**Workspace:** `/private/tmp/a2a-opencode-go-docs-20260905` · `docs/opencode-go-usage-20260905` · **Measured state:** `[MEASURED]` HEAD `efedd893309379fa6c4592ce21cd043feb4d4e8f` · Tree DIRTY · Probe `git status --short --branch` · Exact four-file repo payload is staged before the first custody commit
**Predecessor:** current `/root` provider-refresh session; no separate predecessor id is exposed
**Truth ordering:** measured live state > explicit owner/contract authority within its scope > this handoff for current operational state > earlier handoffs and non-authoritative summaries. A conflict between tiers stays OPEN in §0 — never resolved by document class alone.
**Provenance:** written live by the worker. `[MEASURED]` claims were probed by this writer; `[INHERITED]` claims were not.

## 0. Gating facts — settle these before starting anything below

**(a) Lane ownership** — `[MEASURED]` `list_agents` shows only `/root` running; both inherited review agents are complete — **RESOLVED 2026-09-04T23:00-0600**
**(b) Custody exposure** — `[MEASURED]` allowlist/docs/example/handoff are staged but uncommitted in this worktree; `/Users/wesleyjinks/bridge-usage.md` is an updated non-Git file with SHA-256 `a7e2a9e823c082443d86d58a97c79c13c742a52bda8323a8bfeeb5524d0b31d8` — **OPEN until repo commit**
**(c) In flight / irreversible** — `[MEASURED]` the one authorized resolution terminated failed and `orb status` reported `Stopped`; no provider prompt ran — **RESOLVED; preserve the failed bundle and do not retry**
**(d) Authorization granted but not exercised** — owner: "authorized. I also need you to verify opencode-go usage and models and document in the usage guide in this repo and the markdown file at ~/ and orobably put an example config in." Owner policy: "OpenRouter should jsit uae free models. Opencode go can ise any of the opencode-go subscription models thay are provided im the plan". No authorization in this slice permits a provider prompt, resolution retry, service restart, promotion, or docs-branch push.

## 1. Resume order

1. Make the first local custody commit, create a detached clean verification worktree under `/Users/wesleyjinks/code`, and run `cargo test --workspace --all-targets --no-fail-fast`; record totals.
2. Re-read `docs/onboarding.md`, `examples/a2a-bridge.opencode-go.toml`, and `/Users/wesleyjinks/bridge-usage.md`; require the five-route table and no stale `ox-alpha` claim.
3. Run a bounded self-refutation against the changed docs, refresh this handoff, and locally commit the repo files after green gates. Stop before push unless the owner binds a destination and payload.
4. For the provider refresh lane, inspect `/private/tmp/a2a-bridge-floating.0GKeJ5/resolution/resolution.json`; any new attempt requires a new output directory and separate exact resolution authorization after OrbStack stability is established.

**STOP conditions:** any provider/model prompt, billable smoke, resolution retry, operator restart, promotion, shared-tag change, or docs-branch push lacks authority and must stop for the owner.

## 2. State ledger

| Item | State | Evidence / correction |
|---|---|---|
| OpenCode CLI/auth inventory | done | `[MEASURED]` OpenCode `1.18.27`; `opencode auth list` saw OpenCode Go/Zen, OpenRouter, and Ollama Cloud environment providers with zero saved credentials |
| OpenCode Go model subset | done | `[MEASURED]` one refreshed then cache-read `opencode models opencode-go` returned exactly 27 `opencode-go/...` IDs; no prompt |
| Namespace discrimination | done | `[MEASURED]` `openrouter/openrouter/free` exists; `opencode models ollama` returned `Provider not found: ollama` |
| Bridge ACP catalog gate | done | `[MEASURED]` merged candidate `models --agent opencode-go --json` reported `model_configurable: true`, modes `build|plan`, and all 27 OpenCode Go IDs; discovery intentionally left current `opencode/big-pickle` |
| Repo guide and example | done | `[MEASURED]` `docs/onboarding.md`, sorted artifact-allowlist entry, and `examples/a2a-bridge.opencode-go.toml`; exact merged candidate validation passed with 1 agent, 0 workflows, 0 prompts |
| Home operator guide | done | `[MEASURED]` `/Users/wesleyjinks/bridge-usage.md` updated; SHA-256 above |
| Static local gates | done | `[MEASURED]` fmt, clippy with warnings denied, staged-diff check, and repo hygiene passed; hygiene validated 41 artifacts and 9 example configs |
| Full workspace suite | next | `[UNKNOWN]` not yet run after the docs edits; trusted-root clean checkout required for path-identity tests |
| Provider-free Codex/Claude resolution | blocked | `[MEASURED]` artifact state `failed`, id `r3c-2fzkujv4dhvnvtqbgaw1yjnl`, code `runtime_nonzero`; OrbStack stopped before Docker image-tag query |

## 3. Corrections to standing documents and memory

| Location | Stale or false assertion | Correction |
|---|---|---|
| `/Users/wesleyjinks/bridge-usage.md` | OpenCode `1.18.21`, `opencode-go/ox-alpha-free`, and a nonexistent merged containerized example | `[MEASURED]` corrected to `1.18.27`, current namespace/model list, and `examples/a2a-bridge.opencode-go.toml` |
| `docs/onboarding.md` | no distinction between OpenCode Go, OpenRouter through OpenCode, and Ollama through OpenCode | `[MEASURED]` five-route table and live discovery commands added |
| `docs/superpowers/reviews/2026-09-04-provider-refresh-secret-scan-handoff.md` in the local-only provider worktree | previously preceded the failed authorized resolution | `[MEASURED]` reconciled in local-only commit `e929fa26` |
| Memory | no update requested | `[INHERITED]` global instruction prohibits memory writes without explicit owner request |

## 4. Open work

| # | Work | State | Exact next action | Blocked by | Identifiers |
|---:|---|---|---|---|---|
| 1 | Full workspace suite | next | Commit, create trusted-root detached worktree, and run the exact suite | none | worktree above |
| 2 | Provider handoff reconciliation | done | None | none | local-only commit `e929fa26` |
| 3 | Docs branch publication | parked | Obtain explicit payload/destination push authority | owner authorization | `docs/opencode-go-usage-20260905` |
| 4 | New provider-free resolution | parked | Establish stable OrbStack, bind new root, request exact authority | owner authorization | failed root `/private/tmp/a2a-bridge-floating.0GKeJ5` |

## 5. Invariants and traps — do not do these

- Never retry the failed resolution automatically — the runbook requires a new private root and separate exact authority.
- Never treat an OpenCode ACP agent ID as a provider filter — its catalog combines every provider visible to OpenCode.
- Never use `openrouter/free` through OpenCode — the ACP model ID is `openrouter/openrouter/free`.
- Never claim local Ollama is configured from an `ollama/...` string alone — current CLI evidence says the provider is absent.
- Never treat a catalog listing as live-inference evidence — no provider prompt was authorized or sent.
- Sandboxed OpenCode commands fail while opening their normal log → run provider-free CLI metadata probes with the explicitly approved host scope.

## 6. Identifiers

| Item | Verbatim |
|---|---|
| Base/merged main | `efedd893309379fa6c4592ce21cd043feb4d4e8f` |
| Docs worktree | `/private/tmp/a2a-opencode-go-docs-20260905` |
| Example | `/private/tmp/a2a-opencode-go-docs-20260905/examples/a2a-bridge.opencode-go.toml` |
| Home guide | `/Users/wesleyjinks/bridge-usage.md` |
| Failed resolution id | `r3c-2fzkujv4dhvnvtqbgaw1yjnl` |
| Failed artifact | `/private/tmp/a2a-bridge-floating.0GKeJ5/resolution/resolution.json` |
| Failed artifact SHA-256 | `e52e242ae2bc9e3d4a9ec19c7023fc12516796ef6e6ef06f549bcec22e09553a` |
| Exact merged candidate | `/private/tmp/a2a-provider-refresh-runtime-dispatch-20260905/target/release/a2a-bridge` |
| Candidate SHA-256 | `030d68f22fe81d87c78620294100f9e020079c37e457798a4d9a2e2f39bb27bc` |

## 7. Refutation verdict and owner questions

**§2c verdict:** SURVIVED — claim: "the new example unambiguously selects a current OpenCode Go subscription model and cannot be confused with OpenRouter or Ollama routing" · pass: SELF-PASS (NOT INDEPENDENT) · evidence tier: STATIC + NO-PROMPT CATALOG · refutation searched for stale model/version claims, wrong provider prefixes, an absent configured model, and an invalid example; none survived · full workspace suite remains pending

**Questions the owner owes an answer to:** 1. After the docs gates and local commit, should this branch be pushed and opened as a PR? 2. Should a later provider-free resolution be attempted after separately stabilizing OrbStack, using a new private output root?
