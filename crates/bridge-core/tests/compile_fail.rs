#[test]
fn compile_fail() {
    trybuild::TestCases::new().compile_fail("../../tests/trybuild/*.rs");
}
