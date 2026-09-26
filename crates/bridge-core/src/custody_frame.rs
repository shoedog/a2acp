//! The ADR-0041 coverage-payload frame, v1 (slice 2B2b1).
//!
//! A frame is a canonical, bounded, self-validating record of one coverage class's filesystem
//! entries (directories, regular files, and symlinks) relative to an implicit class root:
//!
//! ```text
//! frame   = header *entry trailer                     ; then end of stream
//! header  = "a2a-cfr1" %x01 class generation
//! entry   = %x01 path mode                            ; directory
//!         / %x02 path mode length content digest      ; regular file
//!         / %x03 path target                          ; symlink
//! trailer = %xFF entries total frame-digest
//! ```
//!
//! Integers are fixed-width little-endian. The header binds the frame to one coverage class and
//! one manifest generation, so a payload cannot be replayed under another role. The frame digest
//! is the SHA-256 of every byte from the magic through `total`, so one changed byte anywhere is
//! refused by the end of decoding. Entries are strictly increasing in component-wise byte order,
//! and each entry's parent is an earlier directory entry. That is depth-first pre-order with
//! siblings sorted by name bytes.
//!
//! The module is pure and effect-free. It works only over caller-supplied [`Read`] and [`Write`]
//! values. It never opens, stats, walks, or writes a path, never spawns a process, and never
//! decides which files belong to a class: a path is data. Slice 2B2b2 supplies the no-follow
//! walker and the streaming producers, and slice 2B3 restores.
//!
//! **Declared v1 limits (task §2.6).** A frame records only the entry type, the permission bits,
//! the content, and the symlink target. It does not represent owner or group, timestamps,
//! extended attributes, ACLs, resource forks, BSD file flags, sparse layout, or hard-link identity
//! (each link is an independent regular file). It has no entry type for a FIFO, socket, or
//! device: the walker must refuse those, and that refusal parks the unit.

use crate::custody_seal::CustodyCoverageClassV1;
use ring::digest;
use std::cmp::Ordering;
use std::fmt;
use std::io::{self, Read, Write};

// ---------------------------------------------------------------------------------------------
// The v1 grammar (§2) and ceilings (§3)
// ---------------------------------------------------------------------------------------------

const MAGIC_V1: [u8; 8] = *b"a2a-cfr1";
const VERSION_V1: u8 = 0x01;

const TAG_DIRECTORY_V1: u8 = 0x01;
const TAG_REGULAR_V1: u8 = 0x02;
const TAG_SYMLINK_V1: u8 = 0x03;
const TAG_TRAILER_V1: u8 = 0xFF;

const SEPARATOR: u8 = b'/';

const MAX_GENERATION_BYTES_V1: usize = 1024;
const MAX_PATH_BYTES_V1: usize = 4096;
const MAX_COMPONENT_BYTES_V1: usize = 255;
const MAX_SYMLINK_TARGET_BYTES_V1: usize = 4095;
/// The only mode bits a frame carries. The setuid, setgid, and sticky bits are refused.
const PERMISSION_BITS_V1: u32 = 0o777;

const MAX_ENTRIES_V1: u64 = 1_048_576;
/// The ADR-0041 per-artifact ceiling, 10 GiB.
const MAX_FRAME_BYTES_V1: u64 = 10 * (1 << 30);

/// Content streams through a fixed buffer of this size on both sides; it is never buffered whole.
const CONTENT_BUFFER_BYTES_V1: usize = 64 * 1024;

const DIGEST_BYTES_V1: u64 = 32;
/// The magic, the version, the class, and the generation length field.
const HEADER_FIXED_BYTES_V1: u64 = 8 + 1 + 1 + 2;
/// The tag, the entry count, the content total, and the frame digest.
const TRAILER_BYTES_V1: u64 = 1 + 8 + 8 + DIGEST_BYTES_V1;

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Why a path component is refused (§2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustodyFrameComponentFaultV1 {
    Empty,
    Dot,
    DotDot,
    Nul,
    Slash,
    TooLong,
}

impl fmt::Display for CustodyFrameComponentFaultV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "empty",
            Self::Dot => "`.`",
            Self::DotDot => "`..`",
            Self::Nul => "contains NUL",
            Self::Slash => "contains `/`",
            Self::TooLong => "longer than 255 bytes",
        })
    }
}

/// Which way a regular file's reader disagreed with its declared length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustodyFrameLengthMismatchV1 {
    Short,
    Long,
}

impl fmt::Display for CustodyFrameLengthMismatchV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Short => "shorter",
            Self::Long => "longer",
        })
    }
}

/// One variant per distinct refusal. A message names its cause only: it never carries a content,
/// path, target, or other frame byte, so a refusal is safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CustodyFrameErrorV1 {
    #[error("coverage frame io failed: {0}")]
    Io(io::ErrorKind),
    #[error("coverage frame magic is not `a2a-cfr1`")]
    BadMagic,
    #[error("coverage frame version is not 1")]
    UnsupportedVersion,
    #[error("coverage frame class code names no v1 coverage class")]
    UnknownClass,
    #[error("the object database is never carried by a coverage frame")]
    ObjectDatabaseNotFramed,
    #[error("coverage frame class is not the expected class")]
    ClassMismatch,
    #[error("coverage frame generation is not the expected generation")]
    GenerationMismatch,
    #[error("coverage frame generation is empty, longer than 1024 bytes, or not UTF-8")]
    InvalidGeneration,
    #[error("coverage frame record tag is unknown")]
    UnknownTag,
    #[error("coverage frame ended before its trailer was complete")]
    Truncated,
    #[error("coverage frame is followed by further bytes")]
    TrailingBytes,
    #[error("coverage frame path component is invalid: {kind}")]
    InvalidComponent { kind: CustodyFrameComponentFaultV1 },
    #[error("coverage frame path is longer than 4096 bytes")]
    PathTooLong,
    #[error("coverage frame entry is not strictly after the previous entry")]
    OutOfOrder,
    #[error("coverage frame entry's parent is not an earlier directory entry")]
    MissingParent,
    #[error("coverage frame mode carries bits outside 0o777")]
    UnsupportedMode,
    #[error("coverage frame symlink target is empty, longer than 4095 bytes, or contains NUL")]
    InvalidSymlinkTarget,
    #[error("regular-file content is {kind} than its declared length")]
    ContentLengthMismatch { kind: CustodyFrameLengthMismatchV1 },
    #[error("regular-file content does not match its digest")]
    ContentDigestMismatch,
    #[error("coverage frame trailer entry count does not match the entries")]
    CountMismatch,
    #[error("coverage frame trailer content total does not match the content")]
    TotalMismatch,
    #[error("coverage frame digest does not match the frame")]
    FrameDigestMismatch,
    #[error("coverage frame entry limit exceeded")]
    EntryLimit,
    #[error("coverage frame byte budget exceeded")]
    ByteBudget,
    #[error("coverage frame codec is poisoned by an earlier refusal")]
    Poisoned,
}

impl CustodyFrameErrorV1 {
    /// The typed refusal carried by a content reader's [`io::Error`], if it carries one.
    pub(crate) fn from_io_error(error: &io::Error) -> Option<Self> {
        error.get_ref()?.downcast_ref::<Self>().copied()
    }

    fn into_io_error(self) -> io::Error {
        let kind = match self {
            Self::Io(kind) => kind,
            Self::Truncated => io::ErrorKind::UnexpectedEof,
            _ => io::ErrorKind::InvalidData,
        };
        io::Error::new(kind, self)
    }
}

type FrameResult<T> = Result<T, CustodyFrameErrorV1>;

// ---------------------------------------------------------------------------------------------
// Header, class codes, paths, and budgets
// ---------------------------------------------------------------------------------------------

/// The fixed, closed §2.2 code table: `CustodyCoverageClassV1::ALL` order, 1-based. The object
/// database travels as the verified Git pack and is never framed.
fn class_code(class: CustodyCoverageClassV1) -> FrameResult<u8> {
    use CustodyCoverageClassV1 as Class;
    Ok(match class {
        Class::RefsAndHead => 1,
        Class::ObjectDatabase => return Err(CustodyFrameErrorV1::ObjectDatabaseNotFramed),
        Class::Index => 3,
        Class::Worktree => 4,
        Class::StashAndReflogs => 5,
        Class::InProgressGitOperations => 6,
        Class::LinkedWorktrees => 7,
        Class::NestedRepositoriesAndSubmodules => 8,
        Class::LfsAndExternalPayloads => 9,
        Class::AlternatesAndSharedStores => 10,
        Class::GitConfigurationAndHooks => 11,
        Class::BridgeEvidence => 12,
        Class::ExternalEvidence => 13,
        Class::ReproducibleOutputs => 14,
    })
}

fn class_from_code(code: u8) -> FrameResult<CustodyCoverageClassV1> {
    use CustodyCoverageClassV1 as Class;
    Ok(match code {
        1 => Class::RefsAndHead,
        2 => return Err(CustodyFrameErrorV1::ObjectDatabaseNotFramed),
        3 => Class::Index,
        4 => Class::Worktree,
        5 => Class::StashAndReflogs,
        6 => Class::InProgressGitOperations,
        7 => Class::LinkedWorktrees,
        8 => Class::NestedRepositoriesAndSubmodules,
        9 => Class::LfsAndExternalPayloads,
        10 => Class::AlternatesAndSharedStores,
        11 => Class::GitConfigurationAndHooks,
        12 => Class::BridgeEvidence,
        13 => Class::ExternalEvidence,
        14 => Class::ReproducibleOutputs,
        _ => return Err(CustodyFrameErrorV1::UnknownClass),
    })
}

fn generation_length_admitted(length: usize) -> bool {
    (1..=MAX_GENERATION_BYTES_V1).contains(&length)
}

/// The frame header: one framed coverage class and one manifest generation id, byte-exact. A
/// decoder refuses a frame whose header differs from the one it expects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CustodyFrameHeaderV1 {
    class: CustodyCoverageClassV1,
    code: u8,
    generation_id: String,
}

impl CustodyFrameHeaderV1 {
    /// Refuses `object_database` and a generation id outside 1..=1024 bytes. The id is `&str`, so
    /// it is UTF-8 by construction; the decoder checks UTF-8 on input.
    pub(crate) fn new(class: CustodyCoverageClassV1, generation_id: &str) -> FrameResult<Self> {
        let code = class_code(class)?;
        if !generation_length_admitted(generation_id.len()) {
            return Err(CustodyFrameErrorV1::InvalidGeneration);
        }
        Ok(Self {
            class,
            code,
            generation_id: generation_id.to_owned(),
        })
    }

    pub(crate) fn class(&self) -> CustodyCoverageClassV1 {
        self.class
    }

    pub(crate) fn generation_id(&self) -> &str {
        &self.generation_id
    }

    fn encode(&self) -> FrameResult<Vec<u8>> {
        let generation = self.generation_id.as_bytes();
        let length =
            u16::try_from(generation.len()).map_err(|_| CustodyFrameErrorV1::InvalidGeneration)?;
        let mut bytes = Vec::with_capacity(12 + generation.len());
        bytes.extend_from_slice(&MAGIC_V1);
        bytes.push(VERSION_V1);
        bytes.push(self.code);
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(generation);
        Ok(bytes)
    }
}

fn path_length_admitted(length: usize) -> bool {
    length <= MAX_PATH_BYTES_V1
}

fn validate_component(component: &[u8]) -> FrameResult<()> {
    use CustodyFrameComponentFaultV1 as Fault;
    let refuse = |kind| Err(CustodyFrameErrorV1::InvalidComponent { kind });
    if component.is_empty() {
        return refuse(Fault::Empty);
    }
    if component.len() > MAX_COMPONENT_BYTES_V1 {
        return refuse(Fault::TooLong);
    }
    if component == b"." {
        return refuse(Fault::Dot);
    }
    if component == b".." {
        return refuse(Fault::DotDot);
    }
    if component.contains(&0) {
        return refuse(Fault::Nul);
    }
    if component.contains(&SEPARATOR) {
        return refuse(Fault::Slash);
    }
    Ok(())
}

/// A validated frame path (§2.3): 1..=4096 bytes, relative to the class root, of components
/// joined by one `/`. Each component is 1..=255 bytes, is neither `.` nor `..`, and holds no NUL
/// or `/`. Every other byte is kept, including non-UTF-8 sequences and `\`, because capture is
/// lossless. Portability and case-fold collisions are restore concerns, so `A` and `a` are
/// distinct paths.
///
/// `Ord` is the canonical §2.5 order: component by component, each by lexicographic byte order,
/// with a proper prefix first. So `a` < `a/b` < `a.b`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CustodyFramePathV1 {
    bytes: Vec<u8>,
}

impl CustodyFramePathV1 {
    pub(crate) fn from_bytes(bytes: &[u8]) -> FrameResult<Self> {
        if !path_length_admitted(bytes.len()) {
            return Err(CustodyFrameErrorV1::PathTooLong);
        }
        for component in bytes.split(|byte| *byte == SEPARATOR) {
            validate_component(component)?;
        }
        Ok(Self {
            bytes: bytes.to_vec(),
        })
    }

    /// Joins validated components. Zero components would name the implicit class root, which is
    /// never an entry, so they are refused as an empty component.
    pub(crate) fn from_components<I, C>(components: I) -> FrameResult<Self>
    where
        I: IntoIterator<Item = C>,
        C: AsRef<[u8]>,
    {
        let mut bytes = Vec::new();
        for component in components {
            let component = component.as_ref();
            validate_component(component)?;
            let separator = usize::from(!bytes.is_empty());
            if !path_length_admitted(bytes.len() + separator + component.len()) {
                return Err(CustodyFrameErrorV1::PathTooLong);
            }
            if separator == 1 {
                bytes.push(SEPARATOR);
            }
            bytes.extend_from_slice(component);
        }
        if bytes.is_empty() {
            return Err(CustodyFrameErrorV1::InvalidComponent {
                kind: CustodyFrameComponentFaultV1::Empty,
            });
        }
        Ok(Self { bytes })
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn components(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.bytes.split(|byte| *byte == SEPARATOR)
    }
}

impl Ord for CustodyFramePathV1 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.components().cmp(other.components())
    }
}

impl PartialOrd for CustodyFramePathV1 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The encoder's §2.4 mode rule. The caller passes `st_mode & 0o7777`. A setuid, setgid, or
/// sticky bit is unsupported state, and so is any bit the caller failed to mask: it is refused,
/// never masked away.
fn encode_mode(mode: u32) -> FrameResult<u16> {
    if mode & !PERMISSION_BITS_V1 != 0 {
        return Err(CustodyFrameErrorV1::UnsupportedMode);
    }
    u16::try_from(mode).map_err(|_| CustodyFrameErrorV1::UnsupportedMode)
}

/// The decoder's §2.4 mode ceiling.
fn decode_mode(mode: u16) -> FrameResult<u32> {
    let mode = u32::from(mode);
    if mode > PERMISSION_BITS_V1 {
        return Err(CustodyFrameErrorV1::UnsupportedMode);
    }
    Ok(mode)
}

fn symlink_target_length_admitted(length: usize) -> bool {
    (1..=MAX_SYMLINK_TARGET_BYTES_V1).contains(&length)
}

/// A symlink target is opaque data: 1..=4095 bytes with no NUL. An absolute or `..` target is
/// kept as it is; containing it is a restore concern.
fn validate_symlink_target(target: &[u8]) -> FrameResult<()> {
    if !symlink_target_length_admitted(target.len()) {
        return Err(CustodyFrameErrorV1::InvalidSymlinkTarget);
    }
    if target.contains(&0) {
        return Err(CustodyFrameErrorV1::InvalidSymlinkTarget);
    }
    Ok(())
}

/// Caller-supplied frame bounds (§3), refused above the v1 ceilings. `max_frame_bytes` counts
/// every frame byte: the header, the records, the content, the digests, and the trailer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CustodyFrameBudgetV1 {
    max_entries: u64,
    max_frame_bytes: u64,
}

impl CustodyFrameBudgetV1 {
    pub(crate) fn new(max_entries: u64, max_frame_bytes: u64) -> FrameResult<Self> {
        if max_entries > MAX_ENTRIES_V1 {
            return Err(CustodyFrameErrorV1::EntryLimit);
        }
        if max_frame_bytes > MAX_FRAME_BYTES_V1 {
            return Err(CustodyFrameErrorV1::ByteBudget);
        }
        Ok(Self {
            max_entries,
            max_frame_bytes,
        })
    }

    pub(crate) fn max_entries(&self) -> u64 {
        self.max_entries
    }

    pub(crate) fn max_frame_bytes(&self) -> u64 {
        self.max_frame_bytes
    }

    /// The bytes left to the header and the entry records. The trailer is reserved up front, so
    /// every admitted record can still be followed by its trailer.
    fn record_limit(&self) -> FrameResult<u64> {
        self.max_frame_bytes
            .checked_sub(TRAILER_BYTES_V1)
            .ok_or(CustodyFrameErrorV1::ByteBudget)
    }
}

// ---------------------------------------------------------------------------------------------
// Canonical order and structure (§2.5)
// ---------------------------------------------------------------------------------------------

/// The order and structure check both sides share. It holds one path and a stack of prefix
/// lengths, so its memory is bounded by the path ceiling, not by the entry count.
#[derive(Default)]
struct CanonicalOrderV1 {
    previous: Option<CustodyFramePathV1>,
    /// The byte lengths of the directory entries that are ancestors of `previous`, or `previous`
    /// itself, shallowest first. Each is a prefix of `previous`, so there are at most 2048.
    open_directories: Vec<usize>,
}

impl CanonicalOrderV1 {
    fn admit(&mut self, path: &CustodyFramePathV1, is_directory: bool) -> FrameResult<()> {
        if self
            .previous
            .as_ref()
            .is_some_and(|previous| path <= previous)
        {
            return Err(CustodyFrameErrorV1::OutOfOrder);
        }
        let bytes = path.as_bytes();
        let previous = self
            .previous
            .as_ref()
            .map_or(&[][..], CustodyFramePathV1::as_bytes);
        // Close every open directory that is not a proper ancestor of this entry. Strict increase
        // makes this exact: every entry between a directory and its descendant is itself a
        // descendant, so a parent that appeared is still open here.
        while let Some(&length) = self.open_directories.last() {
            let is_ancestor = bytes.len() > length
                && bytes[length] == SEPARATOR
                && previous.get(..length) == Some(&bytes[..length]);
            if is_ancestor {
                break;
            }
            self.open_directories.pop();
        }
        let parent_length = bytes
            .iter()
            .rposition(|byte| *byte == SEPARATOR)
            .unwrap_or(0);
        if self.open_directories.last().copied().unwrap_or(0) != parent_length {
            return Err(CustodyFrameErrorV1::MissingParent);
        }
        if is_directory {
            self.open_directories.push(bytes.len());
        }
        self.previous = Some(path.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Receipts
// ---------------------------------------------------------------------------------------------

/// The encoder's record of one regular file: the streamed length and the SHA-256 it computed
/// while streaming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CustodyFrameFileReceiptV1 {
    length: u64,
    sha256: [u8; 32],
}

impl CustodyFrameFileReceiptV1 {
    pub(crate) fn length(&self) -> u64 {
        self.length
    }

    pub(crate) fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}

/// The encoder's record of one finished frame. `frame_sha256` is the SHA-256 of every emitted
/// byte, the trailer's frame digest included, so it is not that frame digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CustodyFrameSummaryV1 {
    entries: u64,
    content_bytes: u64,
    frame_bytes: u64,
    frame_sha256: [u8; 32],
}

impl CustodyFrameSummaryV1 {
    pub(crate) fn entries(&self) -> u64 {
        self.entries
    }

    pub(crate) fn content_bytes(&self) -> u64 {
        self.content_bytes
    }

    pub(crate) fn frame_bytes(&self) -> u64 {
        self.frame_bytes
    }

    pub(crate) fn frame_sha256(&self) -> [u8; 32] {
        self.frame_sha256
    }
}

// ---------------------------------------------------------------------------------------------
// Byte-level helpers
// ---------------------------------------------------------------------------------------------

fn sha256_of(context: digest::Context) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(context.finish().as_ref());
    out
}

fn io_refusal(error: &io::Error) -> CustodyFrameErrorV1 {
    CustodyFrameErrorV1::Io(error.kind())
}

/// One `read`, retried on `Interrupted`. A reader that claims more bytes than it was given is
/// refused rather than trusted.
fn read_some(reader: &mut dyn Read, buffer: &mut [u8]) -> FrameResult<usize> {
    loop {
        match reader.read(buffer) {
            Ok(read) if read <= buffer.len() => return Ok(read),
            Ok(_) => return Err(CustodyFrameErrorV1::Io(io::ErrorKind::InvalidData)),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(io_refusal(&error)),
        }
    }
}

/// Fills `buffer`, or refuses: an end of stream first is [`CustodyFrameErrorV1::Truncated`].
fn read_exactly(reader: &mut dyn Read, buffer: &mut [u8]) -> FrameResult<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = read_some(reader, &mut buffer[filled..])?;
        if read == 0 {
            return Err(CustodyFrameErrorV1::Truncated);
        }
        filled += read;
    }
    Ok(())
}

/// The bytes a caller's buffer or the fixed buffer can take of `remaining` content bytes.
fn chunk_length(remaining: u64, capacity: usize) -> usize {
    usize::try_from(remaining).map_or(capacity, |remaining| remaining.min(capacity))
}

/// The tag, length-prefixed path, and nothing else of an entry record.
fn record_prefix(tag: u8, path: &CustodyFramePathV1) -> FrameResult<Vec<u8>> {
    let bytes = path.as_bytes();
    let length = u16::try_from(bytes.len()).map_err(|_| CustodyFrameErrorV1::PathTooLong)?;
    let mut record = Vec::with_capacity(1 + 2 + bytes.len() + 2 + 8);
    record.push(tag);
    record.extend_from_slice(&length.to_le_bytes());
    record.extend_from_slice(bytes);
    Ok(record)
}

// ---------------------------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------------------------

/// The encoder's output: every emitted byte is counted and added to the frame digest.
struct FrameSinkV1<W> {
    sink: W,
    frame: digest::Context,
    frame_bytes: u64,
}

impl<W: Write> FrameSinkV1<W> {
    fn emit(&mut self, mut bytes: &[u8]) -> FrameResult<()> {
        let total = bytes.len() as u64;
        let digested = bytes;
        while !bytes.is_empty() {
            match self.sink.write(bytes) {
                Ok(0) => return Err(CustodyFrameErrorV1::Io(io::ErrorKind::WriteZero)),
                Ok(written) => {
                    bytes = bytes
                        .get(written..)
                        .ok_or(CustodyFrameErrorV1::Io(io::ErrorKind::InvalidData))?;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(io_refusal(&error)),
            }
        }
        self.frame.update(digested);
        self.frame_bytes = self
            .frame_bytes
            .checked_add(total)
            .ok_or(CustodyFrameErrorV1::ByteBudget)?;
        Ok(())
    }
}

/// A validating streaming encoder (§4). It enforces the same order, structure, and bounds as
/// the decoder. It refuses out-of-order input rather than sorting it: a streaming producer emits
/// in canonical order, and sorting would buffer the whole set.
///
/// Any refusal poisons the encoder. Every later call returns
/// [`CustodyFrameErrorV1::Poisoned`], and [`Self::finish`] never succeeds. Bytes already written
/// to the sink are the caller's to discard.
pub(crate) struct CustodyFrameEncoderV1<W: Write> {
    out: FrameSinkV1<W>,
    budget: CustodyFrameBudgetV1,
    record_limit: u64,
    order: CanonicalOrderV1,
    entries: u64,
    content_bytes: u64,
    buffer: Vec<u8>,
    poisoned: bool,
}

impl<W: Write> CustodyFrameEncoderV1<W> {
    /// Writes the header, after admitting it and the reserved trailer against the budget.
    pub(crate) fn new(
        sink: W,
        header: &CustodyFrameHeaderV1,
        budget: CustodyFrameBudgetV1,
    ) -> FrameResult<Self> {
        let record_limit = budget.record_limit()?;
        let header_bytes = header.encode()?;
        let mut encoder = Self {
            out: FrameSinkV1 {
                sink,
                frame: digest::Context::new(&digest::SHA256),
                frame_bytes: 0,
            },
            budget,
            record_limit,
            order: CanonicalOrderV1::default(),
            entries: 0,
            content_bytes: 0,
            buffer: vec![0; CONTENT_BUFFER_BYTES_V1],
            poisoned: false,
        };
        encoder.admit_bytes(header_bytes.len() as u64)?;
        encoder.out.emit(&header_bytes)?;
        Ok(encoder)
    }

    pub(crate) fn directory(&mut self, path: &CustodyFramePathV1, mode: u32) -> FrameResult<()> {
        self.guarded(|encoder| {
            let mode = encode_mode(mode)?;
            encoder.order.admit(path, true)?;
            let mut record = record_prefix(TAG_DIRECTORY_V1, path)?;
            record.extend_from_slice(&mode.to_le_bytes());
            encoder.admit_record(record.len() as u64)?;
            encoder.out.emit(&record)?;
            encoder.entries += 1;
            Ok(())
        })
    }

    pub(crate) fn symlink(&mut self, path: &CustodyFramePathV1, target: &[u8]) -> FrameResult<()> {
        self.guarded(|encoder| {
            validate_symlink_target(target)?;
            encoder.order.admit(path, false)?;
            let length = u16::try_from(target.len())
                .map_err(|_| CustodyFrameErrorV1::InvalidSymlinkTarget)?;
            let mut record = record_prefix(TAG_SYMLINK_V1, path)?;
            record.extend_from_slice(&length.to_le_bytes());
            record.extend_from_slice(target);
            encoder.admit_record(record.len() as u64)?;
            encoder.out.emit(&record)?;
            encoder.entries += 1;
            Ok(())
        })
    }

    /// Streams exactly `declared_length` bytes from `reader` through the fixed buffer, computing
    /// their SHA-256 on the way, then probes one byte to prove the reader is exhausted. The whole
    /// record, declared content included, is admitted before any byte of it is written.
    pub(crate) fn regular_file(
        &mut self,
        path: &CustodyFramePathV1,
        mode: u32,
        declared_length: u64,
        reader: &mut dyn Read,
    ) -> FrameResult<CustodyFrameFileReceiptV1> {
        self.guarded(|encoder| {
            let mode = encode_mode(mode)?;
            encoder.order.admit(path, false)?;
            let mut prefix = record_prefix(TAG_REGULAR_V1, path)?;
            prefix.extend_from_slice(&mode.to_le_bytes());
            prefix.extend_from_slice(&declared_length.to_le_bytes());
            let size = (prefix.len() as u64 + DIGEST_BYTES_V1)
                .checked_add(declared_length)
                .ok_or(CustodyFrameErrorV1::ByteBudget)?;
            encoder.admit_record(size)?;
            let content_bytes = encoder
                .content_bytes
                .checked_add(declared_length)
                .ok_or(CustodyFrameErrorV1::ByteBudget)?;
            encoder.out.emit(&prefix)?;
            let sha256 = encoder.stream_content(declared_length, reader)?;
            encoder.out.emit(&sha256)?;
            encoder.entries += 1;
            encoder.content_bytes = content_bytes;
            Ok(CustodyFrameFileReceiptV1 {
                length: declared_length,
                sha256,
            })
        })
    }

    /// Writes the trailer and returns the frame's summary.
    pub(crate) fn finish(mut self) -> FrameResult<CustodyFrameSummaryV1> {
        self.guarded(|encoder| {
            let mut trailer = Vec::with_capacity(17);
            trailer.push(TAG_TRAILER_V1);
            trailer.extend_from_slice(&encoder.entries.to_le_bytes());
            trailer.extend_from_slice(&encoder.content_bytes.to_le_bytes());
            encoder.out.emit(&trailer)?;
            let frame_digest = sha256_of(encoder.out.frame.clone());
            encoder.out.emit(&frame_digest)?;
            encoder
                .out
                .sink
                .flush()
                .map_err(|error| io_refusal(&error))?;
            Ok(CustodyFrameSummaryV1 {
                entries: encoder.entries,
                content_bytes: encoder.content_bytes,
                frame_bytes: encoder.out.frame_bytes,
                frame_sha256: sha256_of(encoder.out.frame.clone()),
            })
        })
    }

    fn guarded<T>(&mut self, step: impl FnOnce(&mut Self) -> FrameResult<T>) -> FrameResult<T> {
        if self.poisoned {
            return Err(CustodyFrameErrorV1::Poisoned);
        }
        let outcome = step(self);
        if outcome.is_err() {
            self.poisoned = true;
        }
        outcome
    }

    /// Refuses a record before any byte of it is written: first the entry limit, then the
    /// record's exact size against the bytes left before the reserved trailer.
    fn admit_record(&self, size: u64) -> FrameResult<()> {
        if self.entries >= self.budget.max_entries {
            return Err(CustodyFrameErrorV1::EntryLimit);
        }
        self.admit_bytes(size)
    }

    fn admit_bytes(&self, size: u64) -> FrameResult<()> {
        let end = self
            .out
            .frame_bytes
            .checked_add(size)
            .ok_or(CustodyFrameErrorV1::ByteBudget)?;
        if end > self.record_limit {
            return Err(CustodyFrameErrorV1::ByteBudget);
        }
        Ok(())
    }

    fn stream_content(
        &mut self,
        declared_length: u64,
        reader: &mut dyn Read,
    ) -> FrameResult<[u8; 32]> {
        let mut content = digest::Context::new(&digest::SHA256);
        let mut remaining = declared_length;
        while remaining > 0 {
            let want = chunk_length(remaining, self.buffer.len());
            let read = read_some(reader, &mut self.buffer[..want])?;
            if read == 0 {
                return Err(CustodyFrameErrorV1::ContentLengthMismatch {
                    kind: CustodyFrameLengthMismatchV1::Short,
                });
            }
            let chunk = &self.buffer[..read];
            content.update(chunk);
            self.out.emit(chunk)?;
            remaining -= read as u64;
        }
        // One probe byte proves the reader ends exactly at the declared length.
        if read_some(reader, &mut self.buffer[..1])? != 0 {
            return Err(CustodyFrameErrorV1::ContentLengthMismatch {
                kind: CustodyFrameLengthMismatchV1::Long,
            });
        }
        Ok(sha256_of(content))
    }
}

// ---------------------------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------------------------

/// One decoded entry. A regular file's content is read through its bounded reader.
#[derive(Debug)]
pub(crate) enum CustodyFrameEntryV1<'a> {
    Directory {
        path: CustodyFramePathV1,
        mode: u32,
    },
    Regular {
        path: CustodyFramePathV1,
        mode: u32,
        length: u64,
        content: CustodyFrameContentReaderV1<'a>,
    },
    Symlink {
        path: CustodyFramePathV1,
        target: Vec<u8>,
    },
}

/// A validating streaming decoder (§4). [`Self::new`] reads and checks the header. Each
/// [`Self::next_entry`] then yields one entry, and it returns `Ok(None)` exactly at a verified
/// trailer followed by the end of the stream.
///
/// **Entries are provisional until `Ok(None)`.** Each record is checked as it is read, but only
/// the trailer proves the frame whole, so a consumer must not treat any yielded entry as verified
/// before then. Any refusal poisons the decoder: every later call returns
/// [`CustodyFrameErrorV1::Poisoned`].
pub(crate) struct CustodyFrameDecoderV1<R: Read> {
    source: R,
    state: DecoderStateV1,
}

impl<R: Read> CustodyFrameDecoderV1<R> {
    pub(crate) fn new(
        mut source: R,
        expected: &CustodyFrameHeaderV1,
        budget: CustodyFrameBudgetV1,
    ) -> FrameResult<Self> {
        let mut state = DecoderStateV1 {
            budget,
            record_limit: budget.record_limit()?,
            admitted: 0,
            frame: digest::Context::new(&digest::SHA256),
            order: CanonicalOrderV1::default(),
            entries: 0,
            content_bytes: 0,
            pending: None,
            buffer: vec![0; CONTENT_BUFFER_BYTES_V1],
            phase: DecoderPhaseV1::Open,
        };
        state.read_header(&mut source, expected)?;
        Ok(Self { source, state })
    }

    /// Drains and verifies any content the consumer left unread, then reads the next record.
    pub(crate) fn next_entry(&mut self) -> FrameResult<Option<CustodyFrameEntryV1<'_>>> {
        match self.state.phase {
            DecoderPhaseV1::Poisoned => return Err(CustodyFrameErrorV1::Poisoned),
            DecoderPhaseV1::Complete => return Ok(None),
            DecoderPhaseV1::Open => {}
        }
        let record = match self.state.next_record(&mut self.source) {
            Ok(record) => record,
            Err(error) => {
                self.state.phase = DecoderPhaseV1::Poisoned;
                return Err(error);
            }
        };
        Ok(record.map(|record| record.into_entry(&mut self.state, &mut self.source)))
    }
}

/// The bounded content reader of one yielded regular file. It reads at most the declared length.
/// The read that exhausts the content verifies its digest, and refuses
/// [`CustodyFrameErrorV1::ContentDigestMismatch`] instead of returning. The next
/// [`CustodyFrameDecoderV1::next_entry`] drains and verifies any unread remainder, so skipping
/// content never skips verification.
///
/// Its errors carry the typed refusal: see [`CustodyFrameErrorV1::from_io_error`].
pub(crate) struct CustodyFrameContentReaderV1<'a> {
    state: &'a mut DecoderStateV1,
    source: &'a mut dyn Read,
}

impl Read for CustodyFrameContentReaderV1<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.state
            .consumer_read(self.source, buffer)
            .map_err(CustodyFrameErrorV1::into_io_error)
    }
}

impl fmt::Debug for CustodyFrameContentReaderV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CustodyFrameContentReaderV1")
            .field(
                "remaining",
                &self.state.pending.as_ref().map(|pending| pending.remaining),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DecoderPhaseV1 {
    Open,
    Complete,
    Poisoned,
}

/// The unread remainder of the last yielded regular file, and its running digest.
struct PendingContentV1 {
    remaining: u64,
    content: digest::Context,
}

#[derive(Clone, Copy)]
enum RecordKindV1 {
    Directory,
    Regular,
    Symlink,
}

/// A decoded record before it borrows the decoder for its content reader.
enum RecordV1 {
    Directory {
        path: CustodyFramePathV1,
        mode: u32,
    },
    Regular {
        path: CustodyFramePathV1,
        mode: u32,
        length: u64,
    },
    Symlink {
        path: CustodyFramePathV1,
        target: Vec<u8>,
    },
}

impl RecordV1 {
    fn into_entry<'a>(
        self,
        state: &'a mut DecoderStateV1,
        source: &'a mut dyn Read,
    ) -> CustodyFrameEntryV1<'a> {
        match self {
            Self::Directory { path, mode } => CustodyFrameEntryV1::Directory { path, mode },
            Self::Regular { path, mode, length } => CustodyFrameEntryV1::Regular {
                path,
                mode,
                length,
                content: CustodyFrameContentReaderV1 { state, source },
            },
            Self::Symlink { path, target } => CustodyFrameEntryV1::Symlink { path, target },
        }
    }
}

struct DecoderStateV1 {
    budget: CustodyFrameBudgetV1,
    record_limit: u64,
    /// The frame bytes admitted against the budget so far, declared content not yet read
    /// included. Every byte is admitted before it is read, and every length before the
    /// allocation it sizes.
    admitted: u64,
    frame: digest::Context,
    order: CanonicalOrderV1,
    entries: u64,
    content_bytes: u64,
    pending: Option<PendingContentV1>,
    buffer: Vec<u8>,
    phase: DecoderPhaseV1,
}

impl DecoderStateV1 {
    fn admit(&mut self, size: u64) -> FrameResult<()> {
        let end = self
            .admitted
            .checked_add(size)
            .ok_or(CustodyFrameErrorV1::ByteBudget)?;
        if end > self.record_limit {
            return Err(CustodyFrameErrorV1::ByteBudget);
        }
        self.admitted = end;
        Ok(())
    }

    /// Reads exactly `buffer.len()` frame bytes and adds them to the frame digest.
    fn read_frame_bytes(&mut self, source: &mut dyn Read, buffer: &mut [u8]) -> FrameResult<()> {
        read_exactly(source, buffer)?;
        self.frame.update(buffer);
        Ok(())
    }

    fn read_u8(&mut self, source: &mut dyn Read) -> FrameResult<u8> {
        let mut bytes = [0; 1];
        self.read_frame_bytes(source, &mut bytes)?;
        Ok(bytes[0])
    }

    fn read_u16(&mut self, source: &mut dyn Read) -> FrameResult<u16> {
        let mut bytes = [0; 2];
        self.read_frame_bytes(source, &mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u64(&mut self, source: &mut dyn Read) -> FrameResult<u64> {
        let mut bytes = [0; 8];
        self.read_frame_bytes(source, &mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn read_header(
        &mut self,
        source: &mut dyn Read,
        expected: &CustodyFrameHeaderV1,
    ) -> FrameResult<()> {
        self.admit(HEADER_FIXED_BYTES_V1)?;
        let mut magic = [0; 8];
        self.read_frame_bytes(source, &mut magic)?;
        if magic != MAGIC_V1 {
            return Err(CustodyFrameErrorV1::BadMagic);
        }
        if self.read_u8(source)? != VERSION_V1 {
            return Err(CustodyFrameErrorV1::UnsupportedVersion);
        }
        let class = class_from_code(self.read_u8(source)?)?;
        if class != expected.class {
            return Err(CustodyFrameErrorV1::ClassMismatch);
        }
        let length = self.read_u16(source)?;
        if !generation_length_admitted(usize::from(length)) {
            return Err(CustodyFrameErrorV1::InvalidGeneration);
        }
        self.admit(u64::from(length))?;
        let mut generation = vec![0; usize::from(length)];
        self.read_frame_bytes(source, &mut generation)?;
        let generation =
            String::from_utf8(generation).map_err(|_| CustodyFrameErrorV1::InvalidGeneration)?;
        if generation != expected.generation_id {
            return Err(CustodyFrameErrorV1::GenerationMismatch);
        }
        Ok(())
    }

    fn next_record(&mut self, source: &mut dyn Read) -> FrameResult<Option<RecordV1>> {
        self.drain_pending(source)?;
        // The tag byte lies inside the reserved trailer bytes, so reading it never passes the
        // budget. It is admitted as an entry's once it is known not to be the trailer's.
        let kind = match self.read_u8(source)? {
            TAG_TRAILER_V1 => {
                self.read_trailer(source)?;
                return Ok(None);
            }
            TAG_DIRECTORY_V1 => RecordKindV1::Directory,
            TAG_REGULAR_V1 => RecordKindV1::Regular,
            TAG_SYMLINK_V1 => RecordKindV1::Symlink,
            _ => return Err(CustodyFrameErrorV1::UnknownTag),
        };
        if self.entries >= self.budget.max_entries {
            return Err(CustodyFrameErrorV1::EntryLimit);
        }
        self.admit(1)?;
        let path = self.read_path(source)?;
        self.order
            .admit(&path, matches!(kind, RecordKindV1::Directory))?;
        let record = match kind {
            RecordKindV1::Directory => {
                self.admit(2)?;
                let mode = decode_mode(self.read_u16(source)?)?;
                RecordV1::Directory { path, mode }
            }
            RecordKindV1::Regular => {
                self.admit(2 + 8)?;
                let mode = decode_mode(self.read_u16(source)?)?;
                let length = self.read_u64(source)?;
                // The declared content and its digest are admitted before any content byte is
                // read.
                self.admit(
                    length
                        .checked_add(DIGEST_BYTES_V1)
                        .ok_or(CustodyFrameErrorV1::ByteBudget)?,
                )?;
                self.content_bytes = self
                    .content_bytes
                    .checked_add(length)
                    .ok_or(CustodyFrameErrorV1::ByteBudget)?;
                self.pending = Some(PendingContentV1 {
                    remaining: length,
                    content: digest::Context::new(&digest::SHA256),
                });
                RecordV1::Regular { path, mode, length }
            }
            RecordKindV1::Symlink => {
                self.admit(2)?;
                let length = self.read_u16(source)?;
                if !symlink_target_length_admitted(usize::from(length)) {
                    return Err(CustodyFrameErrorV1::InvalidSymlinkTarget);
                }
                self.admit(u64::from(length))?;
                let mut target = vec![0; usize::from(length)];
                self.read_frame_bytes(source, &mut target)?;
                validate_symlink_target(&target)?;
                RecordV1::Symlink { path, target }
            }
        };
        self.entries += 1;
        Ok(Some(record))
    }

    fn read_path(&mut self, source: &mut dyn Read) -> FrameResult<CustodyFramePathV1> {
        self.admit(2)?;
        let length = self.read_u16(source)?;
        if !path_length_admitted(usize::from(length)) {
            return Err(CustodyFrameErrorV1::PathTooLong);
        }
        self.admit(u64::from(length))?;
        let mut bytes = vec![0; usize::from(length)];
        self.read_frame_bytes(source, &mut bytes)?;
        CustodyFramePathV1::from_bytes(&bytes)
    }

    fn read_trailer(&mut self, source: &mut dyn Read) -> FrameResult<()> {
        let entries = self.read_u64(source)?;
        let total = self.read_u64(source)?;
        let expected = sha256_of(self.frame.clone());
        let mut frame_digest = [0; 32];
        read_exactly(source, &mut frame_digest)?;
        if entries != self.entries {
            return Err(CustodyFrameErrorV1::CountMismatch);
        }
        if total != self.content_bytes {
            return Err(CustodyFrameErrorV1::TotalMismatch);
        }
        if frame_digest != expected {
            return Err(CustodyFrameErrorV1::FrameDigestMismatch);
        }
        // One probe byte proves the end of the stream.
        if read_some(source, &mut [0; 1])? != 0 {
            return Err(CustodyFrameErrorV1::TrailingBytes);
        }
        self.phase = DecoderPhaseV1::Complete;
        Ok(())
    }

    /// Reads content of the pending regular file into `buffer`, at most its remainder.
    fn read_content_bytes(
        &mut self,
        source: &mut dyn Read,
        buffer: &mut [u8],
    ) -> FrameResult<usize> {
        let Some(pending) = self.pending.as_mut() else {
            return Ok(0);
        };
        let want = chunk_length(pending.remaining, buffer.len());
        if want == 0 {
            return Ok(0);
        }
        let read = read_some(source, &mut buffer[..want])?;
        if read == 0 {
            return Err(CustodyFrameErrorV1::Truncated);
        }
        let chunk = &buffer[..read];
        pending.content.update(chunk);
        pending.remaining -= read as u64;
        self.frame.update(chunk);
        Ok(read)
    }

    /// Reads the pending file's digest and compares it with the content read.
    fn finish_content(&mut self, source: &mut dyn Read) -> FrameResult<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let mut stored = [0; 32];
        self.read_frame_bytes(source, &mut stored)?;
        if sha256_of(pending.content) != stored {
            return Err(CustodyFrameErrorV1::ContentDigestMismatch);
        }
        Ok(())
    }

    /// Drains and verifies the remainder of content the consumer left unread.
    fn drain_pending(&mut self, source: &mut dyn Read) -> FrameResult<()> {
        let mut buffer = std::mem::take(&mut self.buffer);
        let drained = self.discard_content(source, &mut buffer);
        self.buffer = buffer;
        drained?;
        self.finish_content(source)
    }

    fn discard_content(&mut self, source: &mut dyn Read, buffer: &mut [u8]) -> FrameResult<()> {
        while self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.remaining > 0)
        {
            self.read_content_bytes(source, buffer)?;
        }
        Ok(())
    }

    fn consumer_read(&mut self, source: &mut dyn Read, buffer: &mut [u8]) -> FrameResult<usize> {
        if self.phase == DecoderPhaseV1::Poisoned {
            return Err(CustodyFrameErrorV1::Poisoned);
        }
        let outcome = self.read_yielded_content(source, buffer);
        if outcome.is_err() {
            self.phase = DecoderPhaseV1::Poisoned;
        }
        outcome
    }

    fn read_yielded_content(
        &mut self,
        source: &mut dyn Read,
        buffer: &mut [u8],
    ) -> FrameResult<usize> {
        let read = self.read_content_bytes(source, buffer)?;
        // The read that exhausts the content verifies it before returning.
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.remaining == 0)
        {
            self.finish_content(source)?;
        }
        Ok(read)
    }
}

#[cfg(test)]
#[path = "custody_frame_tests.rs"]
mod tests;
