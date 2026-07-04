use tokio::sync::mpsc::Receiver;

#[derive(Debug, PartialEq)]
pub enum TurnOutcome {
    Completed(String),
    Cancelled,
}

/// Drive one warm turn: accumulate the agent's streamed deltas, but a cancel
/// signal must win IMMEDIATELY (abort-first), even if a reply delta is ready at
/// the same time.
pub async fn run_turn(mut cancel: Receiver<()>, mut deltas: Receiver<String>) -> TurnOutcome {
    let mut acc = String::new();
    loop {
        tokio::select! {
            // `biased;` is intentional: cancellation must pre-empt draining
            // another delta, so we poll the cancel arm first every iteration. A
            // chatty agent could otherwise keep starving the cancel.
            biased;

            _ = cancel.recv() => return TurnOutcome::Cancelled,
            delta = deltas.recv() => match delta {
                Some(d) => acc.push_str(&d),
                None => return TurnOutcome::Completed(acc),
            },
        }
    }
}
