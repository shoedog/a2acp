//! Pure capability-driven resolution of model and effort against advertised config options.

use agent_client_protocol::schema::v1::{
    SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory as Cat,
    SessionConfigSelectOptions,
};
use bridge_core::catalog::{is_blocked_model_id, AgentCaps};
use bridge_core::domain::Effort;

/// Static shorthand to advertised-id map, applied before validation.
pub const MODEL_ALIASES: &[(&str, &str)] = &[("opus", "default"), ("gpt-5-6-sol", "gpt-5.6-sol")];

pub fn apply_alias(want: &str) -> &str {
    MODEL_ALIASES
        .iter()
        .find(|(alias, _)| *alias == want)
        .map(|(_, value)| *value)
        .unwrap_or(want)
}

pub fn is_blocked_model(model: &str) -> bool {
    let mapped = apply_alias(model);
    is_blocked_model_id(model) || is_blocked_model_id(mapped)
}

fn allowed_model_values(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !is_blocked_model(value))
        .cloned()
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModelDecision {
    Default,
    Apply(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ModelNotAdvertised {
    pub want: String,
    pub valid: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModelResolutionError {
    Blocked { want: String, valid: Vec<String> },
    NotAdvertised(ModelNotAdvertised),
}

/// `want == None` leaves the agent default. Otherwise: prefer the RAW id when the agent
/// advertises it directly, then fall back to the alias map. The raw-first order matters
/// because the advertised id is adapter-version-dependent for non-blocked models. Blocked
/// models fail before validation even if an agent advertises them.
pub fn resolve_model(
    want: Option<&str>,
    values: &[String],
) -> Result<ModelDecision, ModelResolutionError> {
    let Some(raw) = want else {
        return Ok(ModelDecision::Default);
    };
    let valid = allowed_model_values(values);
    if is_blocked_model(raw) {
        return Err(ModelResolutionError::Blocked {
            want: raw.to_string(),
            valid,
        });
    }
    if valid.iter().any(|value| value == raw) {
        return Ok(ModelDecision::Apply(raw.to_string()));
    }
    let mapped = apply_alias(raw);
    if valid.iter().any(|value| value == mapped) {
        Ok(ModelDecision::Apply(mapped.to_string()))
    } else {
        Err(ModelResolutionError::NotAdvertised(ModelNotAdvertised {
            want: raw.to_string(),
            valid,
        }))
    }
}

fn select_values(sel_options: &SessionConfigSelectOptions) -> Vec<String> {
    match sel_options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|option| option.value.0.to_string())
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| {
                group
                    .options
                    .iter()
                    .map(|option| option.value.0.to_string())
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn matches_category_or_id(opt: &SessionConfigOption, cat: Cat, ids: &[&str]) -> bool {
    match &opt.category {
        Some(category) if category == &cat => true,
        None | Some(Cat::Other(_)) => ids.iter().any(|id| *id == &*opt.id.0),
        _ => false,
    }
}

/// Returns `(config_id, current_value, values)` for the first matching Select option.
fn find_select(
    opts: &[SessionConfigOption],
    cat: Cat,
    ids: &[&str],
) -> Option<(String, String, Vec<String>)> {
    opts.iter().find_map(|opt| {
        if !matches_category_or_id(opt, cat.clone(), ids) {
            return None;
        }
        match &opt.kind {
            SessionConfigKind::Select(select) => Some((
                opt.id.0.to_string(),
                select.current_value.0.to_string(),
                select_values(&select.options),
            )),
            _ => None,
        }
    })
}

pub fn model_values(opts: &[SessionConfigOption]) -> Option<(String, String, Vec<String>)> {
    find_select(opts, Cat::Model, &["model"]).map(|(id, current, values)| {
        let values = allowed_model_values(&values);
        (id, current, values)
    })
}

/// Returns `(config_id, current_value, values)` for the advertised mode select, if any.
/// Mirrors `model_values` against the `Mode` category (id `"mode"`).
pub fn mode_values(opts: &[SessionConfigOption]) -> Option<(String, String, Vec<String>)> {
    find_select(opts, Cat::Mode, &["mode"])
}

#[derive(Debug, PartialEq, Eq)]
pub struct AdvertisedEffort {
    pub config_id: String,
    pub levels: Vec<String>,
}

pub fn effort_opt(opts: &[SessionConfigOption]) -> Option<AdvertisedEffort> {
    find_select(opts, Cat::ThoughtLevel, &["effort", "reasoning_effort"]).map(
        |(config_id, _current, values)| AdvertisedEffort {
            config_id,
            levels: values
                .into_iter()
                .filter(|value| value != "default")
                .collect(),
        },
    )
}

/// Map advertised ACP `configOptions` (claude/codex) -> AgentCaps. effort_opt already
/// filters out the "default" pseudo-level (see effort_opt at model_effort.rs:128).
pub fn caps_from_config_options(opts: &[SessionConfigOption]) -> AgentCaps {
    let (current_model, models, model_configurable) = match model_values(opts) {
        Some((_, current, values)) if !values.is_empty() => {
            let current = (!is_blocked_model(&current)).then_some(current);
            (current, values, true)
        }
        None => (None, Vec::new(), false),
        Some(_) => (None, Vec::new(), false),
    };
    let effort_levels = effort_opt(opts).map(|e| e.levels).unwrap_or_default();
    let (current_mode, modes) = match mode_values(opts) {
        Some((_, current, values)) => (Some(current), values),
        None => (None, Vec::new()),
    };
    AgentCaps {
        current_model,
        models,
        model_configurable,
        effort_levels,
        modes,
        current_mode,
    }
}

pub const EFFORT_ORDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];

fn rank(level: &str) -> Option<usize> {
    EFFORT_ORDER
        .iter()
        .position(|candidate| *candidate == level)
}

pub fn effort_level_name(effort: Effort) -> &'static str {
    match effort {
        Effort::Minimal | Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum EffortDecision {
    Skip,
    Apply {
        config_id: String,
        level: String,
    },
    FellBack {
        config_id: String,
        from: String,
        to: String,
    },
    Unsupported {
        from: String,
    },
}

pub fn resolve_effort(want: Option<Effort>, adv: &AdvertisedEffort) -> EffortDecision {
    let Some(want) = want.map(effort_level_name) else {
        return EffortDecision::Skip;
    };
    if adv.levels.iter().any(|level| level == want) {
        return EffortDecision::Apply {
            config_id: adv.config_id.clone(),
            level: want.to_string(),
        };
    }

    let Some(want_rank) = rank(want) else {
        return EffortDecision::Unsupported {
            from: want.to_string(),
        };
    };
    let best = adv
        .levels
        .iter()
        .filter_map(|level| rank(level).map(|level_rank| (level_rank, level)))
        .filter(|(level_rank, _)| *level_rank <= want_rank)
        .max_by_key(|(level_rank, _)| *level_rank);

    match best {
        Some((_, to)) => EffortDecision::FellBack {
            config_id: adv.config_id.clone(),
            from: want.to_string(),
            to: to.clone(),
        },
        None => EffortDecision::Unsupported {
            from: want.to_string(),
        },
    }
}

/// Walk-down predicate: internal error plus an invalid/unsupported-value marker.
pub fn is_unsupported_effort_error(
    code: i64,
    message: &str,
    data: Option<&serde_json::Value>,
) -> bool {
    if code != -32603 {
        return false;
    }
    let mut text = message.to_ascii_lowercase();
    if let Some(data) = data {
        text.push(' ');
        text.push_str(&data.to_string().to_ascii_lowercase());
    }
    text.contains("invalid value")
        || text.contains("unsupported")
        || text.contains("unsupported-value")
        || text.contains("not support")
        || text.contains("model_not_found")
}

pub fn resolved_log_line(
    agent: &str,
    model_current: &str,
    effort_outcome: &EffortDecision,
) -> String {
    let effort = match effort_outcome {
        EffortDecision::Skip => "skipped".to_string(),
        EffortDecision::Apply { level, .. } => level.clone(),
        EffortDecision::FellBack { from, to, .. } => format!("{to} (fell back from {from})"),
        EffortDecision::Unsupported { from } => format!("unsupported ({from})"),
    };
    format!("model_effort_resolved agent={agent} model={model_current} effort={effort}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{SessionConfigSelectGroup, SessionConfigSelectOption};
    use serde_json::json;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn claude_values() -> Vec<String> {
        strings(&[
            "default",
            "claude-fable-5[1m]",
            "sonnet",
            "sonnet[1m]",
            "haiku",
        ])
    }

    fn options(values: &[&str]) -> Vec<SessionConfigSelectOption> {
        values
            .iter()
            .map(|value| SessionConfigSelectOption::new((*value).to_string(), (*value).to_string()))
            .collect()
    }

    fn select_opt(
        id: &str,
        category: Option<Cat>,
        current: &str,
        values: &[&str],
    ) -> SessionConfigOption {
        let opt = SessionConfigOption::select(
            id.to_string(),
            id.to_string(),
            current.to_string(),
            options(values),
        );
        match category {
            Some(category) => opt.category(category),
            None => opt,
        }
    }

    #[test]
    fn none_model_uses_default() {
        assert_eq!(
            resolve_model(None, &claude_values()).unwrap(),
            ModelDecision::Default
        );
    }

    #[test]
    fn advertised_model_is_applied() {
        assert_eq!(
            resolve_model(Some("haiku"), &claude_values()).unwrap(),
            ModelDecision::Apply("haiku".into())
        );
    }

    #[test]
    fn raw_advertised_fable_id_is_blocked() {
        let values = strings(&["default", "fable", "sonnet", "sonnet[1m]", "haiku"]);
        let err = resolve_model(Some("fable"), &values).unwrap_err();
        assert_eq!(
            err,
            ModelResolutionError::Blocked {
                want: "fable".into(),
                valid: strings(&["default", "sonnet", "sonnet[1m]", "haiku"])
            }
        );
    }

    #[test]
    fn fable_alias_is_blocked() {
        let err = resolve_model(Some("fable"), &claude_values()).unwrap_err();
        assert_eq!(
            err,
            ModelResolutionError::Blocked {
                want: "fable".into(),
                valid: strings(&["default", "sonnet", "sonnet[1m]", "haiku"])
            }
        );
    }

    #[test]
    fn concrete_fable_model_is_blocked() {
        let err = resolve_model(Some("claude-fable-5[1m]"), &claude_values()).unwrap_err();
        assert_eq!(
            err,
            ModelResolutionError::Blocked {
                want: "claude-fable-5[1m]".into(),
                valid: strings(&["default", "sonnet", "sonnet[1m]", "haiku"])
            }
        );
    }

    #[test]
    fn future_fable_family_model_is_blocked() {
        let values = strings(&["default", "claude-fable-5.1[1m]", "sonnet"]);
        let err = resolve_model(Some("claude-fable-5.1[1m]"), &values).unwrap_err();
        assert_eq!(
            err,
            ModelResolutionError::Blocked {
                want: "claude-fable-5.1[1m]".into(),
                valid: strings(&["default", "sonnet"])
            }
        );
    }

    #[test]
    fn raw_advertised_opus_id_preferred_over_alias() {
        let values = strings(&["default", "opus", "sonnet"]);
        assert_eq!(
            resolve_model(Some("opus"), &values).unwrap(),
            ModelDecision::Apply("opus".into())
        );
    }

    #[test]
    fn opus_alias_falls_back_to_default() {
        assert_eq!(
            resolve_model(Some("opus"), &claude_values()).unwrap(),
            ModelDecision::Apply("default".into())
        );
    }

    #[test]
    fn typo_errs_with_valid_list() {
        let err = resolve_model(Some("bogus"), &claude_values()).unwrap_err();
        assert_eq!(
            err,
            ModelResolutionError::NotAdvertised(ModelNotAdvertised {
                want: "bogus".into(),
                valid: strings(&["default", "sonnet", "sonnet[1m]", "haiku"])
            })
        );
    }

    #[test]
    fn fable_blocks_even_when_target_not_advertised() {
        assert_eq!(
            resolve_model(Some("fable"), &["sonnet".to_string()]).unwrap_err(),
            ModelResolutionError::Blocked {
                want: "fable".into(),
                valid: strings(&["sonnet"])
            }
        );
    }

    #[test]
    fn codex_base_id_is_valid() {
        assert_eq!(
            resolve_model(Some("gpt-5.5"), &["gpt-5.5".to_string()]).unwrap(),
            ModelDecision::Apply("gpt-5.5".into())
        );
    }

    #[test]
    fn hyphenated_gpt_5_6_sol_alias_maps_to_canonical_id() {
        assert_eq!(
            resolve_model(Some("gpt-5-6-sol"), &["gpt-5.6-sol".to_string()]).unwrap(),
            ModelDecision::Apply("gpt-5.6-sol".into())
        );
    }

    #[test]
    fn raw_advertised_hyphenated_gpt_5_6_sol_wins_over_alias() {
        assert_eq!(
            resolve_model(
                Some("gpt-5-6-sol"),
                &["gpt-5-6-sol".to_string(), "gpt-5.6-sol".to_string()]
            )
            .unwrap(),
            ModelDecision::Apply("gpt-5-6-sol".into())
        );
    }

    #[test]
    fn model_values_reads_ungrouped_model_option() {
        let opts = [select_opt(
            "model",
            Some(Cat::Model),
            "default",
            &["default", "haiku"],
        )];
        assert_eq!(
            model_values(&opts),
            Some((
                "model".into(),
                "default".into(),
                strings(&["default", "haiku"])
            ))
        );
    }

    #[test]
    fn model_values_flattens_grouped_model_option() {
        let groups = vec![
            SessionConfigSelectGroup::new(
                "claude",
                "Claude",
                options(&["default", "claude-fable-5[1m]", "haiku"]),
            ),
            SessionConfigSelectGroup::new("codex", "Codex", options(&["gpt-5.5"])),
        ];
        let opt =
            SessionConfigOption::select("model", "model", "default", groups).category(Cat::Model);
        assert_eq!(
            model_values(&[opt]),
            Some((
                "model".into(),
                "default".into(),
                strings(&["default", "haiku", "gpt-5.5"])
            ))
        );
    }

    #[test]
    fn caps_from_config_options_filters_blocked_models() {
        let opts = vec![select_opt(
            "model",
            Some(Cat::Model),
            "sonnet",
            &[
                "default",
                "claude-fable-5[1m]",
                "claude-fable-5.1[1m]",
                "fable",
                "sonnet",
            ],
        )];
        let caps = caps_from_config_options(&opts);
        assert_eq!(caps.current_model.as_deref(), Some("sonnet"));
        assert_eq!(caps.models, vec!["default", "sonnet"]);
        assert!(caps.model_configurable);
    }

    #[test]
    fn model_values_falls_back_to_id_for_other_category() {
        let opts = [select_opt(
            "model",
            Some(Cat::Other("_vendor".into())),
            "haiku",
            &["haiku"],
        )];
        assert_eq!(
            model_values(&opts),
            Some(("model".into(), "haiku".into(), strings(&["haiku"])))
        );
    }

    #[test]
    fn model_values_falls_back_to_id_for_missing_category() {
        let opts = [select_opt("model", None, "haiku", &["haiku"])];
        assert_eq!(
            model_values(&opts),
            Some(("model".into(), "haiku".into(), strings(&["haiku"])))
        );
    }

    #[test]
    fn non_matching_option_is_not_model_values() {
        let opts = [select_opt(
            "mode",
            Some(Cat::Mode),
            "read-only",
            &["read-only"],
        )];
        assert_eq!(model_values(&opts), None);
    }

    #[test]
    fn mode_values_reads_mode_select() {
        let opts = [select_opt(
            "mode",
            Some(Cat::Mode),
            "default",
            &["default", "plan"],
        )];
        assert_eq!(
            mode_values(&opts),
            Some((
                "mode".into(),
                "default".into(),
                strings(&["default", "plan"])
            ))
        );
    }

    #[test]
    fn effort_opt_reads_thought_level_and_filters_default() {
        let opts = [select_opt(
            "reasoning_effort",
            Some(Cat::ThoughtLevel),
            "high",
            &["default", "low", "medium", "high", "xhigh"],
        )];
        assert_eq!(
            effort_opt(&opts),
            Some(AdvertisedEffort {
                config_id: "reasoning_effort".into(),
                levels: strings(&["low", "medium", "high", "xhigh"])
            })
        );
    }

    #[test]
    fn caps_from_config_options_maps_all_three() {
        let opts = vec![
            select_opt(
                "model",
                Some(Cat::Model),
                "sonnet",
                &["default", "sonnet", "haiku"],
            ),
            select_opt(
                "reasoning_effort",
                Some(Cat::ThoughtLevel),
                "high",
                &["low", "medium", "high"],
            ),
            select_opt("mode", Some(Cat::Mode), "default", &["default", "plan"]),
        ];
        let caps = caps_from_config_options(&opts);
        assert_eq!(caps.current_model.as_deref(), Some("sonnet"));
        assert_eq!(caps.models, vec!["default", "sonnet", "haiku"]);
        assert_eq!(caps.effort_levels, vec!["low", "medium", "high"]);
        assert_eq!(caps.modes, vec!["default", "plan"]);
        assert_eq!(caps.current_mode.as_deref(), Some("default"));
    }

    #[test]
    fn none_effort_skips() {
        let adv = AdvertisedEffort {
            config_id: "effort".into(),
            levels: strings(&["low", "medium", "high"]),
        };
        assert_eq!(resolve_effort(None, &adv), EffortDecision::Skip);
    }

    #[test]
    fn high_effort_applies_when_advertised() {
        let adv = AdvertisedEffort {
            config_id: "effort".into(),
            levels: strings(&["low", "medium", "high"]),
        };
        assert_eq!(
            resolve_effort(Some(Effort::High), &adv),
            EffortDecision::Apply {
                config_id: "effort".into(),
                level: "high".into()
            }
        );
    }

    #[test]
    fn xhigh_falls_back_to_high_when_high_is_best_available() {
        let adv = AdvertisedEffort {
            config_id: "effort".into(),
            levels: strings(&["low", "medium", "high"]),
        };
        assert_eq!(
            resolve_effort(Some(Effort::Xhigh), &adv),
            EffortDecision::FellBack {
                config_id: "effort".into(),
                from: "xhigh".into(),
                to: "high".into()
            }
        );
    }

    #[test]
    fn max_effort_applies_when_advertised() {
        let adv = AdvertisedEffort {
            config_id: "effort".into(),
            levels: strings(&["low", "medium", "high", "max"]),
        };
        assert_eq!(
            resolve_effort(Some(Effort::Max), &adv),
            EffortDecision::Apply {
                config_id: "effort".into(),
                level: "max".into()
            }
        );
    }

    #[test]
    fn max_effort_falls_back_to_xhigh_for_codex_levels() {
        let adv = AdvertisedEffort {
            config_id: "reasoning_effort".into(),
            levels: strings(&["low", "medium", "high", "xhigh"]),
        };
        assert_eq!(
            resolve_effort(Some(Effort::Max), &adv),
            EffortDecision::FellBack {
                config_id: "reasoning_effort".into(),
                from: "max".into(),
                to: "xhigh".into()
            }
        );
    }

    #[test]
    fn effort_level_name_maps_max_to_max() {
        assert_eq!(effort_level_name(Effort::Max), "max");
    }

    #[test]
    fn unsupported_effort_error_matches_message_or_data() {
        assert!(is_unsupported_effort_error(
            -32603,
            "Invalid value for effort",
            None
        ));
        assert!(is_unsupported_effort_error(
            -32603,
            "internal error",
            Some(&json!("unsupported value: xhigh"))
        ));
        assert!(!is_unsupported_effort_error(-32603, "usage_update", None));
        assert!(!is_unsupported_effort_error(-32000, "Invalid value", None));
    }

    #[test]
    fn resolved_log_line_uses_resolved_values() {
        let line = resolved_log_line(
            "claude",
            "sonnet",
            &EffortDecision::FellBack {
                config_id: "effort".into(),
                from: "xhigh".into(),
                to: "high".into(),
            },
        );
        assert_eq!(
            line,
            "model_effort_resolved agent=claude model=sonnet effort=high (fell back from xhigh)"
        );
    }
}
