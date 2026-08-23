You are a READ-ONLY agent. Do not modify anything, run no builds, make no network calls beyond your model.

Read `crates/bridge-worktree/src/custody.rs` in your current working directory and answer three questions
about the constant `LEGAL_CUSTODY_TRANSITIONS_V1`, which is a list of `(from_state, to_state)` pairs.

1. How many pairs does it contain?
2. Which states appear as a TARGET (second element) but NEVER as a SOURCE (first element)? These are the
   terminal states.
3. `WorktreeCustodyRecordV1` has no `source` field, and the state `UnusedSettled` maps to
   `ClaimPresenceV1::Forbidden`. Explain, in at most three sentences, why that combination means a record
   left stranded in `UnusedSettled` can never be authorized for removal by a later sweep.

Output EXACTLY this shape and nothing else, then STOP:

ROWS: <number>
TERMINAL: <comma-separated state names, alphabetical>
WHY: <your three sentences>

Task context (may be ignored if empty):

{{input}}
