# SandboxConfig::validate

`SandboxConfig` describes the host paths a containerized agent may bind-mount.
`validate()` is the security gate: every mount must live under `allowed_root`
so an agent cannot mount arbitrary host paths.

Contract:
- Every path that `effective_mounts()` will actually bind-mount MUST pass the
  `under_root(allowed_root)` check in `validate()`.
- A config that mounts anything outside `allowed_root` must be rejected.

The change adds an `overrides` layer (`Overrides.extra_mounts`) so a per-agent
config can add mounts on top of the base list, and updates `effective_mounts()`
to include them.
