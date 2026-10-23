//! Pure configuration admission; no provider/catalog/store access in this staging slice.
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
    for agent in &cfg.agents {
        let candidate = BudgetCandidateV1 {
            agent: AgentId::parse(agent.id.clone())
                .map_err(|_| invalid("invalid governance agent"))?,
            model: agent.model.clone(),
        };
        policy
            .allowance_for(&candidate, true)
            .map_err(|_| invalid("governance token cap requires an exact turn allowance"))?;
    }
    Ok(())
}
/// All snapshot entrances, including direct deserialization, fail before resolution or storage.
pub fn refuse_staged_execution(cfg: &RegistryConfig) -> Result<(), ConfigError> {
    validate(cfg)?;
    if cfg.governance.is_some() {
        return Err(ConfigError::Registry(
            "budget execution is unavailable during governance staging".into(),
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
    fn disabled_and_staged_zero() {
        assert!(parse("").unwrap().into_snapshot().is_ok());
        let cfg = parse(RULE).unwrap();
        assert_eq!(
            cfg.governance.as_ref().unwrap().rules[0].limits.turns,
            Some(0)
        );
        assert!(cfg
            .into_snapshot()
            .unwrap_err()
            .to_string()
            .contains("unavailable during governance staging"));
        let direct: RegistryConfig = toml::from_str(&format!("{BASE}{RULE}")).unwrap();
        assert!(direct.into_snapshot().is_err());
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
    fn token_caps_require_exact_allowances() {
        let token = RULE.replace("turns=0", "terminal_tokens=0");
        let allowance = "\n[[governance.allowances]]\ntarget={agent='a'}\nterminal_tokens=100\n";
        assert!(parse(&token).is_err());
        assert!(parse(&format!("{token}{allowance}")).is_ok());
        for bad in [
            allowance.replace("100", "0"),
            allowance.replace("agent='a'", "agent='missing'"),
            allowance.replace("agent='a'", "agent='a',model='m'"),
            format!("{allowance}{allowance}"),
        ] {
            assert!(parse(&format!("{token}{bad}")).is_err());
        }
    }
}
