# LSP-MCP Go (gopls) host-path proof (Task 1)

**Date:** 2026-06-15 · **Verdict: host path is viable; ZERO bridge plumbing changes needed beyond one `LangServerConfig`. Proven `program_argv = ["gopls", "serve"]` (bare `gopls` ALSO works — choose `serve` for explicitness); `spawn_env = []`; readiness = the EXISTING no-progress settle (gopls emits NO `$/progress` — basedpyright-shape, NOT rust-analyzer-shape); the existing reader-thread `workspace/configuration` handler already answers gopls correctly (replies `[null]`, session does not hang).**

Empirically proves the gopls behavior the later Go tasks depend on. Every gate quotes a REAL stdio request/response transcript captured by a throwaway Content-Length-framed driver, not an assertion. The template being matched is the EXISTING `pyright_config` / `PyrightReady` / `Readiness::Pyright` path in `crates/lsp-mcp/src/lang.rs` + the `handshake`/`wait_ready`/`build_server_reply`/`classify` consumer in `crates/lsp-mcp/src/lsp/mod.rs` — Go reuses that seam verbatim and just adds one `LangServerConfig`.

## Environment

- **go:** `go version go1.26.2 darwin/arm64` at `/Users/wesleyjinks/.local/share/mise/installs/go/1.26.2/bin/go` (mise-managed).
- **gopls:** `golang.org/x/tools/gopls v0.22.0` (`gopls version`) at `command -v gopls` = `/Users/wesleyjinks/.local/share/mise/installs/go/1.26.2/bin/gopls`. On `$PATH`.
- **`go env`:** `GOPATH=/Users/wesleyjinks/go`, `GOROOT=/Users/wesleyjinks/.local/share/mise/installs/go/1.26.2`, `GOFLAGS=` (empty), `GOMODCACHE=/Users/wesleyjinks/go/pkg/mod`, `GOBIN=` (empty).
  - NOTE on this host: mise installs `go` AND `gopls` into `$GOROOT/bin` (= the same dir that is on `$PATH`). `$GOPATH/bin` (`/Users/wesleyjinks/go/bin`) does **not exist** (`go install` has never been run). The module cache `$GOMODCACHE` exists and holds the third-party dep.
- **Driver:** throwaway stdio JSON-RPC client at `/tmp/gospike/drive_gopls.py` (LSP `Content-Length: N\r\n\r\n{json}` framing — the SAME framing `crate::lsp::codec` uses to talk to the language server; NOT the newline-delimited MCP-side framing). It answers server→client requests (`workspace/configuration` → array; everything else → `-32601`), mirroring the bridge's `build_server_reply`. A second throwaway driver `/tmp/gospike/drive_bridge_reply.py` replays the bridge's EXACT reply shape (`[null]` for the non-python `gopls` section) to prove Gate 4a against the real handler shape.
- **Target module (throwaway):** `/tmp/gospike` — `go mod init example.com/gospike` + `main.go` importing one third-party package `github.com/google/uuid` (`go get github.com/google/uuid` → v1.6.0, `go mod download`). `go build ./...` = OK. Probe symbols: in-workspace `Greeter`/`Greeter.Greet`; stdlib `fmt.Println`; third-party `uuid.New`. The third-party dep landed in the module cache at `/Users/wesleyjinks/go/pkg/mod/github.com/google/uuid@v1.6.0/`.

A key protocol fact discovered up front: gopls **pulls** config via a server→client `workspace/configuration` request with `items:[{scopeUri, section:"gopls"}]` (NOT `python`). The bridge's existing `build_server_reply` returns a length-matched array with the non-python item as `null` → gopls accepts `result:[null]` and the session proceeds (Gate 4a). gopls emits NO push/pull for a `python` section, so nothing in the existing python-aware reply path mis-fires.

---

## Gate 1 — exact stdio invocation — `["gopls","serve"]` AND bare `["gopls"]` BOTH yield a working LSP session

Drove gopls over stdio TWICE (`gopls` bare, and `gopls serve`), each: `initialize` (advertising `workspace.symbol` + hierarchical `documentSymbol`, NO `window/workDoneProgress`) → `initialized` → `textDocument/didOpen main.go` → `workspace/symbol` → `textDocument/definition`. Both variants returned identical, correct results.

```
// argv=["gopls"]:
+0.001s -> initialize
+0.046s <- RESPONSE id=1 result={capabilities:{definitionProvider:true, referencesProvider:true,
                                  implementationProvider:true, documentSymbolProvider:true,
                                  callHierarchyProvider:true, hoverProvider:true, workspaceSymbolProvider:…}}
+0.046s initialize OK
...
+0.210s workspace/symbol 'Greeter' returned in 0.164s with 92 hits: ['Greeter','Greeter.Name','Greeter.Greet', …]
+0.325s definition(uuid.New) returned 1 location  [MODULE CACHE]
+0.351s stdlib definition(fmt.Println) -> .../go/1.26.2/src/fmt/print.go  [STDLIB resolved]
+1.553s VERDICT: argv=['gopls'] -> WORKING LSP session

// argv=["gopls","serve"]:
+0.038s <- RESPONSE id=1 result={capabilities:{…}}  initialize OK
+0.220s workspace/symbol 'Greeter' returned in 0.167s with 92 hits
+0.349s definition(uuid.New) returned 1 location  [MODULE CACHE]
+0.379s stdlib definition(fmt.Println) -> .../src/fmt/print.go  [STDLIB resolved]
+1.567s VERDICT: argv=['gopls','serve'] -> WORKING LSP session
```

The `initialize` result advertises every capability the bridge's 7 nav tools need: `definitionProvider`, `referencesProvider`, `implementationProvider`, `documentSymbolProvider`, `callHierarchyProvider`, `hoverProvider`, `workspaceSymbolProvider`.

**PROVEN `program_argv` (Task 4):** `["gopls", "serve"]`. Bare `["gopls"]` is byte-for-byte equivalent here (gopls's default subcommand IS `serve`), but `serve` is the explicit, documented stdio mode and matches the `rust-analyzer` / `basedpyright-langserver --stdio` convention of being unambiguous → **ship `["gopls", "serve"]`** (after `resolve_lsp_server` rewrites `argv[0]` to an absolute path — see Gate 6). Either is correct; `serve` is the recommendation, not a hard requirement.

---

## Gate 2 — env needs — NOTHING must be injected; `spawn_env = []`

gopls resolved BOTH the stdlib import (`fmt.Println`) and the third-party import (`uuid.New`) using ONLY the inherited environment (the driver injected no env; the bridge spawns with `current_dir(repo)` + inherited env + only `cfg.spawn_env`):

```
stdlib definition(fmt.Println) -> file://.../go/1.26.2/src/fmt/print.go            [STDLIB resolved via GOROOT]
definition(uuid.New)          -> file://.../go/pkg/mod/github.com/google/uuid@v1.6.0/version4.go  [MODULE CACHE via GOMODCACHE]
```

`go env`: `GOPATH=/Users/wesleyjinks/go`, `GOROOT=/Users/wesleyjinks/.local/share/mise/installs/go/1.26.2`, `GOFLAGS=` (empty). gopls discovers GOROOT/GOPATH/GOMODCACHE itself by shelling out to `go env` (it finds `go` on the inherited `$PATH`); it needs no `CARGO_TARGET_DIR`-style injection and no per-language env. The Rust path injects `CARGO_TARGET_DIR` only when a target cache is given; **Go injects NOTHING.**

**Task 4 implication:** `spawn_env = []` (empty `Vec`), exactly like `pyright_config`. The ONLY env requirement is the implicit one already satisfied by the existing spawn: `go` must be findable on the inherited `$PATH` at runtime (it is — same mise bin dir as gopls).

---

## Gate 3 — readiness signal — gopls emits NO `$/progress`; REUSE the no-progress settle (basedpyright-shape)

Grepped BOTH full captured transcripts (`/tmp/gospike/transcript_bare.txt`, `transcript_serve.txt`) for progress signals:

```
$/progress occurrences:           transcript_bare.txt:0   transcript_serve.txt:0
window/workDoneProgress occurrences: 0 in both
ALL notification methods gopls emitted (bare):
   1 NOTIFICATION 'textDocument/publishDiagnostics'
   4 NOTIFICATION 'window/logMessage'
   2 NOTIFICATION 'window/showMessage'   ("Loading packages..." then "Finished loading packages.")
```

gopls signals workspace load via `window/showMessage` ("Loading packages..." → "Finished loading packages.") and `window/logMessage`, NOT `$/progress`/`window/workDoneProgress`. The `Readiness` machine's `on_notification` matches neither of those window methods → for a `Pyright`-shaped machine `began` stays `false`, so the **no-progress settle is the load-bearing readiness path** (identical to basedpyright).

**No-progress request test (the core of the gate):** `workspace/symbol query='Greeter'` was issued IMMEDIATELY after `initialized`+`didOpen` (t≈0.046s) and **RETURNED in 0.164s with 92 hits** — it did NOT pay a full timeout waiting for a progress cycle. (gopls's load of this tiny module finished at ~0.18s; the request was answered the moment indexing settled.) A short post-settings settle window (the existing `PYRIGHT_SETTLE = 1500ms`) comfortably covers this.

**DECISION (Task 4): REUSE the no-progress settle. Implement `Readiness::Gopls(GoplsReady)` mirroring `PyrightReady` exactly** — `settled_at` stamped when `handshake` finishes `initialized` (gopls has no `post_init_config`, so `settled_at` must be stamped after `initialized` even though there is no `didChangeConfiguration` push — see Task-4 note below), a `settled_no_progress(settle)` gate OR'd into `wait_ready` via a `gopls_settled` helper alongside the existing `pyright_settled`. Do NOT parse `$/progress` (rust-analyzer-shape) — gopls never sends it. The belt-and-suspenders begin/end parsing in the Pyright variant is harmless to mirror but never fires for gopls.

> **Task-4 wiring note (settle-clock origin):** `handshake` currently stamps `settled_at` ONLY inside the `if let Some((method,params)) = post_init_config` block (because the Pyright path always has a `didChangeConfiguration` push). Go has **no** `post_init_config` (Gate 4a — gopls pulls, it is not pushed). So Task 4 must stamp the Gopls settle-clock after `initialized` UNCONDITIONALLY for the Gopls variant (e.g. an `else` arm, or stamp for any non-Rust readiness), or `settled_at` stays `None` and the settle never fires → the first call would pay the full timeout. This is the one consumer-side touch Task 4 needs beyond adding the variant; it is additive and does not change the Pyright path.

---

## Gate 4 — `workspace/configuration` server-request + third-party-by-name

### 4a — gopls sends `workspace/configuration`; the EXISTING reader-thread handler answers it correctly and the session does NOT hang

gopls sent exactly ONE server-initiated request method during init (and a second identical one later):

```
+0.047s <- SERVER_REQUEST id=1 method='workspace/configuration'
            params={"items":[{"scopeUri":"file:///tmp/gospike","section":"gopls"}]}
         -> replied workspace/configuration result=[1 x {}]   (driver)
...
+0.210s workspace/symbol 'Greeter' returned in 0.164s with 92 hits   <-- AFTER the reply; session did NOT hang
```

Critically, the requested section is `"gopls"` (NOT `"python"`). The bridge's existing `build_server_reply` (`crates/lsp-mcp/src/lsp/mod.rs`) handles `workspace/configuration` by returning a length-matched array where the python item carries `{pythonPath}` and **every other section is `null`** — so for gopls it returns `result:[null]`. I replayed the bridge's EXACT shape against real gopls (`/tmp/gospike/drive_bridge_reply.py`):

```
initialize: OK
[bridge-shape reply] workspace/configuration sections=['gopls'] -> result=[null]
workspace/symbol AFTER bridge-shape config reply: OK (92 hits) -> session did NOT hang
[bridge-shape reply] workspace/configuration sections=['gopls'] -> result=[null]   (2nd pull, also fine)
```

**Finding:** gopls accepts `result:[null]` for its `gopls` config section and proceeds normally. The `-32601`-for-non-python-section concern in the task brief is a NON-ISSUE: `workspace/configuration` is NOT routed to the `-32601` arm — it has its own dedicated handler that already returns a valid array. (`-32601` would only fire for a DIFFERENT server-request method, which gopls did not send.) The reader thread's existing `classify` correctly tags this as `Inbound::ServerRequest{id, method}` (id+method present) and replies on the shared stdin without touching `pending`.

**Task 4/6 implication: NO reader-thread change is needed for Go.** The existing `build_server_reply` + `classify` already drive gopls correctly. gopls is fully usable with the default `null` config (it falls back to its own sensible defaults). Go's `LangServerConfig` carries `post_init_config: None` (no push needed; gopls pulls and we answer the pull with defaults).

### 4b — `workspace/symbol` DOES return third-party symbols by name; positional def jumps into the module cache

Unlike basedpyright (which indexes only workspace symbols), gopls's `workspace/symbol` **DOES surface third-party symbols by name** — they resolve into the module cache:

```
workspace/symbol query='uuid.New': 15 hits, 15 in module-cache (/pkg/mod/)
   - 'uuid.New'   @ file://.../go/pkg/mod/github.com/google/uuid@v1.6.0/version4.go
   - 'uuid.NewV6' @ .../uuid@v1.6.0/version6.go
   - 'uuid.NewMD5'@ .../uuid@v1.6.0/hash.go   ...
workspace/symbol query='New':  100 hits, 14 in module-cache (a broad query returns stdlib + third-party, capped at 100)
workspace/symbol query='UUID': 100 hits, 82 in module-cache
```

And `textDocument/definition` at the `uuid.New()` USAGE site jumps into the module cache (the consumer's name-addressed nav resolves a name→position via `workspace/symbol` then go-to-def — this works for Go):

```
definition(uuid.New) -> 1 location:
   file://.../go/pkg/mod/github.com/google/uuid@v1.6.0/version4.go  range={line:12,char:5-8}  [MODULE CACHE]
```

**Finding (BETTER than the basedpyright caveat):** gopls's `workspace/symbol` is NOT workspace-only — it indexes the build graph including imported third-party packages, so the bridge's name-addressed API (`definition("uuid.New")`, `hover`, `references`) reaches third-party symbols BY NAME, returning module-cache locations. Two caveats for Task-5 test authoring:
1. A broad bare query (e.g. `"New"`) returns many hits across stdlib+third-party (gopls caps the result list at ~100); a fuzzy/qualified query (`"uuid.New"`) narrows it. Task-5 live tests should query qualified names or assert on a specific hit rather than an exact count.
2. Hit `name`s use Go's qualified form (`Greeter.Name`, `uuid.New`, `vendor/...dnsmessage.Type.String`) and `containerName` carries the package import path (`github.com/google/uuid`, `example.com/gospike`). The consumer's `workspace_symbol` already passes `name` through verbatim — no shape change needed.

So unlike the Python "third-party-by-name is a known gap" note, **Go third-party-by-name WORKS** — a `definition`/`hover` on `uuid.New` returns a real module-cache location, not null. A reviewer of the Task-5 DoD should EXPECT third-party-by-name to resolve for Go.

---

## Gate 5 — `--lang auto` detection LOGIC (no binary) — predicates validated

The Task-2 predicate to add is: a `go.mod` file at the repo root marks **go**. Combined with the existing Rust (`Cargo.toml`) and Python (`setup.py`/`setup.cfg`/`requirements*.txt`/real-`pyproject`/`.py`-scan) markers, the multi-marker matrix is ambiguity-on-collision (mirroring the existing rust+python `bail!`). Constructed the three scenarios on the host and verified marker presence:

| Scenario | Markers present | Correct `detect_lang` verdict |
|---|---|---|
| `gate5/go_only` | `go.mod` only | **go** (single unambiguous root) |
| `gate5/go_rust` | `go.mod` + `Cargo.toml` | **AMBIGUOUS → refuse** (require explicit `--lang`) |
| `gate5/go_py` | `go.mod` + `pyproject.toml` with `[project]` | **AMBIGUOUS → refuse** (require explicit `--lang`) |

```
-- go_only --  go.mod=yes Cargo.toml=no  pyproject[project]=no
-- go_rust --  go.mod=yes Cargo.toml=yes pyproject[project]=no
-- go_py   --  go.mod=yes Cargo.toml=no  pyproject[project]=yes
```

The existing `detect_lang` is a 2-way `match (is_rust, is_python)` that `bail!`s on `(true,true)`. **Task 2 must generalize this to 3-way** (`is_go` from `repo.join("go.mod").is_file()`): exactly one true → that lang; two-or-more true → ambiguous-`bail!`; zero true → cannot-detect-`bail!`. The `go.mod`+`pyproject[project]` case is ambiguous because `[project]` is a REAL python marker per the existing `has_real_pyproject` (a tooling-only `pyproject` would NOT collide). `go.mod` is a single deterministic file check — no scan needed (a Go module ALWAYS has a `go.mod` at its root), so it is cheaper than the Python `.py`-scan and adds no false positives (`go.mod` does not appear in Rust/Python repos).

**Task 2 implication:** the three Go predicates are sound. The two refuse paths (go+rust, go+python) are real and reachable and must `bail!` with "pass an explicit --lang", consistent with the existing rust+python refuse.

---

## Gate 6 — gopls binary resolution — on `$PATH` here; `resolve_lsp_server` extension de-risks off-PATH hosts

Where gopls resolves on this host:

```
command -v gopls            -> /Users/wesleyjinks/.local/share/mise/installs/go/1.26.2/bin/gopls   (ON $PATH)
go env GOROOT               -> /Users/wesleyjinks/.local/share/mise/installs/go/1.26.2
  $GOROOT/bin/gopls         -> EXISTS (-rwxr-xr-x, 41 MB)   [== the command -v location; mise puts both go+gopls here]
dirname $(command -v go)    -> /Users/wesleyjinks/.local/share/mise/installs/go/1.26.2/bin   (the same bin dir)
go env GOPATH               -> /Users/wesleyjinks/go
  $GOPATH/bin               -> DOES NOT EXIST (go install never run; empty)
```

On THIS host gopls is on `$PATH`, so the existing `resolve_lsp_server` (PATH → `~/.local/bin` → `~/.cargo/bin`) already finds it via the PATH branch and rewrites `argv[0]` to the absolute path. **But** the standard off-PATH location for a `go install golang.org/x/tools/gopls@latest`-installed gopls is `$GOPATH/bin/gopls` (or `$GOBIN/gopls`), which the current `resolve_lsp_server` does NOT search — and the bridge spawns the agent's MCP server with a PATH that can lack the go bin dir (exactly the basedpyright `~/.local/bin` gap fixed in C1 commit `4edd134`).

**Task 3 implication:** extend `resolve_lsp_server`/`resolve_lsp_server_with_env` so that for the `gopls` server name it ALSO checks `$GOBIN/gopls` then `$GOPATH/bin/gopls` (and optionally `$GOROOT/bin/gopls`) after the PATH/`~/.local/bin`/`~/.cargo/bin` candidates — reading those via `go env` or the `GOBIN`/`GOPATH` env vars (kept injectable like the existing `path_var`/`home_dir` params so the unit tests stay hermetic). On this host the value is latent (PATH already wins), so the live DoD won't exercise it here; it de-risks containers and `go install` hosts where gopls lands in `$GOPATH/bin` off `$PATH`. The fallback-to-bare-name behavior (degrade to the OS "not found" error, no panic) must be preserved.

---

## Decisions for the plan (summary)

1. **`program_argv = ["gopls", "serve"]`** (Gate 1) — bare `["gopls"]` is equivalent; ship `serve` for explicitness. `argv[0]` is rewritten to an absolute path by `resolve_lsp_server` (Gate 6).
2. **`spawn_env = []`** (Gate 2) — gopls resolves stdlib (GOROOT) + third-party (GOMODCACHE) from the inherited env; the only implicit requirement (`go` on `$PATH`) is already met. No injection.
3. **Readiness = the EXISTING no-progress settle** (Gate 3) — gopls emits NO `$/progress`/`window/workDoneProgress` (only `window/showMessage`/`logMessage`); implement `Readiness::Gopls(GoplsReady)` mirroring `PyrightReady` + a `gopls_settled` OR-branch in `wait_ready`. **Task-4 must stamp the settle-clock after `initialized` even though Go has no `post_init_config`** (the current stamp is gated behind the push) — otherwise `settled_at` stays `None` and the first call pays the full timeout.
4. **NO reader-thread change** (Gate 4a) — gopls's `workspace/configuration` pull (section `gopls`) is already answered correctly by the existing `build_server_reply` (`result:[null]`), proven not to hang. Go's `post_init_config = None`.
5. **Third-party-by-name WORKS for Go** (Gate 4b) — gopls's `workspace/symbol` indexes the build graph; `definition`/`hover`/`references` on `uuid.New` return real module-cache locations (NOT the basedpyright null-by-name gap). Task-5 live tests can assert third-party-by-name resolves; prefer qualified queries (gopls caps broad queries at ~100 hits).
6. **3-way detection** (Gate 5) — generalize `detect_lang`'s 2-way match to 3-way with `is_go = go.mod at root`; one-true → that lang, two+-true → ambiguous-`bail!`, zero → cannot-detect-`bail!`. go+rust and go+python (with a real `[project]`) are ambiguous-refuse.
7. **Extend `resolve_lsp_server` for gopls** (Gate 6) — add `$GOBIN`/`$GOPATH/bin`/`$GOROOT/bin` candidates after the existing PATH/`~/.local/bin`/`~/.cargo/bin` ones (latent on this PATH-having host; de-risks `go install` / container hosts). Keep injectable for hermetic tests; preserve the bare-name fallback.

## Reproduction / artifacts

All throwaway scripts and dirs live under `/tmp/gospike/` and `/tmp/gate5/`: the module (`go.mod`, `main.go`), the framed driver `drive_gopls.py`, the bridge-reply-shape driver `drive_bridge_reply.py`, the captured transcripts `transcript_bare.txt` / `transcript_serve.txt`, and the Gate-5 scenario dirs. No production code landed; nothing was written into `~/code/a2a-bridge` source (only this verdict file). gopls/go stay as the mise-managed install.
