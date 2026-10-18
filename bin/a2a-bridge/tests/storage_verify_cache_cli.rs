//! No real runtime: a disposable shim runs only the production measurement script on fixture bytes.
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

struct Fixture {
    dir: tempfile::TempDir,
    config: std::path::PathBuf,
    cache: std::path::PathBuf,
    volume: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("payload");
        std::fs::create_dir(&cache).unwrap();
        let runtime = dir.path().join("runtime");
        let volume = bridge_controller::verify::cache_volume_name(
            "a2a-verify-cache",
            &dir.path().display().to_string(),
        );
        std::fs::write(
            &runtime,
            r#"#!/usr/bin/env python3
import os,sys,subprocess
from pathlib import Path
root=Path(__file__).parent
args=sys.argv[1:]
if args[:2] == ['volume','ls']:
    print('a2a-verify-cache-0123456789abcdef')
    print('foreign-cache-0123456789abcdef')
elif args[0] == 'ps':
    if (root/'unknown').exists(): sys.exit(1)
    if (root/'live').exists(): print('live-container')
elif args[0] == 'run':
    mount=args[args.index('--mount')+1].split(',')
    assert 'volume-nocopy' in mount, mount
    assert ('readonly' in mount) == (args[-1] == 'preview'), mount
    if (root/'notice').exists(): print('Pulling image: harmless notice', file=sys.stderr)
    if (root/'nonzero').exists(): print('{}'); sys.exit(7)
    if (root/'broken-measure').exists(): print('unavailable'); sys.exit(0)
    i=args.index('-c')
    code = args[i+1]
    if (root/'delete-failure').exists():
        code = "import shutil\ndef fail(*args, **kw): raise PermissionError('fixture deletion denied')\nshutil.rmtree = fail\n" + code
    sys.exit(subprocess.call(['python3','-c',code,str(root/'payload'),args[i+3],args[i+4]]))
else:
    sys.exit(1)
"#.replace("a2a-verify-cache-0123456789abcdef", &volume),
        )
        .unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(&config, format!("default='stub'\nagents=[]\n[server]\n[verify]\nruntime={:?}\nimage='fixture'\ncache='a2a-verify-cache'\negress='open'\n", runtime.display().to_string())).unwrap();
        Self {
            dir,
            config,
            cache,
            volume,
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_a2a-bridge"))
            .args(["storage"])
            .args(args)
            .arg("--config")
            .arg(&self.config)
            .env("A2A_LEASE_DIR", self.dir.path().join("locks"))
            .output()
            .unwrap()
    }
    fn large(&self) {
        let legacy = self.cache.join("legacy-target");
        std::fs::create_dir(&legacy).unwrap();
        // Sparse logical bytes exercise the real 20 GiB default without allocating disk space.
        std::fs::File::create(legacy.join("artifact"))
            .unwrap()
            .set_len(21 * 1024 * 1024 * 1024)
            .unwrap();
    }
}
fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", out))
}

#[test]
fn preview_and_apply_measure_only_configured_verify_volumes() {
    let f = Fixture::new();
    f.large();
    let preview = f.run(&["reap", "--verify-cache", "--dry-run", "--json"]);
    assert!(preview.status.success(), "{preview:?}");
    let rows = json(&preview);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["limit_bytes"], 20_u64 * 1024 * 1024 * 1024);
    assert_eq!(
        rows[0]["excess_bytes"].as_u64().unwrap(),
        rows[0]["measured_bytes"].as_u64().unwrap() - 20_u64 * 1024 * 1024 * 1024
    );
    assert!(f.cache.join("legacy-target/artifact").exists());
    let applied = f.run(&["reap", "--verify-cache", "--apply", "--json"]);
    assert!(applied.status.success(), "{applied:?}");
    assert!(json(&applied)[0]["retained_bytes"].as_u64().unwrap() < 20_u64 * 1024 * 1024 * 1024);
    assert!(!f.cache.join("legacy-target").exists());
}

#[test]
fn live_unknown_and_unmeasurable_caches_are_retained_with_errors() {
    for marker in ["live", "unknown", "broken-measure", "delete-failure"] {
        let f = Fixture::new();
        f.large();
        std::fs::write(f.dir.path().join(marker), "").unwrap();
        let out = f.run(&["reap", "--verify-cache", "--apply", "--json"]);
        assert!(!out.status.success(), "{marker}: {out:?}");
        assert!(json(&out)[0]["error"].is_string());
        if marker == "delete-failure" {
            let value = json(&out);
            assert!(value[0]["measured_bytes"].as_u64().unwrap() > 20_u64 * 1024 * 1024 * 1024);
            assert!(value[0]["retained_bytes"].as_u64().unwrap() > 20_u64 * 1024 * 1024 * 1024);
            assert!(value[0]["error"]
                .as_str()
                .unwrap()
                .contains("deferred excess"));
        }
        assert!(f.cache.join("legacy-target/artifact").exists());
    }
}

#[test]
fn report_exposes_measurements_limits_and_failures_in_json_and_text() {
    let f = Fixture::new();
    f.large();
    let out = f.run(&["report", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let value = json(&out);
    assert!(
        value["verify_caches"][0]["measured_bytes"]
            .as_u64()
            .unwrap()
            >= 21_u64 * 1024 * 1024 * 1024
    );
    assert!(value["verify_caches"][0]["runtime"]
        .as_str()
        .unwrap()
        .ends_with("runtime"));
    let text = f.run(&["report"]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("limit=21474836480")
            && text.contains(&format!(
                "excess={}",
                value["verify_caches"][0]["excess_bytes"]
            )),
        "{text}"
    );
    std::fs::write(f.dir.path().join("broken-measure"), "").unwrap();
    let value = json(&f.run(&["report", "--json"]));
    assert!(value["verify_caches"][0]["error"].is_string());
    assert!(value["verify_caches"][0]["measured_bytes"].is_null());
}

#[test]
fn held_verify_lock_defers_manual_trim() {
    let f = Fixture::new();
    f.large();
    let _guard = bridge_core::liveness::acquire_persistent_lock_in(
        &bridge_controller::verify::cache_volume_lock_dir(f.dir.path()).unwrap(),
        &bridge_controller::verify::cache_volume_lock_id(&f.volume),
    )
    .unwrap();
    let out = f.run(&["reap", "--verify-cache", "--apply", "--json"]);
    assert!(!out.status.success());
    assert!(json(&out)[0]["error"].as_str().unwrap().contains("lock"));
    assert!(f.cache.join("legacy-target/artifact").exists());
}

#[test]
fn helper_protocol_separates_stderr_and_rejects_malformed_or_nonzero_results() {
    for marker in ["notice", "broken-measure", "nonzero"] {
        let f = Fixture::new();
        std::fs::write(f.dir.path().join(marker), "").unwrap();
        let out = f.run(&["reap", "--verify-cache", "--dry-run", "--json"]);
        let rows = json(&out);
        if marker == "notice" {
            assert!(out.status.success(), "{out:?}");
            assert!(rows[0]["error"].is_null());
        } else {
            assert!(!out.status.success(), "{out:?}");
            let error = rows[0]["error"].as_str().unwrap();
            assert!(
                error.contains(if marker == "nonzero" {
                    "exited 7"
                } else {
                    "measurement unavailable"
                }),
                "{error}"
            );
        }
    }
}

#[test]
fn report_shows_configured_isolation_mode_and_reason() {
    let f = Fixture::new();
    let mut raw = std::fs::read_to_string(&f.config).unwrap();
    raw.push_str("\n[[languages]]\nid='rust'\nfetch='cargo fetch'\nwarm_cache='warm'\ndep_cache_path='/cargo'\nverify_cache_path='/cache'\n[[languages.verify]]\nname='test'\ncmd='make test'\ngate=true\n");
    std::fs::write(&f.config, raw).unwrap();
    let out = f.run(&["report", "--json"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        json(&out)["verify_caches"][0]["cache_modes"][0],
        "cache: isolated (non-plain command: test)"
    );
    let out = f.run(&["report"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("cache: isolated (non-plain command: test)")
    );
}
