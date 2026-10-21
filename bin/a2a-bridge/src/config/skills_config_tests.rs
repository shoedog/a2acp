use super::super::RegistryConfig;
const BASE: &str = "default='reviewer'\n[server]\n[[agents]]\nid='reviewer'\ncmd='adapter'\n";
#[test]
fn skills_config_empty_compatibility_and_staged_refusal() {
    let config = RegistryConfig::parse(BASE).unwrap();
    assert!(config.agents[0].skills.is_empty());
    config.into_snapshot().unwrap();
    let explicit_empty = RegistryConfig::parse(&format!("{BASE}skills=[]\n")).unwrap();
    explicit_empty.into_snapshot().unwrap();
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
