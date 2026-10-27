use std::path::Path;

#[test]
fn default_profile_bounds_tests_without_retries() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.config/nextest.toml");
    let config: toml::Value = std::fs::read_to_string(path).unwrap().parse().unwrap();
    let profile = &config["profile"]["default"];
    assert_eq!(profile["retries"].as_integer(), Some(0));
    assert!(
        profile
            .get("slow-timeout")
            .and_then(|timeout| timeout.get("terminate-after"))
            .and_then(toml::Value::as_integer)
            .is_some_and(|count| count > 0),
        "default profile must terminate slow tests after a finite positive number of periods"
    );
}
