# gpt-5.6-sol container failure — root-cause correction

The 2026-07-10 handoff attributed the containerized `gpt-5.6-sol` failure to a model-selection API
mismatch between bridge-acp and codex-acp 1.1.2. A faithful replay and a real bridge trace falsified
that diagnosis.

## Settled root cause

The bridge failed before `session/new`. After `initialize`, it unconditionally sent
`authenticate(methodId="chat-gpt")` because codex-acp advertised that login method. The container already
had a valid mounted `auth.json`; invoking the browser-login action again attempted `xdg-open` in a
browserless container and the real bridge waited until its handshake timeout.

codex-acp 1.1.2 still advertises legacy `model` and `reasoning_effort` config options in addition to its
new effort-suffixed `models` field. Correctly shaped `session/set_config_option` requests for
`gpt-5.6-sol` and `xhigh` both succeed, so bridge-acp does not need a models-field compatibility shim.

## Evidence that separated the hypotheses

1. The persisted harness originally used the wrong keys (`configOptionId` and `valueId`). Correcting them
   to `configId` and `value` made `gpt-5.5 -> gpt-5.6-sol` selection succeed.
2. Adding the bridge's effort step made the full direct sequence succeed:
   model switch, `reasoning_effort=xhigh`, `agent-full-access`, then a sol prompt.
3. The unmodified bridge trace stopped after outgoing `authenticate("chat-gpt")`; it never sent
   `session/new`.
4. With `pre_authenticated = true`, a real `container_rw` workflow skipped authenticate, applied
   `model=gpt-5.6-sol` and `reasoning_effort=xhigh`, and completed with `SMOKE_OK`. Response usage named
   `gpt-5.6-sol`.

## Fix

- Add an explicit `pre_authenticated` agent setting, default `false`.
- Reject `pre_authenticated = true` together with `auth_method`, and reject it for API agents where it
  would be meaningless.
- Thread the setting through cold, warm, catalog, and container ACP factories.
- Set it on shipped Codex configs that rely on `codex login` or a mounted `auth.json`.
- Keep codex-acp 1.1.2 and the kiro musl image fix from `c95084e`; both remain independently valuable.

The corrected reproduction harness is `docs/superpowers/2026-07-10-acp_drive-sol-repro.py`.
