You are validating that the bridge host reviewer's `lsp` MCP nav works on a **Go** repository (this session's cwd is a Go module — `go.mod` at root, served by gopls via `--lang auto`).

Use ONLY the `lsp` MCP tools for the navigation below — do NOT use shell, grep, or file-reading for this; the point is to exercise the type-resolved language-server path. Perform exactly these and report what each returns:

1. `workspace_symbol` for `New` — confirm it finds the top-level function.
2. `definition` of `New` — report the `file:line` it resolves to.
3. `hover` on `New` — report the type-resolved signature (e.g. `func New() UUID`).
4. `references` to `UUID` — report at least one `file:line`.

Then write a 4-line report: for each of the four calls, one line — the tool, and the concrete result it returned (or "EMPTY" if it returned nothing). End with one line: `LSP-GO-NAV: WORKING` if at least `hover` returned a real type signature AND `definition` resolved to a file:line, else `LSP-GO-NAV: BROKEN`. READ-ONLY; be terse; do not review the code or propose changes; STOP after the report.
