//! `--lang auto` detection predicates (spec §1). Uses tempdirs to build marker fixtures.
use lsp_mcp::lang::{detect_lang, Lang};
use std::fs;

fn td() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn cargo_toml_is_rust() {
    let d = td();
    fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    assert_eq!(detect_lang(d.path()).unwrap(), Lang::Rust);
}

#[test]
fn setup_py_is_python() {
    let d = td();
    fs::write(d.path().join("setup.py"), "from setuptools import setup\n").unwrap();
    assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
}

#[test]
fn requirements_txt_is_python() {
    let d = td();
    fs::write(d.path().join("requirements-dev.txt"), "pytest\n").unwrap();
    assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
}

#[test]
fn pyproject_with_real_section_is_python() {
    let d = td();
    fs::write(d.path().join("pyproject.toml"), "[project]\nname=\"x\"\n").unwrap();
    assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
}

#[test]
fn tooling_only_pyproject_is_not_python_by_marker_but_py_scan_wins() {
    let d = td();
    // ONLY a tooling table — not a real project/dep section → not python by the pyproject marker...
    fs::write(
        d.path().join("pyproject.toml"),
        "[tool.black]\nline-length=100\n",
    )
    .unwrap();
    // ...but a real .py file at the root makes it python via the shallow scan.
    fs::write(d.path().join("app.py"), "x = 1\n").unwrap();
    assert_eq!(detect_lang(d.path()).unwrap(), Lang::Python);
}

#[test]
fn tooling_only_pyproject_with_no_py_is_unknown() {
    let d = td();
    fs::write(
        d.path().join("pyproject.toml"),
        "[tool.ruff]\nline-length=100\n",
    )
    .unwrap();
    assert!(
        detect_lang(d.path()).is_err(),
        "tooling-only pyproject + no .py → cannot detect"
    );
}

#[test]
fn py_scan_excludes_venv_and_dotdirs() {
    let d = td();
    fs::create_dir_all(d.path().join(".venv/lib")).unwrap();
    fs::write(d.path().join(".venv/lib/dep.py"), "x=1\n").unwrap();
    fs::create_dir_all(d.path().join("node_modules/pkg")).unwrap();
    fs::write(d.path().join("node_modules/pkg/m.py"), "x=1\n").unwrap();
    // .py only inside excluded dirs → NOT python.
    assert!(
        detect_lang(d.path()).is_err(),
        "excluded-dir .py must not count"
    );
}

#[test]
fn py_scan_finds_py_in_subdir() {
    // No root markers; single `.py` is inside a non-excluded subdir `src/`.
    // This locks the positive recursion path: the scan MUST descend into subdirs, not just root.
    let d = td();
    fs::create_dir_all(d.path().join("src")).unwrap();
    fs::write(d.path().join("src/app.py"), "x = 1\n").unwrap();
    assert_eq!(
        detect_lang(d.path()).unwrap(),
        Lang::Python,
        "shallow scan should recurse into `src/` and find app.py"
    );
}

#[test]
fn both_rust_and_python_markers_are_ambiguous() {
    let d = td();
    fs::write(d.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    fs::write(d.path().join("setup.py"), "from setuptools import setup\n").unwrap();
    let err = detect_lang(d.path()).unwrap_err().to_string();
    assert!(
        err.contains("ambiguous"),
        "both markers → ambiguous refusal, got {err}"
    );
}
