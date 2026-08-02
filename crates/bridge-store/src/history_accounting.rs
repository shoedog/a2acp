//! Physical accounting primitives for configured workflow history V2.
//!
//! The configured store deliberately accounts only roots owned by workflow
//! history. Unrelated primary-store pages are excluded; every owned B-tree,
//! including overflow pages and indexes, is measured through `dbstat` after
//! materialization. All mutable counters are exact-width big-endian blobs so a
//! later value change cannot enlarge the allocation row that was measured.

use bridge_core::workflow_history::{
    LedgerError, LedgerUnavailableReason as R, MAX_CHARGED_BYTES, MAX_TERMINAL_ROWS,
};
use rusqlite::OptionalExtension;

pub(crate) const ACCOUNTING_VERSION_V2: i64 = 2;
pub(crate) const ALLOCATION_KIND_CONFIGURED: i64 = 2;
pub(crate) const ALLOCATION_KIND_PLATFORM: i64 = 3;
pub(crate) const ALLOCATION_STATE_MIGRATING: i64 = 2;
pub(crate) const ALLOCATION_STATE_READY: i64 = 3;

pub(crate) const TICKET_STATE_RESERVED: i64 = 2;
pub(crate) const TICKET_STATE_WAL_DEBT: i64 = 3;
pub(crate) const TICKET_STATE_RETIRED: i64 = 4;

pub(crate) const MAINTENANCE_TICKET_OWNER: &str = "__maintenance__";
pub(crate) const OPERATOR_TICKET_OWNER: &str = "__operator__";

/// Allocation-owned tables. Indexes whose `sqlite_schema.tbl_name` belongs to
/// this roster are discovered and measured automatically.
pub(crate) const HISTORY_ALLOCATION_TABLES_V2: &[&str] = &[
    "workflow_attempt_summaries",
    "workflow_history_attachment",
    "workflow_history_rewrite_reserve",
    "workflow_attempt_node_terminals",
    "workflow_history_mutation_reserve",
    "workflow_history_allocation",
];

pub(crate) const CANONICAL_HISTORY_ALLOCATION_V2_DDL: &str = r#"
    CREATE TABLE workflow_history_allocation (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1),
        allocation_kind INTEGER NOT NULL CHECK(allocation_kind IN (2,3)),
        allocation_state INTEGER NOT NULL CHECK(allocation_state IN (2,3)),
        accounting_version INTEGER NOT NULL CHECK(accounting_version=2),
        history_page_bytes BLOB NOT NULL
            CHECK(typeof(history_page_bytes)='blob' AND length(history_page_bytes)=8),
        future_wal_reserve_bytes BLOB NOT NULL
            CHECK(typeof(future_wal_reserve_bytes)='blob' AND length(future_wal_reserve_bytes)=8),
        wal_debt_bytes BLOB NOT NULL
            CHECK(typeof(wal_debt_bytes)='blob' AND length(wal_debt_bytes)=8),
        maintenance_reserve_bytes BLOB NOT NULL
            CHECK(typeof(maintenance_reserve_bytes)='blob' AND length(maintenance_reserve_bytes)=8),
        transient_journal_reserve_bytes BLOB NOT NULL
            CHECK(typeof(transient_journal_reserve_bytes)='blob' AND length(transient_journal_reserve_bytes)=8),
        charged_bytes BLOB NOT NULL
            CHECK(typeof(charged_bytes)='blob' AND length(charged_bytes)=8),
        slots_used BLOB NOT NULL
            CHECK(typeof(slots_used)='blob' AND length(slots_used)=8),
        terminal_rows BLOB NOT NULL
            CHECK(typeof(terminal_rows)='blob' AND length(terminal_rows)=8),
        wal_epoch BLOB NOT NULL
            CHECK(typeof(wal_epoch)='blob' AND length(wal_epoch)=8)
    )"#;

pub(crate) const CANONICAL_HISTORY_MUTATION_RESERVE_DDL: &str = r#"
    CREATE TABLE workflow_history_mutation_reserve (
        attempt_id TEXT NOT NULL
            CHECK(length(attempt_id) BETWEEN 1 AND 64),
        mutation_kind INTEGER NOT NULL CHECK(mutation_kind BETWEEN 2 AND 12),
        ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4294967295),
        reserve BLOB NOT NULL CHECK(typeof(reserve)='blob' AND length(reserve)=8),
        state INTEGER NOT NULL CHECK(state IN (2,3,4)),
        PRIMARY KEY(attempt_id, mutation_kind, ordinal)
    ) WITHOUT ROWID"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub(crate) enum MutationKindV2 {
    Admission = 2,
    PromptAcceptance = 3,
    NodeTerminal = 4,
    TriggerBarrier = 5,
    AttemptTerminal = 6,
    CleanupSettlement = 7,
    FinalActivity = 8,
    BootReconciliation = 9,
    Retention = 10,
    PinChange = 11,
    Migration = 12,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MutationTicketV2 {
    pub(crate) attempt_id: String,
    pub(crate) kind: MutationKindV2,
    pub(crate) ordinal: u32,
    pub(crate) reserve: u64,
    pub(crate) state: i64,
}

pub(crate) fn read_mutation_tickets(
    conn: &rusqlite::Connection,
) -> Result<Vec<MutationTicketV2>, LedgerError> {
    let mut statement = conn
        .prepare(
            "SELECT attempt_id, mutation_kind, ordinal, reserve, state
             FROM workflow_history_mutation_reserve
             ORDER BY attempt_id, mutation_kind, ordinal",
        )
        .map_err(|error| sqlite_error(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| sqlite_error(&error))?;
    let mut tickets = Vec::new();
    for row in rows {
        let (attempt_id, kind, ordinal, reserve, state) =
            row.map_err(|error| sqlite_error(&error))?;
        let kind = MutationKindV2::parse(kind).ok_or_else(|| LedgerError::new(R::Corruption))?;
        let ordinal = u32::try_from(ordinal).map_err(|_| LedgerError::new(R::Corruption))?;
        if !matches!(
            state,
            TICKET_STATE_RESERVED | TICKET_STATE_WAL_DEBT | TICKET_STATE_RETIRED
        ) {
            return Err(LedgerError::new(R::Corruption));
        }
        tickets.push(MutationTicketV2 {
            attempt_id,
            kind,
            ordinal,
            reserve: decode_u64(reserve)?,
            state,
        });
    }
    Ok(tickets)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TicketComponentsV2 {
    pub(crate) future_wal_reserve_bytes: u64,
    pub(crate) wal_debt_bytes: u64,
    pub(crate) maintenance_reserve_bytes: u64,
    pub(crate) transient_journal_reserve_bytes: u64,
}

pub(crate) fn ticket_components(
    regime: JournalRegimeV2,
    tickets: &[MutationTicketV2],
) -> Result<TicketComponentsV2, LedgerError> {
    use std::collections::BTreeMap;

    let maintenance = tickets
        .iter()
        .filter(|ticket| ticket.attempt_id == MAINTENANCE_TICKET_OWNER)
        .collect::<Vec<_>>();
    let reserved_maintenance = maintenance
        .iter()
        .copied()
        .filter(|ticket| ticket.state == TICKET_STATE_RESERVED)
        .collect::<Vec<_>>();
    if maintenance.is_empty()
        || reserved_maintenance.len() != 1
        || maintenance
            .iter()
            .any(|ticket| ticket.kind != MutationKindV2::Retention || ticket.reserve == 0)
    {
        return Err(LedgerError::new(R::Corruption));
    }

    let mut future_wal = 0_u64;
    let mut wal_debt = 0_u64;
    let mut transient_journal = 0_u64;
    let mut settlement_alternatives: BTreeMap<&str, (Option<u64>, Option<u64>)> = BTreeMap::new();
    for ticket in tickets {
        if ticket.attempt_id == MAINTENANCE_TICKET_OWNER {
            match ticket.state {
                TICKET_STATE_RESERVED | TICKET_STATE_RETIRED => {}
                TICKET_STATE_WAL_DEBT if regime == JournalRegimeV2::Wal => {
                    wal_debt = wal_debt
                        .checked_add(ticket.reserve)
                        .ok_or_else(|| LedgerError::new(R::Corruption))?;
                }
                _ => return Err(LedgerError::new(R::Corruption)),
            }
            continue;
        }
        if ticket.reserve == 0 {
            return Err(LedgerError::new(R::Corruption));
        }
        match ticket.state {
            TICKET_STATE_RESERVED => {
                if matches!(
                    ticket.kind,
                    MutationKindV2::AttemptTerminal | MutationKindV2::BootReconciliation
                ) {
                    let entry = settlement_alternatives
                        .entry(ticket.attempt_id.as_str())
                        .or_default();
                    let target = if ticket.kind == MutationKindV2::AttemptTerminal {
                        &mut entry.0
                    } else {
                        &mut entry.1
                    };
                    if target.replace(ticket.reserve).is_some() {
                        return Err(LedgerError::new(R::Corruption));
                    }
                } else {
                    future_wal = future_wal
                        .checked_add(ticket.reserve)
                        .ok_or_else(|| LedgerError::new(R::Corruption))?;
                }
                transient_journal = transient_journal.max(ticket.reserve);
            }
            TICKET_STATE_WAL_DEBT => {
                if regime != JournalRegimeV2::Wal {
                    return Err(LedgerError::new(R::Corruption));
                }
                wal_debt = wal_debt
                    .checked_add(ticket.reserve)
                    .ok_or_else(|| LedgerError::new(R::Corruption))?;
            }
            TICKET_STATE_RETIRED => {}
            _ => return Err(LedgerError::new(R::Corruption)),
        }
    }
    for (_, (terminal, boot)) in settlement_alternatives {
        let reserve = terminal.into_iter().chain(boot).max().unwrap_or(0);
        future_wal = future_wal
            .checked_add(reserve)
            .ok_or_else(|| LedgerError::new(R::Corruption))?;
    }

    Ok(match regime {
        JournalRegimeV2::Wal => TicketComponentsV2 {
            future_wal_reserve_bytes: future_wal,
            wal_debt_bytes: wal_debt,
            maintenance_reserve_bytes: reserved_maintenance[0].reserve,
            transient_journal_reserve_bytes: 0,
        },
        JournalRegimeV2::Rollback => TicketComponentsV2 {
            future_wal_reserve_bytes: 0,
            wal_debt_bytes: 0,
            maintenance_reserve_bytes: reserved_maintenance[0].reserve,
            transient_journal_reserve_bytes: transient_journal,
        },
    })
}

impl MutationKindV2 {
    pub(crate) const fn code(self) -> i64 {
        self as i64
    }

    pub(crate) fn parse(value: i64) -> Option<Self> {
        match value {
            2 => Some(Self::Admission),
            3 => Some(Self::PromptAcceptance),
            4 => Some(Self::NodeTerminal),
            5 => Some(Self::TriggerBarrier),
            6 => Some(Self::AttemptTerminal),
            7 => Some(Self::CleanupSettlement),
            8 => Some(Self::FinalActivity),
            9 => Some(Self::BootReconciliation),
            10 => Some(Self::Retention),
            11 => Some(Self::PinChange),
            12 => Some(Self::Migration),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JournalRegimeV2 {
    Wal,
    Rollback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AllocationV2 {
    pub(crate) allocation_kind: i64,
    pub(crate) allocation_state: i64,
    pub(crate) history_page_bytes: u64,
    pub(crate) future_wal_reserve_bytes: u64,
    pub(crate) wal_debt_bytes: u64,
    pub(crate) maintenance_reserve_bytes: u64,
    pub(crate) transient_journal_reserve_bytes: u64,
    pub(crate) charged_bytes: u64,
    pub(crate) slots_used: u64,
    pub(crate) terminal_rows: u64,
    pub(crate) wal_epoch: u64,
}

impl AllocationV2 {
    pub(crate) fn checked_charge(&self) -> Result<u64, LedgerError> {
        self.history_page_bytes
            .checked_add(self.future_wal_reserve_bytes)
            .and_then(|value| value.checked_add(self.wal_debt_bytes))
            .and_then(|value| value.checked_add(self.maintenance_reserve_bytes))
            .and_then(|value| value.checked_add(self.transient_journal_reserve_bytes))
            .ok_or_else(|| LedgerError::new(R::Corruption))
    }

    pub(crate) fn validate_shape(&self, regime: JournalRegimeV2) -> Result<(), LedgerError> {
        if !matches!(
            self.allocation_kind,
            ALLOCATION_KIND_CONFIGURED | ALLOCATION_KIND_PLATFORM
        ) || !matches!(
            self.allocation_state,
            ALLOCATION_STATE_MIGRATING | ALLOCATION_STATE_READY
        ) || self.charged_bytes != self.checked_charge()?
            || self.charged_bytes > MAX_CHARGED_BYTES
            || self.slots_used > MAX_TERMINAL_ROWS
            || self.terminal_rows > self.slots_used
        {
            return Err(LedgerError::new(R::Corruption));
        }
        match regime {
            JournalRegimeV2::Wal if self.transient_journal_reserve_bytes != 0 => {
                Err(LedgerError::new(R::Corruption))
            }
            JournalRegimeV2::Rollback
                if self.future_wal_reserve_bytes != 0 || self.wal_debt_bytes != 0 =>
            {
                Err(LedgerError::new(R::Corruption))
            }
            _ => Ok(()),
        }
    }
}

pub(crate) fn encode_u64(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

fn decode_u64(value: Vec<u8>) -> Result<u64, LedgerError> {
    let bytes: [u8; 8] = value
        .try_into()
        .map_err(|_| LedgerError::new(R::Corruption))?;
    Ok(u64::from_be_bytes(bytes))
}

pub(crate) fn read_allocation_v2(
    conn: &rusqlite::Connection,
) -> Result<Option<AllocationV2>, LedgerError> {
    let row = conn
        .query_row(
            "SELECT allocation_kind, allocation_state, accounting_version,
                    history_page_bytes, future_wal_reserve_bytes, wal_debt_bytes,
                    maintenance_reserve_bytes, transient_journal_reserve_bytes,
                    charged_bytes, slots_used, terminal_rows, wal_epoch
             FROM workflow_history_allocation WHERE singleton=1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, Vec<u8>>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
                    row.get::<_, Vec<u8>>(8)?,
                    row.get::<_, Vec<u8>>(9)?,
                    row.get::<_, Vec<u8>>(10)?,
                    row.get::<_, Vec<u8>>(11)?,
                ))
            },
        )
        .optional()
        .map_err(|error| {
            if matches!(
                error,
                rusqlite::Error::FromSqlConversionFailure(_, _, _)
                    | rusqlite::Error::InvalidColumnType(_, _, _)
            ) {
                LedgerError::new(R::Corruption)
            } else {
                sqlite_error(&error)
            }
        })?;
    let Some((
        kind,
        state,
        version,
        pages,
        future,
        debt,
        maintenance,
        journal,
        charged,
        slots,
        terminals,
        epoch,
    )) = row
    else {
        return Ok(None);
    };
    if version != ACCOUNTING_VERSION_V2 {
        return Err(LedgerError::new(R::Corruption));
    }
    Ok(Some(AllocationV2 {
        allocation_kind: kind,
        allocation_state: state,
        history_page_bytes: decode_u64(pages)?,
        future_wal_reserve_bytes: decode_u64(future)?,
        wal_debt_bytes: decode_u64(debt)?,
        maintenance_reserve_bytes: decode_u64(maintenance)?,
        transient_journal_reserve_bytes: decode_u64(journal)?,
        charged_bytes: decode_u64(charged)?,
        slots_used: decode_u64(slots)?,
        terminal_rows: decode_u64(terminals)?,
        wal_epoch: decode_u64(epoch)?,
    }))
}

pub(crate) fn write_allocation_v2(
    conn: &rusqlite::Connection,
    allocation: &AllocationV2,
) -> Result<(), LedgerError> {
    let changed = conn
        .execute(
            "UPDATE workflow_history_allocation SET
                 allocation_kind=?1, allocation_state=?2,
                 history_page_bytes=?3, future_wal_reserve_bytes=?4,
                 wal_debt_bytes=?5, maintenance_reserve_bytes=?6,
                 transient_journal_reserve_bytes=?7, charged_bytes=?8,
                 slots_used=?9, terminal_rows=?10, wal_epoch=?11
             WHERE singleton=1 AND accounting_version=2",
            rusqlite::params![
                allocation.allocation_kind,
                allocation.allocation_state,
                encode_u64(allocation.history_page_bytes).as_slice(),
                encode_u64(allocation.future_wal_reserve_bytes).as_slice(),
                encode_u64(allocation.wal_debt_bytes).as_slice(),
                encode_u64(allocation.maintenance_reserve_bytes).as_slice(),
                encode_u64(allocation.transient_journal_reserve_bytes).as_slice(),
                encode_u64(allocation.charged_bytes).as_slice(),
                encode_u64(allocation.slots_used).as_slice(),
                encode_u64(allocation.terminal_rows).as_slice(),
                encode_u64(allocation.wal_epoch).as_slice(),
            ],
        )
        .map_err(|error| sqlite_error(&error))?;
    if changed != 1 {
        return Err(LedgerError::new(R::Corruption));
    }
    Ok(())
}

pub(crate) fn journal_regime(conn: &rusqlite::Connection) -> Result<JournalRegimeV2, LedgerError> {
    let mode: String = conn
        .query_row("PRAGMA main.journal_mode", [], |row| row.get(0))
        .map_err(|error| sqlite_error(&error))?;
    match mode.to_ascii_lowercase().as_str() {
        "wal" => Ok(JournalRegimeV2::Wal),
        "delete" | "truncate" | "persist" => Ok(JournalRegimeV2::Rollback),
        _ => Err(LedgerError::new(R::UnsupportedConfiguration)),
    }
}

pub(crate) fn validate_auto_vacuum(conn: &rusqlite::Connection) -> Result<(), LedgerError> {
    let mode: i64 = conn
        .query_row("PRAGMA main.auto_vacuum", [], |row| row.get(0))
        .map_err(|error| sqlite_error(&error))?;
    match mode {
        0 | 2 => Ok(()),
        _ => Err(LedgerError::new(R::UnsupportedConfiguration)),
    }
}

pub(crate) fn validate_only_main_writable_database(
    conn: &rusqlite::Connection,
) -> Result<(), LedgerError> {
    let mut statement = conn
        .prepare("PRAGMA database_list")
        .map_err(|error| sqlite_error(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .map_err(|error| sqlite_error(&error))?;
    for row in rows {
        let (name, path) = row.map_err(|error| sqlite_error(&error))?;
        if name != "main" && !path.is_empty() {
            return Err(LedgerError::new(R::UnsupportedConfiguration));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeasuredHistoryPagesV2 {
    pub(crate) pages: u64,
    pub(crate) bytes: u64,
    pub(crate) page_size: u64,
}

pub(crate) fn measure_history_pages(
    conn: &rusqlite::Connection,
) -> Result<MeasuredHistoryPagesV2, LedgerError> {
    let page_size: i64 = conn
        .query_row("PRAGMA main.page_size", [], |row| row.get(0))
        .map_err(|error| sqlite_error(&error))?;
    let page_size = u64::try_from(page_size).map_err(|_| LedgerError::new(R::Corruption))?;
    if page_size == 0 {
        return Err(LedgerError::new(R::Corruption));
    }

    // The explicit string roster is intentionally duplicated in this closed
    // query rather than assembled from untrusted schema names.
    let mut statement = conn
        .prepare(
            "SELECT d.pgsize
             FROM dbstat('main') AS d
             WHERE d.name IN (
                 SELECT name FROM sqlite_schema
                 WHERE (type='table' AND name IN (
                     'workflow_attempt_summaries',
                     'workflow_history_attachment',
                     'workflow_history_rewrite_reserve',
                     'workflow_attempt_node_terminals',
                     'workflow_history_mutation_reserve',
                     'workflow_history_allocation'
                 )) OR (type='index' AND tbl_name IN (
                     'workflow_attempt_summaries',
                     'workflow_history_attachment',
                     'workflow_history_rewrite_reserve',
                     'workflow_attempt_node_terminals',
                     'workflow_history_mutation_reserve',
                     'workflow_history_allocation'
                 ))
             )",
        )
        .map_err(|error| {
            let _ = error;
            LedgerError::new(R::UnsupportedConfiguration)
        })?;
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| sqlite_error(&error))?;
    let mut bytes = 0_u64;
    for row in rows {
        let pgsize = row.map_err(|error| sqlite_error(&error))?;
        let pgsize = u64::try_from(pgsize).map_err(|_| LedgerError::new(R::Corruption))?;
        bytes = bytes
            .checked_add(pgsize)
            .ok_or_else(|| LedgerError::new(R::Corruption))?;
    }
    if bytes % page_size != 0 {
        return Err(LedgerError::new(R::Corruption));
    }
    Ok(MeasuredHistoryPagesV2 {
        pages: bytes / page_size,
        bytes,
        page_size,
    })
}

fn dirty_pages(root_union_pages: u64) -> Result<u64, LedgerError> {
    root_union_pages
        .checked_mul(3)
        .and_then(|value| value.checked_add(2))
        .ok_or_else(|| LedgerError::new(R::CapacityProtected))
}

pub(crate) fn future_mutation_reserve(
    regime: JournalRegimeV2,
    measured_pages: u64,
    page_size: u64,
) -> Result<u64, LedgerError> {
    let post_bound = measured_pages
        .checked_mul(2)
        .and_then(|value| value.checked_add(2))
        .ok_or_else(|| LedgerError::new(R::CapacityProtected))?;
    let root_union = measured_pages
        .checked_add(post_bound)
        .ok_or_else(|| LedgerError::new(R::CapacityProtected))?;
    reserve_for_root_union(regime, root_union, page_size)
}

pub(crate) fn admission_reserve(
    regime: JournalRegimeV2,
    pages_before: u64,
    pages_after: u64,
    page_size: u64,
) -> Result<u64, LedgerError> {
    let root_union = pages_before
        .checked_add(pages_after)
        .ok_or_else(|| LedgerError::new(R::CapacityProtected))?;
    reserve_for_root_union(regime, root_union, page_size)
}

fn reserve_for_root_union(
    regime: JournalRegimeV2,
    root_union: u64,
    page_size: u64,
) -> Result<u64, LedgerError> {
    let dirty = dirty_pages(root_union)?;
    match regime {
        JournalRegimeV2::Wal => {
            let frame_bytes = page_size
                .checked_add(24)
                .ok_or_else(|| LedgerError::new(R::CapacityProtected))?;
            dirty
                .checked_mul(frame_bytes)
                .and_then(|value| value.checked_add(32))
                .ok_or_else(|| LedgerError::new(R::CapacityProtected))
        }
        JournalRegimeV2::Rollback => {
            const MAX_SECTOR_SIZE: u64 = 65_536;
            let headers = dirty
                .checked_add(1)
                .and_then(|value| value.checked_mul(MAX_SECTOR_SIZE))
                .ok_or_else(|| LedgerError::new(R::CapacityProtected))?;
            let record_bytes = page_size
                .checked_add(8)
                .ok_or_else(|| LedgerError::new(R::CapacityProtected))?;
            headers
                .checked_add(
                    dirty
                        .checked_mul(record_bytes)
                        .ok_or_else(|| LedgerError::new(R::CapacityProtected))?,
                )
                .ok_or_else(|| LedgerError::new(R::CapacityProtected))
        }
    }
}

pub(crate) fn sqlite_error(error: &rusqlite::Error) -> LedgerError {
    if let rusqlite::Error::SqliteFailure(raw, _) = error {
        let reason = match raw.code {
            rusqlite::ErrorCode::DiskFull => R::CapacityProtected,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => R::Locked,
            rusqlite::ErrorCode::ReadOnly => R::ReadOnlyDatabase,
            _ => R::Io,
        };
        return LedgerError::with_sqlite_codes(reason, raw.code as i32, raw.extended_code);
    }
    LedgerError::new(R::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_equations_are_checked_and_match_the_reviewed_bounds() {
        // H=5 => R=3H+2=17; D(R)=53.
        assert_eq!(
            future_mutation_reserve(JournalRegimeV2::Wal, 5, 4096).unwrap(),
            32 + 53 * (4096 + 24),
        );
        assert_eq!(
            future_mutation_reserve(JournalRegimeV2::Rollback, 5, 4096).unwrap(),
            (53 + 1) * 65_536 + 53 * (4096 + 8),
        );
    }

    #[test]
    fn fixed_width_counter_encoding_is_order_preserving_and_exact() {
        let low = encode_u64(255);
        let high = encode_u64(256);
        assert_eq!(low.len(), 8);
        assert_eq!(high.len(), 8);
        assert!(low < high);
        assert_eq!(decode_u64(high.to_vec()).unwrap(), 256);
        assert_eq!(decode_u64(vec![0; 7]).unwrap_err().reason, R::Corruption);
    }
}
