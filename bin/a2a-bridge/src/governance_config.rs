//! Pure route admission; caps are active for local workflows and served direct turns.
use crate::config::{ConfigError, RegistryConfig};
use bridge_core::{governance::*, ids::AgentId};

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernanceConfigV1 {
    pub rules: Vec<BudgetRuleV1>,
    #[serde(default)]
    pub allowances: Vec<TurnAllowanceV1>,
}
pub fn validate(cfg: &RegistryConfig) -> Result<(), ConfigError> {
    let Some(caps) = &cfg.governance else {
        return Ok(());
    };
    let invalid = |reason: &str| ConfigError::Registry(reason.to_owned());
    let policy =
        BudgetPolicyV1::new(caps.rules.clone(), caps.allowances.clone()).map_err(invalid)?;
    let known = |id: &AgentId| cfg.agents.iter().any(|a| a.id == id.as_str());
    if policy
        .rules()
        .iter()
        .any(|r| matches!(&r.selector, BudgetSelectorV1::UtcDayAgent(a) if !known(a)))
        || policy.allowances().iter().any(|a| !known(&a.target.agent))
    {
        return Err(invalid("governance references an unknown agent"));
    }
    Ok(())
}
/// All general snapshot entrances fail before resolution or storage.
/// Activated commands use a dedicated validated entrance.
pub fn local_workflow_snapshot(
    cfg: RegistryConfig,
) -> Result<bridge_core::domain::RegistrySnapshot, ConfigError> {
    validate(&cfg)?;
    cfg.into_validation_snapshot()
}
/// Only serve and MCP direct construction may use this entrance.
pub fn served_direct_snapshot(
    cfg: &RegistryConfig,
) -> Result<bridge_core::domain::RegistrySnapshot, ConfigError> {
    cfg.validation_snapshot()
}

pub fn policy(cfg: &RegistryConfig) -> Result<Option<std::sync::Arc<BudgetPolicyV1>>, ConfigError> {
    validate(cfg)?;
    cfg.governance
        .as_ref()
        .map(|caps| {
            BudgetPolicyV1::new(caps.rules.clone(), caps.allowances.clone())
                .map(std::sync::Arc::new)
                .map_err(|e| ConfigError::Registry(e.into()))
        })
        .transpose()
}

/// Process-lifetime freeze, including an initially absent policy. Runs before
/// emitting a snapshot, hence before recovery or registry/billing application.
pub fn admit_reload(cfg: &RegistryConfig, frozen: Option<[u8; 32]>) -> Result<(), ConfigError> {
    if policy(cfg)?.as_ref().map(|p| p.digest()) != frozen {
        return Err(ConfigError::Registry(
            "governance reload refused; retaining last-good configuration; changing caps requires restart".into(),
        ));
    }
    Ok(())
}

pub fn refuse_staged_execution(cfg: &RegistryConfig) -> Result<(), ConfigError> {
    validate(cfg)?;
    if cfg.governance.is_some() {
        return Err(ConfigError::Registry(
            "budget execution is unavailable during governance staging for this route; only local run-workflow, serve and mcp direct support caps".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: &str = "default='a'\n[[agents]]\nid='a'\ncmd='never-spawn'\n[server]\n";
    const RULE: &str =
        "\n[[governance.rules]]\nid='request'\nselector='each_request'\nlimits={turns=0}\n";
    fn parse(extra: &str) -> Result<RegistryConfig, ConfigError> {
        RegistryConfig::parse(&format!("{BASE}{extra}"))
    }
    #[test]
    fn startup_single_parse_and_policy_reload_controls() {
        let raw = format!("{BASE}{RULE}");
        let cfg = RegistryConfig::parse(&raw).unwrap();
        let frozen = policy(&cfg).unwrap().unwrap().digest();
        let snapshot = served_direct_snapshot(&cfg).unwrap();
        assert_eq!(snapshot.default.as_str(), "a");
        assert!(cfg.into_snapshot().is_err()); // generic consumers stay staged
        for changed in [BASE.to_owned(), raw.replace("turns=0", "turns=1")] {
            assert!(admit_reload(&RegistryConfig::parse(&changed).unwrap(), Some(frozen)).is_err());
        }
        assert!(admit_reload(&RegistryConfig::parse(&raw).unwrap(), None).is_err());
        assert!(admit_reload(&RegistryConfig::parse(BASE).unwrap(), None).is_ok());
        let second = RULE
            .replace("id='request'", "id='daily'")
            .replace("selector='each_request'", "selector={utc_day_agent='a'}");
        let p = RegistryConfig::parse(&format!("{BASE}{RULE}{second}")).unwrap();
        let reordered = RegistryConfig::parse(&format!("{BASE}{second}{RULE}")).unwrap();
        assert!(admit_reload(&reordered, Some(policy(&p).unwrap().unwrap().digest())).is_ok());
    }
    #[tokio::test]
    async fn disabled_and_staged_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let args = vec!["--config".into(), path.to_string_lossy().into_owned()];
        let raw = format!("{BASE}{RULE}").replace(
            "cmd='never-spawn'",
            &format!(
                "cmd='sh'\nargs=['-c', 'touch {}/provider-called']",
                dir.path().display()
            ),
        );
        let raw = format!("{raw}\n[store]\npath='{}/usage.db'\n", dir.path().display());
        std::fs::write(&path, &raw).unwrap();
        crate::validate_cmd(&args).unwrap();
        assert_eq!(
            parse(RULE).unwrap().governance.unwrap().rules[0]
                .limits
                .turns,
            Some(0)
        );
        assert!(crate::models_cmd(&args)
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable during governance staging"));
        assert!(crate::validate_registry_config_contents(&raw).is_err()); // smoke entrance
        let direct: RegistryConfig = toml::from_str(&raw).unwrap();
        assert!(direct.into_snapshot().is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1); // no store or provider output
        std::fs::write(&path, BASE).unwrap();
        crate::validate_cmd(&args).unwrap();
        assert!(parse("").unwrap().into_snapshot().is_ok());
        std::fs::write(&path, raw.replace("turns=0", "turns=-1")).unwrap();
        assert!(crate::validate_cmd(&args).is_err());
    }
    #[test]
    fn malformed_caps_refuse() {
        for extra in [
            "[governance]\nrules=[]".to_owned(),
            RULE.replace("turns=0", "turns=-1"),
            RULE.replace("turns=0", "turns=18446744073709551616"),
            RULE.replace("turns=0", "typo=1"),
            RULE.replace("turns=0", ""),
            RULE.replace("id='request'", "id=''"),
            format!("{RULE}{RULE}"),
            format!("{RULE}{}", RULE.replace("id='request'", "id='other'")),
            RULE.replace(
                "selector='each_request'",
                "selector={utc_day_agent='missing'}",
            ),
        ] {
            assert!(parse(&extra).is_err(), "{extra}");
        }
    }
    #[test]
    fn token_caps_validate_without_default_or_dispatched_allowances() {
        let token = RULE.replace("turns=0", "terminal_tokens=0");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let args = vec!["--config".into(), path.to_string_lossy().into_owned()];
        for base in [
            BASE.to_owned(),
            BASE.replace("id='a'", "id='a'\nmodel='default'"),
        ] {
            let raw = format!("{base}{token}");
            let cfg = RegistryConfig::parse(&raw).unwrap();
            std::fs::write(&path, &raw).unwrap();
            crate::validate_cmd(&args).unwrap();
            let caps = cfg.governance.unwrap();
            let policy = BudgetPolicyV1::new(caps.rules, caps.allowances).unwrap();
            let dispatched = BudgetCandidateV1 {
                agent: AgentId::parse("a").unwrap(),
                model: Some("dispatched".into()),
            };
            assert_eq!(
                policy.allowance_for(&dispatched, true).unwrap_err().reason,
                BudgetRefusalReasonV1::MissingAllowance
            );
        }
    }
    #[test]
    fn declared_allowances_require_valid_unique_exact_targets() {
        let token = RULE.replace("turns=0", "terminal_tokens=0");
        let allowance = "\n[[governance.allowances]]\ntarget={agent='a'}\nterminal_tokens=100\n";
        assert!(parse(&format!("{token}{allowance}")).is_ok());
        let explicit = allowance.replace("agent='a'", "agent='a',model='m'");
        assert!(parse(&format!("{token}{explicit}")).is_ok());
        assert!(parse(&format!("{token}{allowance}{explicit}")).is_ok());
        for bad in [
            allowance.replace("100", "0"),
            allowance.replace("100", "-1"),
            allowance.replace("agent='a'", "agent='missing'"),
            allowance.replace("agent='a'", "agent='a',model=''"),
            format!("{allowance}{allowance}"),
            format!("{explicit}{explicit}"),
        ] {
            assert!(parse(&format!("{token}{bad}")).is_err());
        }
    }
}
