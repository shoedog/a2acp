//! One warm `claude` process per bridge SessionId. Mirrors AcpBackend's
//! AgentSession (turn lock, lazy proc, tolerant reader → per-turn mpsc) but is
//! single-session-per-process, so it adds `terminated` (pool teardown flag) and
//! a per-turn timeout, and serializes turns with a single active mpsc sender.
use crate::config::ClaudeConfig;
use crate::wire::{self, ClaudeEvent};
use bridge_core::error::BridgeError;
use bridge_core::ports::STOP_REASON_CANCELLED;
use bridge_core::process::Supervised;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex, OnceCell};

/// Per-turn event routed from the reader to the active `prompt` stream.
#[derive(Debug)]
pub enum TurnEvent {
    Text(String),
    Done { stop_reason: String },
    Failed(BridgeError),
}

/// The warm process. One active turn at a time (serialized by `turn_lock`).
pub struct SessionProc {
    stdin: Mutex<tokio::process::ChildStdin>,
    pub turn_lock: Arc<Mutex<()>>,
    /// Set by the pool's `invalidate_slot` (reaper/LRU/cancel/timeout). A `prompt`
    /// that acquired the turn lock AFTER a reap observes this and respawns (§3.2).
    pub terminated: AtomicBool,
    /// Cancel latch: when set, an EOF / error `result` for the in-flight turn maps
    /// to Canceled, not Failed (§4 cancel precedence). NOT reset at turn start — a
    /// cancelled proc is always invalidated+removed, so no stale latch can leak.
    pub cancel_requested: AtomicBool,
    /// True while a turn is in flight (set in `begin_turn`, cleared in `end_turn`).
    /// The reaper uses this to count IDLE procs only for the `max_warm` cap (§3.3).
    pub in_turn: AtomicBool,
    /// The single active turn's sender (one turn at a time).
    turn_tx: StdMutex<Option<mpsc::UnboundedSender<TurnEvent>>>,
    supervised: StdMutex<Option<Supervised>>,
    pub claude_session_id: StdMutex<Option<String>>,
    pub last_used: StdMutex<Instant>,
}

impl SessionProc {
    pub fn touch(&self) {
        if let Ok(mut t) = self.last_used.lock() {
            *t = Instant::now();
        }
    }
    pub fn idle_for(&self) -> std::time::Duration {
        self.last_used
            .lock()
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }
    /// Write the user envelope for a turn (caller holds the turn lock).
    pub async fn write_turn(&self, text: &str) -> Result<(), BridgeError> {
        let line = wire::user_envelope(text);
        let mut w = self.stdin.lock().await;
        w.write_all(line.as_bytes())
            .await
            .map_err(|_| BridgeError::AgentCrashed)?;
        w.write_all(b"\n")
            .await
            .map_err(|_| BridgeError::AgentCrashed)?;
        w.flush().await.map_err(|_| BridgeError::AgentCrashed)?;
        Ok(())
    }
    /// Register the active turn's sender; returns the receiver the stream drains.
    pub fn begin_turn(&self) -> mpsc::UnboundedReceiver<TurnEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.in_turn.store(true, Ordering::SeqCst);
        if let Ok(mut g) = self.turn_tx.lock() {
            *g = Some(tx);
        }
        rx
    }
    pub fn end_turn(&self) {
        self.in_turn.store(false, Ordering::SeqCst);
        if let Ok(mut g) = self.turn_tx.lock() {
            *g = None;
        }
    }
    /// True iff no turn is currently in flight (used by the idle-retention cap).
    pub fn is_idle(&self) -> bool {
        !self.in_turn.load(Ordering::SeqCst)
    }
    /// Terminate the child (idempotent take-once). Sets `terminated` first so a
    /// racing `prompt` revalidation observes it.
    pub async fn terminate(&self, grace: std::time::Duration) {
        self.terminated.store(true, Ordering::SeqCst);
        let sup = self.supervised.lock().ok().and_then(|mut g| g.take());
        if let Some(sup) = sup {
            sup.terminate(grace).await;
        }
    }
    fn route(&self, ev: TurnEvent) {
        if let Ok(g) = self.turn_tx.lock() {
            if let Some(tx) = g.as_ref() {
                let _ = tx.send(ev);
            }
        }
    }
}

/// A map slot holding the lazily-minted proc. `OnceCell` cannot be reset, so the
/// pool teardown REMOVES the slot from the map (a fresh prompt re-inserts a new
/// slot → cold respawn).
pub struct SessionSlot {
    pub proc: OnceCell<Arc<SessionProc>>,
    /// SLOT-level cancel latch. Set by `cancel()` even when the proc is not yet
    /// minted (the spawn window), so a `prompt` that minted/locked AFTER the cancel
    /// observes it and ends the turn Canceled instead of running it (review #2a).
    /// Lives on the slot (not the proc) precisely because it must survive the
    /// no-proc-yet window.
    pub cancel_requested: AtomicBool,
}

impl Default for SessionSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionSlot {
    pub fn new() -> Self {
        Self {
            proc: OnceCell::new(),
            cancel_requested: AtomicBool::new(false),
        }
    }
}

/// Spawn one warm `claude` process and its reader task; await the init line
/// (bounded by `init_timeout`) so we capture the session id and surface an
/// auth/trust failure as a bounded error rather than a hang (§4).
pub async fn spawn_proc(cmd: &str, cfg: &ClaudeConfig) -> Result<Arc<SessionProc>, BridgeError> {
    let mut args: Vec<String> = vec![
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if let Some(m) = &cfg.model {
        args.push("--model".into());
        args.push(m.clone());
    }
    args.extend(cfg.extra_args.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    // Supervised spawns with process_group(0) + kill_on_drop + piped stdio, in the
    // configured trusted cwd (Task 3 added the cwd param).
    let mut sup = Supervised::spawn(cmd, &arg_refs, Some(cfg.cwd.as_path()))
        .map_err(|_| BridgeError::AgentCrashed)?;
    let child = sup.child_mut();
    let stdin = child.stdin.take().ok_or(BridgeError::AgentCrashed)?;
    let stdout = child.stdout.take().ok_or(BridgeError::AgentCrashed)?;

    let (init_tx, init_rx) = oneshot::channel::<String>();
    let proc = Arc::new(SessionProc {
        stdin: Mutex::new(stdin),
        turn_lock: Arc::new(Mutex::new(())),
        terminated: AtomicBool::new(false),
        cancel_requested: AtomicBool::new(false),
        in_turn: AtomicBool::new(false),
        turn_tx: StdMutex::new(None),
        supervised: StdMutex::new(Some(sup)),
        claude_session_id: StdMutex::new(None),
        last_used: StdMutex::new(Instant::now()),
    });

    // Reader task: NDJSON line loop, tolerant drop, per-turn routing.
    let reader_proc = Arc::clone(&proc);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut init_tx = Some(init_tx);
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if let Some(ev) = wire::parse_line(&line) {
                        match ev {
                            ClaudeEvent::Init { session_id } => {
                                if let Ok(mut g) = reader_proc.claude_session_id.lock() {
                                    *g = Some(session_id.clone());
                                }
                                if let Some(tx) = init_tx.take() {
                                    let _ = tx.send(session_id);
                                }
                            }
                            ClaudeEvent::Text(t) => reader_proc.route(TurnEvent::Text(t)),
                            ClaudeEvent::ResultOk { stop_reason } => {
                                reader_proc.route(TurnEvent::Done {
                                    stop_reason: stop_reason.unwrap_or_else(|| "end_turn".into()),
                                })
                            }
                            ClaudeEvent::ResultErr { subtype } => {
                                if reader_proc.cancel_requested.load(Ordering::SeqCst) {
                                    reader_proc.route(TurnEvent::Done {
                                        stop_reason: STOP_REASON_CANCELLED.into(),
                                    });
                                } else {
                                    tracing::warn!(subtype, "claude result error");
                                    reader_proc.route(TurnEvent::Failed(BridgeError::AgentCrashed));
                                }
                            }
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    // EOF / read error. If a cancel is pending, the in-flight turn
                    // is Canceled; otherwise it's a crash mid-turn.
                    reader_proc.terminated.store(true, Ordering::SeqCst);
                    if reader_proc.cancel_requested.load(Ordering::SeqCst) {
                        reader_proc.route(TurnEvent::Done {
                            stop_reason: STOP_REASON_CANCELLED.into(),
                        });
                    } else {
                        reader_proc.route(TurnEvent::Failed(BridgeError::AgentCrashed));
                    }
                    break;
                }
            }
        }
    });

    // Bounded wait for the init line.
    match tokio::time::timeout(cfg.init_timeout, init_rx).await {
        Ok(Ok(_sid)) => Ok(proc),
        // Channel dropped (reader hit EOF before init) → not authenticated / trust prompt / crash.
        Ok(Err(_)) => {
            proc.terminate(cfg.cancel_grace).await;
            Err(BridgeError::AgentNotAuthenticated)
        }
        Err(_) => {
            proc.terminate(cfg.cancel_grace).await;
            Err(BridgeError::AgentNotAuthenticated)
        }
    }
}
