//! Per-turn activity liveness: the silence cutoff and the turn work cutoff.
//!
//! Callers supply monotonic elapsed milliseconds since the turn started. This module reads no
//! clock, starts no timer and performs no wait: the ACP backend owns the timer and asks the
//! tracker which deadline is due. The legacy `[agents.watchdog]` is not modeled here; the backend
//! keeps its own bound and only reports whether it is due, so the equal-instant cause priority
//! (legacy watchdog, then work cutoff, then silence) is decided in one place.

use crate::domain::AgentLivenessConfig;
use crate::execution_policy::DEFAULT_WORK_CUTOFF_MS;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Default silence cutoff: 30 minutes without activity while no tool call or permission is open.
pub const DEFAULT_SILENCE_CUTOFF_MS: u64 = 1_800_000;

/// Longest liveness duration any source may set: 30 days, the legacy watchdog's cap.
pub const MAX_LIVENESS_SECS: u64 = 30 * 24 * 3600;

/// Simultaneously open tool calls tracked per turn. A turn with more open at once counts as
/// paused for the rest of the turn.
pub const MAX_OPEN_TOOL_CALLS: usize = 4096;

/// Bytes of the raw tool call id kept for display. The set is keyed by the id's digest.
pub const TOOL_ID_PREVIEW_MAX_BYTES: usize = 64;

/// Bytes of a tool call's title (and kind) kept for display.
pub const TOOL_TITLE_MAX_BYTES: usize = 128;

/// A configured liveness duration: explicitly off, or a bound in milliseconds. Carried as
/// `Option<_>` wherever absence means inherit, so `Off` never collapses into absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessOverrideV1 {
    Off,
    Ms(u64),
}

/// A configured duration that cannot be a liveness bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessDurationError {
    /// More than [`MAX_LIVENESS_SECS`].
    TooLong { secs: u64 },
}

impl fmt::Display for LivenessDurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { secs } => write!(
                f,
                "{secs}s exceeds the {MAX_LIVENESS_SECS}s (30 days) liveness maximum"
            ),
        }
    }
}

impl std::error::Error for LivenessDurationError {}

/// The one checked parser for a liveness duration from any source (agent config, request
/// metadata, workflow config, CLI): whole seconds, `0` = off, at most 30 days, converted to
/// milliseconds with checked arithmetic.
pub fn parse_liveness_secs_v1(secs: u64) -> Result<LivenessOverrideV1, LivenessDurationError> {
    if secs == 0 {
        return Ok(LivenessOverrideV1::Off);
    }
    if secs > MAX_LIVENESS_SECS {
        return Err(LivenessDurationError::TooLong { secs });
    }
    secs.checked_mul(1_000)
        .map(LivenessOverrideV1::Ms)
        .ok_or(LivenessDurationError::TooLong { secs })
}

/// One per-turn liveness value carried on a turn request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TurnLivenessValueV1 {
    /// Use the agent's `[agents.liveness]` value, else the built-in default.
    #[default]
    Inherit,
    Off,
    Ms(u64),
}

/// The per-turn liveness request staged on `TurnMeta`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnLivenessRequestV1 {
    pub silence: TurnLivenessValueV1,
    pub work_cutoff: TurnLivenessValueV1,
    /// A client is waiting on this turn and dies with it, so the work cutoff defaults to off.
    pub client_watching: bool,
    /// Heartbeat registration key. Carried for the client heartbeat; the engine ignores it.
    pub observer_key: Option<String>,
}

/// The bounds one turn runs under. `None` is unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTurnLivenessV1 {
    pub silence_ms: Option<u64>,
    pub work_cutoff_ms: Option<u64>,
}

/// Resolve a turn's bounds: request, then the agent's `[agents.liveness]`, then the default
/// (silence 30 min; work cutoff off for a watched turn, 2 h otherwise). A zero duration is off.
#[must_use]
pub fn resolve_turn_liveness_v1(
    request: &TurnLivenessRequestV1,
    agent: Option<&AgentLivenessConfig>,
) -> ResolvedTurnLivenessV1 {
    let silence_default = Some(DEFAULT_SILENCE_CUTOFF_MS);
    let work_default = (!request.client_watching).then_some(DEFAULT_WORK_CUTOFF_MS);
    ResolvedTurnLivenessV1 {
        silence_ms: resolve_value(
            request.silence,
            agent.and_then(|agent| agent.silence_cutoff),
            silence_default,
        ),
        work_cutoff_ms: resolve_value(
            request.work_cutoff,
            agent.and_then(|agent| agent.work_cutoff),
            work_default,
        ),
    }
}

fn resolve_value(
    request: TurnLivenessValueV1,
    agent: Option<LivenessOverrideV1>,
    default: Option<u64>,
) -> Option<u64> {
    let ms = match request {
        TurnLivenessValueV1::Off => None,
        TurnLivenessValueV1::Ms(ms) => Some(ms),
        TurnLivenessValueV1::Inherit => match agent {
            Some(LivenessOverrideV1::Off) => None,
            Some(LivenessOverrideV1::Ms(ms)) => Some(ms),
            None => default,
        },
    };
    ms.filter(|ms| *ms != 0)
}

/// Which bound ended a turn. Declaration order is the equal-instant priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnLivenessCauseV1 {
    LegacyWatchdog,
    WorkCutoff,
    Silence,
}

/// The totals of one ACP usage update. Two updates with the same key are the same usage: a
/// repeat is not activity. Cost is `(amount bits, currency)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageTotalsKeyV1 {
    used: u64,
    size: u64,
    cost: Option<(u64, String)>,
}

impl UsageTotalsKeyV1 {
    #[must_use]
    pub fn new(used: u64, size: u64, cost: Option<(f64, &str)>) -> Self {
        Self {
            used,
            size,
            cost: cost.map(|(amount, currency)| (amount.to_bits(), currency.to_owned())),
        }
    }
}

/// A tool call status as the tracker reads it. Any other status is `None` on the observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallPhaseV1 {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// One ACP tool call or tool call update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolObservationV1<'a> {
    /// The raw `toolCallId`, before any display cap.
    pub raw_id: &'a str,
    pub status: Option<ToolCallPhaseV1>,
    pub title: Option<&'a str>,
    pub kind: Option<&'a str>,
}

/// What one ACP `session/update` was, as far as liveness is concerned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TurnActivityV1<'a> {
    /// Any update other than usage or a tool call, including unmodeled variants.
    Update,
    Usage(&'a UsageTotalsKeyV1),
    Tool(ToolObservationV1<'a>),
}

/// One open tool call, bounded for display whatever the agent sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenToolV1 {
    pub id_preview: String,
    pub title: String,
    pub kind: String,
    pub opened_ms: u64,
}

/// What an observation changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnLivenessTransitionV1 {
    /// The observation was activity.
    pub counted: bool,
    /// The turn went from paused to unpaused; a timer must recompute the silence deadline.
    pub unpaused: bool,
    /// This observation set the sticky open-tool overflow.
    pub overflowed: bool,
}

/// The liveness state of one turn.
#[derive(Debug, Clone)]
pub struct TurnLivenessTracker {
    resolved: ResolvedTurnLivenessV1,
    last_activity_ms: u64,
    last_unpause_ms: u64,
    open: HashMap<[u8; 32], OpenToolV1>,
    open_capacity: usize,
    open_overflowed: bool,
    pending_permissions: u64,
    last_usage: Option<UsageTotalsKeyV1>,
}

impl TurnLivenessTracker {
    /// A tracker for a turn that starts at elapsed `0`.
    #[must_use]
    pub fn new(resolved: ResolvedTurnLivenessV1) -> Self {
        Self::with_open_tool_capacity(resolved, MAX_OPEN_TOOL_CALLS)
    }

    /// Like [`Self::new`] with a smaller open-tool capacity, so a test can reach the overflow
    /// without thousands of observations. Production uses [`MAX_OPEN_TOOL_CALLS`].
    #[doc(hidden)]
    #[must_use]
    pub fn with_open_tool_capacity(resolved: ResolvedTurnLivenessV1, capacity: usize) -> Self {
        Self {
            resolved,
            last_activity_ms: 0,
            last_unpause_ms: 0,
            open: HashMap::new(),
            open_capacity: capacity,
            open_overflowed: false,
            pending_permissions: 0,
            last_usage: None,
        }
    }

    #[must_use]
    pub fn resolved(&self) -> ResolvedTurnLivenessV1 {
        self.resolved
    }

    /// Observe one `session/update`.
    pub fn observe(
        &mut self,
        now_ms: u64,
        activity: TurnActivityV1<'_>,
    ) -> TurnLivenessTransitionV1 {
        let was_paused = self.paused();
        let mut overflowed = false;
        match activity {
            TurnActivityV1::Update => {}
            TurnActivityV1::Usage(key) => {
                // Compare with the previous update, not a high-water mark, so a drop after
                // compaction is a change.
                if self.last_usage.as_ref() == Some(key) {
                    return TurnLivenessTransitionV1::default();
                }
                self.last_usage = Some(key.clone());
            }
            TurnActivityV1::Tool(tool) => overflowed = self.observe_tool(now_ms, tool),
        }
        self.counted(now_ms, was_paused, overflowed)
    }

    /// A permission request arrived. Pauses the clock until it resolves.
    pub fn permission_arrived(&mut self, now_ms: u64) -> TurnLivenessTransitionV1 {
        let was_paused = self.paused();
        self.pending_permissions = self.pending_permissions.saturating_add(1);
        self.counted(now_ms, was_paused, false)
    }

    /// A permission request resolved, on any path.
    pub fn permission_resolved(&mut self, now_ms: u64) -> TurnLivenessTransitionV1 {
        let was_paused = self.paused();
        self.pending_permissions = self.pending_permissions.saturating_sub(1);
        self.counted(now_ms, was_paused, false)
    }

    fn observe_tool(&mut self, now_ms: u64, tool: ToolObservationV1<'_>) -> bool {
        let key: [u8; 32] = ring::digest::digest(&ring::digest::SHA256, tool.raw_id.as_bytes())
            .as_ref()
            .try_into()
            .expect("SHA-256 digest is 32 bytes");
        match tool.status {
            Some(ToolCallPhaseV1::Pending | ToolCallPhaseV1::InProgress) => {
                if let Some(open) = self.open.get_mut(&key) {
                    if let Some(title) = tool.title {
                        open.title = bounded(title, TOOL_TITLE_MAX_BYTES);
                    }
                    if let Some(kind) = tool.kind {
                        open.kind = bounded(kind, TOOL_TITLE_MAX_BYTES);
                    }
                    return false;
                }
                if self.open.len() >= self.open_capacity {
                    let first = !self.open_overflowed;
                    self.open_overflowed = true;
                    return first;
                }
                self.open.insert(
                    key,
                    OpenToolV1 {
                        id_preview: bounded(tool.raw_id, TOOL_ID_PREVIEW_MAX_BYTES),
                        title: bounded(tool.title.unwrap_or("?"), TOOL_TITLE_MAX_BYTES),
                        kind: bounded(tool.kind.unwrap_or("?"), TOOL_TITLE_MAX_BYTES),
                        opened_ms: now_ms,
                    },
                );
                false
            }
            Some(ToolCallPhaseV1::Completed | ToolCallPhaseV1::Failed) => {
                self.open.remove(&key);
                false
            }
            None => false,
        }
    }

    fn counted(
        &mut self,
        now_ms: u64,
        was_paused: bool,
        overflowed: bool,
    ) -> TurnLivenessTransitionV1 {
        self.last_activity_ms = self.last_activity_ms.max(now_ms);
        let unpaused = was_paused && !self.paused();
        if unpaused {
            self.last_unpause_ms = self.last_unpause_ms.max(now_ms);
        }
        TurnLivenessTransitionV1 {
            counted: true,
            unpaused,
            overflowed,
        }
    }

    /// The silence clock is paused while a tool call or a permission request is open.
    #[must_use]
    pub fn paused(&self) -> bool {
        !self.open.is_empty() || self.open_overflowed || self.pending_permissions > 0
    }

    /// When the silence cutoff is due, while the clock runs. `None` while paused or off.
    #[must_use]
    pub fn silence_due_ms(&self) -> Option<u64> {
        if self.paused() {
            return None;
        }
        let anchor = self.last_activity_ms.max(self.last_unpause_ms);
        self.resolved
            .silence_ms
            .and_then(|silence| anchor.checked_add(silence))
    }

    /// When the turn work cutoff is due, whatever is open. `None` when off.
    #[must_use]
    pub fn work_due_ms(&self) -> Option<u64> {
        self.resolved.work_cutoff_ms
    }

    /// The bound due at `now_ms`, by the fixed priority: legacy watchdog, work cutoff, silence.
    #[must_use]
    pub fn due_cause(&self, now_ms: u64, legacy_watchdog_due: bool) -> Option<TurnLivenessCauseV1> {
        if legacy_watchdog_due {
            Some(TurnLivenessCauseV1::LegacyWatchdog)
        } else if self.work_due_ms().is_some_and(|due| now_ms >= due) {
            Some(TurnLivenessCauseV1::WorkCutoff)
        } else if self.silence_due_ms().is_some_and(|due| now_ms >= due) {
            Some(TurnLivenessCauseV1::Silence)
        } else {
            None
        }
    }

    #[must_use]
    pub fn last_activity_ms(&self) -> u64 {
        self.last_activity_ms
    }

    #[must_use]
    pub fn pending_permissions(&self) -> u64 {
        self.pending_permissions
    }

    #[must_use]
    pub fn open_overflowed(&self) -> bool {
        self.open_overflowed
    }

    pub fn open_tools(&self) -> impl Iterator<Item = &OpenToolV1> {
        self.open.values()
    }
}

fn bounded(text: &str, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    fn default_tracker() -> TurnLivenessTracker {
        TurnLivenessTracker::new(resolve_turn_liveness_v1(
            &TurnLivenessRequestV1::default(),
            None,
        ))
    }

    fn tool(raw_id: &str, status: ToolCallPhaseV1) -> TurnActivityV1<'_> {
        TurnActivityV1::Tool(ToolObservationV1 {
            raw_id,
            status: Some(status),
            title: None,
            kind: None,
        })
    }

    fn due(tracker: &TurnLivenessTracker, now_ms: u64) -> Option<TurnLivenessCauseV1> {
        tracker.due_cause(now_ms, false)
    }

    #[test]
    fn default_request_resolves_to_thirty_minutes_silence_and_two_hours_work() {
        let resolved = resolve_turn_liveness_v1(&TurnLivenessRequestV1::default(), None);
        assert_eq!(resolved.silence_ms, Some(30 * MIN));
        assert_eq!(resolved.work_cutoff_ms, Some(120 * MIN));

        let watched = resolve_turn_liveness_v1(
            &TurnLivenessRequestV1 {
                client_watching: true,
                ..TurnLivenessRequestV1::default()
            },
            None,
        );
        assert_eq!(watched.silence_ms, Some(30 * MIN));
        assert_eq!(
            watched.work_cutoff_ms, None,
            "a watched turn has no work cutoff"
        );
    }

    #[test]
    fn request_beats_agent_config_beats_default() {
        let agent = AgentLivenessConfig {
            silence_cutoff: Some(LivenessOverrideV1::Off),
            work_cutoff: Some(LivenessOverrideV1::Ms(10 * MIN)),
        };
        let inherit = resolve_turn_liveness_v1(&TurnLivenessRequestV1::default(), Some(&agent));
        assert_eq!(inherit.silence_ms, None);
        assert_eq!(inherit.work_cutoff_ms, Some(10 * MIN));

        let watched = resolve_turn_liveness_v1(
            &TurnLivenessRequestV1 {
                client_watching: true,
                ..TurnLivenessRequestV1::default()
            },
            Some(&agent),
        );
        assert_eq!(
            watched.work_cutoff_ms,
            Some(10 * MIN),
            "an explicit agent value beats the watched default"
        );

        let request = resolve_turn_liveness_v1(
            &TurnLivenessRequestV1 {
                silence: TurnLivenessValueV1::Ms(5 * MIN),
                work_cutoff: TurnLivenessValueV1::Off,
                ..TurnLivenessRequestV1::default()
            },
            Some(&agent),
        );
        assert_eq!(request.silence_ms, Some(5 * MIN));
        assert_eq!(request.work_cutoff_ms, None);

        let zero = resolve_turn_liveness_v1(
            &TurnLivenessRequestV1 {
                silence: TurnLivenessValueV1::Ms(0),
                work_cutoff: TurnLivenessValueV1::Ms(0),
                ..TurnLivenessRequestV1::default()
            },
            None,
        );
        assert_eq!((zero.silence_ms, zero.work_cutoff_ms), (None, None));
    }

    /// I1 test 20: the one shared parser.
    #[test]
    fn shared_parser_accepts_thirty_days_and_refuses_more() {
        assert_eq!(
            parse_liveness_secs_v1(MAX_LIVENESS_SECS),
            Ok(LivenessOverrideV1::Ms(2_592_000_000))
        );
        assert_eq!(
            parse_liveness_secs_v1(MAX_LIVENESS_SECS + 1),
            Err(LivenessDurationError::TooLong {
                secs: MAX_LIVENESS_SECS + 1
            })
        );
        assert_eq!(
            parse_liveness_secs_v1(u64::MAX),
            Err(LivenessDurationError::TooLong { secs: u64::MAX })
        );
        assert_eq!(parse_liveness_secs_v1(0), Ok(LivenessOverrideV1::Off));
        assert_eq!(parse_liveness_secs_v1(1), Ok(LivenessOverrideV1::Ms(1_000)));
    }

    #[test]
    fn silence_is_due_from_the_last_activity() {
        let mut tracker = default_tracker();
        assert_eq!(tracker.silence_due_ms(), Some(30 * MIN));
        assert_eq!(due(&tracker, 30 * MIN - 1), None);
        assert_eq!(due(&tracker, 30 * MIN), Some(TurnLivenessCauseV1::Silence));

        tracker.observe(10 * MIN, TurnActivityV1::Update);
        assert_eq!(tracker.silence_due_ms(), Some(40 * MIN));
        assert_eq!(due(&tracker, 39 * MIN), None);
    }

    /// I1 test 17 (pure half): the usage key.
    #[test]
    fn usage_key_compares_with_the_previous_update_including_optional_cost() {
        let mut tracker = default_tracker();
        let first = UsageTotalsKeyV1::new(10, 100, None);
        assert!(tracker.observe(MIN, TurnActivityV1::Usage(&first)).counted);
        assert!(
            !tracker
                .observe(2 * MIN, TurnActivityV1::Usage(&first))
                .counted,
            "a repeated None cost is unchanged"
        );
        assert_eq!(tracker.last_activity_ms(), MIN);

        let priced = UsageTotalsKeyV1::new(10, 100, Some((0.5, "USD")));
        assert!(
            tracker
                .observe(3 * MIN, TurnActivityV1::Usage(&priced))
                .counted,
            "None -> Some cost is a change"
        );
        assert!(
            !tracker
                .observe(4 * MIN, TurnActivityV1::Usage(&priced))
                .counted
        );
        let euro = UsageTotalsKeyV1::new(10, 100, Some((0.5, "EUR")));
        assert!(
            tracker
                .observe(5 * MIN, TurnActivityV1::Usage(&euro))
                .counted,
            "a currency-only change is a change"
        );
        let compacted = UsageTotalsKeyV1::new(4, 100, Some((0.5, "EUR")));
        assert!(
            tracker
                .observe(6 * MIN, TurnActivityV1::Usage(&compacted))
                .counted,
            "a drop after compaction is a change"
        );
        assert_eq!(tracker.last_activity_ms(), 6 * MIN);
    }

    #[test]
    fn tool_status_opens_closes_or_changes_nothing() {
        let mut tracker = default_tracker();
        tracker.observe(MIN, tool("a", ToolCallPhaseV1::Pending));
        assert!(tracker.paused());
        assert_eq!(tracker.silence_due_ms(), None);
        assert_eq!(due(&tracker, 100 * MIN), None);

        tracker.observe(
            2 * MIN,
            TurnActivityV1::Tool(ToolObservationV1 {
                raw_id: "a",
                status: None,
                title: Some("renamed"),
                kind: None,
            }),
        );
        assert!(tracker.paused(), "no status changes nothing");

        let transition = tracker.observe(3 * MIN, tool("a", ToolCallPhaseV1::Failed));
        assert!(transition.unpaused);
        assert!(!tracker.paused());
        assert_eq!(tracker.silence_due_ms(), Some(33 * MIN));
    }

    /// I1 test 3 (pure half).
    #[test]
    fn unknown_tool_ids_open_on_progress_and_ignore_closes() {
        let mut tracker = default_tracker();
        let transition = tracker.observe(MIN, tool("never-opened", ToolCallPhaseV1::Completed));
        assert!(
            transition.counted,
            "a close for an unknown id is still activity"
        );
        assert!(!tracker.paused());

        tracker.observe(
            2 * MIN,
            TurnActivityV1::Tool(ToolObservationV1 {
                raw_id: "update-only",
                status: Some(ToolCallPhaseV1::InProgress),
                title: None,
                kind: None,
            }),
        );
        assert!(tracker.paused());
        let open: Vec<_> = tracker.open_tools().collect();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "?");
        assert_eq!(open[0].kind, "?");
        assert_eq!(open[0].opened_ms, 2 * MIN);
    }

    /// I1 test 8 (pure half), at the production capacity.
    #[test]
    fn open_tool_slots_free_on_close_and_overflow_only_past_capacity() {
        let mut tracker = default_tracker();
        for round in 0..3 {
            for index in 0..MAX_OPEN_TOOL_CALLS {
                let id = format!("{round}-{index}");
                assert!(
                    !tracker
                        .observe(MIN, tool(&id, ToolCallPhaseV1::InProgress))
                        .overflowed
                );
            }
            for index in 0..MAX_OPEN_TOOL_CALLS {
                let id = format!("{round}-{index}");
                tracker.observe(MIN, tool(&id, ToolCallPhaseV1::Completed));
            }
        }
        assert!(
            !tracker.open_overflowed(),
            "many tools opened and closed never overflow"
        );
        assert!(!tracker.paused());

        for index in 0..MAX_OPEN_TOOL_CALLS {
            tracker.observe(2 * MIN, tool(&index.to_string(), ToolCallPhaseV1::Pending));
        }
        assert!(!tracker.open_overflowed());
        let transition = tracker.observe(2 * MIN, tool("one-too-many", ToolCallPhaseV1::Pending));
        assert!(
            transition.overflowed,
            "the 4097th simultaneous open overflows"
        );
        assert!(
            !tracker
                .observe(2 * MIN, tool("again", ToolCallPhaseV1::Pending))
                .overflowed,
            "the overflow is reported once"
        );
        for index in 0..MAX_OPEN_TOOL_CALLS {
            tracker.observe(
                3 * MIN,
                tool(&index.to_string(), ToolCallPhaseV1::Completed),
            );
        }
        assert!(tracker.open_overflowed(), "the overflow is sticky");
        assert!(tracker.paused(), "an overflowed set fails open");
        assert_eq!(
            due(&tracker, 1_000 * MIN),
            Some(TurnLivenessCauseV1::WorkCutoff)
        );
    }

    #[test]
    fn small_capacity_seam_reaches_the_same_overflow() {
        let mut tracker = TurnLivenessTracker::with_open_tool_capacity(
            resolve_turn_liveness_v1(&TurnLivenessRequestV1::default(), None),
            2,
        );
        tracker.observe(MIN, tool("a", ToolCallPhaseV1::Pending));
        tracker.observe(MIN, tool("b", ToolCallPhaseV1::Pending));
        tracker.observe(MIN, tool("a", ToolCallPhaseV1::Completed));
        assert!(
            !tracker
                .observe(MIN, tool("c", ToolCallPhaseV1::Pending))
                .overflowed
        );
        assert!(
            tracker
                .observe(MIN, tool("d", ToolCallPhaseV1::Pending))
                .overflowed
        );
        assert!(tracker.open_overflowed());
    }

    /// I1 tests 14 and 21 (pure half): the set is keyed by the raw id's digest.
    #[test]
    fn raw_ids_sharing_a_display_preview_are_tracked_separately() {
        let shared = "x".repeat(TOOL_ID_PREVIEW_MAX_BYTES);
        let first = format!("{shared}{}", "a".repeat(4096));
        let second = format!("{shared}{}", "b".repeat(4096));
        let mut tracker = default_tracker();
        tracker.observe(MIN, tool(&first, ToolCallPhaseV1::InProgress));
        tracker.observe(MIN, tool(&second, ToolCallPhaseV1::InProgress));
        let previews: Vec<_> = tracker
            .open_tools()
            .map(|open| open.id_preview.clone())
            .collect();
        assert_eq!(previews, vec![shared.clone(), shared.clone()]);

        tracker.observe(2 * MIN, tool(&first, ToolCallPhaseV1::Completed));
        assert!(tracker.paused(), "closing one keeps the other open");
        tracker.observe(3 * MIN, tool(&second, ToolCallPhaseV1::Completed));
        assert!(!tracker.paused());
    }

    #[test]
    fn display_fields_truncate_on_utf8_boundaries() {
        let mut tracker = default_tracker();
        let id = "é".repeat(TOOL_ID_PREVIEW_MAX_BYTES);
        let title = "日".repeat(TOOL_TITLE_MAX_BYTES);
        tracker.observe(
            MIN,
            TurnActivityV1::Tool(ToolObservationV1 {
                raw_id: &id,
                status: Some(ToolCallPhaseV1::Pending),
                title: Some(&title),
                kind: Some("execute"),
            }),
        );
        let open = tracker.open_tools().next().unwrap();
        assert_eq!(open.id_preview, "é".repeat(TOOL_ID_PREVIEW_MAX_BYTES / 2));
        assert!(open.title.len() <= TOOL_TITLE_MAX_BYTES);
        assert_eq!(open.title, "日".repeat(TOOL_TITLE_MAX_BYTES / 3));
        assert_eq!(open.kind, "execute");
    }

    /// I1 test 15 (pure half).
    #[test]
    fn pending_permissions_are_counted() {
        let mut tracker = default_tracker();
        tracker.permission_arrived(MIN);
        tracker.permission_arrived(MIN);
        assert!(!tracker.permission_resolved(2 * MIN).unpaused);
        assert!(
            tracker.paused(),
            "one of two pending keeps the clock paused"
        );
        assert!(tracker.permission_resolved(3 * MIN).unpaused);
        assert_eq!(tracker.silence_due_ms(), Some(33 * MIN));
        tracker.permission_resolved(4 * MIN);
        assert_eq!(
            tracker.pending_permissions(),
            0,
            "an extra resolve saturates"
        );
    }

    /// I1 test 22 (pure half).
    #[test]
    fn equal_instant_deadlines_report_the_fixed_priority() {
        let tracker = TurnLivenessTracker::new(ResolvedTurnLivenessV1 {
            silence_ms: Some(MIN),
            work_cutoff_ms: Some(MIN),
        });
        assert_eq!(due(&tracker, MIN), Some(TurnLivenessCauseV1::WorkCutoff));
        assert_eq!(
            tracker.due_cause(MIN, true),
            Some(TurnLivenessCauseV1::LegacyWatchdog)
        );
        assert_eq!(
            tracker.due_cause(0, true),
            Some(TurnLivenessCauseV1::LegacyWatchdog)
        );
    }

    #[test]
    fn work_cutoff_fires_regardless_of_open_tools() {
        let mut tracker = TurnLivenessTracker::new(ResolvedTurnLivenessV1 {
            silence_ms: Some(30 * MIN),
            work_cutoff_ms: Some(60 * MIN),
        });
        tracker.observe(MIN, tool("long", ToolCallPhaseV1::InProgress));
        tracker.permission_arrived(MIN);
        assert_eq!(due(&tracker, 59 * MIN), None);
        assert_eq!(
            due(&tracker, 60 * MIN),
            Some(TurnLivenessCauseV1::WorkCutoff)
        );
    }

    #[test]
    fn silence_due_never_overflows() {
        let mut tracker = TurnLivenessTracker::new(ResolvedTurnLivenessV1 {
            silence_ms: Some(u64::MAX),
            work_cutoff_ms: None,
        });
        tracker.observe(MIN, TurnActivityV1::Update);
        assert_eq!(tracker.silence_due_ms(), None);
        assert_eq!(due(&tracker, u64::MAX), None);
    }
}
