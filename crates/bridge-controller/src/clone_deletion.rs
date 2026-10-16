//! Positive allowlist for removing a landed implement clone. Unknown state keeps it.
use crate::{
    custody_push::{custody_status, ls_remote_ref, CustodyConfig},
    evidence::EvidenceConfig,
    implement::{run_clone_git, run_git},
    implement_resume::ImplementCheckpoint,
};
use std::{fs, path::Path};

/// Shared `ls-files -v` parser: only ordinary cached (`H`) entries are disposable.
pub fn index_flags_disposition(listing: &str) -> Result<String, String> {
    let mut flagged: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut entries = 0usize;
    for line in listing.lines() {
        if line.trim().is_empty() {
            continue;
        }
        entries += 1;
        let first = line.chars().next().unwrap_or('?');
        if first == 'H' {
            continue;
        }
        total += 1;
        if flagged.len() < 5 {
            flagged.push(line.trim().to_string());
        }
    }
    if total > 0 {
        return Err(format!(
            "{total} index entry/entries carry a non-`H` flag (assume-unchanged, skip-worktree, \
             unmerged or sparse), so `git status` is blind to their worktree bytes — first {}: {}",
            flagged.len(),
            flagged.join(" | ")
        ));
    }
    Ok(format!(
        "{entries} index entr(y/ies), all plain `H` (cached)"
    ))
}

fn clone_query(clone: &Path, args: &[&str]) -> Result<String, String> {
    let out = run_clone_git(
        clone,
        &[&["-c", "core.hooksPath=/dev/null"][..], args].concat(),
    )
    .map_err(|_| "clone git query failed")?;
    if !out.status.success() {
        return Err("clone git query failed".into());
    }
    String::from_utf8(out.stdout).map_err(|_| "invalid clone git output".into())
}
fn refs(clone: &Path, source: &Path, task: &str, sha: &str) -> Result<(), String> {
    let branch = format!("refs/heads/implement/{task}");
    if clone_query(clone, &["symbolic-ref", "HEAD"])?.trim() != branch {
        return Err("HEAD is not the run branch".into());
    }
    let listing = clone_query(
        clone,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname) %(objecttype) %(symref)",
        ],
    )?;
    let mut run_found = false;
    for line in listing.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        // Symbolic refs are skipped; their targets are checked separately.
        if fields.len() == 4 {
            continue;
        }
        if fields.len() != 3 {
            return Err("unrecognized ref output".into());
        }
        let (name, oid, kind) = (fields[0], fields[1], fields[2]);
        if name == branch {
            if oid != sha || kind != "commit" {
                return Err("run branch differs from pushed HEAD".into());
            }
            run_found = true;
            continue;
        }
        if !["refs/heads/", "refs/remotes/", "refs/tags/"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            return Err(format!("unrecognized ref namespace: {name}"));
        }
        let args = match kind {
            "commit" => vec!["for-each-ref", "--format=%(objectname)", "--contains", oid],
            "tag" => vec!["for-each-ref", "--format=%(objectname)"],
            _ => return Err(format!("unrecognized ref object: {name}")),
        };
        let out = run_git(Some(source), &args).map_err(|_| "source ref query failed")?;
        let values = String::from_utf8_lossy(&out.stdout);
        if !out.status.success()
            || if kind == "tag" {
                !values.lines().any(|v| v == oid)
            } else {
                values.trim().is_empty()
            }
        {
            return Err(format!("source does not retain exact object for {name}"));
        }
    }
    if !run_found {
        return Err("run branch missing".into());
    }
    Ok(())
}
fn nested_git(dir: &Path, root: bool) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|_| "cannot walk clone")? {
        let entry = entry.map_err(|_| "cannot read clone entry")?;
        if entry.file_name() == ".git" {
            if root {
                continue;
            }
            return Err(format!("nested .git: {}", entry.path().display()));
        }
        if entry
            .file_type()
            .map_err(|_| "cannot inspect clone entry")?
            .is_dir()
        {
            nested_git(&entry.path(), false)?;
        }
    }
    Ok(())
}
fn present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("cannot inspect git directory".into()),
    }
}
fn shape(clone: &Path) -> Result<(), String> {
    index_flags_disposition(&clone_query(clone, &["ls-files", "-v"])?)?;
    if clone_query(clone, &["ls-files", "-s"])?
        .lines()
        .any(|l| l.starts_with("160000 "))
    {
        return Err("index contains a gitlink".into());
    }
    let git = clone.join(".git");
    for name in ["modules", "rebase-merge", "rebase-apply", "sequencer"] {
        if present(&git.join(name))? {
            return Err(format!("git directory contains {name}"));
        }
    }
    let worktrees = git.join("worktrees");
    if present(&worktrees)?
        && fs::read_dir(&worktrees)
            .map_err(|_| "cannot inspect worktrees")?
            .next()
            .is_some()
    {
        return Err("git directory contains worktrees".into());
    }
    for entry in fs::read_dir(&git).map_err(|_| "cannot inspect git directory")? {
        let name = entry.map_err(|_| "cannot inspect git entry")?.file_name();
        let name = name.to_string_lossy();
        if name.ends_with("_HEAD") && name != "ORIG_HEAD" && name != "FETCH_HEAD" {
            return Err(format!("git directory contains {name}"));
        }
    }
    nested_git(clone, true)
}
/// Returns every failed check. Config admission happens before any in-clone Git call.
pub fn failures(
    clone: &Path,
    ck: &ImplementCheckpoint,
    custody: &CustodyConfig,
    evidence: &EvidenceConfig,
) -> Vec<String> {
    let mut failures = Vec::new();
    let admitted = crate::clone_config::check_clone_with_policy(clone);
    if let Err(e) = &admitted {
        failures.push(format!("check 0: clone config not allowlisted: {e}"));
    }
    let head = if admitted.is_ok() {
        clone_query(clone, &["rev-parse", "HEAD"]).map(|h| h.trim().to_owned())
    } else {
        Err("clone config not allowlisted".into())
    };
    let mut check = |n, result: Result<(), String>| {
        if let Err(e) = result {
            failures.push(format!("check {n}: {e}"));
        }
    };
    check(
        1,
        if !custody.push {
            Err("push disabled".into())
        } else {
            ls_remote_ref(
                &ck.source_repo,
                clone,
                custody,
                &format!("refs/heads/{}{}", custody.ref_prefix, ck.task_id),
            )
            .and_then(|remote| match &head {
                Ok(h) if remote.as_ref() == Some(h) => Ok(()),
                _ => Err("remote run ref is not at HEAD".into()),
            })
        },
    );
    if admitted.is_ok() {
        check(
            2,
            custody_status(clone).and_then(|dirty| {
                if dirty {
                    Err("clone is dirty".into())
                } else {
                    Ok(())
                }
            }),
        );
    }
    check(
        3,
        head.as_ref().map_err(Clone::clone).and_then(|h| {
            crate::evidence::verify_merged_archive(clone, ck, h, evidence)
                .map_err(|e| e.to_string())
        }),
    );
    if admitted.is_ok() {
        check(
            4,
            head.as_ref()
                .map_err(Clone::clone)
                .and_then(|h| refs(clone, &ck.source_repo, &ck.task_id, h)),
        );
        check(5, shape(clone));
    }
    failures
}
