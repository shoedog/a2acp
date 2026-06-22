use bridge_core::domain::{InjectMode, Part, QueuedInject};

/// Assemble a turn's parts: optional summary seed, then PrependNextTurn injects (FIFO), then the
/// base parts (order preserved), then AppendNextTurn injects (FIFO). Pure + total.
pub fn assemble_turn_parts(
    seed: Option<&str>,
    injects: &[QueuedInject],
    base: Vec<Part>,
) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    if let Some(s) = seed {
        out.push(Part {
            text: format!("[Summary of earlier context in this session]\n{s}"),
        });
    }
    for inj in injects
        .iter()
        .filter(|i| i.mode == InjectMode::PrependNextTurn)
    {
        out.push(Part {
            text: format!("[Injected context]\n{}", inj.text),
        });
    }
    out.extend(base);
    for inj in injects
        .iter()
        .filter(|i| i.mode == InjectMode::AppendNextTurn)
    {
        out.push(Part {
            text: format!("[Injected context]\n{}", inj.text),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::assemble_turn_parts;
    use bridge_core::domain::{InjectMode, Part, QueuedInject};

    #[test]
    fn assemble_orders_seed_prepend_base_append() {
        let parts = assemble_turn_parts(
            Some("S"),
            &[
                QueuedInject {
                    text: "P".into(),
                    mode: InjectMode::PrependNextTurn,
                    dedupe_key: None,
                },
                QueuedInject {
                    text: "A".into(),
                    mode: InjectMode::AppendNextTurn,
                    dedupe_key: None,
                },
            ],
            vec![Part { text: "B".into() }],
        );

        let texts: Vec<_> = parts.into_iter().map(|p| p.text).collect();
        assert_eq!(
            texts,
            vec![
                "[Summary of earlier context in this session]\nS",
                "[Injected context]\nP",
                "B",
                "[Injected context]\nA",
            ]
        );
    }
}
