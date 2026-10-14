// heartbeat.rs — the client heartbeat of `submit` and `run-workflow` (activity liveness I3).
//
// While a run is in flight the client writes one stderr line per beat, first after one
// interval and then every interval, and never anything on stdout. A beat reports what the
// run's turns are doing: the in-process heartbeat board for a local `run-workflow`, the
// bridge-private `TaskLiveness` RPC for a served one. Every poll has its own deadline, so a
// stalled server can never silence the heartbeat: a poll that times out, fails, or meets an
// older server ("method not found") prints the elapsed-only line and the next beat polls again.

use std::future::Future;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bridge_core::turn_liveness::{
    parse_liveness_secs_v1, LivenessOverrideV1, TurnLivenessBoard, TurnLivenessSnapshotV1,
    TOOL_TITLE_MAX_BYTES,
};

/// Overrides the default interval; a command-line flag beats it.
pub(crate) const HEARTBEAT_ENV: &str = "A2A_BRIDGE_HEARTBEAT_SECS";
const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
const HEARTBEAT_SECS_FLAG: &str = "--heartbeat-secs";
const NO_HEARTBEAT_FLAG: &str = "--no-heartbeat";
/// The longest any one poll may take, whatever the interval.
const POLL_DEADLINE_MAX: Duration = Duration::from_secs(10);
/// Turns one beat describes; the rest are counted.
const MAX_LISTED_TURNS: usize = 4;
const AGENT_MAX_BYTES: usize = 64;

/// The heartbeat interval from `--heartbeat-secs N` (`0` is off), `--no-heartbeat`, else
/// `A2A_BRIDGE_HEARTBEAT_SECS`, else 60 seconds. `None` is off. A flag beats the env.
pub(crate) fn heartbeat_interval(
    args: &[String],
    env: Option<&str>,
) -> Result<Option<Duration>, String> {
    let mut secs_flag = None;
    let mut off_flag = false;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            HEARTBEAT_SECS_FLAG => {
                let value = args
                    .get(idx + 1)
                    .ok_or_else(|| format!("{HEARTBEAT_SECS_FLAG} requires a value"))?;
                secs_flag = Some(value.as_str());
                idx += 2;
            }
            NO_HEARTBEAT_FLAG => {
                off_flag = true;
                idx += 1;
            }
            _ => idx += 1,
        }
    }
    match (off_flag, secs_flag) {
        (true, Some(_)) => Err(format!(
            "{NO_HEARTBEAT_FLAG} and {HEARTBEAT_SECS_FLAG} cannot be combined"
        )),
        (true, None) => Ok(None),
        (false, Some(value)) => parse_interval(value, HEARTBEAT_SECS_FLAG),
        (false, None) => match env {
            Some(value) => parse_interval(value, HEARTBEAT_ENV),
            None => Ok(Some(DEFAULT_INTERVAL)),
        },
    }
}

/// Whole seconds through the shared checked parser: `0` is off, at most 30 days.
fn parse_interval(value: &str, source: &str) -> Result<Option<Duration>, String> {
    let secs = value
        .parse::<u64>()
        .map_err(|_| format!("{source} expects whole seconds, got {value:?}"))?;
    match parse_liveness_secs_v1(secs).map_err(|error| format!("{source} {value}: {error}"))? {
        LivenessOverrideV1::Off => Ok(None),
        LivenessOverrideV1::Ms(ms) => Ok(Some(Duration::from_millis(ms))),
    }
}

/// `args` without the heartbeat flags, for a subcommand parser that refuses unknown flags or
/// reads the first bare argument as a route.
pub(crate) fn strip_heartbeat_flags(args: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(args.len());
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            HEARTBEAT_SECS_FLAG => idx += 2,
            NO_HEARTBEAT_FLAG => idx += 1,
            _ => {
                kept.push(args[idx].clone());
                idx += 1;
            }
        }
    }
    kept
}

/// One poll's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BeatLiveness {
    /// The board (or the server) answered: the turns in flight, possibly none.
    Turns(Vec<TurnLivenessSnapshotV1>),
    /// Nothing is known: the poll timed out or failed, or the server predates `TaskLiveness`.
    Unavailable,
}

/// When the last SSE frame of a served run arrived; shared with the stream loop.
#[derive(Clone, Default)]
pub(crate) struct FrameClock(Arc<Mutex<Option<tokio::time::Instant>>>);

impl FrameClock {
    pub(crate) fn frame(&self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tokio::time::Instant::now());
    }

    fn age(&self) -> Option<Duration> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map(|at| at.elapsed())
    }
}

/// A poll can take at most half an interval, and never more than 10 seconds.
fn poll_deadline(interval: Duration) -> Duration {
    POLL_DEADLINE_MAX.min(interval / 2)
}

/// Write beats to `out` until the returned future is dropped. `poll` runs once per beat under
/// its own deadline; `frames`, when given, adds the age of the last SSE frame.
pub(crate) async fn run_heartbeat<P, F, W>(
    interval: Duration,
    mut poll: P,
    frames: Option<FrameClock>,
    mut out: W,
) where
    P: FnMut() -> F,
    F: Future<Output = BeatLiveness>,
    W: Write,
{
    let started = tokio::time::Instant::now();
    let deadline = poll_deadline(interval);
    let mut ticker = tokio::time::interval_at(started + interval, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let elapsed = started.elapsed();
        let liveness = tokio::time::timeout(deadline, poll())
            .await
            .unwrap_or(BeatLiveness::Unavailable);
        let last_frame = frames.as_ref().map(FrameClock::age);
        let mut line = format_beat(elapsed, last_frame, &liveness);
        line.push('\n');
        // One write per line, so a beat never interleaves with other stderr output.
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }
}

/// A heartbeat running beside the command; dropping it stops the beats.
pub(crate) struct HeartbeatTask(tokio::task::JoinHandle<()>);

impl Drop for HeartbeatTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Start beating on stderr every `interval` (`None` is off).
pub(crate) fn spawn_heartbeat<P, F>(
    interval: Option<Duration>,
    poll: P,
    frames: Option<FrameClock>,
) -> Option<HeartbeatTask>
where
    P: FnMut() -> F + Send + 'static,
    F: Future<Output = BeatLiveness> + Send + 'static,
{
    let interval = interval?;
    Some(HeartbeatTask(tokio::spawn(run_heartbeat(
        interval,
        poll,
        frames,
        std::io::stderr(),
    ))))
}

/// A poller reading this process's own heartbeat board (a local `run-workflow`).
pub(crate) fn board_poller(
    board: Arc<TurnLivenessBoard>,
    observer_key: String,
) -> impl FnMut() -> std::future::Ready<BeatLiveness> + Send + 'static {
    move || std::future::ready(BeatLiveness::Turns(board.snapshot(&observer_key)))
}

/// A poller asking a running `serve` for the turns of `task` over `TaskLiveness`.
pub(crate) fn rpc_poller(
    url: String,
    task: String,
) -> impl FnMut() -> std::pin::Pin<Box<dyn Future<Output = BeatLiveness> + Send>> + Send + 'static {
    move || {
        let (url, task) = (url.clone(), task.clone());
        Box::pin(async move {
            match bridge_a2a_outbound::A2aClient::loopback(&url)
                .rpc("TaskLiveness", serde_json::json!({ "taskId": task }))
                .await
            {
                Ok(response) => liveness_from_rpc(&response),
                Err(_) => BeatLiveness::Unavailable,
            }
        })
    }
}

/// A `TaskLiveness` response. `TaskNotFound` is a definite answer (no turn in flight); any
/// other error, an older server's "method not found" included, leaves the liveness unknown.
pub(crate) fn liveness_from_rpc(response: &serde_json::Value) -> BeatLiveness {
    if let Some(error) = response.get("error") {
        return if error.get("message").and_then(serde_json::Value::as_str) == Some("task not found")
        {
            BeatLiveness::Turns(Vec::new())
        } else {
            BeatLiveness::Unavailable
        };
    }
    response
        .pointer("/result/turns")
        .cloned()
        .and_then(|turns| serde_json::from_value(turns).ok())
        .map_or(BeatLiveness::Unavailable, BeatLiveness::Turns)
}

/// One beat, always exactly one line:
/// `[heartbeat] elapsed=14m00s turns=1 agent=codex last_activity=2m10s open_tool="cargo test"
/// tool_kind=execute tool_age=9m02s silence=paused cutoff=30m`, or
/// `[heartbeat] elapsed=14m00s liveness=unavailable`. `last_frame` is `Some` for a served
/// stream: the age of its last SSE frame, `None` before the first.
pub(crate) fn format_beat(
    elapsed: Duration,
    last_frame: Option<Option<Duration>>,
    liveness: &BeatLiveness,
) -> String {
    let mut line = format!("[heartbeat] elapsed={}", age(elapsed));
    match last_frame {
        Some(Some(frame)) => line.push_str(&format!(" last_frame={}", age(frame))),
        Some(None) => line.push_str(" last_frame=none"),
        None => {}
    }
    let turns = match liveness {
        BeatLiveness::Unavailable => {
            line.push_str(" liveness=unavailable");
            return line;
        }
        BeatLiveness::Turns(turns) => turns,
    };
    line.push_str(&format!(" turns={}", turns.len()));
    for (index, turn) in turns.iter().take(MAX_LISTED_TURNS).enumerate() {
        line.push_str(if index == 0 { " " } else { " | " });
        push_turn(&mut line, turn);
    }
    if turns.len() > MAX_LISTED_TURNS {
        line.push_str(&format!(" | +{} more", turns.len() - MAX_LISTED_TURNS));
    }
    line
}

fn push_turn(line: &mut String, turn: &TurnLivenessSnapshotV1) {
    line.push_str("agent=");
    push_token(line, &turn.agent, AGENT_MAX_BYTES);
    line.push_str(&format!(
        " last_activity={}",
        age(Duration::from_millis(turn.last_activity_age_ms))
    ));
    if let Some(tool) = turn.open_tools.first() {
        line.push_str(" open_tool=\"");
        let title = truncate_utf8(&tool.title, TOOL_TITLE_MAX_BYTES);
        push_escaped(line, title);
        if title.len() < tool.title.len() {
            line.push('…');
        }
        line.push_str("\" tool_kind=");
        push_token(line, &tool.kind, TOOL_TITLE_MAX_BYTES);
        line.push_str(&format!(
            " tool_age={}",
            age(Duration::from_millis(tool.age_ms))
        ));
    }
    if turn.open_tool_count > 1 || turn.open_overflowed {
        line.push_str(&format!(" open_tools={}", turn.open_tool_count));
        if turn.open_overflowed {
            line.push('+');
        }
    }
    match turn.silence_cutoff_ms {
        None => line.push_str(" silence=off"),
        Some(cutoff) => line.push_str(&format!(
            " silence={} cutoff={}",
            if turn.paused { "paused" } else { "running" },
            bound(cutoff)
        )),
    }
    if let Some(cutoff) = turn.work_cutoff_ms {
        line.push_str(&format!(" work_cutoff={}", bound(cutoff)));
    }
    if turn.pending_permissions > 0 {
        line.push_str(&format!(" permission_pending={}", turn.pending_permissions));
    }
}

/// An agent-controlled word: bare when it is plain, else quoted and escaped.
fn push_token(line: &mut String, text: &str, max_bytes: usize) {
    let text = truncate_utf8(text, max_bytes);
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'));
    if plain {
        line.push_str(text);
    } else {
        line.push('"');
        push_escaped(line, text);
        line.push('"');
    }
}

/// JSON-style escaping: quotes, backslashes, newlines, tabs and every control character
/// (ESC included), so agent text can neither break the line nor drive the terminal.
fn push_escaped(line: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '"' => line.push_str("\\\""),
            '\\' => line.push_str("\\\\"),
            '\n' => line.push_str("\\n"),
            '\r' => line.push_str("\\r"),
            '\t' => line.push_str("\\t"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                line.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
            c => line.push(c),
        }
    }
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `14m00s`, `2m10s`, `45s`, `1h02m03s`.
fn age(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// A configured bound: `30m`, `2h`, `1h30m`, `45s`.
fn bound(ms: u64) -> String {
    if !ms.is_multiple_of(1000) {
        return format!("{ms}ms");
    }
    let secs = ms / 1000;
    let mut text = String::new();
    for (value, unit) in [
        (secs / 3600, 'h'),
        (secs % 3600 / 60, 'm'),
        (secs % 60, 's'),
    ] {
        if value > 0 {
            text.push_str(&format!("{value}{unit}"));
        }
    }
    if text.is_empty() {
        text.push_str("0s");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::turn_liveness::OpenToolSnapshotV1;

    const MIN: u64 = 60_000;

    fn args(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn turn() -> TurnLivenessSnapshotV1 {
        TurnLivenessSnapshotV1 {
            turn_id: "turn-1".into(),
            agent: "codex".into(),
            elapsed_ms: 14 * MIN,
            last_activity_age_ms: 2 * MIN + 10_000,
            open_tool_count: 1,
            open_tools: vec![OpenToolSnapshotV1 {
                title: "cargo test".into(),
                kind: "execute".into(),
                age_ms: 9 * MIN + 2_000,
            }],
            open_overflowed: false,
            pending_permissions: 0,
            paused: true,
            silence_cutoff_ms: Some(30 * MIN),
            work_cutoff_ms: None,
        }
    }

    /// A captured stderr.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn lines(&self) -> Vec<String> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    #[test]
    fn a_beat_with_an_open_tool_matches_the_design_line() {
        assert_eq!(
            format_beat(
                Duration::from_secs(14 * 60),
                None,
                &BeatLiveness::Turns(vec![turn()])
            ),
            "[heartbeat] elapsed=14m00s turns=1 agent=codex last_activity=2m10s \
             open_tool=\"cargo test\" tool_kind=execute tool_age=9m02s silence=paused cutoff=30m"
        );
    }

    #[test]
    fn an_idle_turn_shows_its_running_clock_and_bounds() {
        let idle = TurnLivenessSnapshotV1 {
            open_tool_count: 0,
            open_tools: Vec::new(),
            paused: false,
            last_activity_age_ms: 45_000,
            work_cutoff_ms: Some(120 * MIN),
            pending_permissions: 1,
            ..turn()
        };
        assert_eq!(
            format_beat(
                Duration::from_secs(3_723),
                None,
                &BeatLiveness::Turns(vec![idle.clone()])
            ),
            "[heartbeat] elapsed=1h02m03s turns=1 agent=codex last_activity=45s \
             silence=running cutoff=30m work_cutoff=2h permission_pending=1"
        );
        let off = TurnLivenessSnapshotV1 {
            silence_cutoff_ms: None,
            work_cutoff_ms: Some(90 * MIN),
            ..idle
        };
        assert!(
            format_beat(Duration::ZERO, None, &BeatLiveness::Turns(vec![off]))
                .ends_with("silence=off work_cutoff=1h30m permission_pending=1")
        );
        assert_eq!(
            format_beat(
                Duration::from_secs(60),
                None,
                &BeatLiveness::Turns(Vec::new())
            ),
            "[heartbeat] elapsed=1m00s turns=0"
        );
    }

    #[test]
    fn an_unavailable_beat_is_elapsed_only() {
        assert_eq!(
            format_beat(
                Duration::from_secs(14 * 60),
                None,
                &BeatLiveness::Unavailable
            ),
            "[heartbeat] elapsed=14m00s liveness=unavailable"
        );
        assert_eq!(
            format_beat(
                Duration::from_secs(120),
                Some(Some(Duration::from_secs(12))),
                &BeatLiveness::Unavailable
            ),
            "[heartbeat] elapsed=2m00s last_frame=12s liveness=unavailable"
        );
        assert_eq!(
            format_beat(
                Duration::from_secs(60),
                Some(None),
                &BeatLiveness::Unavailable
            ),
            "[heartbeat] elapsed=1m00s last_frame=none liveness=unavailable"
        );
    }

    #[test]
    fn fan_out_turns_share_one_line() {
        let many: Vec<_> = (0..6)
            .map(|index| TurnLivenessSnapshotV1 {
                agent: format!("agent{index}"),
                ..turn()
            })
            .collect();
        let line = format_beat(Duration::from_secs(60), None, &BeatLiveness::Turns(many));
        assert_eq!(line.lines().count(), 1);
        assert!(line.starts_with("[heartbeat] elapsed=1m00s turns=6 agent=agent0 "));
        assert!(line.contains(" | agent=agent3 "));
        assert!(!line.contains("agent=agent4"));
        assert!(line.ends_with(" | +2 more"));
    }

    /// Agent text with a newline, quotes, a backslash, ESC and a multibyte character across
    /// the truncation boundary is one escaped line that cannot drive the terminal.
    #[test]
    fn agent_controlled_text_is_truncated_then_escaped_onto_one_line() {
        let title = format!(
            "{}\n\"quoted\"\t\\ \u{1b}[31mred\u{7}{}é",
            "a".repeat(40),
            "b".repeat(66)
        );
        // "é" straddles byte 128, so truncation drops it whole.
        assert_eq!(title.len(), 129);
        let hostile = TurnLivenessSnapshotV1 {
            agent: "co dex\u{1b}".into(),
            open_tools: vec![OpenToolSnapshotV1 {
                title: title.clone(),
                kind: "exe\ncute".into(),
                age_ms: 1_000,
            }],
            ..turn()
        };
        let line = format_beat(
            Duration::from_secs(60),
            None,
            &BeatLiveness::Turns(vec![hostile]),
        );
        assert_eq!(line.lines().count(), 1, "{line}");
        assert!(!line.chars().any(|c| c.is_control()), "{line:?}");
        assert!(line.contains("agent=\"co dex\\u001b\""), "{line}");
        assert!(line.contains(&format!(
            "open_tool=\"{}\\n\\\"quoted\\\"\\t\\\\ \\u001b[31mred\\u0007{}…\"",
            "a".repeat(40),
            "b".repeat(66)
        )));
        assert!(!line.contains('é'));
        assert!(line.contains("tool_kind=\"exe\\ncute\""), "{line}");
    }

    #[test]
    fn a_flag_beats_the_env_and_zero_or_no_heartbeat_turns_it_off() {
        assert_eq!(
            heartbeat_interval(&args(&[]), None),
            Ok(Some(Duration::from_secs(60)))
        );
        assert_eq!(
            heartbeat_interval(&args(&[]), Some("30")),
            Ok(Some(Duration::from_secs(30)))
        );
        assert_eq!(
            heartbeat_interval(&args(&["--heartbeat-secs", "5"]), Some("30")),
            Ok(Some(Duration::from_secs(5)))
        );
        assert_eq!(heartbeat_interval(&args(&[]), Some("0")), Ok(None));
        assert_eq!(
            heartbeat_interval(&args(&["--heartbeat-secs", "0"]), Some("30")),
            Ok(None)
        );
        assert_eq!(
            heartbeat_interval(&args(&["--no-heartbeat"]), Some("30")),
            Ok(None)
        );
        assert_eq!(
            heartbeat_interval(&args(&["--heartbeat-secs", "45"]), Some("0")),
            Ok(Some(Duration::from_secs(45))),
            "a flag beats an env that turns it off"
        );
        for refused in [
            heartbeat_interval(&args(&["--heartbeat-secs"]), None),
            heartbeat_interval(&args(&["--heartbeat-secs", "soon"]), None),
            heartbeat_interval(&args(&["--heartbeat-secs", "2592001"]), None),
            heartbeat_interval(&args(&["--no-heartbeat", "--heartbeat-secs", "5"]), None),
            heartbeat_interval(&args(&[]), Some("-1")),
        ] {
            assert!(refused.is_err(), "{refused:?}");
        }
        assert_eq!(
            strip_heartbeat_flags(&args(&[
                "review",
                "--heartbeat-secs",
                "5",
                "--input",
                "x",
                "--no-heartbeat"
            ])),
            args(&["review", "--input", "x"])
        );
    }

    /// `--no-heartbeat` writes nothing at all.
    #[tokio::test(start_paused = true)]
    async fn no_heartbeat_is_silent() {
        let interval = heartbeat_interval(&args(&["--no-heartbeat"]), Some("1")).unwrap();
        let task = spawn_heartbeat(
            interval,
            || std::future::ready(BeatLiveness::Unavailable),
            None,
        );
        assert!(task.is_none());
    }

    /// A 150 s wait beats twice (60 s, 120 s), each a whole line on the captured stderr.
    #[tokio::test(start_paused = true)]
    async fn a_150_second_wait_writes_two_lines() {
        let stderr = Captured::default();
        let board = Arc::new(TurnLivenessBoard::default());
        let beat = run_heartbeat(
            Duration::from_secs(60),
            board_poller(board, "exec-1".into()),
            None,
            stderr.clone(),
        );
        let _ = tokio::time::timeout(Duration::from_secs(150), beat).await;
        assert_eq!(
            stderr.lines(),
            vec![
                "[heartbeat] elapsed=1m00s turns=0",
                "[heartbeat] elapsed=2m00s turns=0"
            ]
        );
    }

    /// A poll that never answers costs its beat the details, never the beat: each later
    /// interval polls again and still prints.
    #[tokio::test(start_paused = true)]
    async fn a_poll_that_never_answers_still_beats_elapsed_only() {
        let stderr = Captured::default();
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let beat = run_heartbeat(
            Duration::from_secs(60),
            {
                let polls = polls.clone();
                move || {
                    polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::future::pending::<BeatLiveness>()
                }
            },
            None,
            stderr.clone(),
        );
        // Each beat waits out its poll's 10 s deadline before printing.
        let _ = tokio::time::timeout(Duration::from_secs(195), beat).await;
        assert_eq!(
            stderr.lines(),
            vec![
                "[heartbeat] elapsed=1m00s liveness=unavailable",
                "[heartbeat] elapsed=2m00s liveness=unavailable",
                "[heartbeat] elapsed=3m00s liveness=unavailable"
            ]
        );
        assert_eq!(polls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    /// `run-workflow --serve` beats report the age of the stream's last SSE frame.
    #[tokio::test(start_paused = true)]
    async fn a_served_stream_beat_reports_its_last_frame_age() {
        let stderr = Captured::default();
        let frames = FrameClock::default();
        let beat = run_heartbeat(
            Duration::from_secs(60),
            || std::future::ready(BeatLiveness::Turns(Vec::new())),
            Some(frames.clone()),
            stderr.clone(),
        );
        let frame_at_20s = async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            frames.frame();
            std::future::pending::<()>().await;
        };
        let _ = tokio::time::timeout(Duration::from_secs(61), async {
            tokio::join!(beat, frame_at_20s);
        })
        .await;
        assert_eq!(
            stderr.lines(),
            vec!["[heartbeat] elapsed=1m00s last_frame=40s turns=0"]
        );
    }

    #[test]
    fn the_poll_deadline_is_half_the_interval_up_to_ten_seconds() {
        assert_eq!(
            poll_deadline(Duration::from_secs(60)),
            Duration::from_secs(10)
        );
        assert_eq!(
            poll_deadline(Duration::from_secs(8)),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn task_liveness_responses_map_to_beats() {
        let turns = serde_json::to_value(vec![turn()]).unwrap();
        assert_eq!(
            liveness_from_rpc(&serde_json::json!({ "result": { "turns": turns } })),
            BeatLiveness::Turns(vec![turn()])
        );
        assert_eq!(
            liveness_from_rpc(&serde_json::json!({
                "error": { "code": -32600, "message": "task not found" }
            })),
            BeatLiveness::Turns(Vec::new())
        );
        for unknown in [
            serde_json::json!({ "error": { "code": -32601, "message": "method not found" } }),
            serde_json::json!({ "error": { "code": -32600, "message": "auth required" } }),
            serde_json::json!({ "result": { "turns": "garbled" } }),
            serde_json::json!({}),
        ] {
            assert_eq!(
                liveness_from_rpc(&unknown),
                BeatLiveness::Unavailable,
                "{unknown}"
            );
        }
    }

    /// A stub `serve` that answers `TaskLiveness` as `respond` says, counting the polls.
    async fn stub_serve(
        respond: fn() -> Option<serde_json::Value>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = polls.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |axum::Json(request): axum::Json<serde_json::Value>| {
                let counted = counted.clone();
                async move {
                    assert_eq!(request["method"], "TaskLiveness");
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    match respond() {
                        Some(body) => axum::Json(body),
                        None => std::future::pending().await,
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (url, polls)
    }

    async fn beats_against(url: &str, count: usize) -> Vec<String> {
        let stderr = Captured::default();
        let beat = run_heartbeat(
            Duration::from_millis(200),
            rpc_poller(url.to_owned(), "exec-1".into()),
            None,
            stderr.clone(),
        );
        let _ = tokio::time::timeout(Duration::from_secs(20), async {
            tokio::select! {
                _ = beat => {}
                _ = async {
                    while stderr.lines().len() < count {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                } => {}
            }
        })
        .await;
        stderr.lines()
    }

    /// An older server answers "method not found": every beat is elapsed-only.
    #[tokio::test]
    async fn an_older_server_falls_back_to_elapsed_only() {
        let (url, polls) = stub_serve(|| {
            Some(serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "error": { "code": -32601, "message": "method not found" }
            }))
        })
        .await;
        let lines = beats_against(&url, 2).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines
            .iter()
            .all(|line| line.ends_with(" liveness=unavailable")));
        assert!(polls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    }

    /// A server that holds every `TaskLiveness` poll forever still gets an elapsed-only beat
    /// each interval, and is polled again each time.
    #[tokio::test]
    async fn a_server_holding_the_poll_forever_still_gets_beats() {
        let (url, polls) = stub_serve(|| None).await;
        let lines = beats_against(&url, 2).await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines
            .iter()
            .all(|line| line.ends_with(" liveness=unavailable")));
        assert!(polls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    }

    /// A served turn's details reach the beat over the RPC.
    #[tokio::test]
    async fn a_serving_bridge_turn_reaches_the_beat() {
        let (url, _) = stub_serve(|| {
            Some(serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "result": { "turns": [turn()] }
            }))
        })
        .await;
        let lines = beats_against(&url, 1).await;
        assert!(
            lines[0].contains(" turns=1 agent=codex last_activity=2m10s open_tool=\"cargo test\""),
            "{lines:?}"
        );
    }
}
