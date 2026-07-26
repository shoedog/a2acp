use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{
    BufRead, BufReader as StdBufReader, Error as IoError, ErrorKind, Seek, SeekFrom, Write,
};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{ChildStdin, Command};
use tokio::sync::Mutex;

const ISSUER_ID: &str = "bridge.acp.codex.commit-wrapper.v1";
const META_KEY: &str = "dev.b2a.attested_prefix";
const CAPABILITIES_METHOD: &str = "_b2a/apc-prefix/capabilities";
const BEGIN_TURN_METHOD: &str = "_b2a/apc-prefix/beginTurn";
const MARKER_PREFIX: &str = "<|b2a_apc_commit_v1:";
const MARKER_SUFFIX: &str = "|>";
const FRAME_MEMORY_LIMIT_BYTES: usize = 1024 * 1024;

type DynError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Debug)]
struct TurnConfig {
    session_id: String,
    turn_id: String,
    marker_nonce: String,
    marker: String,
    enabled: bool,
}

struct PromptBuffer {
    request_id: Value,
    session_id: String,
    turn: TurnConfig,
    frames: Arc<Mutex<FrameBuffer>>,
}

/// Buffer of exact wire lines (§4.2 narrow proxy: frames the wrapper does not
/// transform must reach the bridge byte-for-byte, so the buffer stores the raw
/// serialized line of every frame, never a re-serialization of a parsed value).
struct FrameBuffer {
    lines: Vec<String>,
    memory_bytes: usize,
    spool: Option<SpoolFile>,
}

struct SpoolFile {
    path: PathBuf,
    file: File,
    count: usize,
}

impl FrameBuffer {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            memory_bytes: 0,
            spool: None,
        }
    }

    fn push_line(&mut self, line: String) -> Result<(), DynError> {
        let entry_bytes = line
            .len()
            .checked_add(1)
            .ok_or("attested-prefix frame buffer size overflow")?;
        if self.spool.is_none()
            && self
                .memory_bytes
                .checked_add(entry_bytes)
                .is_some_and(|bytes| bytes <= FRAME_MEMORY_LIMIT_BYTES)
        {
            self.memory_bytes += entry_bytes;
            self.lines.push(line);
            return Ok(());
        }

        if self.spool.is_none() {
            let mut spool = SpoolFile::create()?;
            for buffered in self.lines.drain(..) {
                spool.write_line(&buffered)?;
            }
            self.memory_bytes = 0;
            self.spool = Some(spool);
        }

        if let Some(spool) = &mut self.spool {
            spool.write_line(&line)?;
        }
        Ok(())
    }

    fn into_lines(mut self) -> Result<Vec<String>, DynError> {
        if let Some(spool) = self.spool.take() {
            spool.read_lines()
        } else {
            Ok(self.lines)
        }
    }
}

async fn push_frame_line_blocking(
    frames: Arc<Mutex<FrameBuffer>>,
    line: String,
) -> Result<(), DynError> {
    tokio::task::spawn_blocking(move || {
        let mut frames = frames.blocking_lock();
        frames.push_line(line)
    })
    .await
    .map_err(|error| {
        IoError::new(
            ErrorKind::Other,
            format!("attested-prefix frame spool task failed: {error}"),
        )
    })?
}

async fn drain_frame_lines(frames: &Arc<Mutex<FrameBuffer>>) -> Result<Vec<String>, DynError> {
    let mut guard = frames.lock().await;
    let frames = std::mem::replace(&mut *guard, FrameBuffer::new());
    drop(guard);
    frames.into_lines()
}

impl SpoolFile {
    fn create() -> Result<Self, DynError> {
        let mut nonce = [0_u8; 16];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| "attested-prefix spool nonce generation failed")?;
        let mut path = std::env::temp_dir();
        path.push(format!(
            "codex-acp-attested-{}-{}.jsonl",
            std::process::id(),
            hex_lower(&nonce)
        ));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok(Self {
            path,
            file,
            count: 0,
        })
    }

    fn write_line(&mut self, line: &str) -> Result<(), DynError> {
        // Buffered lines come from a line reader, so they contain no newline;
        // the spool round-trip therefore reproduces the exact wire bytes.
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.count += 1;
        Ok(())
    }

    fn read_lines(mut self) -> Result<Vec<String>, DynError> {
        self.file.flush()?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut reader = StdBufReader::new(&mut self.file);
        let mut lines = Vec::with_capacity(self.count);
        let mut line = String::new();
        while reader.read_line(&mut line)? != 0 {
            if line.ends_with('\n') {
                line.pop();
            }
            lines.push(std::mem::take(&mut line));
        }
        if lines.len() != self.count {
            return Err("attested-prefix spool frame count mismatch".into());
        }
        Ok(lines)
    }
}

impl Drop for SpoolFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Default)]
struct PerSessionState {
    active_turn: Option<TurnConfig>,
    prompt: Option<PromptBuffer>,
}

#[derive(Default)]
struct WrapperState {
    sessions: HashMap<String, PerSessionState>,
}

pub async fn run_from_args(argv: Vec<String>) -> Result<(), DynError> {
    let (cmd, args) = parse_args(argv)?;
    run(cmd, args).await
}

fn parse_args(argv: Vec<String>) -> Result<(String, Vec<String>), DynError> {
    if argv.first().is_some_and(|arg| arg == "--codex-acp") {
        let Some(cmd) = argv.get(1).cloned() else {
            return Err("--codex-acp requires an absolute command".into());
        };
        if !PathBuf::from(&cmd).is_absolute() {
            return Err("--codex-acp requires an absolute command".into());
        }
        let rest = if argv.get(2).is_some_and(|arg| arg == "--") {
            argv[3..].to_vec()
        } else {
            argv[2..].to_vec()
        };
        return Ok((cmd, rest));
    }
    Err("codex-acp-attested requires --codex-acp /absolute/path/to/codex-acp".into())
}

async fn run(cmd: String, args: Vec<String>) -> Result<(), DynError> {
    let mut child = Command::new(&cmd)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let child_stdin = child.stdin.take().ok_or("child stdin unavailable")?;
    let child_stdout = child.stdout.take().ok_or("child stdout unavailable")?;

    let state = Arc::new(Mutex::new(WrapperState::default()));
    let bridge_stdout = Arc::new(Mutex::new(BufWriter::new(tokio::io::stdout())));
    let child_stdin = Arc::new(Mutex::new(BufWriter::new(child_stdin)));

    let stdin_task = tokio::spawn(proxy_bridge_to_child(
        Arc::clone(&state),
        Arc::clone(&bridge_stdout),
        Arc::clone(&child_stdin),
    ));
    let mut stdout_task = tokio::spawn(proxy_child_to_bridge(
        Arc::clone(&state),
        Arc::clone(&bridge_stdout),
        child_stdout,
    ));

    let mut completed_stdout: Option<Result<(), DynError>> = None;
    let status = tokio::select! {
        status = child.wait() => status?,
        joined = &mut stdout_task => {
            stdin_task.abort();
            match joined {
                Ok(Ok(())) => {
                    completed_stdout = Some(Ok(()));
                    child.wait().await?
                }
                Ok(Err(error)) => {
                    let _ = child.kill().await;
                    return Err(error);
                }
                Err(error) => {
                    let _ = child.kill().await;
                    return Err(format!("child stdout proxy failed: {error}").into());
                }
            }
        }
    };
    stdin_task.abort();
    if let Some(result) = completed_stdout {
        result?;
    } else {
        match stdout_task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) => return Err(format!("child stdout proxy failed: {error}").into()),
        }
    }
    if status.success() {
        Ok(())
    } else {
        Err(format!("child {cmd:?} exited with {status}").into())
    }
}

async fn proxy_bridge_to_child(
    state: Arc<Mutex<WrapperState>>,
    bridge_stdout: Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    child_stdin: Arc<Mutex<BufWriter<ChildStdin>>>,
) -> Result<(), DynError> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let parsed: Result<Value, _> = serde_json::from_str(&line);
        if let Ok(value) = parsed {
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                if method == CAPABILITIES_METHOD {
                    write_json_line(
                        &bridge_stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "id": value.get("id").cloned().unwrap_or(Value::Null),
                            "result": {
                                "protocol_version": 1,
                                "issuer_id": ISSUER_ID,
                            }
                        }),
                    )
                    .await?;
                    continue;
                }
                if method == BEGIN_TURN_METHOD {
                    let response = handle_begin_turn(&state, &value).await;
                    write_json_line(&bridge_stdout, response).await?;
                    continue;
                }
                if method == "session/prompt" {
                    maybe_start_prompt_buffer(&state, &value).await;
                }
            }
        }
        let mut writer = child_stdin.lock().await;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    Ok(())
}

async fn proxy_child_to_bridge(
    state: Arc<Mutex<WrapperState>>,
    bridge_stdout: Arc<Mutex<BufWriter<tokio::io::Stdout>>>,
    child_stdout: tokio::process::ChildStdout,
) -> Result<(), DynError> {
    proxy_child_lines(state, &bridge_stdout, BufReader::new(child_stdout)).await
}

/// Child-to-bridge proxy loop over already-line-framed input.
///
/// §4.2 narrow-proxy contract: ordinary ACP frames pass through byte-for-byte.
/// The only permitted rewrites are reserved-metadata removal and the targeted
/// assistant text transformation at prompt flush, so every frame carries its
/// exact wire line forward and is re-serialized only when one of those two
/// transformations actually changed it.
async fn proxy_child_lines<R, W>(
    state: Arc<Mutex<WrapperState>>,
    bridge_stdout: &Arc<Mutex<BufWriter<W>>>,
    reader: R,
) -> Result<(), DynError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(mut value) = serde_json::from_str::<Value>(&line) else {
            write_raw_line(bridge_stdout, &line).await?;
            continue;
        };
        // Reserved-metadata filtering (§4.2) is the only unconditional
        // transformation. When nothing was removed, `line` is still the exact
        // wire form of `value`; otherwise the stripped value is re-serialized
        // once and that becomes the frame's wire form from here on.
        let raw = if strip_reserved_meta(&mut value) {
            serde_json::to_string(&value)?
        } else {
            line
        };

        let terminal_session = {
            let guard = state.lock().await;
            guard.sessions.iter().find_map(|(session_id, session)| {
                session
                    .prompt
                    .as_ref()
                    .filter(|prompt| is_prompt_terminal(&value, &prompt.request_id))
                    .map(|_| session_id.clone())
            })
        };

        if let Some(session_id) = terminal_session {
            let prompt = state
                .lock()
                .await
                .sessions
                .get_mut(&session_id)
                .and_then(|session| session.prompt.take());
            if let Some(mut prompt) = prompt {
                if is_successful_prompt_terminal(&value, &prompt.request_id) {
                    flush_prompt_buffer(bridge_stdout, &mut prompt, &raw).await?;
                } else {
                    for buffered in drain_frame_lines(&prompt.frames).await? {
                        write_raw_line(bridge_stdout, &buffered).await?;
                    }
                    write_raw_line(bridge_stdout, &raw).await?;
                }
            }
            continue;
        }

        let mut guard = state.lock().await;
        let frame_session = frame_session_id(&value).map(ToOwned::to_owned);
        if let Some(session_id) = frame_session {
            if let Some(prompt) = guard
                .sessions
                .get_mut(&session_id)
                .and_then(|session| session.prompt.as_mut())
            {
                if should_buffer_prompt_frame(&value) {
                    let frames = Arc::clone(&prompt.frames);
                    drop(guard);
                    push_frame_line_blocking(frames, raw).await?;
                } else {
                    drop(guard);
                    write_raw_line(bridge_stdout, &raw).await?;
                }
            } else {
                drop(guard);
                write_raw_line(bridge_stdout, &raw).await?;
            }
        } else {
            drop(guard);
            write_raw_line(bridge_stdout, &raw).await?;
        }
    }
    Ok(())
}

async fn handle_begin_turn(state: &Arc<Mutex<WrapperState>>, value: &Value) -> Value {
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let params = value.get("params").and_then(Value::as_object);
    let parsed = params.and_then(parse_begin_turn_params);
    let Some(turn) = parsed else {
        return rpc_error(id, -32602, "invalid attested-prefix beginTurn params");
    };
    let mut guard = state.lock().await;
    let session = guard.sessions.entry(turn.session_id.clone()).or_default();
    if session.active_turn.is_some() || session.prompt.is_some() {
        return rpc_error(id, -32000, "attested-prefix turn already active");
    }
    session.active_turn = Some(turn.clone());
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "schema_version": 1,
            "turn_id": turn.turn_id,
            "accepted": true,
        }
    })
}

fn parse_begin_turn_params(params: &Map<String, Value>) -> Option<TurnConfig> {
    if params.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let session_id = params
        .get("session_id")
        .or_else(|| params.get("sessionId"))?
        .as_str()?
        .to_string();
    let turn_id = params.get("turn_id")?.as_str()?.to_string();
    let enabled = params.get("enabled")?.as_bool()?;
    let marker_nonce = params.get("marker_nonce")?.as_str()?.to_string();
    if session_id.is_empty() || !valid_turn_id(&turn_id) || !valid_nonce(&marker_nonce) {
        return None;
    }
    Some(TurnConfig {
        session_id,
        marker: format!("{MARKER_PREFIX}{marker_nonce}{MARKER_SUFFIX}"),
        turn_id,
        marker_nonce,
        enabled,
    })
}

async fn maybe_start_prompt_buffer(state: &Arc<Mutex<WrapperState>>, value: &Value) {
    let Some(id) = value.get("id").cloned() else {
        return;
    };
    let Some(session_id) = value
        .get("params")
        .and_then(|params| params.get("sessionId"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return;
    };
    let mut guard = state.lock().await;
    if let Some(session) = guard.sessions.get_mut(&session_id) {
        if let Some(turn) = session.active_turn.take() {
            if turn.session_id == session_id {
                session.prompt = Some(PromptBuffer {
                    request_id: id,
                    session_id,
                    turn,
                    frames: Arc::new(Mutex::new(FrameBuffer::new())),
                });
            } else {
                session.active_turn = Some(turn);
            }
        }
    }
}

fn frame_session_id(value: &Value) -> Option<&str> {
    value.get("params")?.get("sessionId")?.as_str()
}

async fn flush_prompt_buffer<W>(
    bridge_stdout: &Arc<Mutex<BufWriter<W>>>,
    prompt: &mut PromptBuffer,
    terminal_raw: &str,
) -> Result<(), DynError>
where
    W: AsyncWrite + Unpin,
{
    let lines = drain_frame_lines(&prompt.frames).await?;
    // Buffered lines are valid JSON by construction (only parsed frames are
    // buffered); a parse failure here is spool corruption and aborts the turn
    // rather than releasing a partially transformed completion (§3.5).
    let mut frames = lines
        .into_iter()
        .map(|raw| {
            let value = serde_json::from_str::<Value>(&raw)
                .map_err(|error| format!("attested-prefix buffered frame corrupted: {error}"))?;
            Ok((raw, value))
        })
        .collect::<Result<Vec<(String, Value)>, DynError>>()?;
    let mut text_positions = Vec::new();
    let mut text_chunks = Vec::new();
    for (idx, (_, frame)) in frames.iter().enumerate() {
        if let Some(chunk) = agent_text_chunk(frame) {
            text_positions.push(idx);
            text_chunks.push(chunk.to_string());
        }
    }

    let chunk_refs = text_chunks.iter().map(String::as_str).collect::<Vec<_>>();
    let MarkerFrameResolution {
        chunks,
        body,
        status,
        prefix_bytes,
    } = if prompt.turn.enabled {
        resolve_marker_chunks(&chunk_refs, &prompt.turn.marker)
    } else {
        // §4.3: with `enabled: false` the wrapper performs no marker
        // recognition; every frame replays untouched below.
        let chunks = text_chunks
            .iter()
            .map(|chunk| chunk.as_bytes().to_vec())
            .collect::<Vec<_>>();
        let body = chunks.iter().flatten().copied().collect::<Vec<_>>();
        MarkerFrameResolution {
            chunks,
            body,
            status: WrapperStatus::Unavailable("sanitization_not_requested"),
            prefix_bytes: 0,
        }
    };

    for ((frame_idx, resolved), original) in
        text_positions.iter().zip(chunks).zip(text_chunks.iter())
    {
        // Targeted text transformation (§4.2): rewrite and re-serialize a
        // frame only when marker resolution actually changed its text bytes;
        // an untouched frame keeps its exact wire line.
        if resolved != original.as_bytes() {
            let (raw, value) = &mut frames[*frame_idx];
            set_agent_text_chunk(value, String::from_utf8(resolved)?);
            *raw = serde_json::to_string(value)?;
        }
    }

    let resolution = MarkerResolution {
        body,
        status,
        prefix_bytes,
    };
    for (raw, _) in &frames {
        write_raw_line(bridge_stdout, raw).await?;
    }
    write_json_line(bridge_stdout, control_frame(prompt, &resolution)).await?;
    write_raw_line(bridge_stdout, terminal_raw).await?;
    Ok(())
}

fn control_frame(prompt: &PromptBuffer, resolution: &MarkerResolution) -> Value {
    let meta_obj = match &resolution.status {
        WrapperStatus::Attested => {
            let sha = digest::digest(&digest::SHA256, &resolution.body);
            json!({
                "schema_version": 1,
                "kind": "attested",
                "issuer_id": ISSUER_ID,
                "turn_id": prompt.turn.turn_id.as_str(),
                "marker_nonce": prompt.turn.marker_nonce.as_str(),
                "process_prefix_bytes": resolution.prefix_bytes.to_string(),
                "body_len_bytes": resolution.body.len().to_string(),
                "body_sha256": hex_lower(sha.as_ref()),
            })
        }
        WrapperStatus::Unavailable(reason) => json!({
            "schema_version": 1,
            "kind": "unavailable",
            "issuer_id": ISSUER_ID,
            "turn_id": prompt.turn.turn_id.as_str(),
            "marker_nonce": prompt.turn.marker_nonce.as_str(),
            "reason": reason,
        }),
    };
    let mut reserved = Map::new();
    reserved.insert(META_KEY.to_string(), meta_obj);
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": prompt.session_id.as_str(),
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "messageId": format!("_b2a_apc_control/{}", prompt.turn.turn_id),
                "content": { "type": "text", "text": "" },
                "_meta": Value::Object(reserved)
            }
        }
    })
}

fn agent_text_chunk(value: &Value) -> Option<&str> {
    let update = value.get("params")?.get("update")?;
    if update.get("sessionUpdate")?.as_str()? != "agent_message_chunk" {
        return None;
    }
    let content = update.get("content")?;
    if content.get("type")?.as_str()? != "text" {
        return None;
    }
    content.get("text")?.as_str()
}

fn set_agent_text_chunk(value: &mut Value, text: String) {
    if let Some(slot) = value
        .get_mut("params")
        .and_then(|params| params.get_mut("update"))
        .and_then(|update| update.get_mut("content"))
        .and_then(|content| content.get_mut("text"))
    {
        *slot = Value::String(text);
    }
}

fn is_prompt_terminal(value: &Value, id: &Value) -> bool {
    value.get("id") == Some(id) && (value.get("result").is_some() || value.get("error").is_some())
}

fn is_successful_prompt_terminal(value: &Value, id: &Value) -> bool {
    value.get("id") == Some(id)
        && value
            .get("result")
            .and_then(|result| result.get("stopReason"))
            .and_then(Value::as_str)
            .is_some()
}

fn should_buffer_prompt_frame(value: &Value) -> bool {
    value.get("method").and_then(Value::as_str) == Some("session/update")
        && value.get("id").is_none()
}

/// Removes every child-supplied reserved `_meta` key (§4.2). Returns whether
/// anything was removed so the caller knows the original wire line no longer
/// matches the value.
fn strip_reserved_meta(value: &mut Value) -> bool {
    let mut removed = false;
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(meta)) = object.get_mut("_meta") {
                removed |= meta.remove(META_KEY).is_some();
            }
            for child in object.values_mut() {
                removed |= strip_reserved_meta(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                removed |= strip_reserved_meta(item);
            }
        }
        _ => {}
    }
    removed
}

async fn write_json_line<W>(writer: &Arc<Mutex<BufWriter<W>>>, value: Value) -> Result<(), DynError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut writer = writer.lock().await;
    writer
        .write_all(serde_json::to_string(&value)?.as_bytes())
        .await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn write_raw_line<W>(writer: &Arc<Mutex<BufWriter<W>>>, line: &str) -> Result<(), DynError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut writer = writer.lock().await;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn valid_turn_id(raw: &str) -> bool {
    raw.len() == 37
        && raw.starts_with("turn_")
        && raw[5..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_nonce(raw: &str) -> bool {
    raw.len() == 32
        && raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapperStatus {
    Attested,
    Unavailable(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkerResolution {
    body: Vec<u8>,
    status: WrapperStatus,
    prefix_bytes: u64,
}

struct MarkerFrameResolution {
    chunks: Vec<Vec<u8>>,
    body: Vec<u8>,
    status: WrapperStatus,
    prefix_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Bytes(Vec<(usize, u8)>),
    Candidate(Vec<usize>),
}

/// Single-chunk convenience over [`resolve_marker_chunks`], used by the marker
/// grammar test table (production flush always goes through the chunked path).
#[cfg(test)]
fn resolve_marker_text(input: &str, marker: &str) -> MarkerResolution {
    let resolved = resolve_marker_chunks(&[input], marker);
    MarkerResolution {
        body: resolved.body,
        status: resolved.status,
        prefix_bytes: resolved.prefix_bytes,
    }
}

fn resolve_marker_chunks(chunks: &[&str], marker: &str) -> MarkerFrameResolution {
    let marker_bytes = marker.as_bytes();
    let mut bytes = Vec::new();
    let mut source = Vec::new();
    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        for byte in chunk.as_bytes() {
            bytes.push(*byte);
            source.push(chunk_idx);
        }
    }

    let mut pieces = Vec::new();
    let mut pending = Vec::new();
    let mut candidates = 0_usize;
    let mut prefix_for_first = 0_u64;
    let mut decoded_len = 0_u64;
    let mut i = 0_usize;
    while i < bytes.len() {
        if bytes[i..].starts_with(marker_bytes) {
            let slash_run = trailing_pending_backslashes(&pending);
            let keep_len = pending.len().saturating_sub(slash_run);
            let slash_sources = pending[keep_len..]
                .iter()
                .map(|(source_idx, _)| *source_idx)
                .collect::<Vec<_>>();
            pending.truncate(keep_len);
            if slash_run % 2 == 1 {
                for source_idx in slash_sources.iter().take((slash_run - 1) / 2) {
                    pending.push((*source_idx, b'\\'));
                }
                for offset in 0..marker_bytes.len() {
                    pending.push((source[i + offset], marker_bytes[offset]));
                }
                i += marker_bytes.len();
                continue;
            }

            for source_idx in slash_sources.iter().take(slash_run / 2) {
                pending.push((*source_idx, b'\\'));
            }
            if !pending.is_empty() {
                decoded_len += pending.len() as u64;
                pieces.push(Piece::Bytes(std::mem::take(&mut pending)));
            }
            if candidates == 0 {
                prefix_for_first = decoded_len;
            }
            candidates += 1;
            pieces.push(Piece::Candidate(source[i..i + marker_bytes.len()].to_vec()));
            i += marker_bytes.len();
        } else {
            pending.push((source[i], bytes[i]));
            i += 1;
        }
    }
    if !pending.is_empty() {
        pieces.push(Piece::Bytes(pending));
    }

    match candidates {
        0 => {
            let (chunks, body) = materialize(&pieces, marker_bytes, chunks.len(), true);
            MarkerFrameResolution {
                chunks,
                body,
                status: WrapperStatus::Unavailable("turn_missing_deliverable_boundary"),
                prefix_bytes: 0,
            }
        }
        1 => {
            let (chunks, body) = materialize(&pieces, marker_bytes, chunks.len(), false);
            let suffix_len = body.len().saturating_sub(prefix_for_first as usize);
            if suffix_len == 0 {
                let (chunks, body) = materialize(&pieces, marker_bytes, chunks.len(), true);
                MarkerFrameResolution {
                    chunks,
                    body,
                    status: WrapperStatus::Unavailable("turn_ended_without_deliverable"),
                    prefix_bytes: 0,
                }
            } else {
                MarkerFrameResolution {
                    chunks,
                    body,
                    status: WrapperStatus::Attested,
                    prefix_bytes: prefix_for_first,
                }
            }
        }
        _ => {
            let (chunks, body) = materialize(&pieces, marker_bytes, chunks.len(), true);
            MarkerFrameResolution {
                chunks,
                body,
                status: WrapperStatus::Unavailable("multiple_commit_markers"),
                prefix_bytes: 0,
            }
        }
    }
}

fn trailing_pending_backslashes(bytes: &[(usize, u8)]) -> usize {
    bytes.iter().rev().take_while(|(_, b)| *b == b'\\').count()
}

fn materialize(
    pieces: &[Piece],
    marker: &[u8],
    chunk_count: usize,
    include_candidates: bool,
) -> (Vec<Vec<u8>>, Vec<u8>) {
    let mut chunks = vec![Vec::new(); chunk_count];
    let mut body = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Bytes(bytes) => {
                for (source_idx, byte) in bytes {
                    chunks[*source_idx].push(*byte);
                    body.push(*byte);
                }
            }
            Piece::Candidate(sources) if include_candidates => {
                for (offset, byte) in marker.iter().enumerate() {
                    let source_idx = sources
                        .get(offset)
                        .copied()
                        .or_else(|| sources.first().copied())
                        .unwrap_or(0);
                    if let Some(chunk) = chunks.get_mut(source_idx) {
                        chunk.push(*byte);
                    }
                    body.push(*byte);
                }
            }
            Piece::Candidate(_) => {}
        }
    }
    (chunks, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "6f0d8e2b7c9145a1b3d74f26e8c0aa59";

    fn marker() -> String {
        format!("{MARKER_PREFIX}{NONCE}{MARKER_SUFFIX}")
    }

    fn resolve(input: &str) -> MarkerResolution {
        resolve_marker_text(input, &marker())
    }

    #[test]
    fn parse_args_requires_explicit_absolute_child() {
        assert!(parse_args(Vec::new()).is_err());
        assert!(parse_args(vec!["--codex-acp".into(), "codex-acp".into()]).is_err());
        let absolute = if cfg!(windows) {
            "C:\\bin\\codex-acp.exe"
        } else {
            "/usr/local/bin/codex-acp"
        };
        assert_eq!(
            parse_args(vec![
                "--codex-acp".into(),
                absolute.into(),
                "--".into(),
                "--model".into(),
                "gpt".into(),
            ])
            .unwrap(),
            (
                absolute.to_string(),
                vec!["--model".to_string(), "gpt".to_string()]
            )
        );
    }

    #[test]
    fn unique_marker_attests_and_removes_marker() {
        let out = resolve(&format!("process {}deliverable", marker()));
        assert_eq!(out.status, WrapperStatus::Attested);
        assert_eq!(out.prefix_bytes, 8);
        assert_eq!(String::from_utf8(out.body).unwrap(), "process deliverable");
    }

    #[test]
    fn zero_multiple_and_empty_suffix_keep_candidate_text() {
        assert_eq!(
            resolve("no marker").status,
            WrapperStatus::Unavailable("turn_missing_deliverable_boundary")
        );
        let multiple = resolve(&format!("a {}b {}c", marker(), marker()));
        assert_eq!(
            multiple.status,
            WrapperStatus::Unavailable("multiple_commit_markers")
        );
        assert_eq!(
            String::from_utf8(multiple.body).unwrap(),
            format!("a {}b {}c", marker(), marker())
        );
        let empty = resolve(&format!("a {}", marker()));
        assert_eq!(
            empty.status,
            WrapperStatus::Unavailable("turn_ended_without_deliverable")
        );
        assert_eq!(
            String::from_utf8(empty.body).unwrap(),
            format!("a {}", marker())
        );
    }

    #[test]
    fn backslash_parity_decodes_literal_and_commit_forms() {
        let m = marker();
        assert_eq!(
            String::from_utf8(resolve(&format!(r"\{m}")).unwrap_body()).unwrap(),
            m
        );
        assert_eq!(
            String::from_utf8(resolve(&format!(r"\\\{m}")).unwrap_body()).unwrap(),
            format!(r"\{m}")
        );
        let committed = resolve(&format!(r"\\{m}x"));
        assert_eq!(committed.status, WrapperStatus::Attested);
        assert_eq!(String::from_utf8(committed.body).unwrap(), r"\x");
    }

    #[test]
    fn backslash_runs_zero_through_nine_follow_parity_table() {
        // §3.3 grammar over the full spec test-plan range (§15.1: "Backslash
        // runs of length 0 through at least 9"): even runs of length 2r decode
        // to r literal backslashes plus a committing marker; odd runs of
        // length 2r+1 decode to r literal backslashes plus the literal marker
        // as data (no candidate).
        let m = marker();
        for run in 0..=9_usize {
            let wire = format!("p {}{m}d", "\\".repeat(run));
            let out = resolve(&wire);
            if run % 2 == 0 {
                let expected_prefix = format!("p {}", "\\".repeat(run / 2));
                assert_eq!(out.status, WrapperStatus::Attested, "run {run}");
                assert_eq!(out.prefix_bytes, expected_prefix.len() as u64, "run {run}");
                assert_eq!(
                    String::from_utf8(out.body).unwrap(),
                    format!("{expected_prefix}d"),
                    "run {run}"
                );
            } else {
                assert_eq!(
                    out.status,
                    WrapperStatus::Unavailable("turn_missing_deliverable_boundary"),
                    "run {run}"
                );
                assert_eq!(
                    String::from_utf8(out.body).unwrap(),
                    format!("p {}{m}d", "\\".repeat((run - 1) / 2)),
                    "run {run}"
                );
            }
        }
    }

    #[test]
    fn marker_inside_code_fence_attests_because_fences_have_no_authority() {
        // §13 + §17 condition 16: fence/Markdown context carries no authority
        // and is never consulted — a unique unescaped marker commits even
        // inside backtick or tilde fences.
        let m = marker();
        let input = format!("before\n```\n{m}\n```\nafter");
        let out = resolve(&input);
        assert_eq!(out.status, WrapperStatus::Attested);
        assert_eq!(out.prefix_bytes, "before\n```\n".len() as u64);
        assert_eq!(
            String::from_utf8(out.body).unwrap(),
            "before\n```\n\n```\nafter"
        );

        let tilde = format!("~~~rust\n{m}deliverable\n~~~");
        let out = resolve(&tilde);
        assert_eq!(out.status, WrapperStatus::Attested);
        assert_eq!(out.prefix_bytes, "~~~rust\n".len() as u64);
        assert_eq!(
            String::from_utf8(out.body).unwrap(),
            "~~~rust\ndeliverable\n~~~"
        );
    }

    #[test]
    fn escaped_marker_is_data_everywhere_including_fences() {
        // The other direction of §17.16: backslash parity is the ONLY way to
        // quote a marker as data. An escaped marker stays literal data whether
        // or not it sits inside a code fence, and its presence never blocks or
        // moves a genuine unescaped commit elsewhere in the stream.
        let m = marker();
        let fenced = format!("process\n```text\n\\{m}\n```\ntail");
        let out = resolve(&fenced);
        assert_eq!(
            out.status,
            WrapperStatus::Unavailable("turn_missing_deliverable_boundary")
        );
        assert_eq!(
            String::from_utf8(out.body).unwrap(),
            format!("process\n```text\n{m}\n```\ntail")
        );

        let mixed = format!("doc:\n```\n\\{m}\n```\n{m}deliverable body");
        let out = resolve(&mixed);
        assert_eq!(out.status, WrapperStatus::Attested);
        let expected_prefix = format!("doc:\n```\n{m}\n```\n");
        assert_eq!(out.prefix_bytes, expected_prefix.len() as u64);
        assert_eq!(
            String::from_utf8(out.body).unwrap(),
            format!("{expected_prefix}deliverable body")
        );
    }

    #[test]
    fn wrong_nonce_is_ordinary_text() {
        let wrong = "<|b2a_apc_commit_v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa|>";
        let out = resolve(wrong);
        assert_eq!(
            out.status,
            WrapperStatus::Unavailable("turn_missing_deliverable_boundary")
        );
        assert_eq!(String::from_utf8(out.body).unwrap(), wrong);
    }

    #[tokio::test]
    async fn every_marker_split_position_flushes_like_one_text_stream() {
        let m = marker();
        for split in 0..=m.len() {
            let mut prompt = prompt_with_frames(vec![
                text_frame("a", format!("αβ{}", &m[..split])),
                text_frame("b", format!("{}γδ", &m[split..])),
            ]);
            let frames = flush_values(&mut prompt).await;
            assert_eq!(agent_text_chunk(&frames[0]), Some("αβ"), "split {split}");
            assert_eq!(agent_text_chunk(&frames[1]), Some("γδ"), "split {split}");
            let control = &frames[2]["params"]["update"]["_meta"][META_KEY];
            assert_eq!(control["kind"], "attested", "split {split}");
            assert_eq!(control["body_len_bytes"], "8", "split {split}");
            assert_eq!(
                control["body_sha256"],
                json!(hex_lower(
                    digest::digest(&digest::SHA256, "αβγδ".as_bytes()).as_ref()
                )),
                "split {split}"
            );
            assert_eq!(
                frames[3]["result"]["stopReason"], "end_turn",
                "split {split}"
            );
        }
    }

    #[tokio::test]
    async fn flush_preserves_non_text_frame_order_around_marker_strip() {
        let mut prompt = prompt_with_frames(vec![
            text_frame("before", "pre"),
            non_text_update_frame("tool-call"),
            text_frame("after", format!("{}post", marker())),
        ]);
        let frames = flush_values(&mut prompt).await;
        assert_eq!(agent_text_chunk(&frames[0]), Some("pre"));
        assert_eq!(frames[1]["params"]["update"]["sessionUpdate"], "tool_call");
        assert_eq!(agent_text_chunk(&frames[2]), Some("post"));
        assert_eq!(
            frames[3]["params"]["update"]["_meta"][META_KEY]["kind"],
            "attested"
        );
    }

    /// Runs the child-to-bridge proxy loop over raw input lines and returns
    /// the exact emitted output lines.
    async fn proxy_output_for(state: Arc<Mutex<WrapperState>>, input: &str) -> Vec<String> {
        let writer = std::sync::Arc::new(Mutex::new(BufWriter::new(Vec::<u8>::new())));
        proxy_child_lines(state, &writer, BufReader::new(input.as_bytes()))
            .await
            .unwrap();
        let bytes = writer.lock().await.get_ref().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(ToOwned::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn passthrough_preserves_exact_bytes_for_untouched_frames() {
        // §4.2 golden: with no active prompt buffer, every parseable frame the
        // wrapper does not transform must be forwarded byte-for-byte — odd
        // whitespace, member order, non-canonical numbers (1.50, 1e3), unicode
        // escapes, and u64-boundary integers must all survive; a serde_json
        // round-trip would rewrite each of them.
        let untouched = [
            "{ \"jsonrpc\" : \"2.0\",\t\"method\":\"session/update\" , \"params\":{\"update\":{\"progress\":1.50,\"count\":1e3},\"sessionId\":\"s\"}}",
            r#"{"method":"session/update","jsonrpc":"2.0","params":{"sessionId":"s","update":{"sessionUpdate":"tool_call","big":18446744073709551615,"name":"A"}}}"#,
            r#"{"jsonrpc":"2.0","id":41,"result":{"ok":true,"elapsed":0.100}}"#,
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"no turn active"}}}}"#,
            "not json at all",
        ];
        let output = proxy_output_for(
            Arc::new(Mutex::new(WrapperState::default())),
            &format!("{}\n", untouched.join("\n")),
        )
        .await;
        assert_eq!(output, untouched, "narrow proxy must not re-serialize");
    }

    #[tokio::test]
    async fn passthrough_reserialization_is_limited_to_reserved_meta_removal() {
        let forged = format!(
            "{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"s\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"x\"}},\"_meta\":{{\"{META_KEY}\":{{\"kind\":\"attested\"}},\"keep\":1.50}}}}}}}}",
        );
        let plain = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"tool_call","weird":  1e2}}}"#;
        let output = proxy_output_for(
            Arc::new(Mutex::new(WrapperState::default())),
            &format!("{forged}\n{plain}\n"),
        )
        .await;
        let stripped: Value = serde_json::from_str(&output[0]).unwrap();
        assert!(
            stripped["params"]["update"]["_meta"]
                .get(META_KEY)
                .is_none(),
            "child-supplied reserved metadata must be removed"
        );
        assert!(
            stripped["params"]["update"]["_meta"].get("keep").is_some(),
            "non-reserved metadata survives the strip"
        );
        // The frame that needed no strip is still byte-exact.
        assert_eq!(output[1], plain);
    }

    #[tokio::test]
    async fn flush_replays_untouched_buffered_frames_byte_for_byte() {
        // Golden: buffered frames the marker transformation does not touch —
        // non-text session updates and text frames without marker bytes —
        // replay with their exact original bytes (odd whitespace and
        // non-canonical numbers included); only the frame whose text actually
        // changed is re-serialized.
        let odd_tool_call = "{ \"jsonrpc\":\"2.0\" ,\"method\":\"session/update\",\"params\":{\"sessionId\":\"s\",\"update\":{\"sessionUpdate\":\"tool_call\",\"progress\":1.50,\"count\":1e3}}}".to_string();
        let untouched_text = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{ "type":"text","text":"pre"}}}}"#.to_string();
        let marked_text =
            serde_json::to_string(&text_frame("after", format!("{}post", marker()))).unwrap();
        let mut prompt = prompt_with_lines(vec![
            untouched_text.clone(),
            odd_tool_call.clone(),
            marked_text.clone(),
        ]);
        let lines = flush_lines(&mut prompt).await;
        assert_eq!(lines[0], untouched_text, "untouched text frame replays raw");
        assert_eq!(lines[1], odd_tool_call, "non-text frame replays raw");
        assert_ne!(lines[2], marked_text, "marker frame is rewritten");
        let rewritten: Value = serde_json::from_str(&lines[2]).unwrap();
        assert_eq!(agent_text_chunk(&rewritten), Some("post"));
        let control: Value = serde_json::from_str(&lines[3]).unwrap();
        assert_eq!(
            control["params"]["update"]["_meta"][META_KEY]["kind"], "attested",
            "wrapper-owned control frame follows the buffered frames"
        );
        assert_eq!(lines[4], TERMINAL_RAW, "terminal frame replays raw");
    }

    #[tokio::test]
    async fn spool_overflow_preserves_buffered_frame_bytes() {
        // Push enough oversized lines to cross FRAME_MEMORY_LIMIT_BYTES so the
        // buffer migrates to the spool file, then confirm the round-trip
        // returns the exact original lines.
        let mut buffer = FrameBuffer::new();
        let big = format!(
            "{{ \"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"s\",\"update\":{{\"sessionUpdate\":\"tool_call\",\"pad\":\"{}\",\"n\":1.50}}}}}}",
            "x".repeat(300 * 1024)
        );
        let lines = vec![big.clone(), big.clone(), big.clone(), big.clone(), big];
        for line in &lines {
            buffer.push_line(line.clone()).unwrap();
        }
        assert!(buffer.spool.is_some(), "test must exercise the spool path");
        assert_eq!(buffer.into_lines().unwrap(), lines);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn terminal_flush_waits_for_inflight_spool_push() {
        let mut prompt = prompt_with_lines(vec![]);
        let big = format!(
            "{{ \"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"s\",\"update\":{{\"sessionUpdate\":\"tool_call\",\"pad\":\"{}\",\"n\":1.50}}}}}}",
            "x".repeat(FRAME_MEMORY_LIMIT_BYTES + 1)
        );
        let frames = Arc::clone(&prompt.frames);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let line = big.clone();
        let writer = std::sync::Arc::new(Mutex::new(BufWriter::new(Vec::<u8>::new())));
        let handle = tokio::task::spawn_blocking(move || {
            let mut guard = frames.blocking_lock();
            guard.push_line(line).unwrap();
            assert!(guard.spool.is_some(), "test must exercise the spool path");
            entered_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(25));
        });
        tokio::task::spawn_blocking(move || {
            entered_rx.recv_timeout(std::time::Duration::from_secs(1))
        })
        .await
        .unwrap()
        .unwrap();

        flush_prompt_buffer(&writer, &mut prompt, TERMINAL_RAW)
            .await
            .unwrap();
        handle.await.unwrap();
        let raw = String::from_utf8(writer.lock().await.get_ref().clone()).unwrap();
        let lines = raw.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
        assert_eq!(lines.first(), Some(&big));
        assert_eq!(lines.last().map(String::as_str), Some(TERMINAL_RAW));
    }

    fn text_frame(message_id: &str, text: impl Into<String>) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": message_id,
                    "content": { "type": "text", "text": text.into() }
                }
            }
        })
    }

    fn non_text_update_frame(message_id: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "tool_call",
                    "messageId": message_id,
                    "toolCallId": "tool-1"
                }
            }
        })
    }

    fn prompt_with_frames(frames: Vec<Value>) -> PromptBuffer {
        prompt_with_lines(
            frames
                .into_iter()
                .map(|frame| serde_json::to_string(&frame).unwrap())
                .collect(),
        )
    }

    fn prompt_with_lines(lines: Vec<String>) -> PromptBuffer {
        let mut buffer = FrameBuffer::new();
        for line in lines {
            buffer.push_line(line).unwrap();
        }
        PromptBuffer {
            request_id: json!(7),
            session_id: "s".into(),
            turn: TurnConfig {
                session_id: "s".into(),
                turn_id: format!("turn_{NONCE}"),
                marker_nonce: NONCE.into(),
                marker: marker(),
                enabled: true,
            },
            frames: Arc::new(Mutex::new(buffer)),
        }
    }

    const TERMINAL_RAW: &str =
        r#"{"jsonrpc": "2.0", "id": 7, "result": {"stopReason": "end_turn"}}"#;

    async fn flush_lines(prompt: &mut PromptBuffer) -> Vec<String> {
        let writer = std::sync::Arc::new(Mutex::new(BufWriter::new(Vec::<u8>::new())));
        flush_prompt_buffer(&writer, prompt, TERMINAL_RAW)
            .await
            .unwrap();
        let bytes = writer.lock().await.get_ref().clone();
        let raw = String::from_utf8(bytes).unwrap();
        raw.lines().map(ToOwned::to_owned).collect()
    }

    async fn flush_values(prompt: &mut PromptBuffer) -> Vec<Value> {
        flush_lines(prompt)
            .await
            .iter()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    #[test]
    fn prompt_buffer_only_buffers_session_update_notifications() {
        assert!(should_buffer_prompt_frame(&text_frame("m", "x")));
        assert!(!should_buffer_prompt_frame(&json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "session/request_permission",
            "params": {}
        })));
        assert!(!should_buffer_prompt_frame(&json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "session/update",
            "params": {}
        })));
    }

    #[test]
    fn begin_turn_param_validation_is_strict() {
        let good = json!({
            "schema_version": 1,
            "session_id": "s",
            "turn_id": format!("turn_{NONCE}"),
            "enabled": true,
            "marker_nonce": NONCE,
        });
        assert!(parse_begin_turn_params(good.as_object().unwrap()).is_some());

        let mut bad_schema = good.clone();
        bad_schema["schema_version"] = json!(2);
        assert!(parse_begin_turn_params(bad_schema.as_object().unwrap()).is_none());

        let mut bad_turn = good.clone();
        bad_turn["turn_id"] = json!(format!("turn-{NONCE}"));
        assert!(parse_begin_turn_params(bad_turn.as_object().unwrap()).is_none());

        let mut bad_nonce = good;
        bad_nonce["marker_nonce"] = json!("ABCDEF0123456789ABCDEF0123456789");
        assert!(parse_begin_turn_params(bad_nonce.as_object().unwrap()).is_none());
    }

    #[tokio::test]
    async fn prompt_without_begin_turn_does_not_start_buffering() {
        let state = std::sync::Arc::new(Mutex::new(WrapperState::default()));
        maybe_start_prompt_buffer(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "session/prompt",
                "params": {"sessionId": "s"}
            }),
        )
        .await;
        let guard = state.lock().await;
        assert!(guard.sessions.is_empty());
    }

    #[tokio::test]
    async fn begin_turn_rejects_malformed_and_overlapping_turns() {
        let state = std::sync::Arc::new(Mutex::new(WrapperState::default()));
        let malformed = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": BEGIN_TURN_METHOD,
            "params": {"schema_version": 2}
        });
        let response = handle_begin_turn(&state, &malformed).await;
        assert_eq!(response["error"]["code"], -32602);

        let valid = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": BEGIN_TURN_METHOD,
            "params": {
                "schema_version": 1,
                "session_id": "s",
                "turn_id": format!("turn_{NONCE}"),
                "enabled": true,
                "marker_nonce": NONCE,
            }
        });
        let accepted = handle_begin_turn(&state, &valid).await;
        assert_eq!(accepted["result"]["accepted"], true);
        let overlapping = handle_begin_turn(&state, &valid).await;
        assert_eq!(overlapping["error"]["code"], -32000);
    }

    #[tokio::test]
    async fn begin_turn_state_is_isolated_by_session() {
        let state = std::sync::Arc::new(Mutex::new(WrapperState::default()));
        let other_nonce = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        for (id, session_id, nonce) in [(1, "s1", NONCE), (2, "s2", other_nonce)] {
            let response = handle_begin_turn(
                &state,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": BEGIN_TURN_METHOD,
                    "params": {
                        "schema_version": 1,
                        "session_id": session_id,
                        "turn_id": format!("turn_{nonce}"),
                        "enabled": true,
                        "marker_nonce": nonce,
                    }
                }),
            )
            .await;
            assert_eq!(response["result"]["accepted"], true);
        }

        maybe_start_prompt_buffer(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "session/prompt",
                "params": {"sessionId": "s2"}
            }),
        )
        .await;
        let guard = state.lock().await;
        assert!(guard
            .sessions
            .get("s1")
            .and_then(|session| session.active_turn.as_ref())
            .is_some());
        assert!(guard
            .sessions
            .get("s2")
            .and_then(|session| session.prompt.as_ref())
            .is_some());
    }

    #[test]
    fn control_frame_uses_reserved_meta_key_value() {
        let prompt = PromptBuffer {
            request_id: json!(7),
            session_id: "s".into(),
            turn: TurnConfig {
                session_id: "s".into(),
                turn_id: format!("turn_{NONCE}"),
                marker_nonce: NONCE.into(),
                marker: marker(),
                enabled: true,
            },
            frames: Arc::new(Mutex::new(FrameBuffer::new())),
        };
        let frame = control_frame(
            &prompt,
            &MarkerResolution {
                body: b"deliverable".to_vec(),
                status: WrapperStatus::Attested,
                prefix_bytes: 3,
            },
        );
        let meta = &frame["params"]["update"]["_meta"];
        assert!(
            meta.get(META_KEY).is_some(),
            "reserved key missing: {meta:?}"
        );
        assert_eq!(meta[META_KEY]["kind"], "attested");
        assert_eq!(meta[META_KEY]["process_prefix_bytes"], "3");
    }

    #[test]
    fn child_reserved_metadata_is_stripped_recursively() {
        let mut frame = json!({
            "params": {
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "x"},
                    "_meta": { "dev.b2a.attested_prefix": {"kind": "attested"}, "keep": true },
                    "nested": {"_meta": { "dev.b2a.attested_prefix": {"kind": "attested"} }}
                }
            }
        });
        assert!(strip_reserved_meta(&mut frame));
        assert!(frame["params"]["update"]["_meta"].get(META_KEY).is_none());
        assert_eq!(frame["params"]["update"]["_meta"]["keep"], true);
        assert!(frame["params"]["update"]["nested"]["_meta"]
            .get(META_KEY)
            .is_none());
        assert!(
            !strip_reserved_meta(&mut frame),
            "second pass must report nothing removed"
        );
    }

    trait BodyExt {
        fn unwrap_body(self) -> Vec<u8>;
    }

    impl BodyExt for MarkerResolution {
        fn unwrap_body(self) -> Vec<u8> {
            self.body
        }
    }
}
