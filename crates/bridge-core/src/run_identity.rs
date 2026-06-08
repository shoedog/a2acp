//! Per-run container identity (Increment A): the label set stamped on every managed container + the pure
//! liveness `classify`. Docker labels + an OS file-lock (see [`crate::liveness`]) ARE the registry — no DB.
//! The process-identity value is `instance_id` (distinct from the executor/task `run_id` execution id); it
//! travels as the `a2a.run` docker label.

/// The label set stamped on every managed (`:rw`/`:ro`) container. Identity values are hashes/ids/paths
/// (docker-label-safe); `repo`/`cwd` are display-only (sanitize at the call site; `None` ⇒ omitted).
#[derive(Clone, Debug)]
pub struct ContainerLabels {
    pub role: String,   // "rw" | "ro"
    pub kind: String,   // "warm" | "perturn" | "oneshot"
    pub agent: String,
    pub owner: String,
    pub run_id: String, // holds the process `instance_id`; emitted as `a2a.run`
    pub host: String,
    pub lease: String,  // absolute lease-file path
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub start: String,  // epoch seconds (display-only)
}

impl ContainerLabels {
    /// `(key, value)` pairs; `a2a.managed=1` always, display-only fields only when `Some`.
    pub fn to_arg_pairs(&self) -> Vec<(String, String)> {
        let mut v = vec![
            ("a2a.managed".into(), "1".into()),
            ("a2a.role".into(), self.role.clone()),
            ("a2a.kind".into(), self.kind.clone()),
            ("a2a.agent".into(), self.agent.clone()),
            ("a2a.owner".into(), self.owner.clone()),
            ("a2a.run".into(), self.run_id.clone()),
            ("a2a.host".into(), self.host.clone()),
            ("a2a.lease".into(), self.lease.clone()),
            ("a2a.start".into(), self.start.clone()),
        ];
        if let Some(r) = &self.repo {
            v.push(("a2a.repo".into(), r.clone()));
        }
        if let Some(c) = &self.cwd {
            v.push(("a2a.cwd".into(), c.clone()));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ContainerLabels {
        ContainerLabels {
            role: "rw".into(),
            kind: "warm".into(),
            agent: "impl".into(),
            owner: "abc".into(),
            run_id: "r1".into(),
            host: "h1".into(),
            lease: "/l/r1.lock".into(),
            repo: Some("/Users/w/code/proj".into()),
            cwd: Some("/Users/w/code/proj".into()),
            start: "1700000000".into(),
        }
    }

    #[test]
    fn container_labels_emit_managed_label_set() {
        let args = sample().to_arg_pairs();
        assert!(args.contains(&("a2a.managed".into(), "1".into())));
        assert!(args.contains(&("a2a.role".into(), "rw".into())));
        assert!(args.contains(&("a2a.run".into(), "r1".into())));
        assert!(args.contains(&("a2a.host".into(), "h1".into())));
        assert!(args.contains(&("a2a.lease".into(), "/l/r1.lock".into())));
        assert!(args
            .iter()
            .any(|(k, v)| k == "a2a.repo" && v == "/Users/w/code/proj"));
    }

    #[test]
    fn container_labels_omit_absent_display_fields() {
        let l = ContainerLabels {
            repo: None,
            cwd: None,
            ..sample()
        };
        let args = l.to_arg_pairs();
        assert!(!args.iter().any(|(k, _)| k == "a2a.repo" || k == "a2a.cwd"));
    }
}
