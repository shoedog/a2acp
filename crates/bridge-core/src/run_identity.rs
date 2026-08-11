//! Per-run container identity (Increment A): the label set stamped on every managed container + the pure
//! liveness `classify`. Docker labels + an OS file-lock (see [`crate::liveness`]) ARE the registry — no DB.
//! The process-identity value is `instance_id` (distinct from the executor/task `run_id` execution id); it
//! travels as the `a2a.run` docker label.

use crate::execution_policy::Sha256HexV1;

/// The label set stamped on every managed (`:rw`/`:ro`) container. Identity values are hashes/ids/paths
/// (docker-label-safe); `repo`/`cwd` are display-only (sanitize at the call site; `None` ⇒ omitted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerLabels {
    pub role: String, // "rw" | "ro"
    pub kind: String, // "warm" | "perturn" | "oneshot"
    pub agent: String,
    pub owner: String,
    pub run_id: String, // holds the process `instance_id`; emitted as `a2a.run`
    pub host: String,
    pub lease: String, // absolute lease-file path
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub start: String, // epoch seconds (display-only)
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

    /// Build the one canonical ownership-label capability used by container
    /// composition, spawn evidence validation, and destructive authority.
    #[must_use]
    pub fn canonical_ownership(&self) -> CanonicalContainerOwnershipV1 {
        CanonicalContainerOwnershipV1::new(self.to_arg_pairs())
    }
}

/// Canonical capability evidence for the labels stamped by this bridge.
/// Runtime maps have no stable order, so the digest uses constructor order.
/// Validation requires every canonical key to remain present and equal while
/// tolerating future or image-supplied `a2a.*` keys that this bridge did not
/// stamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalContainerOwnershipV1 {
    ordered: Vec<(String, String)>,
    digest: Sha256HexV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContainerOwnershipErrorV1 {
    #[error("container ownership labels contain a duplicate key")]
    Duplicate,
    #[error("container ownership labels differ from the canonical set")]
    Mismatch,
}

impl CanonicalContainerOwnershipV1 {
    fn new(ordered: Vec<(String, String)>) -> Self {
        let bytes = serde_json::to_vec(&ordered)
            .expect("canonical container ownership labels are JSON encodable");
        Self {
            ordered,
            digest: Sha256HexV1::digest(&bytes),
        }
    }

    #[must_use]
    pub fn ordered(&self) -> &[(String, String)] {
        &self.ordered
    }

    #[must_use]
    pub fn digest(&self) -> &Sha256HexV1 {
        &self.digest
    }

    pub fn validate_observed(
        &self,
        observed: &[(String, String)],
    ) -> Result<(), ContainerOwnershipErrorV1> {
        use std::collections::BTreeMap;
        let expected: BTreeMap<&str, &str> = self
            .ordered
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let mut actual = BTreeMap::new();
        for (key, value) in observed.iter().filter(|(key, _)| key.starts_with("a2a.")) {
            if actual.insert(key.as_str(), value.as_str()).is_some() {
                return Err(ContainerOwnershipErrorV1::Duplicate);
            }
        }
        if expected
            .iter()
            .all(|(key, value)| actual.get(key).copied() == Some(*value))
        {
            Ok(())
        } else {
            Err(ContainerOwnershipErrorV1::Mismatch)
        }
    }
}

/// One per bridge PROCESS (a one-shot `implement`/`run-workflow`, or a `serve`). `instance_id` is the
/// process-identity (label `a2a.run`) — deliberately distinct from the executor/task `run_id` execution id.
#[derive(Clone, Debug)]
pub struct RunHandle {
    pub instance_id: String,
    pub host: String,
    pub lease: String,
    pub start: String, // epoch seconds
}

impl RunHandle {
    /// Build the per-container label set for one mint. `kind` is set PER MINT by the caller (warm/perturn/
    /// oneshot) so it's never stale; `owner` is the per-agent `container_owner` hash.
    pub fn labels(
        &self,
        role: &str,
        kind: &str,
        agent: &str,
        owner: &str,
        repo: Option<&str>,
        cwd: Option<&str>,
    ) -> ContainerLabels {
        ContainerLabels {
            role: role.into(),
            kind: kind.into(),
            agent: agent.into(),
            owner: owner.into(),
            run_id: self.instance_id.clone(),
            host: self.host.clone(),
            lease: self.lease.clone(),
            repo: repo.map(sanitize_display),
            cwd: cwd.map(sanitize_display),
            start: self.start.clone(),
        }
    }
}

/// Liveness verdict for a managed container's owner. Only `Dead` permits an automatic reap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Alive,
    Dead,
    Unknown,
}

/// PURE. Classify a container's owner from its labels + a lease probe. Fail-safe toward SPARING: a different
/// host, a missing `a2a.lease`, or an unreadable lease all yield `Unknown` (treated as Alive by callers).
/// Only same-host + a FREE lease lock yields `Dead`.
pub fn classify(
    labels: &std::collections::HashMap<String, String>,
    my_host: &str,
    probe: &dyn crate::liveness::LeaseProbe,
) -> Verdict {
    match labels.get("a2a.host") {
        Some(h) if h != my_host => return Verdict::Unknown, // another machine
        None => return Verdict::Unknown,
        _ => {}
    }
    let Some(lease) = labels.get("a2a.lease") else {
        return Verdict::Unknown;
    };
    match probe.try_state(lease) {
        Some(true) => Verdict::Dead,   // lock free ⇒ owner gone
        Some(false) => Verdict::Alive, // lock held ⇒ owner alive
        None => Verdict::Unknown,      // absent/unreadable ⇒ spare
    }
}

/// Display-label hygiene: printable ASCII + space + `/`, length-capped — never breaks label syntax.
fn sanitize_display(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ' || *c == '/')
        .take(200)
        .collect()
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

    #[test]
    fn canonical_ownership_has_exact_order_digest_and_required_canonical_keys() {
        let ownership = sample().canonical_ownership();
        assert_eq!(ownership.ordered(), sample().to_arg_pairs());
        assert_eq!(
            ownership.digest().as_str(),
            "4126b2d6672d795aaf23bd4b819f6b3449e9484466bd87525c2e81119624f055"
        );

        let mut reordered = ownership.ordered().to_vec();
        reordered.reverse();
        reordered.push(("com.example.display".into(), "ignored".into()));
        assert_eq!(ownership.validate_observed(&reordered), Ok(()));

        let mut extra_owned = ownership.ordered().to_vec();
        extra_owned.push(("a2a.future".into(), "image-supplied".into()));
        assert_eq!(ownership.validate_observed(&extra_owned), Ok(()));

        let mut missing = ownership.ordered().to_vec();
        missing.retain(|(key, _)| key != "a2a.owner");
        assert_eq!(
            ownership.validate_observed(&missing),
            Err(ContainerOwnershipErrorV1::Mismatch)
        );

        let mut changed = ownership.ordered().to_vec();
        changed[1].1 = "ro".into();
        assert_eq!(
            ownership.validate_observed(&changed),
            Err(ContainerOwnershipErrorV1::Mismatch)
        );

        let mut duplicate = ownership.ordered().to_vec();
        duplicate.push(("a2a.owner".into(), "abc".into()));
        assert_eq!(
            ownership.validate_observed(&duplicate),
            Err(ContainerOwnershipErrorV1::Duplicate)
        );
    }

    struct FakeProbe(std::collections::HashMap<String, Option<bool>>);
    impl crate::liveness::LeaseProbe for FakeProbe {
        fn try_state(&self, p: &str) -> Option<bool> {
            self.0.get(p).copied().flatten()
        }
    }
    fn labels_for(host: &str, lease: &str) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::from([
            ("a2a.host".into(), host.into()),
            ("a2a.lease".into(), lease.into()),
        ])
    }
    fn probe_with(state: Option<bool>) -> FakeProbe {
        FakeProbe(std::collections::HashMap::from([("/l".to_string(), state)]))
    }

    #[test]
    fn classify_covers_all_verdicts() {
        use Verdict::*;
        let me = "h1";
        // other host → Unknown (even if lease would look free)
        assert_eq!(
            classify(&labels_for("h2", "/l"), me, &probe_with(Some(true))),
            Unknown
        );
        // same host, lease free → Dead
        assert_eq!(
            classify(&labels_for("h1", "/l"), me, &probe_with(Some(true))),
            Dead
        );
        // same host, lease held → Alive
        assert_eq!(
            classify(&labels_for("h1", "/l"), me, &probe_with(Some(false))),
            Alive
        );
        // same host, lease absent/unreadable → Unknown
        assert_eq!(
            classify(&labels_for("h1", "/l"), me, &probe_with(None)),
            Unknown
        );
        // missing host label → Unknown
        let no_host =
            std::collections::HashMap::from([("a2a.lease".to_string(), "/l".to_string())]);
        assert_eq!(classify(&no_host, me, &probe_with(Some(true))), Unknown);
    }

    #[test]
    fn run_handle_builds_label_with_instance_id_as_run() {
        let h = RunHandle {
            instance_id: "r1".into(),
            host: "h1".into(),
            lease: "/l/r1.lock".into(),
            start: "1700".into(),
        };
        let l = h.labels("rw", "warm", "impl", "owner9", Some("/repo"), Some("/cwd"));
        assert_eq!(l.run_id, "r1"); // instance_id flows into the a2a.run label
        assert_eq!(l.owner, "owner9");
        assert_eq!(l.role, "rw");
        assert_eq!(l.kind, "warm");
        assert_eq!(l.host, "h1");
    }
}
