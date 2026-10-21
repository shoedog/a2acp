use super::*;
pub(super) const NAV: &str = include_str!("../../tests/fixtures/skills/navigator.md");
const LITERAL: &str = include_str!("../../tests/fixtures/skills/literal-braces.md");
fn decl(id: &str, path: &str) -> SkillDeclaration {
    SkillDeclaration {
        id: id.into(),
        path: path.into(),
        delivery: "prompt".into(),
        unsupported: Default::default(),
    }
}
fn rejects(source: &str, reason: &str) {
    let err = validate_frontmatter(source).expect_err(source).to_string();
    assert!(err.contains(reason), "{err:?} should name {reason:?}");
    assert!(!err.contains(source), "source leaked");
}
#[cfg(unix)]
#[test]
fn skills_read_bound_even_for_growing_input() {
    use std::io::Read;
    assert_eq!(
        read_bounded(std::io::repeat(b'x').take(MAX_SKILL_BYTES as u64))
            .unwrap()
            .len(),
        MAX_SKILL_BYTES
    );
    assert!(read_bounded(std::io::repeat(b'x'))
        .unwrap_err()
        .to_string()
        .contains("32 KiB"));
}
#[test]
fn skills_frontmatter_accepted_forms() {
    for source in [
        NAV,
        LITERAL,
        "---\r\nname: nav\r\ndescription: text\r\n---\r\nbody",
    ] {
        validate_frontmatter(source).unwrap();
    }
    for name in ["nav", "'nav''s'", "\"nav\\u0069\\x67\\U00000061tor\""] {
        for description in [
            "plain words # comment",
            "'it''s text'",
            "\"text\\nline\"",
            "|\n  first\n  second",
            "|-\n  text",
            "|+\n  text",
            ">\n  folded\n  text",
            ">-\n  text",
            ">+\n  text",
        ] {
            validate_frontmatter(&format!(
                "---\nname: {name}\ndescription: {description}\nother: 'ignored'\n---\nbody"
            ))
            .unwrap();
        }
    }
    for escape in [
        "0",
        "a",
        "b",
        "t",
        "n",
        "v",
        "f",
        "r",
        "e",
        " ",
        "\"",
        "/",
        "\\",
        "N",
        "_",
        "L",
        "P",
        "x41",
        "u0041",
        "U00000041",
    ] {
        validate_frontmatter(&format!(
            "---\nname: nav\ndescription: \"text\\{escape}\"\n---"
        ))
        .unwrap();
    }
}
#[test]
fn skills_frontmatter_refusals() {
    for (source, reason) in [
        ("name: nav\ndescription: d", "frontmatter"),
        ("\u{feff}---\nname: nav\ndescription: d\n---", "frontmatter"),
        ("---\nname: nav\ndescription: d", "closing"),
        ("---\ndescription: d\n---", "name"),
        ("---\nname: nav\n---", "description"),
        ("---\nname: ''\ndescription: d\n---", "nonempty"),
        ("---\nname: nav\ndescription: '  '\n---", "nonempty"),
        (
            "---\nname: nav\nname: other\ndescription: d\n---",
            "duplicate",
        ),
        (
            "---\nname: nav\ndescription: d\ndescription: e\n---",
            "duplicate",
        ),
        ("---\nname: [nav]\ndescription: d\n---", "flow"),
        ("---\nname: nav\ndescription: {x: y}\n---", "flow"),
        ("---\nname: &anchor nav\ndescription: d\n---", "anchor"),
        ("---\nname: *alias\ndescription: d\n---", "alias"),
        ("---\nname: !!str nav\ndescription: d\n---", "tag"),
        (
            "---\nname: nav\ndescription: d\nextra:\n  nested: value\n---",
            "nested",
        ),
        ("---\nname: nav\ndescription: d\n\textra: v\n---", "tab"),
        ("---\nname: nav\ndescription: |\n\ttext\n---", "tab"),
        ("---\nname: nav\ndescription: \"bad\\q\"\n---", "escape"),
        ("---\nname: nav\ndescription: \"bad\\uD800\"\n---", "escape"),
        ("---\nname: nav\ndescription: \"bad\\xQ0\"\n---", "escape"),
        ("---\nname: nav\ndescription: \"unterminated\n---", "quoted"),
        ("---\nname: nav\ndescription: 'unterminated\n---", "quoted"),
        ("---\nname: nav\ndescription: 'text' junk\n---", "quoted"),
        ("---\nname: nav\ndescription: |2\n  text\n---", "block"),
        ("---\nname: |\n  nav\ndescription: d\n---", "block"),
        ("---\nname: nav\ndescription: |\n  \n---", "nonempty"),
        (
            "---\nname: nav\ndescription: |\n   first\n  second\n---",
            "indentation",
        ),
        (
            "---\nname: nav\ndescription: d\nextra: |\n  text\n---",
            "block",
        ),
        ("---\nname: nav\ndescription: nested: value\n---", "nested"),
        ("---\nname: nav\ndescription: d\nextra: [x]\n---", "flow"),
        ("---\nname: nav\ndescription: d\n- list\n---", "top-level"),
        ("---\nname: nav\ndescription: d\n  extra: v\n---", "nested"),
        ("---\nname : nav\ndescription: d\n---", "key"),
        ("---\nname:nav\ndescription: d\n---", "separation"),
        ("---\nname: nav\ndescription: d\0\n---", "control"),
        ("---\nname: nav\ndescription: @bad\n---", "indicator"),
        ("---\nname: nav\ndescription: 'text'#bad\n---", "quoted"),
        (
            "---\nname: nav\ndescription: \"bad\\UFFFFFFFF\"\n---",
            "escape",
        ),
    ] {
        rejects(source, reason);
    }
    for value in [
        "", "null", "Null", "~", "true", "FALSE", "12", "1.5", "0xFF", ".inf", ".NaN",
    ] {
        rejects(
            &format!("---\nname: nav\ndescription: {value}\n---"),
            if value.is_empty() { "nested" } else { "string" },
        );
    }
}
#[test]
fn skills_frontmatter_indicator_edges_refused() {
    let mut accepted = Vec::new();
    for value in ["]end", "}end", "-\titem", "?\tkey"] {
        if validate_frontmatter(&format!("---\nname: nav\ndescription: {value}\n---")).is_ok() {
            accepted.push(value);
        }
    }
    assert!(
        accepted.is_empty(),
        "unsupported indicators accepted: {accepted:?}"
    );
}
#[test]
fn skills_frontmatter_string_and_escape_edges_accepted() {
    let mut rejected = Vec::new();
    for value in [
        "0xnavigation",
        "tRuE",
        "inf",
        "NaN",
        "_123",
        "0XFF",
        "\"text\\\t\"",
    ] {
        if validate_frontmatter(&format!("---\nname: nav\ndescription: {value}\n---")).is_err() {
            rejected.push(value);
        }
    }
    assert!(
        rejected.is_empty(),
        "valid string scalars rejected: {rejected:?}"
    );
}
#[test]
fn skills_declaration_validation() {
    for declarations in [
        vec![decl("", "skill.md")],
        vec![decl(" ", "skill.md")],
        vec![decl("nav", "")],
        vec![decl("nav", " ")],
        vec![decl("nav", "a"), decl("nav", "b")],
        vec![SkillDeclaration {
            delivery: "native".into(),
            ..decl("nav", "a")
        }],
        vec![SkillDeclaration {
            delivery: String::new(),
            ..decl("nav", "a")
        }],
    ] {
        assert!(validate_declarations(&declarations).is_err());
    }
    validate_declarations(&[]).unwrap();
    validate_declarations(&[decl("a", "x"), decl("b", "x")]).unwrap();
}
#[cfg(unix)]
fn temp() -> tempfile::TempDir {
    // Canonicalize temporary roots: some Unix platforms expose temp storage through an alias.
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
#[cfg(unix)]
#[test]
fn skills_freeze_config_relative_order_origin_and_metadata() {
    let t = temp();
    let root = t.path();
    std::fs::create_dir(root.join("config")).unwrap();
    std::fs::create_dir(root.join("checkout")).unwrap();
    assert_ne!(std::env::current_dir().unwrap(), root.join("config"));
    let path = root.join("config/skill.md");
    let source = LITERAL.replace('\n', "\r\n");
    std::fs::write(&path, &source).unwrap();
    std::fs::write(root.join("checkout/skill.md"), NAV).unwrap();
    let declarations = [
        decl("first", "skill.md"),
        decl("second", "../checkout/skill.md"),
    ];
    let before: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let selection = admit(
        &declarations,
        &root.join("config/bridge.toml"),
        &root.join("checkout"),
    )
    .unwrap();
    assert_eq!(
        selection
            .skills()
            .iter()
            .map(FrozenSkill::id)
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let frozen = &selection.skills()[0];
    assert_eq!(frozen.source().as_bytes(), source.as_bytes());
    let digest = ring::digest::digest(&ring::digest::SHA256, source.as_bytes());
    let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(frozen.sha256(), hex);
    assert_eq!(frozen.byte_count(), source.len());
    assert_eq!(frozen.origin(), Origin::External);
    assert_eq!(selection.skills()[1].origin(), Origin::Checkout);
    for (route, label) in [
        (ExecutionRoute::Direct, "direct"),
        (ExecutionRoute::Implement, "implement"),
        (
            ExecutionRoute::WorkflowPendingComposition,
            "workflow_pending_composition",
        ),
    ] {
        let metadata = frozen.preview(route);
        assert_eq!(metadata.status, format!("not_delivered({label})"));
        assert_eq!(metadata.id, "first");
        assert_eq!(metadata.sha256, hex);
        assert_eq!(metadata.bytes, source.len());
        assert_eq!(metadata.delivery, Delivery::Prompt);
        assert_eq!(metadata.origin, Origin::External);
        let rendered = format!("{metadata:?} {selection:?} {declarations:?}");
        assert!(!rendered.contains(&root.display().to_string()));
        assert!(!rendered.contains("skill.md"));
        assert!(!rendered.contains("${VALUE}"));
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, NAV).unwrap();
    let changed = admit(
        &declarations,
        &root.join("config/bridge.toml"),
        &root.join("checkout"),
    )
    .unwrap();
    assert_ne!(changed.skills()[0].sha256(), frozen.sha256());
    std::fs::remove_file(path).unwrap();
    assert_eq!(frozen.source(), source);
    let after: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert!(admit(&declarations, Path::new("relative.toml"), root).is_err());
    assert!(admit(
        &declarations,
        &root.join("bridge.toml"),
        Path::new("relative")
    )
    .is_err());
}
#[cfg(unix)]
#[test]
fn skills_size_boundaries_and_multibyte_totals() {
    let t = temp();
    let root = t.path();
    let mut source = NAV.to_owned();
    source.push_str(&"x".repeat(MAX_SKILL_BYTES - source.len()));
    std::fs::write(root.join("a"), &source).unwrap();
    std::fs::write(root.join("b"), &source).unwrap();
    let declarations = [decl("a", "a"), decl("b", "b")];
    let selection = admit(&declarations, &root.join("config.toml"), root).unwrap();
    assert_eq!(
        selection
            .skills()
            .iter()
            .map(FrozenSkill::byte_count)
            .sum::<usize>(),
        MAX_SELECTION_BYTES
    );
    std::fs::write(root.join("c"), NAV).unwrap();
    let shorter = format!("{NAV}{}", "x".repeat(MAX_SKILL_BYTES - 2 * NAV.len() + 1));
    assert_eq!(
        MAX_SKILL_BYTES + shorter.len() + NAV.len(),
        MAX_SELECTION_BYTES + 1
    );
    std::fs::write(root.join("b"), &shorter).unwrap();
    assert!(admit(
        &[decl("a", "a"), decl("b", "b"), decl("c", "c")],
        &root.join("config.toml"),
        root
    )
    .unwrap_err()
    .to_string()
    .contains("64 KiB"));
    source.push('x');
    std::fs::write(root.join("a"), &source).unwrap();
    assert!(admit(&declarations, &root.join("config.toml"), root)
        .unwrap_err()
        .to_string()
        .contains("32 KiB"));
    let prefix = "---\nname: nav\ndescription: d\n---\n";
    let unicode = format!(
        "{prefix}{}x",
        "雪".repeat((MAX_SKILL_BYTES - prefix.len()) / 3)
    );
    let unicode = format!("{unicode}{}", "x".repeat(MAX_SKILL_BYTES - unicode.len()));
    assert_eq!(unicode.len(), MAX_SKILL_BYTES);
    std::fs::write(root.join("a"), &unicode).unwrap();
    std::fs::write(root.join("b"), &unicode).unwrap();
    admit(&declarations, &root.join("config.toml"), root).unwrap();
    std::fs::write(root.join("b"), &shorter).unwrap();
    assert!(admit(
        &[decl("a", "a"), decl("b", "b"), decl("c", "c")],
        &root.join("config.toml"),
        root
    )
    .is_err());
    std::fs::write(root.join("a"), format!("{unicode}x")).unwrap();
    assert!(admit(&declarations, &root.join("config.toml"), root).is_err());
}
#[cfg(unix)]
#[test]
fn skills_safe_open_refuses_special_and_bad_sources() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let t = temp();
    let root = t.path();
    std::fs::write(root.join("valid"), NAV).unwrap();
    std::fs::write(root.join("invalid"), [0xff]).unwrap();
    std::fs::write(root.join("malformed"), "private body without header").unwrap();
    std::fs::create_dir(root.join("dir")).unwrap();
    std::fs::write(root.join("dir/valid"), NAV).unwrap();
    symlink(root.join("valid"), root.join("link")).unwrap();
    symlink(root.join("dir"), root.join("parent-link")).unwrap();
    let fifo = std::ffi::CString::new(root.join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    for path in [
        "missing",
        "invalid",
        "malformed",
        "dir",
        "link",
        "parent-link/valid",
        "parent-link/../valid",
        "valid\0",
        "fifo",
    ] {
        let start = std::time::Instant::now();
        let err = admit(&[decl("nav", path)], &root.join("config.toml"), root).unwrap_err();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        let rendered = format!("{err:?} {err}");
        assert!(!rendered.contains(&root.display().to_string()));
        assert!(!rendered.contains("private body"));
    }
    admit(
        &[decl("nav", root.join("valid").to_str().unwrap())],
        &root.join("config.toml"),
        root,
    )
    .unwrap();
}
#[cfg(not(unix))]
#[test]
fn skills_nonunix_refuses_without_fallback() {
    assert!(admit(
        &[decl("nav", "skill.md")],
        Path::new("config.toml"),
        Path::new("checkout")
    )
    .unwrap_err()
    .to_string()
    .contains("unsupported"));
}
