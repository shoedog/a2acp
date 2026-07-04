# M2 — Release Engineering (spec, v1)

**Status:** Draft, pending codex xhigh spec-review.
**Source:** strategic-analysis M2, pulled forward by the identity ruling ("personal tool, published well") — the first concrete publish step. Repo is public; zero version tags exist today.
**Branch:** `feat/m2-release-eng`.

## M2-A: Version + CHANGELOG

1. Bump `[workspace.package] version` 0.1.0 → **0.2.0** (every member inherits). Rationale:
   the AGPL relicense + waves 1–3 are a real boundary; 0.1.0 becomes the name for the
   pre-relicense era.
2. New `CHANGELOG.md` (Keep-a-Changelog format, newest first):
   - `[0.2.0] - 2026-07-03`: relicense Apache-2.0→AGPL-3.0-only + CLA; wave 1 (WAL
     pragmas, `Configuring` claim state/lock fix, async worktree git, CI/MSRV pins);
     wave 2 (README rewrite, artifact purge to docs/history, ADR-0032 tier model);
     wave 3 (uniform `--help`, **BREAKING: silent config auto-write removed** — bare
     `a2a-bridge`/`serve`/`mcp` without a config now error; `a2a-bridge doctor`; A2A
     golden wire tripwire). Cite merge SHAs.
   - `[0.1.0]`: one-paragraph historical summary (the pre-relicense increments/slices;
     point at docs/adr/ + docs/history/ for the archaeology). No retro-tagging of 0.1.0.
3. README: a short "Releases" note in the install section (see M2-C).

## M2-B: `.github/workflows/release.yml`

**Triggers:** `push: tags: ['v*']` (real release) + `workflow_dispatch` (dry-run: build
and upload artifacts to the workflow run only, NO release created) — the dispatch mode
is the pre-merge live gate.

**Targets (3):**
- `aarch64-apple-darwin` (macos-14 runner — the dev platform)
- `x86_64-unknown-linux-gnu` (ubuntu-24.04)
- `aarch64-unknown-linux-gnu` (ubuntu-24.04-arm runner — native, avoids cross-compiling
  the bundled SQLite C amalgamation; if that runner tier is unavailable to this repo,
  CUT this target rather than adding a cross toolchain this wave)

**Per-target job:** checkout; dtolnay/rust-toolchain@master with `toolchain: 1.94.0`
(same pin discipline as ci.yml); `cargo build --release -p a2a-bridge -j 2`; smoke:
`target/release/a2a-bridge --help` exits 0; package
`a2a-bridge-v{VERSION}-{target}.tar.gz` containing the binary + LICENSE + README.md;
emit `.sha256` per artifact.

**Release job (tag trigger only):** needs all target jobs; verifies the tag version ==
`[workspace.package].version` (fail loudly on mismatch); assembles `SHA256SUMS`;
`gh release create "$TAG" --title --notes-file <(extract the CHANGELOG section)` +
uploads. Uses only `GITHUB_TOKEN` with workflow-level `permissions: contents: write`
(nothing else); no third-party release actions (gh CLI only); actions pinned to major
tags consistent with ci.yml's existing style.

**Security posture (normative):** no secrets beyond `GITHUB_TOKEN`; no `pull_request`
trigger overlap; no script injection via tag names (quote `$GITHUB_REF_NAME`); release
notes come from the committed CHANGELOG, not from workflow inputs.

## M2-C: Install paths

1. `bin/a2a-bridge/Cargo.toml`: `[package.metadata.binstall]` with
   `pkg-url = "{ repo }/releases/download/v{ version }/a2a-bridge-v{ version }-{ target }.tar.gz"`,
   `bin-dir = "a2a-bridge"` shape per cargo-binstall conventions (verify the exact
   template variables against binstall's documented defaults — the archive layout must
   match what M2-B produces).
2. README install section rewritten to a 3-option table: (a) prebuilt binaries from
   GitHub Releases (+ the macOS Gatekeeper caveat: unsigned binary, `xattr -d
   com.apple.quarantine` or right-click-open — signing/notarization explicitly out of
   scope, documented); (b) `cargo binstall a2a-bridge` — NOTE: binstall from git repo
   requires `--git` since the crate is NOT on crates.io (crates.io publishing is a
   separate, deliberate non-goal this wave — AGPL + workspace-dep hygiene decision
   deferred); state the exact working command; (c) `cargo install --path bin/a2a-bridge`
   from source (existing).

## Explicit non-goals (this wave)

crates.io publishing; Windows builds; homebrew tap; macOS signing/notarization;
cargo-dist migration (noted as the upgrade path if target count grows); release
automation beyond tag-push (no auto-tagging).

## Definition of done

1. On the branch: version bump, CHANGELOG, release.yml, binstall metadata, README
   install section.
2. Gates: ci.yml green (fmt/clippy/test/deny/hygiene unaffected by version bump —
   verify Cargo.lock updates cleanly); `actionlint` on release.yml if available
   locally, else careful review.
3. **Live gate (pre-merge):** `workflow_dispatch` dry-run on the branch builds all
   targets and uploads run artifacts; download the macOS artifact, extract, run
   `./a2a-bridge --help` → exit 0.
4. Whole-branch dual review (opus + codex xhigh — codex focus: workflow supply-chain
   posture + binstall template correctness) → fold → merge.
5. **Post-merge:** tag `v0.2.0` on the merge commit, push the tag, verify the release
   appears with 3 artifacts + SHA256SUMS; then update README's release badge/link if
   any step revealed drift (follow-up commit if needed).

## Risks

- Runner availability (ubuntu-24.04-arm) — cut-don't-cross rule above.
- Bundled SQLite (cc) per target — native runners sidestep it; the smoke step catches
  a broken binary before release.
- Version/tag mismatch — guarded by the explicit check in the release job.
- binstall template drift vs artifact naming — the review + a documented manual
  binstall test post-release.
