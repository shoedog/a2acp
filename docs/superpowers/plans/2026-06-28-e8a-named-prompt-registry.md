# E8a — Named Prompt Registry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add a config `[[prompts]]` registry so workflow nodes reference a named prompt by id
(`prompt = "<id>"`) as an alternative to `prompt_file`, with a `prompt list`/`show` discovery CLI —
resolved once at config-load, leaving the runtime byte-for-byte unchanged.

**Architecture:** A new `PromptId` newtype + a `[[prompts]]` TOML block on `RegistryConfig`. A shared
resolver (`resolve_one`/`resolve_prompt_registry`) runs at the SOLE config→graph seam
(`RegistryConfig::load_workflows`, `config.rs:987`, the `read_to_string` at `:1014`). Each node resolves
its `prompt_template` from exactly one of `prompt`/`prompt_file`. A prompt-only config parse powers the CLI
without triggering agent/DAG validation. Nothing below the seam changes.

**Tech Stack:** Rust, serde/toml, the existing `config.rs` load path, `bridge-core::ids` newtypes.

**Binding spec:** `docs/superpowers/specs/2026-06-28-e8a-named-prompt-registry.md` — `## v2` (SR-FIX) + `## v3`
(RR-FIX) supersede v1 §3–§6. **Anchors may have drifted ±3 lines — verify NAMES, not line numbers.**

---

### Task 1: `PromptId` newtype (permissive grammar, derives `Ord`)

**Files:**
- Modify: `crates/bridge-core/src/ids.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `ids.rs`:
```rust
#[test]
fn prompt_id_accepts_namespaced_and_mixed_case_rejects_blank_and_ws() {
    for ok in ["review-correctness", "_preamble/review-readonly", "design.synth", "Smoke_Read"] {
        assert!(PromptId::parse(ok).is_ok(), "{ok} should parse");
    }
    for bad in ["", "  ", "a b", "tab\there", "ctrl\u{0}x"] {
        assert!(PromptId::parse(bad).is_err(), "{bad:?} should reject");
    }
    // Ord is derivable -> usable as a BTreeMap key (compile + order check).
    let mut m = std::collections::BTreeMap::new();
    m.insert(PromptId::parse("b").unwrap(), 1);
    m.insert(PromptId::parse("a").unwrap(), 2);
    assert_eq!(m.keys().next().unwrap().as_str(), "a");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p bridge-core --lib prompt_id_accepts -j 1`
Expected: FAIL — `PromptId` not found.

- [ ] **Step 3: Implement `PromptId`**

In `ids.rs`, after the `id_newtype!` definitions, add a hand-written newtype (the existing macros derive no
`Ord` and forbid `/`,`.`,uppercase — `PromptId` needs all three):
```rust
/// Prompt registry id. Deliberately MORE permissive than `id_newtype_strict!` (admits uppercase,
/// `/`, `.`) so E8b namespaced partials (`_preamble/review-readonly`) need no grammar change. Derives
/// `Ord` so it can key a `BTreeMap` (the resolved registry / `prompt list` ordering).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct PromptId(String);
impl PromptId {
    pub fn parse(s: impl Into<String>) -> Result<Self, BridgeError> {
        let s = s.into();
        let trimmed = s.trim();
        let ok = !trimmed.is_empty()
            && trimmed.len() == s.len() // no leading/trailing whitespace
            && s.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.')
            });
        if !ok {
            return Err(BridgeError::InvalidRequest { field: "PromptId" });
        }
        Ok(Self(s))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p bridge-core --lib prompt_id_accepts -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/bridge-core/src/ids.rs
git commit -m "feat(core): PromptId newtype — permissive (/ _ - .) + Ord (E8a T1)"
```

---

### Task 2: `PromptEntryToml` + `RegistryConfig.prompts` parse

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `config.rs`:
```rust
#[test]
fn prompts_block_parses_file_text_and_description() {
    let toml = format!(
        "default = \"codex\"\n{AGENT_FOOTER}\n\
         [[prompts]]\nid = \"rev\"\nfile = \"r.md\"\ndescription = \"reviewer\"\n\
         [[prompts]]\nid = \"smoke\"\ntext = \"hi\"\n{SERVER_FOOTER}"
    );
    let cfg: RegistryConfig = toml::from_str(&toml).expect("parse");
    assert_eq!(cfg.prompts.len(), 2);
    assert_eq!(cfg.prompts[0].id, "rev");
    assert_eq!(cfg.prompts[0].file.as_deref(), Some("r.md"));
    assert_eq!(cfg.prompts[0].description.as_deref(), Some("reviewer"));
    assert_eq!(cfg.prompts[1].text.as_deref(), Some("hi"));
}
```
(Use the existing `AGENT_FOOTER`/`SERVER_FOOTER` test consts; if none exists, inline a minimal
`[[agents]] id=\"codex\" cmd=\"codex-acp\"` + `[server] addr=\"127.0.0.1:8080\"`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib prompts_block_parses -j 1`
Expected: FAIL — no `prompts` field.

- [ ] **Step 3: Implement the struct + field**

Add the struct near `WorkflowNodeToml` (`config.rs:~283`):
```rust
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptEntryToml {
    pub id: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}
```
Add to `RegistryConfig` (`config.rs:117`):
```rust
    #[serde(default)]
    pub prompts: Vec<PromptEntryToml>,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib prompts_block_parses -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): [[prompts]] PromptEntryToml + RegistryConfig.prompts (E8a T2)"
```

---

### Task 3: `WorkflowNodeToml` — `prompt`/`prompt_file` both optional

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn node_accepts_prompt_ref_and_prompt_file_independently() {
    let by_ref: WorkflowNodeToml =
        toml::from_str("id=\"n\"\nagent=\"a\"\nprompt=\"rev\"\n").unwrap();
    assert_eq!(by_ref.prompt.as_deref(), Some("rev"));
    assert!(by_ref.prompt_file.is_none());
    let by_file: WorkflowNodeToml =
        toml::from_str("id=\"n\"\nagent=\"a\"\nprompt_file=\"p.md\"\n").unwrap();
    assert_eq!(by_file.prompt_file.as_deref(), Some("p.md"));
    // neither parses (validated later, not at serde)
    let neither: WorkflowNodeToml = toml::from_str("id=\"n\"\nagent=\"a\"\n").unwrap();
    assert!(neither.prompt.is_none() && neither.prompt_file.is_none());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib node_accepts_prompt_ref -j 1`
Expected: FAIL — `prompt_file` is required `String`; `prompt` field missing.

- [ ] **Step 3: Implement the field changes**

In `WorkflowNodeToml` (`config.rs:~283`):
```rust
    #[serde(default)]
    pub prompt_file: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib node_accepts_prompt_ref -j 1`
Expected: PASS. (The `load_workflows` body at `:1014` will not compile yet — Task 5 fixes it; if needed to
keep the crate compiling between tasks, temporarily `n.prompt_file.as_deref().unwrap_or_default()` — Task 5
replaces this region wholesale.)

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): WorkflowNodeToml prompt_file Option + prompt ref (E8a T3)"
```

---

### Task 4: `ResolvedPrompt` + `resolve_one` + `resolve_prompt_registry`

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn resolve_prompt_registry_file_text_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("r.md"), "REVIEW {{input}}").unwrap();
    // happy: file + text + empty text permitted
    let ok = vec![
        PromptEntryToml { id: "rev".into(), file: Some("r.md".into()), text: None, description: Some("d".into()) },
        PromptEntryToml { id: "s".into(), file: None, text: Some("hi".into()), description: None },
        PromptEntryToml { id: "e".into(), file: None, text: Some("".into()), description: None },
    ];
    let reg = resolve_prompt_registry(&ok, dir.path()).unwrap();
    assert_eq!(reg.get(&PromptId::parse("rev").unwrap()).unwrap().template, "REVIEW {{input}}");
    assert_eq!(reg.get(&PromptId::parse("s").unwrap()).unwrap().template, "hi");
    assert_eq!(reg.get(&PromptId::parse("e").unwrap()).unwrap().template, ""); // empty permitted
    // both file+text -> err
    assert!(resolve_prompt_registry(
        &[PromptEntryToml { id: "x".into(), file: Some("r.md".into()), text: Some("t".into()), description: None }],
        dir.path()).is_err());
    // neither -> err
    assert!(resolve_prompt_registry(
        &[PromptEntryToml { id: "x".into(), file: None, text: None, description: None }],
        dir.path()).is_err());
    // dup id -> err
    assert!(resolve_prompt_registry(
        &[PromptEntryToml { id: "d".into(), file: None, text: Some("a".into()), description: None },
          PromptEntryToml { id: "d".into(), file: None, text: Some("b".into()), description: None }],
        dir.path()).is_err());
    // unreadable file -> err
    assert!(resolve_prompt_registry(
        &[PromptEntryToml { id: "m".into(), file: Some("missing.md".into()), text: None, description: None }],
        dir.path()).is_err());
}
```
(Add `tempfile` to `[dev-dependencies]` if not already present — it is used elsewhere in the crate.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib resolve_prompt_registry_file_text -j 1`
Expected: FAIL — items not defined.

- [ ] **Step 3: Implement the resolver**

Add to `config.rs` (module scope, near `load_workflows`):
```rust
#[derive(Debug, Clone)]
pub enum PromptSource {
    File(std::path::PathBuf),
    Text,
}

#[derive(Debug, Clone)]
pub struct ResolvedPrompt {
    pub template: String,
    pub description: Option<String>,
    pub source: PromptSource,
}

/// Resolve ONE entry: exactly-one-of file/text (+ read). Empty template permitted (matches an empty
/// `prompt_file` today). `base` = the config file's directory.
fn resolve_one(entry: &PromptEntryToml, base: &std::path::Path) -> Result<ResolvedPrompt, ConfigError> {
    use bridge_core::ids::PromptId;
    PromptId::parse(entry.id.clone())
        .map_err(|_| ConfigError::Registry(format!("prompt id {:?} is invalid", entry.id)))?;
    match (&entry.file, &entry.text) {
        (Some(f), None) => {
            let path = base.join(f);
            let template = std::fs::read_to_string(&path).map_err(|e| {
                ConfigError::Registry(format!("prompt {:?} file {:?}: {e}", entry.id, path))
            })?;
            Ok(ResolvedPrompt { template, description: entry.description.clone(), source: PromptSource::File(path) })
        }
        (None, Some(t)) => Ok(ResolvedPrompt {
            template: t.clone(),
            description: entry.description.clone(),
            source: PromptSource::Text,
        }),
        _ => Err(ConfigError::Registry(format!(
            "prompt {:?} must set exactly one of `file` or `text`",
            entry.id
        ))),
    }
}

/// Eagerly resolve ALL registered prompts (fail-fast at boot). Dup-id rejected. Used by the load seam
/// (NOT by `prompt list`, which is lazy).
fn resolve_prompt_registry(
    prompts: &[PromptEntryToml],
    base: &std::path::Path,
) -> Result<std::collections::BTreeMap<bridge_core::ids::PromptId, ResolvedPrompt>, ConfigError> {
    use bridge_core::ids::PromptId;
    let mut map = std::collections::BTreeMap::new();
    for entry in prompts {
        let id = PromptId::parse(entry.id.clone())
            .map_err(|_| ConfigError::Registry(format!("prompt id {:?} is invalid", entry.id)))?;
        let resolved = resolve_one(entry, base)?;
        if map.insert(id, resolved).is_some() {
            return Err(ConfigError::Registry(format!("duplicate prompt id {:?}", entry.id)));
        }
    }
    Ok(map)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib resolve_prompt_registry_file_text -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): resolve_one + resolve_prompt_registry (BTreeMap<PromptId,ResolvedPrompt>) (E8a T4)"
```

---

### Task 5: Wire the resolver into `load_workflows` (the seam)

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs` (the node loop in `load_workflows`, `~:1000–1044`)

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn load_workflows_resolves_named_prompt_and_is_byte_identical_to_prompt_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("r.md"), "REVIEW {{input}}\n").unwrap();
    let named = format!(
        "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
         [[prompts]]\nid=\"rev\"\nfile=\"r.md\"\n\
         [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"n\"\nagent=\"codex\"\nprompt=\"rev\"\ninputs=[]\n\
         [server]\naddr=\"127.0.0.1:8080\"\n");
    let by_file = format!(
        "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
         [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"n\"\nagent=\"codex\"\nprompt_file=\"r.md\"\ninputs=[]\n\
         [server]\naddr=\"127.0.0.1:8080\"\n");
    let g_named = toml::from_str::<RegistryConfig>(&named).unwrap().load_workflows(dir.path()).unwrap();
    let g_file = toml::from_str::<RegistryConfig>(&by_file).unwrap().load_workflows(dir.path()).unwrap();
    // byte-identical prompt_template (file= path == prompt_file path: same read)
    assert_eq!(g_named[&"w".parse().unwrap()].nodes[0].prompt_template,
               g_file[&"w".parse().unwrap()].nodes[0].prompt_template);
    assert_eq!(g_named[&"w".parse().unwrap()].nodes[0].prompt_template, "REVIEW {{input}}\n");
}

#[test]
fn load_workflows_rejects_unknown_ref_dup_and_both_neither() {
    let dir = tempfile::tempdir().unwrap();
    let unknown = "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
        [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"n\"\nagent=\"codex\"\nprompt=\"ghost\"\ninputs=[]\n\
        [server]\naddr=\"127.0.0.1:8080\"\n";
    let err = toml::from_str::<RegistryConfig>(unknown).unwrap().load_workflows(dir.path()).unwrap_err();
    assert!(format!("{err}").contains("ghost")); // names the id (and lists available)
    let both = "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
        [[prompts]]\nid=\"rev\"\ntext=\"t\"\n\
        [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"n\"\nagent=\"codex\"\nprompt=\"rev\"\nprompt_file=\"x.md\"\ninputs=[]\n\
        [server]\naddr=\"127.0.0.1:8080\"\n";
    assert!(toml::from_str::<RegistryConfig>(both).unwrap().load_workflows(dir.path()).is_err());
    let neither = "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
        [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"n\"\nagent=\"codex\"\ninputs=[]\n\
        [server]\naddr=\"127.0.0.1:8080\"\n";
    assert!(toml::from_str::<RegistryConfig>(neither).unwrap().load_workflows(dir.path()).is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib load_workflows_resolves_named_prompt -j 1`
Expected: FAIL (resolution not wired; or the temporary `unwrap_or_default` from T3 mis-resolves `prompt=`).

- [ ] **Step 3: Implement — build the registry before the loop, resolve per node**

In `load_workflows`, BEFORE the `for w in &self.workflows` loop, build the registry once:
```rust
        let prompt_registry = resolve_prompt_registry(&self.prompts, base)?;
        let available_ids = || {
            prompt_registry.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
        };
```
Then REPLACE the `let tpl = std::fs::read_to_string(base.join(&n.prompt_file))…?;` block (`~:1014`) with:
```rust
                let tpl = match (&n.prompt, &n.prompt_file) {
                    (Some(id_str), None) => {
                        let id = bridge_core::ids::PromptId::parse(id_str.clone()).map_err(|_| {
                            ConfigError::Registry(format!(
                                "workflow {} node {} prompt id {:?} is invalid",
                                w.id, n.id, id_str
                            ))
                        })?;
                        prompt_registry
                            .get(&id)
                            .map(|r| r.template.clone())
                            .ok_or_else(|| {
                                ConfigError::Registry(format!(
                                    "workflow {} node {} references unknown prompt {:?}; available: [{}]",
                                    w.id, n.id, id_str, available_ids()
                                ))
                            })?
                    }
                    (None, Some(path)) => std::fs::read_to_string(base.join(path)).map_err(|e| {
                        ConfigError::Registry(format!(
                            "workflow {} node {} prompt_file {:?}: {e}",
                            w.id, n.id, path
                        ))
                    })?,
                    _ => {
                        return Err(ConfigError::Registry(format!(
                            "workflow {} node {} must set exactly one of `prompt` or `prompt_file`",
                            w.id, n.id
                        )))
                    }
                };
```
(`prompt_template: tpl` in the `WorkflowNode { … }` construction is unchanged.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib load_workflows_resolves_named_prompt -j 1 && cargo test -p a2a-bridge --lib load_workflows_rejects_unknown_ref -j 1`
Expected: PASS both.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): resolve node prompt via registry at the load_workflows seam (E8a T5)"
```

---

### Task 6: Prompt-only config parse (CLI helper, resilient to unrelated config errors)

**Files:**
- Modify: `bin/a2a-bridge/src/config.rs`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn prompt_only_parse_ignores_unrelated_sections_and_errors() {
    // an unknown agent / bogus workflow would fail load_workflows, but prompt-only parse still reads prompts
    let toml = "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
        [[prompts]]\nid=\"rev\"\nfile=\"r.md\"\ndescription=\"d\"\n\
        [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"n\"\nagent=\"GHOST\"\nprompt=\"rev\"\ninputs=[]\n\
        [server]\naddr=\"127.0.0.1:8080\"\n";
    let prompts = parse_prompts_only(toml).unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].id, "rev");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib prompt_only_parse_ignores -j 1`
Expected: FAIL — `parse_prompts_only` not defined.

- [ ] **Step 3: Implement the minimal parse**
```rust
/// Deserialize ONLY the `[[prompts]]` array (tolerant of every other section), so `prompt list/show`
/// work even when the agent/workflow config has unrelated errors. No agent/DAG/snapshot validation.
pub fn parse_prompts_only(toml_str: &str) -> Result<Vec<PromptEntryToml>, ConfigError> {
    #[derive(serde::Deserialize)]
    struct PromptsOnly {
        #[serde(default)]
        prompts: Vec<PromptEntryToml>,
    }
    let parsed: PromptsOnly = toml::from_str(toml_str)
        .map_err(|e| ConfigError::Registry(format!("config parse: {e}")))?;
    Ok(parsed.prompts)
}
```
(`PromptsOnly` has no `deny_unknown_fields`, so serde ignores `agents`/`workflows`/`server`/etc.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib prompt_only_parse_ignores -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/config.rs
git commit -m "feat(config): parse_prompts_only — prompt-only CLI parse (E8a T6)"
```

---

### Task 7: `prompt list` CLI (lazy — id + description, no file I/O)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Write the failing test**

Add to `main.rs` tests:
```rust
#[test]
fn prompt_list_sorts_ids_no_file_io() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("a2a-bridge.toml");
    // note: `file` points at a MISSING file — `list` must still work (no read).
    std::fs::write(&cfg, "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
        [[prompts]]\nid=\"zeta\"\nfile=\"missing.md\"\ndescription=\"z\"\n\
        [[prompts]]\nid=\"alpha\"\ntext=\"hi\"\n[server]\naddr=\"127.0.0.1:8080\"\n").unwrap();
    let out = super::prompt_list_lines(&cfg).unwrap();
    assert_eq!(out, vec!["alpha — (no description)".to_string(), "zeta — z".to_string()]);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib prompt_list_sorts_ids -j 1`
Expected: FAIL — `prompt_list_lines` not defined.

- [ ] **Step 3: Implement (a testable core + the command shell)**
```rust
/// Pure-ish core for `prompt list`: read prompts (no file I/O on `file=`), sort by id.
fn prompt_list_lines(config_path: &std::path::Path) -> Result<Vec<String>, BoxError> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("prompt: read {config_path:?}: {e}"))?;
    let mut prompts = config::parse_prompts_only(&raw)
        .map_err(|e| format!("{e}"))?;
    // sort by id (deterministic) — separate id-sort, NOT the resolved BTreeMap (RR-FIX-3).
    prompts.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(prompts
        .iter()
        .map(|p| format!("{} — {}", p.id, p.description.as_deref().unwrap_or("(no description)")))
        .collect())
}
```
(Use the real module path for `parse_prompts_only`; in this crate it is `crate::config::parse_prompts_only`
if `config` is a local module, else the crate path. Verify the actual `mod config;` location in `main.rs`.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib prompt_list_sorts_ids -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(cli): prompt list — lazy id+description, sorted, no file I/O (E8a T7)"
```

---

### Task 8: `prompt show <id>` CLI (resolve the one entry; discovery error)

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn prompt_show_resolves_one_and_errors_on_unknown() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("r.md"), "REVIEW {{input}}\n").unwrap();
    let cfg = dir.path().join("a2a-bridge.toml");
    std::fs::write(&cfg, "default=\"codex\"\n[[agents]]\nid=\"codex\"\ncmd=\"codex-acp\"\n\
        [[prompts]]\nid=\"rev\"\nfile=\"r.md\"\n[[prompts]]\nid=\"s\"\ntext=\"hi\"\n\
        [server]\naddr=\"127.0.0.1:8080\"\n").unwrap();
    assert_eq!(super::prompt_show_text(&cfg, "rev").unwrap(), "REVIEW {{input}}\n");
    assert_eq!(super::prompt_show_text(&cfg, "s").unwrap(), "hi");
    let err = super::prompt_show_text(&cfg, "ghost").unwrap_err().to_string();
    assert!(err.contains("ghost") && err.contains("rev")); // unknown + available ids
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib prompt_show_resolves_one -j 1`
Expected: FAIL — `prompt_show_text` not defined.

- [ ] **Step 3: Implement**
```rust
/// Resolve and return ONE prompt's raw template. Reads only the requested entry's file (resilient to
/// other entries' bad `file=`); unknown id -> error listing available ids.
fn prompt_show_text(config_path: &std::path::Path, id: &str) -> Result<String, BoxError> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("prompt: read {config_path:?}: {e}"))?;
    let prompts = config::parse_prompts_only(&raw).map_err(|e| format!("{e}"))?;
    // dup-id scan (no read) for a clean error, matching load-seam semantics.
    let mut seen = std::collections::HashSet::new();
    for p in &prompts {
        if !seen.insert(p.id.as_str()) {
            return Err(format!("duplicate prompt id {:?}", p.id).into());
        }
    }
    let base = config_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    match prompts.iter().find(|p| p.id == id) {
        Some(entry) => Ok(config::resolve_one(entry, base)
            .map_err(|e| format!("{e}"))?
            .template),
        None => {
            let mut ids: Vec<&str> = prompts.iter().map(|p| p.id.as_str()).collect();
            ids.sort_unstable();
            Err(format!("unknown prompt {id:?}; available: [{}]", ids.join(", ")).into())
        }
    }
}
```
(Expose `resolve_one` for the CLI as `pub fn resolve_one_pub` re-export, or make `resolve_one` `pub` —
whichever matches the crate's existing visibility convention. Keep `show` reading ONLY the found entry.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib prompt_show_resolves_one -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/main.rs bin/a2a-bridge/src/config.rs
git commit -m "feat(cli): prompt show <id> — resolve one entry, discovery error (E8a T8)"
```

---

### Task 9: `prompt` subcommand dispatch + usage

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs`

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn prompt_cmd_dispatch_help_and_unknown_sub() {
    assert!(super::prompt_cmd(&["--help".to_string()]).is_ok());
    assert!(super::prompt_cmd(&["bogus".to_string()]).is_err());
    assert!(super::prompt_cmd(&[]).is_err()); // missing subcommand
    // --resolved is reserved for E8b -> rejected
    assert!(super::prompt_cmd(&["show".to_string(), "x".to_string(), "--resolved".to_string()]).is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib prompt_cmd_dispatch -j 1`
Expected: FAIL — `prompt_cmd` not defined.

- [ ] **Step 3: Implement dispatch + usage + the `prompt_cmd` shell**

Add a `PROMPT_USAGE` const (model on `TASK_SPEC_USAGE`):
```rust
const PROMPT_USAGE: &str = "\
usage: a2a-bridge prompt list [--config <path>]
       a2a-bridge prompt show <id> [--config <path>]

Inspect the named prompt registry ([[prompts]]). `list` shows ids + descriptions;
`show <id>` prints the raw template. Default config is ./a2a-bridge.toml.";
```
Add `prompt_cmd`:
```rust
fn prompt_cmd(args: &[String]) -> Result<(), BoxError> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{PROMPT_USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--resolved") {
        return Err(format!("prompt: --resolved is reserved for a later release\n{PROMPT_USAGE}").into());
    }
    // --config <path> default ./a2a-bridge.toml (run-workflow model)
    let mut config = std::path::PathBuf::from("a2a-bridge.toml");
    let mut positional: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--config" {
            config = it.next().ok_or("prompt: --config requires a value")?.into();
        } else {
            positional.push(a);
        }
    }
    match positional.first().map(|s| s.as_str()) {
        Some("list") => {
            for line in prompt_list_lines(&config)? {
                println!("{line}");
            }
            Ok(())
        }
        Some("show") => {
            let id = positional.get(1).ok_or_else(|| format!("prompt show: expected <id>\n{PROMPT_USAGE}"))?;
            print!("{}", prompt_show_text(&config, id)?);
            Ok(())
        }
        Some(other) => Err(format!("prompt: unknown subcommand {other:?}\n{PROMPT_USAGE}").into()),
        None => Err(format!("prompt: missing subcommand\n{PROMPT_USAGE}").into()),
    }
}
```
Wire dispatch: add `Some("prompt") => TopSubcommand::Prompt` to `parse_top_subcommand` (`~:168`), the
`TopSubcommand::Prompt` enum variant, the `TopSubcommand::Prompt => return prompt_cmd(&raw_args[2..])` arm
(`~:4668`), add `prompt` to `TOP_USAGE` (`~:97`) and to the unknown-subcommand expected list (`~:4679`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib prompt_cmd_dispatch -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(cli): a2a-bridge prompt {list,show} dispatch + usage (E8a T9)"
```

---

### Task 10: Migration of the variance set + the determinism golden

**Files:**
- Modify: `examples/a2a-bridge.workflows.toml`, `examples/a2a-bridge.containerized.toml`
  (+ `.podman.toml` if mechanical)
- Modify: `bin/a2a-bridge/src/config.rs` (a synthetic-pair golden test)

- [ ] **Step 1: Write the failing golden test (synthetic old/new pair)**
```rust
#[test]
fn migrated_named_graph_byte_identical_to_prompt_file_for_file_backed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("rev.md"), "You are a reviewer.\n{{input}}\n").unwrap();
    let old = "default=\"c\"\n[[agents]]\nid=\"c\"\ncmd=\"codex-acp\"\n\
        [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"a\"\nagent=\"c\"\nprompt_file=\"rev.md\"\ninputs=[]\n\
        [[workflows.nodes]]\nid=\"b\"\nagent=\"c\"\nprompt_file=\"rev.md\"\ninputs=[]\n[server]\naddr=\"127.0.0.1:8080\"\n";
    let new = "default=\"c\"\n[[agents]]\nid=\"c\"\ncmd=\"codex-acp\"\n[[prompts]]\nid=\"rev\"\nfile=\"rev.md\"\n\
        [[workflows]]\nid=\"w\"\n[[workflows.nodes]]\nid=\"a\"\nagent=\"c\"\nprompt=\"rev\"\ninputs=[]\n\
        [[workflows.nodes]]\nid=\"b\"\nagent=\"c\"\nprompt=\"rev\"\ninputs=[]\n[server]\naddr=\"127.0.0.1:8080\"\n";
    let go = toml::from_str::<RegistryConfig>(old).unwrap().load_workflows(dir.path()).unwrap();
    let gn = toml::from_str::<RegistryConfig>(new).unwrap().load_workflows(dir.path()).unwrap();
    let w = "w".parse().unwrap();
    for node in ["a", "b"] {
        let n = node.parse().unwrap();
        let fo = go[&w].nodes.iter().find(|x| x.id == n).unwrap();
        let fn_ = gn[&w].nodes.iter().find(|x| x.id == n).unwrap();
        assert_eq!(fo.prompt_template, fn_.prompt_template); // byte-identical, reuse across nodes
    }
}
```

- [ ] **Step 2: Run to verify it fails (then passes — it exercises T5)**

Run: `cargo test -p a2a-bridge --lib migrated_named_graph_byte_identical -j 1`
Expected: PASS once T5 is in (this test guards the migration claim; it should be GREEN — if it fails, T5 is
wrong). Keep it as the regression guard.

- [ ] **Step 3: Migrate the example configs**

`examples/a2a-bridge.workflows.toml` — add `[[prompts]]` entries (one per review prompt, `file=`) and change
each node `prompt_file = "../prompts/X.md"` → `prompt = "X"`. Verify the 3 workflows
(code-review/spec-review/plan-review) still resolve. Example:
```toml
[[prompts]]
id = "review-correctness"
file = "../prompts/review-correctness.md"
description = "code-review correctness lens"
# … one per node prompt …
```
`examples/a2a-bridge.containerized.toml` — register `review-implement` ONCE and reference it from all 5
nodes; add ONE inline `text=` for the single-line `smoke-reply`:
```toml
[[prompts]]
id = "review-implement"
file = "../prompts/review-implement.md"
description = "shared implement-review prompt (reused 5×)"
[[prompts]]
id = "smoke-reply"
text = "Reply with the single word PONG. Do not use any tools."
description = "one-line smoke (inline text=)"
```
(Keep multi-line smokes — `smoke-read`, `impl-smoke` — as `file=`.) Then change those nodes to
`prompt = "review-implement"` / `prompt = "smoke-reply"`.

- [ ] **Step 4: Verify the migrated configs load**

Run: `cargo build -j 2 && ./target/debug/a2a-bridge prompt list --config examples/a2a-bridge.workflows.toml && ./target/debug/a2a-bridge prompt list --config examples/a2a-bridge.containerized.toml`
Expected: both list the registered ids without error; `cargo test -p a2a-bridge --lib migrated_named_graph -j 1` PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/config.rs examples/a2a-bridge.workflows.toml examples/a2a-bridge.containerized.toml
git commit -m "feat(examples): migrate reference + containerized configs to named prompts (E8a T10)"
```

---

### Task 11: `init` scaffold emits `[[prompts]]` + `prompt = "<id>"`

**Files:**
- Modify: `bin/a2a-bridge/src/main.rs` (`init_cmd`, `~:4070`, and the `INIT_*` template consts)

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn init_scaffold_resolves_named_prompts() {
    let dir = tempfile::tempdir().unwrap();
    super::init_cmd_at(dir.path()).unwrap(); // a path-injectable variant of init_cmd
    let cfg = dir.path().join("a2a-bridge.toml");
    // the scaffolded config uses [[prompts]] + prompt="<id>" and resolves with no dangling ref.
    let lines = super::prompt_list_lines(&cfg).unwrap();
    assert!(!lines.is_empty(), "init should scaffold at least one named prompt");
    let raw = std::fs::read_to_string(&cfg).unwrap();
    assert!(raw.contains("[[prompts]]") && raw.contains("prompt = \""));
    // every node prompt ref resolves (load_workflows succeeds)
    let base = dir.path();
    toml::from_str::<RegistryConfig>(&raw).unwrap().load_workflows(base).unwrap();
}
```
(If `init_cmd` is not path-injectable, add a thin `init_cmd_at(dir)` that `init_cmd` calls with CWD — keeps
the test hermetic.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p a2a-bridge --lib init_scaffold_resolves_named_prompts -j 1`
Expected: FAIL — scaffold still emits `prompt_file`, or `init_cmd_at` missing.

- [ ] **Step 3: Implement**

Update the `init_cmd` config template to include a `[[prompts]]` section and change scaffolded workflow
nodes from `prompt_file = "prompts/X.md"` to `prompt = "X"`. Ensure the `prompts/` files are written (they
already are) — order within `init` is irrelevant (refs resolve at later config-load, RR-FIX-4). Add
`init_cmd_at(dir)` if needed for the test.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p a2a-bridge --lib init_scaffold_resolves_named_prompts -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add bin/a2a-bridge/src/main.rs
git commit -m "feat(init): scaffold [[prompts]] + prompt=\"<id>\" nodes (E8a T11)"
```

---

## Final gate (controller, clean host env)

- [ ] `cargo build --all-targets -j 2` clean.
- [ ] `cargo clippy -p bridge-core -p a2a-bridge --all-targets -j 1` — 0 warnings (the touched crates;
  `--all-targets -j 1` to avoid the OOM stall on this machine).
- [ ] `cargo fmt --all` clean.
- [ ] `cargo test -p bridge-core -p a2a-bridge -j 1` green (incl. all E8a tests + back-compat: every
  existing `prompt_file` test still passes).
- [ ] Whole-branch dual review (codex + claude) → fold → live-gate (run the migrated `code-review` via the
  bridge with real agents; confirm named-prompt output == prompt_file behavior) → merge `--no-ff` → push →
  memory.

## Notes / out of scope (from the re-reviews)

- `bin/a2a-bridge/tests/integration_run_workflow.rs:~97` is a **test-only** parallel parser with its own
  `Node { prompt_file: String }` — out of scope; do NOT feed it a `prompt="<id>"` fixture.
- Anchors above may have drifted ±3 lines — verify NAMES (`load_workflows`, `WorkflowNodeToml`,
  `parse_top_subcommand`, `init_cmd`, `TOP_USAGE`), not line numbers.
- Acceptance criterion 7 ("no diff below the seam") is a review-checklist inspection item, not a test; the
  determinism golden (Task 10) is the automated guard.
- E8b (composition / `{{> partial}}` includes + `prompt show --resolved`) builds on this `BTreeMap<PromptId,
  ResolvedPrompt>` substrate with no breaking change.
