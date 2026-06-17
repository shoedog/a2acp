# Spike: TS `/node_modules` resolution + warm-fetch (JS/TS slice keystone)

**Date:** 2026-06-17. **Question:** can the JS/TS slice reuse the fixed-path dep-cache machinery
(`dep_cache_path="/node_modules"`, like `/cargo`/`/pyvenv`) instead of needing a cwd-relative
`<cwd>/node_modules` mount (which would nest under the repo mount → S6 issue + a new bridge capability)?

## Spike 1 — resolution: does tsserver/tsc resolve 3rd-party types from a FIXED `/node_modules`?

TypeScript's node module resolution walks `node_modules` up every ancestor directory **to the filesystem
root**, so `/node_modules` is the final candidate. Test (node:24-slim + `tsc`): install
`lodash`+`@types/lodash` ONLY at `/node_modules` (repo at `/repo` has NO local node_modules), then
`tsc --noEmit` on a file importing lodash:

```
$ tsc --noEmit            # from /repo, no local node_modules
TSC-OK: lodash type resolved purely via the /node_modules ROOT WALK
$ tsc --noEmit --traceResolution | grep lodash
Found 'package.json' at '/node_modules/lodash/package.json'.
```

**RESULT: ✅ confirmed.** `dep_cache_path="/node_modules"` works. tsserver shares tsc's resolver → the
containerized nav path resolves 3rd-party types from a fixed `/node_modules` mount. **No cwd-relative
mount, no S6 nesting, no new bridge capability — the fixed-path machinery is reused unchanged.** (Caveat:
a repo with its OWN committed `node_modules` shadows `/node_modules` — acceptable; then its own deps win.)

## Spike 2 — warm-fetch: how to POPULATE the mounted `/node_modules` volume from the repo's lock?

The fetch container mounts the repo at `/work` (package.json + package-lock.json) and the dep-cache volume
at `/node_modules`. We must install `/work`'s locked deps INTO `/node_modules`.

- ❌ `cd /work && npm ci --prefix /` → **EUSAGE**: *"npm ci can only install with an existing
  package-lock.json"* — `--prefix /` makes npm look for the lock at `/`, not `/work`.
- ✅ **copy the manifests to `/` then `npm ci` at `/`:**
  ```
  cp /work/package.json /work/package-lock.json / && cd / && npm ci
  # → /node_modules/lodash YES, /node_modules/@types/lodash YES, exit 0
  ```

**RESULT: ✅** the fetch is `cp /work/{package.json,package-lock.json} / && cd / && npm ci` (with a
`npm install` fallback for a no-lock repo, `|| true` so a no-deps repo still gets a usable bare server).
The **verify** path needs the same (its container also lacks `/node_modules` and the clone is `:ro`):
`verify_cache_path="/node_modules"` + the typecheck cmd copies manifests + `npm ci` into the mounted
`/node_modules` verify vol, THEN `tsc --noEmit` (which resolves via the walk). This mirrors python's
verify-venv-into-`/cache` pattern (codex/Opus B1).

## Implications for the spec

- `dep_cache_path = "/node_modules"`, `verify_cache_path = "/node_modules"` (the verify install target).
- fetch + verify use the copy-manifests-to-`/` + `npm ci` shape (NOT `--prefix /`).
- Reuse of `cache_binding`/`apply_warm_lsp`/`warm_lsp_deps_step`/`compose_warm_fetch`/`compose_verify`
  holds (all fixed-absolute paths; `/node_modules` is a root child, not under the `/work` mount → S6-safe).
