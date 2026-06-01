mod harness;
#[test]
fn fake_materializes_executable_script() {
    let (cmd, cfg) = harness::fake_default("smoke");
    assert!(std::path::Path::new(&cmd).exists());
    assert!(cfg.extra_args.iter().any(|a| a == "--fake-config"));
}
