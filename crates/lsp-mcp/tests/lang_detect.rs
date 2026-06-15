//! `--lang auto` detection predicates (spec §1). Uses tempdirs to build marker fixtures.
use lsp_mcp::lang::{detect_lang, resolve_python_path, Lang, PyResolve};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn td() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn make_exe(p: &std::path::Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "#!/bin/sh\n").unwrap();
    let mut perm = std::fs::metadata(p).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(p, perm).unwrap();
}

/// A regular file with NO execute bits — exists but is not a usable interpreter.
fn make_nonexec(p: &std::path::Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "not executable\n").unwrap();
    let mut perm = std::fs::metadata(p).unwrap().permissions();
    perm.set_mode(0o644);
    std::fs::set_permissions(p, perm).unwrap();
}

#[test]
fn explicit_flag_wins() {
    let d = td();
    let py = d.path().join("custom/python");
    make_exe(&py);
    match resolve_python_path(d.path(), Some(&py), None) {
        PyResolve::Resolved(p) => assert_eq!(p, py),
        _ => panic!("explicit valid path must Resolve"),
    }
}

#[test]
fn explicit_invalid_path_is_hard_error() {
    let d = td();
    // (a) missing explicit path → Hard (caller bails; NO silent python3 fallback).
    let missing = d.path().join("nope/python");
    assert!(
        matches!(
            resolve_python_path(d.path(), Some(&missing), None),
            PyResolve::Hard(_)
        ),
        "missing explicit path → Hard error, not Fallback"
    );
    // (b) present-but-non-executable explicit path → Hard.
    let noexec = d.path().join("bin/python");
    make_nonexec(&noexec);
    assert!(
        matches!(
            resolve_python_path(d.path(), Some(&noexec), None),
            PyResolve::Hard(_)
        ),
        "non-executable explicit path → Hard error, not Fallback"
    );
}

#[test]
fn nonexecutable_venv_python_is_not_usable() {
    let d = td();
    // A `.venv/bin/python` that exists but isn't executable must NOT be accepted as a venv interpreter.
    make_nonexec(&d.path().join(".venv/bin/python"));
    assert!(
        matches!(
            resolve_python_path(d.path(), None, None),
            PyResolve::Fallback
        ),
        "non-executable venv python is skipped → Fallback"
    );
}

#[test]
fn virtual_env_beats_dot_venv() {
    let d = td();
    let ve = d.path().join("ve");
    make_exe(&ve.join("bin/python"));
    make_exe(&d.path().join(".venv/bin/python"));
    match resolve_python_path(d.path(), None, Some(ve.as_path())) {
        PyResolve::Resolved(p) => {
            assert_eq!(
                p,
                ve.join("bin/python"),
                "$VIRTUAL_ENV precedes <repo>/.venv"
            )
        }
        _ => panic!("must Resolve to the $VIRTUAL_ENV interpreter"),
    }
}

#[test]
fn dot_venv_then_venv() {
    let d = td();
    make_exe(&d.path().join("venv/bin/python")); // only `venv`, no `.venv`
    match resolve_python_path(d.path(), None, None) {
        PyResolve::Resolved(p) => assert_eq!(p, d.path().join("venv/bin/python")),
        _ => panic!("must Resolve to <repo>/venv/bin/python"),
    }
}

#[test]
fn no_venv_falls_back_to_python3_with_warning() {
    let d = td(); // empty repo, no venv, no $VIRTUAL_ENV, no explicit override
    assert!(
        matches!(resolve_python_path(d.path(), None, None), PyResolve::Fallback),
        "no venv + no explicit override → Fallback (caller uses python3 + LOGGED WARNING, not silent)"
    );
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

#[test]
fn relative_explicit_python_path_resolves_against_repo() {
    // A RELATIVE explicit path must be joined onto `repo` (not the process cwd): basedpyright is spawned
    // with `current_dir(repo)` and consumes `pythonPath` relative to the repo cwd. The discovered absolute
    // path must agree with what basedpyright sees, not what the process-cwd-relative stat would return.
    let d = td();
    let abs = d.path().join(".venv/bin/python");
    make_exe(&abs);
    // Pass the RELATIVE form — ".venv/bin/python" is relative to the repo root.
    let relative = std::path::Path::new(".venv/bin/python");
    match resolve_python_path(d.path(), Some(relative), None) {
        PyResolve::Resolved(p) => assert_eq!(
            p, abs,
            "relative explicit path must resolve to the repo-joined absolute path"
        ),
        PyResolve::Hard(p) => panic!("relative explicit path should Resolve, got Hard({p:?})"),
        PyResolve::Fallback => panic!("relative explicit path must not fall back to python3"),
    }
}
