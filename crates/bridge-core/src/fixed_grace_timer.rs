use crate::execution_policy::{
    ControlEventIdV1, FanOutPolicyNameV1, PolicyNodeRefV1, PolicyTriggerV1,
    EXECUTION_POLICY_SCHEMA_V1,
};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum FixedGraceTimerStateV1 {
    Unarmed {},
    Armed {
        schema_version: u16,
        trigger_id: ControlEventIdV1,
        node: PolicyNodeRefV1,
        grace_ms: u64,
        armed_at_elapsed_ms: u64,
        recorded_node_deadline_elapsed_ms: u64,
    },
    Fired {
        schema_version: u16,
        trigger: PolicyTriggerV1,
        armed_at_elapsed_ms: u64,
        fired_at_elapsed_ms: u64,
        recorded_node_deadline_elapsed_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FixedGraceTimerV1(FixedGraceTimerStateV1);

impl<'de> Deserialize<'de> for FixedGraceTimerV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let state = FixedGraceTimerStateV1::deserialize(deserializer)?;
        let valid = match &state {
            FixedGraceTimerStateV1::Unarmed {} => true,
            FixedGraceTimerStateV1::Armed {
                schema_version,
                grace_ms,
                armed_at_elapsed_ms,
                ..
            } => {
                *schema_version == EXECUTION_POLICY_SCHEMA_V1
                    && *grace_ms > 0
                    && armed_at_elapsed_ms.checked_add(*grace_ms).is_some()
            }
            FixedGraceTimerStateV1::Fired {
                schema_version,
                trigger,
                armed_at_elapsed_ms,
                fired_at_elapsed_ms,
                ..
            } => {
                *schema_version == EXECUTION_POLICY_SCHEMA_V1
                    && trigger.encode_canonical().is_ok()
                    && trigger.grace_ms.is_some_and(|grace_ms| {
                        *fired_at_elapsed_ms >= *armed_at_elapsed_ms
                            && *fired_at_elapsed_ms - *armed_at_elapsed_ms >= grace_ms
                    })
            }
        };
        valid
            .then_some(Self(state))
            .ok_or_else(|| serde::de::Error::custom("invalid fixed-grace timer state"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedGraceTimerErrorV1 {
    AlreadyArmedOrFired,
    NotArmed,
    AlreadyFired,
    InvalidGrace,
    ArithmeticOverflow,
    ElapsedBeforeArm,
}

use FixedGraceTimerErrorV1 as TimerError;

impl Default for FixedGraceTimerV1 {
    fn default() -> Self {
        Self::new()
    }
}

impl FixedGraceTimerV1 {
    #[must_use]
    pub const fn new() -> Self {
        Self(FixedGraceTimerStateV1::Unarmed {})
    }

    pub fn arm(
        &mut self,
        node: PolicyNodeRefV1,
        trigger_id: ControlEventIdV1,
        grace_ms: u64,
        armed_at_elapsed_ms: u64,
        recorded_node_deadline_elapsed_ms: u64,
    ) -> Result<(), TimerError> {
        if !matches!(&self.0, FixedGraceTimerStateV1::Unarmed {}) {
            return Err(TimerError::AlreadyArmedOrFired);
        }
        if grace_ms == 0 {
            return Err(TimerError::InvalidGrace);
        }
        armed_at_elapsed_ms
            .checked_add(grace_ms)
            .ok_or(TimerError::ArithmeticOverflow)?;
        self.0 = FixedGraceTimerStateV1::Armed {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            trigger_id,
            node,
            grace_ms,
            armed_at_elapsed_ms,
            recorded_node_deadline_elapsed_ms,
        };
        Ok(())
    }

    pub fn observe_elapsed(
        &mut self,
        elapsed_ms: u64,
    ) -> Result<Option<PolicyTriggerV1>, TimerError> {
        let (trigger_id, node, grace_ms, armed_at_elapsed_ms, recorded_node_deadline_elapsed_ms) =
            match &self.0 {
                FixedGraceTimerStateV1::Unarmed {} => return Err(TimerError::NotArmed),
                FixedGraceTimerStateV1::Fired { .. } => return Err(TimerError::AlreadyFired),
                FixedGraceTimerStateV1::Armed {
                    trigger_id,
                    node,
                    grace_ms,
                    armed_at_elapsed_ms,
                    recorded_node_deadline_elapsed_ms,
                    ..
                } => {
                    if elapsed_ms < *armed_at_elapsed_ms {
                        return Err(TimerError::ElapsedBeforeArm);
                    }
                    if elapsed_ms - *armed_at_elapsed_ms < *grace_ms {
                        return Ok(None);
                    }
                    (
                        trigger_id.clone(),
                        node.clone(),
                        *grace_ms,
                        *armed_at_elapsed_ms,
                        *recorded_node_deadline_elapsed_ms,
                    )
                }
            };
        let trigger = PolicyTriggerV1 {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            id: trigger_id,
            node,
            policy: FanOutPolicyNameV1::FixedGrace,
            grace_ms: Some(grace_ms),
        };
        self.0 = FixedGraceTimerStateV1::Fired {
            schema_version: EXECUTION_POLICY_SCHEMA_V1,
            trigger: trigger.clone(),
            armed_at_elapsed_ms,
            fired_at_elapsed_ms: elapsed_ms,
            recorded_node_deadline_elapsed_ms,
        };
        Ok(Some(trigger))
    }

    #[must_use]
    pub const fn recorded_node_deadline_elapsed_ms(&self) -> Option<u64> {
        match &self.0 {
            FixedGraceTimerStateV1::Unarmed {} => None,
            FixedGraceTimerStateV1::Armed {
                recorded_node_deadline_elapsed_ms,
                ..
            }
            | FixedGraceTimerStateV1::Fired {
                recorded_node_deadline_elapsed_ms,
                ..
            } => Some(*recorded_node_deadline_elapsed_ms),
        }
    }
}
