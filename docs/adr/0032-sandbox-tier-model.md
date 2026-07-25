# ADR-0032 — Sandbox Tier Model (Formalizing the De-Facto Two-Tier Posture)

**Date:** 2026-07-03
**Status:** Accepted

**Builds on:** ADR-0013 (containment + egress lockdown), ADR-0016 (Slice A `:ro` readers), ADR-0017 (the
enforced `[sandbox]` block), ADR-0018 (Slice B2a `:rw` `ContainerRwBackend`), ADR-0024 (warm loop —
codex's in-container bwrap absence), ADR-0030 (podman runtime seam). Docs/config only — no Rust changes.

---

## Context

The container stack (ADR-0013, ADR-0016–0018, ADR-0021, ADR-0030) is calibrated for **untrusted
content**: `:ro` kernel mounts (the only hard read-only guarantee, since no agent CLI can be reliably
tool-restricted via flags — `claude-agent-acp` has none at all), default-deny egress, the two-layer
`[sandbox]` validation, and the `:rw` quarantine-clone + creds-XOR-egress verify split for write-capable
work. Every load-bearing layer there traces to a concrete hole that actually existed (`docs/2026-07-03-
strategic-analysis.md` §4).

But the repo's **own** development never ran that way. Slices 0–10 were built by codex on the **host**,
under `sandbox_mode="danger-full-access"`, with only a prompt-level "do not commit" — eleven
`*-impl-codex.toml` configs prove it (`docs/2026-07-03-strategic-analysis.md` §4). Review configs ran
host-side too, using codex's *native* `sandbox_mode="read-only"` sandbox or, for claude (which has no
such flag), a bare prompt contract. This is a real, working **second tier** — trusted-own-repo,
host-native — that no ADR ever named. Leaving it unnamed is the actual risk: the strategic analysis and
the container-posture review both concluded the posture is **not overprotective, but under-enforced**
where it matters (`[sandbox]` is opt-in per entry with no name-only fallback containment; the "no commit"
host constraint is prompt-level only, unenforced; `mount`/`allowed_cwd_root` are boot-fixed; `run-workflow`
doesn't enforce the cwd gate) — formalizing the two-tier reality, not relaxing the container tier, is the
fix.

## Decision

Four named tiers. Each is defined by what actually enforces it (kernel mount, agent-native flag, or
prompt text only), the content class it is approved for, and its known gap:

| Tier | Enforced by | Approved for | Known gap |
|---|---|---|---|
| **0 — tools-off** | Prompt contract only (workflow `inputs=[]` + an explicit "no tools, everything you need is inlined" prompt clause). No kernel/agent-native mechanism at all. | Inlined-context review of **any** content, including adversarial — the payload is pasted text, not a mounted filesystem, so even a fully compromised agent has no repo to reach. | Nothing stops a tool-using agent (claude/codex/kiro are all full coding agents) from *attempting* a tool call if it disregards the prompt; whatever the process's ambient cwd happens to be is reachable if it does. Dogfooded, not kernel-proven. |
| **1 — host + agent-native sandbox** | The agent's own sandbox, invoked via CLI flags on an un-containerized host process. codex: `-c sandbox_mode="read-only" -c approval_policy="never"`. Unaudited by the bridge — it trusts the flag, it does not verify codex honored it. | Read-only work on **trusted own-repo content only.** | No kernel isolation at the bridge layer, no egress lock — a prompt-injected instruction in adversarial content could still exfiltrate over the host's normal network. **claude has no equivalent flag; claude's Tier-1 ceiling is Tier 0.** kiro has no host-native RO flag in this repo either — it only appears at Tier 2. |
| **2 — container `:ro` + default-deny egress** | Kernel-enforced: identical-path Docker/Podman `:ro` bind mount + `--network` on an `--internal` net + a default-deny tinyproxy allowlist, all composed by `compose_sandbox` (`bridge-core/sandbox.rs`) and enforced at two validation layers — parse (S0/S2) + snapshot (S1/S3/S4/S5/S6, `bridge-registry/registry.rs`) — so a misconfigured block is a **load error**, not silent degradation (ADR-0017). | Any content an adversary could have authored — third-party PRs, dependencies, issue text, untrusted repos. **Required**, not optional, for this class. | `[sandbox]` is opt-in per agent entry (an entry without it gets only a name-allowlist check); `mount`/`allowed_cwd_root` are boot-fixed (a hot-edit needs a restart); `run-workflow` doesn't enforce the cwd gate (only `serve`+A2A does); the model-endpoint channel stays open (see accepted risk below). |
| **3 — container `:rw` + quarantine clone + verify creds-XOR-egress** | Kernel-enforced `:rw` bind mount to a per-task quarantine clone (`kind="container_rw"` is the only kind that *permits* `access="rw"`); fresh container per turn (or one warm container per `implement` run, ADR-0024), reliably reaped; the **host**, not the agent, runs the commit with hooks neutralized; `[verify]` runs in a *separate* container on a *separate* network with **no creds volumes**, so agent-creds egress and verify-registry egress never coexist in one container. | All write-capable/implement work, **regardless of trust in the target repo** — the threat is agent-authored code nobody has reviewed yet, not the operator's trustworthiness, so even the operator's own repo goes through the full quarantine + split. | Docker/Podman itself is the boundary — a container-escape bug defeats it (accepted, same as Tier 2); no warm-pool yet outside `implement`; review/verify are advisory (operator accepts at merge), not a hard gate. |

**Never-relax list** (from the container-posture review, `docs/2026-07-03-strategic-analysis.md` §4 —
each item is load-bearing regardless of how trusted the content looks):

1. **Verify creds-XOR-egress split** — the `[verify]` container never mounts creds, and the
   creds-bearing agent egress never runs on the verify network. The threat is agent-*authored* code, not
   agent trustworthiness, so this holds even for a fully trusted implementor.
2. **Cred discipline** — isolated, writable copies of OAuth/token files; **never** mount `~`. Near-zero
   cost, closes the single biggest blast-radius mistake.
3. **Symlink canonicalization** on every `:rw` target — `check_rw_target` canonicalizes *both* the mount
   anchor and the target before the containment check, so a symlink can't walk a write outside the root.
   Free; no reason to skip it.
4. **Hook neutralization** on the host-side commit — `core.hooksPath=/dev/null` plus `--no-verify`
   (`--no-verify` alone is insufficient: `prepare-commit-msg`/`post-commit` still fire, and an agent with
   write access could set `core.hooksPath` itself). Git hooks execute arbitrary code at commit time; the
   quarantine clone is otherwise-untrusted output.

**Accepted risk:** the **model-endpoint exfiltration channel** is not closed by any tier, including 2 and
3. An agent (or a prompt-injected instruction it read) can always smuggle data into what it sends the
model provider — that channel *is* the allowed egress target, so it cannot be locked down without
breaking the agent's ability to function. Accepted because the provider is already trusted with the code
at every tier from 0 upward (talking to the model is the whole point); this is orthogonal to
containment, not a gap in it (ADR-0013).

## Consequences

- **Host-run `danger-full-access` implement configs are retired from the sanctioned examples.** The
  eleven `*-impl-codex.toml` configs that built slices 0–10 ran codex on the host, fully write-capable,
  with only an unenforced prompt-level "do not commit." That practice does not fit any of the four
  tiers cleanly — it is host-side like Tier 1, but write-capable like Tier 3, with none of Tier 3's
  quarantine/verify/hook machinery and none of Tier 1's read-only guarantee. It is documented here,
  once, as a **Tier-1-adjacent escape hatch this repo alone used to bootstrap itself** before the
  containerized `:rw` path (Tier 3) existed — not a sanctioned pattern going forward, for this repo or
  any other, and not present in the tiers preset or the kept example configs.
- **`sandbox_mode="danger-full-access"` is explicitly NOT the same finding when it appears *inside* a
  `kind="container_rw"` agent** (the kept `a2a-bridge.containerized.toml`'s `impl` agent). That is
  **Tier 3, and remains sanctioned**: Docker/Podman is the security boundary there, not codex's own
  sandbox. codex's internal bubblewrap sandbox is **absent from the toolchain image** — the earlier
  in-container "repo-blindness" finding was bwrap failing to initialize, not a containment gap (ADR-0024)
  — so disabling it with `danger-full-access` is *correct*, not a downgrade. The string's presence alone
  must not read as a violation; the kind (`container_rw` vs raw `acp`) is what carries the tier.
- **The kept containerized config's reviewers are intentionally mixed-tier**, not uniformly Tier 2:
  claude runs prompt-contract Tier 0 (host-side, no RO flag exists for it), codex runs agent-native
  Tier 1 (host-side `sandbox_mode="read-only"`), kiro runs container Tier 2 (`[agents.sandbox]`
  `access="ro"`). This ADR labels that combination **"trusted-own-repo posture"** — a deliberate choice
  for reviewing this repo's own code, not a template for reviewing third-party content. The
  `examples/a2a-bridge.tiers.toml` preset (below) provides the pure-Tier-2 alternative — every reader
  uniformly containerized — for whenever the content under review is not fully trusted.
- Future `[sandbox]`-less agent entries should state their tier in a comment at the point of definition
  (as the tiers preset and `a2a-bridge.containerized.toml` now do), so the trust boundary is visible at
  the config, not only in this ADR.

## Presets file

`examples/a2a-bridge.tiers.toml` — one `[[agents]]` entry per tier (`tier0-review`, `tier1-codex-ro`,
`tier2-reader`, `tier3-impl`), each carrying a `# Tier N — approved for: …` comment and flags copied
verbatim from the shipped configs (`a2a-bridge.workflows.toml` for the Tier 0/1 base cmds,
`a2a-bridge.containerized.toml` for the Tier 1 read-only args and the Tier 2/3 `[agents.sandbox]`
blocks) — nothing invented. It is a parse-level reference, not a runnable `serve` config (no
workflows/prompts): `a2a-bridge validate --config examples/a2a-bridge.tiers.toml`.

## Operational clarification (2026-07-11) — degraded containers and host fallback

For **trusted own-repo read-only** work, Tier 0/1 host execution is a normal operating mode, not an
emergency security bypass. Containerized Tier 2 is opt-in defense-in-depth for this content class.
When container infrastructure is degraded, an operator may explicitly rerun the work through an
eligible host entry after confirming the input is trusted own-repo content.

This does not permit a silent runtime downgrade:

- Third-party or otherwise untrusted content still requires Tier 2 and fails closed when container
  isolation is unavailable.
- Write-capable `implement` work still requires Tier 3, including on an operator-owned repo. A host
  write fallback would change this ADR's safety decision and requires a separate owner-approved ADR.
- A generic agent/model/prompt failure is not proof that the container boundary is degraded. Fallback
  is eligible only for classified infrastructure failures such as runtime, image, network, mount, or
  container credential setup.
- Once a prompt may have been accepted, the bridge must not replay it automatically on the host; doing
  so can duplicate cost or side effects. An operator must make the retry decision with the first
  attempt's phase and terminal state visible.

A future automated fallback policy must therefore be explicit in config/request state, carry a trusted
content classification, name the permitted host target, emit an audit event explaining the downgrade,
and default to disabled. The current bridge does not implement that automatic policy.
