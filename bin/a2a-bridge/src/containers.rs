//! `a2a-bridge containers list|reap` — the operator surface over Increment A's managed containers. Docker
//! labels + the per-run `flock` lease ARE the registry (no DB), so this module is the read/cleanup view of
//! that registry. The PURE cores here (record parse, row format, reap plan) are unit-tested; the Docker
//! shell-out + lease probing live in `main.rs`'s `containers_cmd`, exercised by the live gate.

use bridge_core::run_identity::Verdict;

/// One managed container as read from `docker ps --format` (the Fold-J template, owner spliced in so we can
/// scope to THIS config's owners). Empty labels arrive as empty strings (shown as `-`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRecord {
    pub run: String,
    pub role: String,
    pub kind: String,
    pub agent: String,
    pub owner: String,
    pub host: String,
    pub lease: String,
    pub start: String,
    pub repo: String,
    pub name: String,
}

/// The exact `docker ps --format` Go template — 10 tab-separated fields in [`parse_record`] order.
pub const LIST_FORMAT: &str = "{{.Label \"a2a.run\"}}\t{{.Label \"a2a.role\"}}\t{{.Label \"a2a.kind\"}}\t{{.Label \"a2a.agent\"}}\t{{.Label \"a2a.owner\"}}\t{{.Label \"a2a.host\"}}\t{{.Label \"a2a.lease\"}}\t{{.Label \"a2a.start\"}}\t{{.Label \"a2a.repo\"}}\t{{.Names}}";

/// PURE. Parse ONE `docker ps --format <LIST_FORMAT>` line (exactly 10 tab fields) into a record; a
/// malformed line (wrong field count) yields `None` and is skipped by the caller.
pub fn parse_record(line: &str) -> Option<ContainerRecord> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != 10 {
        return None;
    }
    Some(ContainerRecord {
        run: f[0].to_string(),
        role: f[1].to_string(),
        kind: f[2].to_string(),
        agent: f[3].to_string(),
        owner: f[4].to_string(),
        host: f[5].to_string(),
        lease: f[6].to_string(),
        start: f[7].to_string(),
        repo: f[8].to_string(),
        name: f[9].to_string(),
    })
}

/// A record plus its computed liveness verdict + staleness flag (the unit the reap plan + row format act on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassifiedRecord {
    pub rec: ContainerRecord,
    pub verdict: Verdict,
    pub stale: bool,
}

/// `containers reap` flags (parsed in `main.rs`). Default (all `false`/`None`) = this-config's owners,
/// Dead-only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReapFlags {
    /// Every owner's Dead containers (this host), not just this config's.
    pub all_dead: bool,
    /// Restrict to one `a2a.run` (Dead-only).
    pub run: Option<String>,
    /// Restrict to one `a2a.owner` (Dead-only).
    pub owner: Option<String>,
    /// Reap Alive-but-stale (no recent output) containers in scope.
    pub stale: bool,
    /// Reap EXACTLY this container name regardless of verdict (the only Alive/legacy override).
    pub force: Option<String>,
}

/// PURE. The container NAMES to reap for `flags` over `records`, scoped to `my_owners` by default. Invariant:
/// the plan NEVER includes an Alive container unless `--stale` (alive+stale) or `--force <name>` (exact).
pub fn reap_plan(
    records: &[ClassifiedRecord],
    flags: &ReapFlags,
    my_owners: &[String],
) -> Vec<String> {
    // `--force <name>`: exactly that name, regardless of verdict / managed-ness (covers legacy + a live one).
    if let Some(name) = &flags.force {
        return vec![name.clone()];
    }
    if flags.stale {
        return records
            .iter()
            .filter(|c| {
                in_scope(&c.rec, flags, my_owners) && c.verdict == Verdict::Alive && c.stale
            })
            .map(|c| c.rec.name.clone())
            .collect();
    }
    records
        .iter()
        .filter(|c| in_scope(&c.rec, flags, my_owners) && c.verdict == Verdict::Dead)
        .map(|c| c.rec.name.clone())
        .collect()
}

/// PURE. Whether a record is in the reap/list scope: `--all-dead` = every owner; `--run`/`--owner` pin one;
/// else this config's owners.
fn in_scope(rec: &ContainerRecord, flags: &ReapFlags, my_owners: &[String]) -> bool {
    if flags.all_dead {
        return true;
    }
    if let Some(run) = &flags.run {
        return &rec.run == run;
    }
    if let Some(owner) = &flags.owner {
        return &rec.owner == owner;
    }
    my_owners.iter().any(|o| o == &rec.owner)
}

/// PURE. Whether a record is shown by `containers list` (default scopes to this config's owners; `--all`
/// shows every managed container on the host).
pub fn list_visible(rec: &ContainerRecord, all: bool, my_owners: &[String]) -> bool {
    all || my_owners.iter().any(|o| o == &rec.owner)
}

/// PURE. Compact human age from seconds (`5s`/`4m`/`2h`/`3d`).
pub fn human_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

fn dash(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

/// The `containers list` table header (column order matches [`format_row`]).
pub const LIST_HEADER: &str =
    "NAME                         ROLE KIND     AGENT      STATE    STALE  AGE   REPO";

/// PURE. One `containers list` row. `now_epoch` lets `age = now - a2a.start` be tested deterministically;
/// an unparseable `start` shows `-`.
pub fn format_row(c: &ClassifiedRecord, now_epoch: u64) -> String {
    let age = c
        .rec
        .start
        .parse::<u64>()
        .ok()
        .map(|s| human_age(now_epoch.saturating_sub(s)))
        .unwrap_or_else(|| "-".to_string());
    let verdict = match c.verdict {
        Verdict::Alive => "alive",
        Verdict::Dead => "dead",
        Verdict::Unknown => "unknown",
    };
    let stale = if c.stale { "stale" } else { "-" };
    format!(
        "{:<28} {:<4} {:<8} {:<10} {:<8} {:<6} {:<5} {}",
        dash(&c.rec.name),
        dash(&c.rec.role),
        dash(&c.rec.kind),
        dash(&c.rec.agent),
        verdict,
        stale,
        age,
        dash(&c.rec.repo),
    )
}

/// PURE. Is `name` a legacy (pre-Increment-A, unlabeled) managed container name? Used for the list-only
/// legacy pass — these carry no `a2a.managed` label so they're invisible to the labeled query.
pub fn is_legacy_name(name: &str) -> bool {
    name.starts_with("a2a-ro-") || name.starts_with("a2a-rw-")
}

pub const CONTAINERS_USAGE: &str = "\
usage: a2a-bridge containers <list|reap> [options]
  list [--config <f>] [--all] [--older-than <dur>]
                      show this config's managed containers (alive/dead/unknown + stale + age + legacy).
                      --all shows every managed container on the host; --older-than sets the stale window.
  reap [--config <f>] [--all-dead] [--run <id>] [--owner <hash>] [--stale [--older-than <dur>]] [--force <name>]
                      reap DEAD (crashed) containers. Default: this config's owners, Dead-only.
                      --all-dead every owner's Dead; --run/--owner pin a scope (Dead-only);
                      --stale reaps Alive-but-idle (no output within the window);
                      --force <name> reaps exactly that container regardless of state (incl. legacy).
  --config <path>     registry config (default: ./a2a-bridge.toml); scopes default list/reap to its owners.";

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(owner: &str, name: &str, run: &str, start: &str) -> ContainerRecord {
        ContainerRecord {
            run: run.into(),
            role: "rw".into(),
            kind: "warm".into(),
            agent: "impl".into(),
            owner: owner.into(),
            host: "h1".into(),
            lease: "/l/r.lock".into(),
            start: start.into(),
            repo: "/repo".into(),
            name: name.into(),
        }
    }
    fn classified(owner: &str, name: &str, v: Verdict, stale: bool) -> ClassifiedRecord {
        ClassifiedRecord {
            rec: rec(owner, name, "r1", "100"),
            verdict: v,
            stale,
        }
    }

    #[test]
    fn parse_record_needs_exactly_ten_fields() {
        let line = "r1\trw\twarm\timpl\town9\th1\t/l/r.lock\t1700\t/repo\ta2a-rw-own9-r1-0";
        let r = parse_record(line).unwrap();
        assert_eq!(r.run, "r1");
        assert_eq!(r.owner, "own9");
        assert_eq!(r.name, "a2a-rw-own9-r1-0");
        assert_eq!(r.repo, "/repo");
        // wrong field count → skipped
        assert!(parse_record("too\tfew\tfields").is_none());
    }

    #[test]
    fn reap_plan_default_is_my_owners_dead_only() {
        let recs = vec![
            classified("mine", "a-dead", Verdict::Dead, false),
            classified("mine", "a-alive", Verdict::Alive, false),
            classified("other", "b-dead", Verdict::Dead, false),
        ];
        let plan = reap_plan(&recs, &ReapFlags::default(), &["mine".into()]);
        assert_eq!(plan, vec!["a-dead".to_string()], "only my owner's Dead");
    }

    #[test]
    fn reap_plan_all_dead_spans_every_owner_but_only_dead() {
        let recs = vec![
            classified("mine", "a-dead", Verdict::Dead, false),
            classified("other", "b-dead", Verdict::Dead, false),
            classified("other", "b-alive", Verdict::Alive, false),
        ];
        let flags = ReapFlags {
            all_dead: true,
            ..Default::default()
        };
        let mut plan = reap_plan(&recs, &flags, &["mine".into()]);
        plan.sort();
        assert_eq!(plan, vec!["a-dead".to_string(), "b-dead".to_string()]);
    }

    #[test]
    fn reap_plan_never_reaps_alive_without_stale_or_force() {
        let recs = vec![classified("mine", "a-alive", Verdict::Alive, true)];
        // default + all-dead must NOT touch an Alive container even if it's stale.
        assert!(reap_plan(&recs, &ReapFlags::default(), &["mine".into()]).is_empty());
        let flags = ReapFlags {
            all_dead: true,
            ..Default::default()
        };
        assert!(reap_plan(&recs, &flags, &["mine".into()]).is_empty());
    }

    #[test]
    fn reap_plan_stale_reaps_alive_and_stale_only() {
        let recs = vec![
            classified("mine", "idle", Verdict::Alive, true),
            classified("mine", "busy", Verdict::Alive, false),
            classified("mine", "gone", Verdict::Dead, true),
        ];
        let flags = ReapFlags {
            stale: true,
            ..Default::default()
        };
        assert_eq!(
            reap_plan(&recs, &flags, &["mine".into()]),
            vec!["idle".to_string()],
            "--stale = Alive AND stale, in scope"
        );
    }

    #[test]
    fn reap_plan_force_targets_exact_name_regardless_of_verdict() {
        let recs = vec![classified("mine", "a-alive", Verdict::Alive, false)];
        let flags = ReapFlags {
            force: Some("a2a-rw-legacy-name".into()),
            ..Default::default()
        };
        // returns the forced name even though it's not in the managed records (legacy override).
        assert_eq!(
            reap_plan(&recs, &flags, &[]),
            vec!["a2a-rw-legacy-name".to_string()]
        );
    }

    #[test]
    fn reap_plan_run_and_owner_scopes_are_dead_only() {
        let recs = vec![
            classified("mine", "a-dead", Verdict::Dead, false),
            classified("other", "b-dead", Verdict::Dead, false),
        ];
        let by_owner = ReapFlags {
            owner: Some("other".into()),
            ..Default::default()
        };
        assert_eq!(
            reap_plan(&recs, &by_owner, &["mine".into()]),
            vec!["b-dead".to_string()]
        );
        let mut by_run = ClassifiedRecord {
            rec: rec("mine", "c-dead", "RUNX", "100"),
            verdict: Verdict::Dead,
            stale: false,
        };
        by_run.rec.run = "RUNX".into();
        let flags = ReapFlags {
            run: Some("RUNX".into()),
            ..Default::default()
        };
        assert_eq!(
            reap_plan(&[by_run], &flags, &["mine".into()]),
            vec!["c-dead".to_string()]
        );
    }

    #[test]
    fn list_visible_default_scopes_to_my_owners() {
        let r = rec("mine", "n", "r", "100");
        assert!(list_visible(&r, false, &["mine".into()]));
        assert!(!list_visible(&r, false, &["other".into()]));
        assert!(
            list_visible(&r, true, &["other".into()]),
            "--all shows everything"
        );
    }

    #[test]
    fn human_age_buckets() {
        assert_eq!(human_age(5), "5s");
        assert_eq!(human_age(125), "2m");
        assert_eq!(human_age(7200), "2h");
        assert_eq!(human_age(180_000), "2d");
    }

    #[test]
    fn format_row_shows_verdict_stale_and_age() {
        let c = ClassifiedRecord {
            rec: rec("mine", "a2a-rw-mine-r1-0", "r1", "100"),
            verdict: Verdict::Dead,
            stale: false,
        };
        let row = format_row(&c, 160); // now=160, start=100 → 60s → "1m"
        assert!(row.contains("a2a-rw-mine-r1-0"));
        assert!(row.contains("dead"));
        assert!(row.contains("1m"));
        assert!(row.contains("/repo"));
        // stale row
        let c2 = ClassifiedRecord {
            verdict: Verdict::Alive,
            stale: true,
            ..c
        };
        assert!(format_row(&c2, 160).contains("stale"));
    }

    #[test]
    fn legacy_name_detection() {
        assert!(is_legacy_name("a2a-ro-deadbeef-x"));
        assert!(is_legacy_name("a2a-rw-deadbeef-x"));
        assert!(!is_legacy_name("a2a-other"));
        assert!(!is_legacy_name("random"));
    }
}
