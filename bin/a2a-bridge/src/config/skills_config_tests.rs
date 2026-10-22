use super::super::RegistryConfig;
const BASE: &str = "default='reviewer'\n[server]\n[[agents]]\nid='reviewer'\ncmd='adapter'\n";
#[test]
fn skills_config_empty_compatibility_and_staged_refusal() {
    let config = RegistryConfig::parse(BASE).unwrap();
    assert!(config.agents[0].skills.is_empty());
    let legacy = format!("{:?}", config.into_snapshot().unwrap());
    let absent = RegistryConfig::parse(&format!("{BASE}# skills omitted\n")).unwrap();
    assert_eq!(format!("{:?}", absent.into_snapshot().unwrap()), legacy);
    let explicit_empty = RegistryConfig::parse(&format!("{BASE}skills=[]\n")).unwrap();
    assert_eq!(
        format!("{:?}", explicit_empty.into_snapshot().unwrap()),
        legacy
    );
    let config = RegistryConfig::parse(&format!(
        "{BASE}[[agents.skills]]\nid='nav'\npath='does-not-exist.md'\ndelivery='prompt'\n"
    ))
    .unwrap();
    let err = config.into_snapshot().unwrap_err().to_string();
    assert!(
        err.contains("skills") && err.contains("unsupported"),
        "{err}"
    );
    assert!(!err.contains("does-not-exist"));
}
#[test]
fn skills_config_schema_errors_omit_source_paths() {
    for fields in [
        "id='nav'\npath='/policy/private-skill.md'\ndelivery='prompt'\nunknown='private-source-marker'",
        "id='nav'\npath=['/policy/private-skill.md']\ndelivery='prompt'",
        "id={bad='/policy/private-skill.md'}\npath='x'\ndelivery='prompt'",
        "id='nav'\npath='x'\ndelivery={bad='/policy/private-skill.md'}",
    ] {
        let error = RegistryConfig::parse(&format!("{BASE}[[agents.skills]]\n{fields}\n")).unwrap_err();
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("/policy/") && !rendered.contains("private-source-marker"), "{rendered}");
    }
}
#[test]
fn skills_config_rejects_invalid_declarations_before_io() {
    for fields in [
        "id=''\npath='x'\ndelivery='prompt'",
        "id='nav'\npath=''\ndelivery='prompt'",
        "id='nav'\npath='x'\ndelivery='native'",
        "path='x'\ndelivery='prompt'",
        "id='nav'\ndelivery='prompt'",
        "id='nav'\npath='x'",
    ] {
        assert!(
            RegistryConfig::parse(&format!("{BASE}[[agents.skills]]\n{fields}\n")).is_err(),
            "{fields}"
        );
    }
    let entry = "[[agents.skills]]\nid='nav'\npath='x'\ndelivery='prompt'\n";
    assert!(RegistryConfig::parse(&format!("{BASE}{entry}{entry}")).is_err());
    RegistryConfig::parse(&format!(
        "{BASE}{entry}[[agents]]\nid='second'\ncmd='adapter'\n{entry}"
    ))
    .unwrap();
}
#[test]
fn skills_config_admission_rejects_duplicate_agents() {
    let config =
        RegistryConfig::parse(&format!("{BASE}[[agents]]\nid='reviewer'\ncmd='adapter'\n"))
            .unwrap();
    assert!(config
        .admit_skills(
            std::path::Path::new("config.toml"),
            std::path::Path::new("checkout")
        )
        .is_err());
    let config = RegistryConfig::parse(BASE).unwrap();
    assert!(config
        .admit_skills(
            std::path::Path::new("config.toml"),
            std::path::Path::new("checkout")
        )
        .unwrap()["reviewer"]
        .skills()
        .is_empty());
}
#[cfg(unix)]
#[test]
fn skills_config_admission_preserves_independent_agents() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let t = tempfile::tempdir_in(root).unwrap();
    std::fs::write(t.path().join("skill.md"), super::tests::NAV).unwrap();
    let entry = "[[agents.skills]]\nid='nav'\npath='skill.md'\ndelivery='prompt'\n";
    let config = RegistryConfig::parse(&format!(
        "{BASE}{entry}[[agents]]\nid='second'\ncmd='adapter'\n{entry}"
    ))
    .unwrap();
    let selections = config
        .admit_skills(&t.path().join("config.toml"), t.path())
        .unwrap();
    assert_eq!(selections.len(), 2);
    assert_eq!(
        selections["reviewer"].skills()[0].sha256(),
        selections["second"].skills()[0].sha256()
    );
}

#[test]
fn skills_collection_shape_errors_are_static_and_private() {
    let mut failures = Vec::new();
    for value in [
        "'/policy/x.md'",
        "['/x']",
        "[{id='nav',path='/x',delivery='prompt'}, '/x']",
        "42",
        "true",
        "{path='/x'}",
        "[42]",
        "[true]",
        "[[42]]",
        "[{id='nav',path='/x',delivery='prompt'}, 42]",
    ] {
        let input = format!("{BASE}skills={value}\n");
        let error = RegistryConfig::parse(&input).unwrap_err();
        for rendered in [format!("{error:?}"), error.to_string()] {
            if rendered.contains("/x")
                || rendered.contains("/policy/")
                || rendered.contains("skills=")
                || !rendered.contains("skills:")
            {
                failures.push(rendered);
            }
        }
    }
    assert!(
        failures.is_empty(),
        "collection diagnostics leaked: {failures:?}"
    );
}

mod r1 {
    use super::super::*;
    use crate::config::skills::tests::{decl, rejects};
    #[cfg(unix)]
    use crate::config::skills::tests::{temp, NAV};

    #[test]
    fn skills_r1_block_names_and_opaque_extras() {
        for indicator in ["|", "|-", "|+", ">", ">-", ">+"] {
            validate_frontmatter(&format!(
                "---\nname: {indicator}\n  navigator\n  skill\ndescription: d\n---\nbody"
            ))
            .unwrap();
        }
        for extra in [
            "metadata:\n  nested:\n    key: value\n  name: ignored\n",
            "allowed-tools:\n  - Read\n  - Search\n",
            "metadata: # ignored\n  literal: [opaque *bytes]\n",
        ] {
            validate_frontmatter(&format!("---\nname: nav\n{extra}description: d\n---")).unwrap();
        }
    }

    #[test]
    fn skills_r1_required_and_nested_negatives() {
        for (source, reason) in [
            (
                "---\nname: nav\ndescription: d\nmetadata:\n  x:\tbad\n---",
                "tab",
            ),
            (
                "---\nname: nav\ndescription: d\nmetadata:\n  x: bad\0\n---",
                "control",
            ),
            (
                "---\nname: nav\ndescription: d\nmetadata:\n  x: value",
                "closing",
            ),
            ("---\nname:\n  nested: wrong\ndescription: d\n---", "empty"),
            (
                "---\nname: nav\ndescription:\n  nested: wrong\n---",
                "empty",
            ),
            ("---\nname: |\n  \ndescription: d\n---", "nonempty"),
            (
                "---\nname: |\n   first\n  second\ndescription: d\n---",
                "indentation",
            ),
            ("---\nname: |\n\tbad\ndescription: d\n---", "tab"),
            (
                "---\nname: nav\nmetadata:\n  name: ignored\nname: other\ndescription: d\n---",
                "duplicate",
            ),
        ] {
            rejects(source, reason);
        }
    }

    #[test]
    fn skills_r1_ids_refuse_whitespace_and_controls() {
        let mut accepted = Vec::new();
        for id in [
            " nav",
            "nav ",
            "nav\n",
            "nav\tx",
            "nav\nx",
            "nav\0x",
            "nav\u{7f}x",
            "\u{a0}nav",
        ] {
            if validate_declarations(&[decl(id, "skill.md")]).is_ok() {
                accepted.push(id);
            }
        }
        assert!(accepted.is_empty(), "invalid IDs accepted: {accepted:?}");
        validate_declarations(&[decl("navigation-skill", "skill.md")]).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn skills_r1_freeze_block_name_and_nested_fixture() {
        let t = temp();
        for source in [
            NAV.to_owned(),
            NAV.replace("name: navigator", "name: >-\n  navigation\n  skill"),
        ] {
            std::fs::write(t.path().join("skill.md"), &source).unwrap();
            let frozen = admit(
                &[decl("nav", "skill.md")],
                &t.path().join("bridge.toml"),
                t.path(),
            )
            .unwrap();
            let skill = &frozen.skills()[0];
            assert_eq!(skill.source().as_bytes(), source.as_bytes());
            let digest = ring::digest::digest(&ring::digest::SHA256, source.as_bytes());
            let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(skill.sha256(), hex);
        }
    }

    #[cfg(unix)]
    #[test]
    fn skills_r1_debug_is_route_neutral() {
        let t = temp();
        std::fs::write(
            t.path().join("skill.md"),
            "---\nname: nav\ndescription: d\n---\n",
        )
        .unwrap();
        let frozen = admit(
            &[decl("nav", "skill.md")],
            &t.path().join("bridge.toml"),
            t.path(),
        )
        .unwrap();
        let debug = format!("{:?}", frozen.skills()[0]);
        assert!(!debug.contains("direct"), "{debug}");
        assert!(debug.contains("nav") && debug.contains(frozen.skills()[0].sha256()));
    }

    #[cfg(unix)]
    #[test]
    fn skills_r1_config_under_symlinked_ancestor() {
        let t = temp();
        std::fs::create_dir(t.path().join("real")).unwrap();
        std::os::unix::fs::symlink(t.path().join("real"), t.path().join("alias")).unwrap();
        std::fs::write(
            t.path().join("real/skill.md"),
            "---\nname: nav\ndescription: d\n---",
        )
        .unwrap();
        let frozen = admit(
            &[decl("nav", "skill.md")],
            &t.path().join("alias/bridge.toml"),
            &t.path().join("real"),
        )
        .unwrap();
        assert_eq!(frozen.skills()[0].origin(), Origin::Checkout);
        assert!(admit(
            &[decl("nav", "missing")],
            &t.path().join("missing/bridge.toml"),
            t.path()
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn skills_r1_case_variant_config_origin() {
        let t = temp();
        let actual = t.path().join("ConfigRoot");
        let variant = t.path().join("configroot");
        std::fs::create_dir(&actual).unwrap();
        if !variant.is_dir() {
            eprintln!("SKIP case-variant config: temporary volume is case-sensitive");
            return;
        }
        std::fs::write(
            actual.join("skill.md"),
            "---\nname: nav\ndescription: d\n---",
        )
        .unwrap();
        let frozen = admit(
            &[decl("nav", "skill.md")],
            &variant.join("bridge.toml"),
            &actual,
        )
        .unwrap();
        assert_eq!(frozen.skills()[0].origin(), Origin::Checkout);
    }
}
