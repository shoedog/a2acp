# W2d note — codex-acp `models` surface and the gpt-5.5/sol thread

The 2026-07-10 handoff asked why `gpt-5.5` survived the bridge's legacy
`configure_model_option` / `session/set_config_option(model, ...)` path while
`gpt-5.6-sol` appeared to crash. Current repo evidence changes that premise:
`docs/superpowers/2026-07-11-gpt56-sol-container-root-cause-correction.md` records
that the real container failure happened before `session/new`, during an unnecessary
`authenticate("chat-gpt")` browser-login action in a pre-authenticated container.

Against the current pinned container (`@agentclientprotocol/codex-acp=1.1.2`,
`@openai/codex=0.144.1`), codex-acp still advertises the legacy `model` and
`reasoning_effort` config options as well as the newer `models` field, and correctly
shaped `session/set_config_option` calls for `gpt-5.6-sol` and `xhigh` succeed.
So the direct answer is: `gpt-5.5` did not survive due to a special resolver
exception; the legacy path itself remained valid for codex-acp 1.1.2, and the
observed sol failure was an authentication/pre-session issue, not a model-selection
API rejection.

This W2d change still adds a feature-detected `models` compatibility path because
the Rust ACP SDK currently drops that response field. When an agent advertises
`models.availableModels`, bridge-acp now validates against that advertised catalog
and uses `session/set_model`, folding requested effort into effort-suffixed model
IDs such as `gpt-5.6-sol[xhigh]`. Agents without `models` continue to use the
legacy config-option path.
