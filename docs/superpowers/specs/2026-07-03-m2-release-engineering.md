# M2 — Release Engineering (spec, v2)

**Status:** APPROVED for implementation — v1 reviewed by codex gpt-5.5 xhigh; all findings folded (job-scoped permissions, preflight tag validation, concurrency, verify-tag, binstall template ruling, ring/cc fact, post-merge dry-run flow).
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

**Preflight job (first, cheap):** validate the ref: tag matches `^v[0-9]+\.[0-9]+\.[0-9]+$`
exactly; derive `VERSION=${TAG#v}`; compare against `[workspace.package].version` — fail
BEFORE any target build burns minutes. (Dispatch mode skips the tag checks.)

**Per-target job:** checkout with `persist-credentials: false`; dtolnay/rust-toolchain@master
`toolchain: 1.94.0`; `cargo build --release --locked -p a2a-bridge -j 2`; smoke:
`target/release/a2a-bridge --help` exits 0; package
`a2a-bridge-v{VERSION}-{target}.tar.gz` (binary + LICENSE + README.md at archive ROOT —
the binstall `bin-dir` template depends on this); checksum via `shasum -a 256`
(portable across macOS/Linux runners); upload-artifact with unique names,
`if-no-files-found: error`, `retention-days: 7`.

**Release job (tag trigger only):** `needs` all targets; job-scoped
`permissions: contents: write` (workflow top-level is `contents: read` — least
privilege); assembles `SHA256SUMS`; `gh release create "$TAG" --verify-tag
--title ... --notes-file <(CHANGELOG section)` with `GH_TOKEN: ${{ github.token }}`;
uploads all artifacts. Workflow-level
`concurrency: { group: release-${{ github.ref }}, cancel-in-progress: false }` so a
re-run/re-push for one tag serializes, never races two releases.

**Security posture (normative):** top-level `permissions: contents: read`; write only
on the release job; `persist-credentials: false` everywhere; no secrets beyond
`GITHUB_TOKEN`; exact tag-shape validation (quoting alone is insufficient); release
notes from the committed CHANGELOG only; gh CLI only (no third-party release actions);
actions pinned consistent with ci.yml style.

**Native-dep note (fact-checked):** bundled SQLite (libsqlite3-sys→cc) AND
rustls→`ring` (also cc) both compile C — native runners sidestep cross-toolchain pain
for BOTH; reqwest is `default-features=false` + `rustls-tls` (no OpenSSL anywhere in
the lock).

**Failed-release runbook (in a comment atop release.yml):** if the workflow fails
after the tag exists: fix, re-run the SAME tag's workflow; if a partial release
exists, upload missing assets with `gh release upload --clobber`; NEVER move or
recreate the tag.

## M2-C: Install paths

1. `bin/a2a-bridge/Cargo.toml` (per review ruling): add
   `repository = "https://github.com/shoedog/a2acp"` (required — `{ repo }` reads the
   manifest field, absent today), then:
   ```toml
   [package.metadata.binstall]
   pkg-url = "{ repo }/releases/download/v{ version }/{ name }-v{ version }-{ target }.tar.gz"
   pkg-fmt = "tgz"
   bin-dir = "{ bin }{ binary-ext }"
   ```
   (`bin-dir` is the templated in-archive binary path; binary at archive root per M2-B.)
2. README install section rewritten to a 3-option table: (a) prebuilt binaries from
   GitHub Releases (+ the macOS Gatekeeper caveat: unsigned binary, `xattr -d
   com.apple.quarantine` or right-click-open — signing/notarization explicitly out of
   scope, documented); (b) `cargo binstall --git https://github.com/shoedog/a2acp a2a-bridge` (the exact
   working command — plain `cargo binstall a2a-bridge` does NOT work off-crates.io;
   crates.io publishing stays a deliberate non-goal this wave); (c) `cargo install --path bin/a2a-bridge`
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
3. Whole-branch dual review (opus + codex xhigh — codex focus: workflow supply-chain
   posture + binstall template correctness) → fold → merge. (workflow_dispatch only
   triggers for workflows on the DEFAULT branch — so the dry-run live gate moves
   post-merge, pre-tag.)
4. **Live gate (post-merge, PRE-TAG):** `gh workflow run release.yml` dispatch
   dry-run from main; all 3 targets build; download the macOS artifact, extract,
   `./a2a-bridge --help` → exit 0. Only then:
5. **Tag:** `v0.2.0` on the merge commit, push, verify the release appears with 3
   artifacts + SHA256SUMS; manual `cargo binstall --git …` test; follow-up commit only
   if drift found.

## Risks

- Runner availability (ubuntu-24.04-arm) — cut-don't-cross rule above.
- Bundled SQLite (cc) per target — native runners sidestep it; the smoke step catches
  a broken binary before release.
- Version/tag mismatch — guarded by the preflight job before any build.
- Agent Card / MCP initialize advertise `env!("CARGO_PKG_VERSION")` → 0.2.0 after the
  bump (verified side effect, intended; ORCH_V untouched).
- binstall template drift vs artifact naming — the review + a documented manual
  binstall test post-release.
