# Containerized Agents — Warm-Pool Slice (future): warm `:rw` containers across serve turns

**Date:** 2026-06-05
**Status:** Deferred / captured (NOT scheduled). Split out of B2a after the dual spec-review showed warm-
per-session is materially more concurrency surface than B2a needs.
**Depends on:** B2a (the per-turn `ContainerRwBackend`) shipping first.

## Why this is its own slice (not B2a)

B2a ships **per-turn** `:rw` containers (fresh container per `prompt`, stream owns the reaper). That is the
design **both** spec reviewers (containerized dogfood + a2a-local codex `gpt-5.5`) validated as correct, and
it fully unblocks B2b (the `implement` workflow runs as **per-node sessions = single-turn = per-turn**, so
it never needs warmth).

Warmth only benefits the **interactive `serve` tweak-loop** (one A2A session, many `SendMessage` turns) by
preserving **conversational memory** across turns. It is *not* needed for work continuity: the
**run-context-owned clone** (B2b) already carries "task n+1 builds on n" through the filesystem. So warmth
is a UX enhancement, deferred until the per-turn foundation is proven.

## The blocker that killed "warm via `forget_session`"

The original B2a draft assumed `forget_session` fires at **session end**, so reaping there would keep the
container warm across turns. **Both reviews refuted this (verified):**
- `server.rs:80–88` documents the binding guard as *"eviction on EVERY producer exit"*, and the producer
  exits at the end of **each turn's stream** (`server.rs:1066–1080`, after the terminal `Done` frame that
  `AcpBackend::prompt` always emits — `acp_backend.rs:177,1202–1211`). So `forget_session` is a
  per-turn/per-binding signal, **not** session-end.
- codex adds: that `forget_session` runs in a **spawned `Drop` task** that may not even run at runtime
  shutdown (`server.rs:102–105`).

So reaping on `forget_session` would destroy the container after every serve turn — the opposite of warm.
`forget_session` must stay **stash-only** (uniform with the ACP/API/test backends, which never reap there).

## The design this slice must build (warm-pool with idle/TTL eviction)

Because a writer's per-task `:rw` target differs per session, it **cannot** multiplex N sessions over one
child the way the `:ro` readers do. And no reliable per-session-end event exists. So warm writers need a
**bounded warm-pool with idle/TTL eviction** — the same shape the **retired `bridge-claude` warm-pool**
used (prior art to lift: tag `bridge-claude-retired` / commit `15f89ac`; see [[v3c-claude-shipped]]).

Required pieces (all surfaced by the reviews):
1. **Warm-pool** `HashMap<SessionId, WarmContainer>` with a last-used timestamp; reuse on a warm hit.
2. **Idle/TTL reaper** — a background task evicts containers idle > TTL (reap `docker rm -f`); reap also on
   `retire`. This replaces the (nonexistent) session-end trigger. `forget_session` stays **stash-only**.
3. **Exactly-once mint** — `OnceCell`/in-flight-entry around spawn so concurrent first-prompts on one
   session don't double-mint or name-conflict (mirror `AcpBackend`'s per-session `OnceCell` + `turn_lock`,
   `acp_backend.rs:852–873,1216–1224`).
4. **Spawn-failure reap** — `AcpBackend::spawn` starts the `docker run` client *before* the handshake can
   fail (`acp_backend.rs:481`, `process.rs:24`); if the handshake fails the container must be reaped even
   though it was never inserted into the pool (else it orphans).
5. **Warm-hit cwd-guard** — on a warm hit, compare the re-stashed `SessionSpec.cwd` to the live container's
   mount; if it changed, **error or re-mint** (don't silently write to the old `:rw` target). Mirror the
   ACP "warm session for a DIFFERENT repo → error" guard (`acp_backend.rs:992`).
6. **Awaited reaper + Drop fallback** — `Drop` can't await and `Supervised::terminate` is async
   (`process.rs:60–80`); define an explicit async `reap()`/eviction path with timeout + best-effort
   logging, with a synchronous best-effort `Drop` only as a backstop.
7. **Falsifiable acceptance gate** — the per-session name is stable, so a "some `a2a-rw-*` present" check
   green-falses on a per-turn re-mint. Assert **container identity** (same container id across two turns of
   one session, plus a nonzero check in the inter-turn gap) — this is the empirical check that catches a
   regression to per-turn.

## Carried over from B2a (the per-turn foundation provides these)
`compose_container_rw`/`reap_argv`/`check_rw_target` (canonicalized), `AgentKind::ContainerRw`, the
validation arm, the spawn injection seam, and the owner-token boot-sweep all land in B2a and are reused
here — this slice adds only the pool + eviction + mint-race + cwd-guard on top.
