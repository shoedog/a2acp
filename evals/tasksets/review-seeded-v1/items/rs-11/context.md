# RunContext::resolve

One `serve` process drives agents across many repositories. Each run carries a
`session_cwd` (the target repo). `resolve(rel)` turns a relative path from the
task-spec into the absolute path the agent will actually touch.

Contract:
- Relative paths MUST resolve under `self.session_cwd` (the configured repo),
  NOT against the bridge process's launch directory. One serve serves many
  repos, so the process cwd is unrelated to (and usually wrong for) the run.

The change adds `resolve`, used to locate task-spec artifacts on disk.
