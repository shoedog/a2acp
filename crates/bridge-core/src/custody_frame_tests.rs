//! Behavioral controls for the ADR-0041 slice 2B2b1 coverage-payload frame.
//!
//! Every `control_NN_*` test belongs to task §5 criterion `NN`. Refusal controls craft their
//! input with [`RawFrame`], an independent test-side writer of the §2.1 grammar that re-seals the
//! trailer. So a removed guard surfaces as a wrong success, not as a later frame-digest refusal.
//! The mutation matrix recorded in the implementation handoff removes one guard at a time and
//! names the control each removal turns red.

use super::*;
use crate::custody_seal::CustodyCoverageClassV1 as Class;
use ring::digest;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::rc::Rc;

use super::{CustodyFrameComponentFaultV1 as Fault, CustodyFrameErrorV1 as E};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

const GENERATION: &str = "generation-7";

/// The §2.2 table, restated independently of the module: every framed class and its code.
const CLASS_CODES: [(Class, u8); 13] = [
    (Class::RefsAndHead, 1),
    (Class::Index, 3),
    (Class::Worktree, 4),
    (Class::StashAndReflogs, 5),
    (Class::InProgressGitOperations, 6),
    (Class::LinkedWorktrees, 7),
    (Class::NestedRepositoriesAndSubmodules, 8),
    (Class::LfsAndExternalPayloads, 9),
    (Class::AlternatesAndSharedStores, 10),
    (Class::GitConfigurationAndHooks, 11),
    (Class::BridgeEvidence, 12),
    (Class::ExternalEvidence, 13),
    (Class::ReproducibleOutputs, 14),
];

/// The golden frame, computed from the §2.1 grammar by an independent Python script (recorded in
/// the handoff), not by this module: class `worktree`, generation `g1`, directory `d` (0o755),
/// regular file `d/f` (0o644, content `hi`), symlink `d/l` -> `f`, and the trailer.
const GOLDEN_FRAME_HEX: &str = concat!(
    "6132612d63667231", // magic "a2a-cfr1"
    "01",               // version
    "04",               // class: worktree
    "0200",             // generation length
    "6731",             // "g1"
    "01",               // directory
    "0100",
    "64",   // "d"
    "ed01", // 0o755
    "02",   // regular file
    "0300",
    "642f66",           // "d/f"
    "a401",             // 0o644
    "0200000000000000", // length 2
    "6869",             // "hi"
    "8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4",
    "03", // symlink
    "0300",
    "642f6c", // "d/l"
    "0100",
    "66",               // "f"
    "ff",               // trailer
    "0300000000000000", // entries 3
    "0200000000000000", // total 2
    "15a417a88fc404bb0dc70ed22ae203618a81da9db425073b29b6b2be70d5c9c7",
);
/// SHA-256 of `hi`.
const GOLDEN_HI_SHA256_HEX: &str =
    "8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4";
/// SHA-256 of the golden frame's bytes from the magic through `total`: the trailer digest.
const GOLDEN_FRAME_DIGEST_HEX: &str =
    "15a417a88fc404bb0dc70ed22ae203618a81da9db425073b29b6b2be70d5c9c7";
/// SHA-256 of the complete golden frame, trailer digest included.
const GOLDEN_FRAME_SHA256_HEX: &str =
    "87e05bcd74e1b372c96ef2cf650333c78ff2bda875b1cfdaf6e25c036ea462a8";

fn header() -> CustodyFrameHeaderV1 {
    header_for(Class::Worktree, GENERATION)
}

fn header_for(class: Class, generation: &str) -> CustodyFrameHeaderV1 {
    CustodyFrameHeaderV1::new(class, generation).expect("a valid frame header")
}

fn golden_header() -> CustodyFrameHeaderV1 {
    header_for(Class::Worktree, "g1")
}

fn budget(max_entries: u64, max_frame_bytes: u64) -> CustodyFrameBudgetV1 {
    CustodyFrameBudgetV1::new(max_entries, max_frame_bytes).expect("a budget under the ceilings")
}

fn roomy() -> CustodyFrameBudgetV1 {
    budget(4096, 64 * 1024 * 1024)
}

fn path(bytes: &[u8]) -> CustodyFramePathV1 {
    CustodyFramePathV1::from_bytes(bytes).expect("a valid frame path")
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
        .collect()
}

fn golden() -> Vec<u8> {
    unhex(GOLDEN_FRAME_HEX)
}

fn golden_items() -> Vec<Item> {
    vec![
        dir(b"d", 0o755),
        file(b"d/f", 0o644, b"hi"),
        link(b"d/l", b"f"),
    ]
}

/// One frame entry as the tests write and read it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Dir(Vec<u8>, u32),
    File(Vec<u8>, u32, Vec<u8>),
    Link(Vec<u8>, Vec<u8>),
}

fn dir(path: &[u8], mode: u32) -> Item {
    Item::Dir(path.to_vec(), mode)
}

fn file(path: &[u8], mode: u32, content: &[u8]) -> Item {
    Item::File(path.to_vec(), mode, content.to_vec())
}

fn link(path: &[u8], target: &[u8]) -> Item {
    Item::Link(path.to_vec(), target.to_vec())
}

fn without_content(item: &Item) -> Item {
    match item {
        Item::File(path, mode, _) => Item::File(path.clone(), *mode, Vec::new()),
        other => other.clone(),
    }
}

#[derive(Debug)]
struct Encoded {
    bytes: Vec<u8>,
    summary: CustodyFrameSummaryV1,
    receipts: Vec<CustodyFrameFileReceiptV1>,
}

fn write_item<W: Write>(
    encoder: &mut CustodyFrameEncoderV1<W>,
    item: &Item,
) -> Result<Option<CustodyFrameFileReceiptV1>, E> {
    match item {
        Item::Dir(bytes, mode) => encoder.directory(&path(bytes), *mode).map(|()| None),
        Item::File(bytes, mode, content) => encoder
            .regular_file(
                &path(bytes),
                *mode,
                content.len() as u64,
                &mut content.as_slice(),
            )
            .map(Some),
        Item::Link(bytes, target) => encoder.symlink(&path(bytes), target).map(|()| None),
    }
}

fn encode(
    header: &CustodyFrameHeaderV1,
    budget: CustodyFrameBudgetV1,
    items: &[Item],
) -> Result<Encoded, E> {
    let mut bytes = Vec::new();
    let mut encoder = CustodyFrameEncoderV1::new(&mut bytes, header, budget)?;
    let mut receipts = Vec::new();
    for item in items {
        receipts.extend(write_item(&mut encoder, item)?);
    }
    let summary = encoder.finish()?;
    Ok(Encoded {
        bytes,
        summary,
        receipts,
    })
}

/// Whether a decoding consumer reads each regular file's content or ignores it.
#[derive(Clone, Copy, Debug)]
enum Consume {
    Read,
    Skip,
}

fn refusal_of(error: &io::Error) -> E {
    E::from_io_error(error).unwrap_or_else(|| {
        panic!("the content reader refused without a typed frame error: {error}")
    })
}

fn decode_from<R: Read>(
    source: R,
    header: &CustodyFrameHeaderV1,
    budget: CustodyFrameBudgetV1,
    consume: Consume,
) -> Result<Vec<Item>, E> {
    let mut decoder = CustodyFrameDecoderV1::new(source, header, budget)?;
    let mut items = Vec::new();
    while let Some(entry) = decoder.next_entry()? {
        items.push(match entry {
            CustodyFrameEntryV1::Directory { path, mode } => {
                Item::Dir(path.as_bytes().to_vec(), mode)
            }
            CustodyFrameEntryV1::Regular {
                path,
                mode,
                length,
                mut content,
            } => {
                let mut bytes = Vec::new();
                if let Consume::Read = consume {
                    content
                        .read_to_end(&mut bytes)
                        .map_err(|error| refusal_of(&error))?;
                    assert_eq!(bytes.len() as u64, length, "the declared length is read");
                }
                Item::File(path.as_bytes().to_vec(), mode, bytes)
            }
            CustodyFrameEntryV1::Symlink { path, target } => {
                Item::Link(path.as_bytes().to_vec(), target)
            }
        });
    }
    Ok(items)
}

fn decode(
    bytes: &[u8],
    header: &CustodyFrameHeaderV1,
    budget: CustodyFrameBudgetV1,
    consume: Consume,
) -> Result<Vec<Item>, E> {
    decode_from(bytes, header, budget, consume)
}

/// Asserts that decoding refuses with exactly `expected`, whether the consumer reads content or
/// skips it.
fn assert_refused(
    bytes: &[u8],
    header: &CustodyFrameHeaderV1,
    budget: CustodyFrameBudgetV1,
    expected: E,
    row: &str,
) {
    for consume in [Consume::Read, Consume::Skip] {
        match decode(bytes, header, budget, consume) {
            Err(error) => assert_eq!(error, expected, "{row} ({consume:?})"),
            Ok(items) => panic!(
                "{row} ({consume:?}): wrong success: decoded {} entries, expected {expected:?}",
                items.len()
            ),
        }
    }
}

fn assert_decodes(bytes: &[u8], header: &CustodyFrameHeaderV1, expected: &[Item], row: &str) {
    let items = decode(bytes, header, roomy(), Consume::Read)
        .unwrap_or_else(|error| panic!("{row}: refused {error:?}"));
    assert_eq!(items, expected, "{row}");
}

/// An independent test-side writer of the §2.1 grammar. It writes whatever it is told, including
/// fields no encoder emits. [`RawFrame::seal`] appends a trailer whose count, total, and frame
/// digest are correct for the bytes written, unless a test overrides them.
struct RawFrame {
    bytes: Vec<u8>,
    entries: u64,
    total: u64,
}

impl RawFrame {
    fn with_header(
        magic: [u8; 8],
        version: u8,
        code: u8,
        generation_length: u16,
        generation: &[u8],
    ) -> Self {
        let mut bytes = magic.to_vec();
        bytes.push(version);
        bytes.push(code);
        bytes.extend_from_slice(&generation_length.to_le_bytes());
        bytes.extend_from_slice(generation);
        Self {
            bytes,
            entries: 0,
            total: 0,
        }
    }

    fn new(code: u8, generation: &[u8]) -> Self {
        Self::with_header(*b"a2a-cfr1", 1, code, len16(generation), generation)
    }

    /// The header [`header`] expects.
    fn standard() -> Self {
        Self::new(4, GENERATION.as_bytes())
    }

    fn raw(mut self, bytes: &[u8]) -> Self {
        self.bytes.extend_from_slice(bytes);
        self
    }

    fn entry(mut self, tag: u8, path: &[u8]) -> Self {
        self.entries += 1;
        self.raw(&[tag]).raw(&len16(path).to_le_bytes()).raw(path)
    }

    fn dir(self, path: &[u8], mode: u16) -> Self {
        self.entry(1, path).raw(&mode.to_le_bytes())
    }

    fn file(self, path: &[u8], mode: u16, content: &[u8]) -> Self {
        self.file_with_digest(path, mode, content, sha256(content))
    }

    fn file_with_digest(self, path: &[u8], mode: u16, content: &[u8], digest: [u8; 32]) -> Self {
        let mut frame = self
            .entry(2, path)
            .raw(&mode.to_le_bytes())
            .raw(&(content.len() as u64).to_le_bytes())
            .raw(content)
            .raw(&digest);
        frame.total += content.len() as u64;
        frame
    }

    fn link(self, path: &[u8], target: &[u8]) -> Self {
        self.entry(3, path)
            .raw(&len16(target).to_le_bytes())
            .raw(target)
    }

    fn seal(self) -> Vec<u8> {
        let (entries, total) = (self.entries, self.total);
        self.seal_with(entries, total)
    }

    fn seal_with(self, entries: u64, total: u64) -> Vec<u8> {
        self.seal_as(0xFF, entries, total, None)
    }

    /// A trailer with any tag, count, and total, and either the correct frame digest or `digest`.
    fn seal_as(mut self, tag: u8, entries: u64, total: u64, digest: Option<[u8; 32]>) -> Vec<u8> {
        self.bytes.push(tag);
        self.bytes.extend_from_slice(&entries.to_le_bytes());
        self.bytes.extend_from_slice(&total.to_le_bytes());
        let digest = digest.unwrap_or_else(|| sha256(&self.bytes));
        self.bytes.extend_from_slice(&digest);
        self.bytes
    }

    fn unsealed(self) -> Vec<u8> {
        self.bytes
    }
}

fn len16(bytes: &[u8]) -> u16 {
    u16::try_from(bytes.len()).expect("a u16 length")
}

/// `count` components joined into a path of exactly `total` bytes: 17 components of 240 bytes
/// give 4096, and the last component takes any extra byte.
fn long_components(total: usize) -> Vec<Vec<u8>> {
    let mut components: Vec<Vec<u8>> = (0..17_u8).map(|level| vec![b'a' + level; 240]).collect();
    let extra = total - 4096;
    components[16].extend(std::iter::repeat_n(b'z', extra));
    components
}

fn joined(components: &[Vec<u8>]) -> Vec<u8> {
    components.join(&b'/')
}

/// A shared sink, so a test can measure what an encoder has written while it still owns it.
#[derive(Clone, Default)]
struct SharedSink(Rc<RefCell<Vec<u8>>>);

impl SharedSink {
    fn written(&self) -> usize {
        self.0.borrow().len()
    }
}

impl Write for SharedSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A sink that accepts `accept` bytes, then fails.
struct FailingSink {
    accept: usize,
}

impl Write for FailingSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.accept == 0 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "sink closed"));
        }
        let written = buffer.len().min(self.accept);
        self.accept -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A source that yields `bytes`, then fails instead of ending.
struct FailingSource {
    bytes: Vec<u8>,
    offset: usize,
}

impl Read for FailingSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.offset == self.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "source failed",
            ));
        }
        let read = buffer.len().min(self.bytes.len() - self.offset);
        buffer[..read].copy_from_slice(&self.bytes[self.offset..self.offset + read]);
        self.offset += read;
        Ok(read)
    }
}

/// A reader that counts its calls and holds `available` bytes; by default it is at end of
/// stream.
#[derive(Default)]
struct CountingReader {
    reads: usize,
    available: usize,
}

impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        let read = buffer.len().min(self.available);
        buffer[..read].fill(0x07);
        self.available -= read;
        Ok(read)
    }
}

/// Delivers at most `step` bytes per call and interrupts every third call, so both sides must
/// handle short reads and `Interrupted`.
struct Trickle<R> {
    inner: R,
    step: usize,
    calls: usize,
}

impl<R> Trickle<R> {
    fn new(inner: R, step: usize) -> Self {
        Self {
            inner,
            step,
            calls: 0,
        }
    }
}

impl<R: Read> Read for Trickle<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.calls.is_multiple_of(3) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let limit = buffer.len().min(self.step);
        self.inner.read(&mut buffer[..limit])
    }
}

/// The round-trip fixture of §5.1, listed in canonical order.
fn round_trip_fixture() -> Vec<Item> {
    let multi_chunk: Vec<u8> = (0..(2 * 64 * 1024 + 4099))
        .map(|index: usize| (index * 31 % 251) as u8)
        .collect();
    vec![
        dir(b"A", 0o755),
        file(b"A/empty-file", 0o644, b""),
        dir(b"a", 0o700),
        file(b"a/b", 0o755, &multi_chunk),
        dir(b"a/empty-dir", 0o750),
        dir(b"a/nested", 0o755),
        dir(b"a/nested/deeper", 0o711),
        file(b"a/nested/deeper/leaf", 0o600, b"leaf\n"),
        file(b"a.b", 0o644, b"a sibling after the whole a/ subtree"),
        file(b"back\\slash", 0o640, b"a backslash is a name byte"),
        dir(b"bytes-\xff\xfe", 0o755),
        file(b"bytes-\xff\xfe/\x80raw", 0o444, b"non-UTF-8 components"),
        dir(b"links", 0o755),
        link(b"links/absolute", b"/etc/passwd"),
        link(b"links/parent", b"../a/b"),
    ]
}

/// The same fixture written by [`RawFrame`], so the encoder is checked against an independent
/// writer of the grammar.
fn raw_frame_of(items: &[Item], frame: RawFrame) -> Vec<u8> {
    items
        .iter()
        .fold(frame, |frame, item| match item {
            Item::Dir(path, mode) => frame.dir(path, u16::try_from(*mode).expect("mode")),
            Item::File(path, mode, content) => {
                frame.file(path, u16::try_from(*mode).expect("mode"), content)
            }
            Item::Link(path, target) => frame.link(path, target),
        })
        .seal()
}

// ---------------------------------------------------------------------------------------------
// §5.1 Round trip
// ---------------------------------------------------------------------------------------------

#[test]
fn control_01_round_trip_yields_exactly_the_encoded_entries() {
    let items = round_trip_fixture();
    let encoded = encode(&header(), roomy(), &items).expect("the fixture encodes");

    assert_decodes(&encoded.bytes, &header(), &items, "slice source");
    let trickled = decode_from(
        Trickle::new(encoded.bytes.as_slice(), 7),
        &header(),
        roomy(),
        Consume::Read,
    )
    .expect("a trickling, interrupting source decodes");
    assert_eq!(trickled, items);
    let skipped = decode(&encoded.bytes, &header(), roomy(), Consume::Skip)
        .expect("a consumer that skips content decodes");
    assert_eq!(
        skipped,
        items.iter().map(without_content).collect::<Vec<_>>()
    );

    // An independent writer of the grammar produces the same bytes.
    assert_eq!(
        encoded.bytes,
        raw_frame_of(&items, RawFrame::standard()),
        "the encoder agrees with the test-side grammar writer"
    );

    // Each receipt carries the streamed length and an independently computed SHA-256.
    let files: Vec<&Vec<u8>> = items
        .iter()
        .filter_map(|item| match item {
            Item::File(_, _, content) => Some(content),
            _ => None,
        })
        .collect();
    assert_eq!(encoded.receipts.len(), files.len());
    for (receipt, content) in encoded.receipts.iter().zip(files) {
        assert_eq!(receipt.length(), content.len() as u64);
        assert_eq!(receipt.sha256(), sha256(content));
    }

    // Content streamed through a trickling, interrupting reader encodes to the same bytes.
    let mut bytes = Vec::new();
    let mut encoder =
        CustodyFrameEncoderV1::new(&mut bytes, &header(), roomy()).expect("the header fits");
    for item in &items {
        match item {
            Item::File(file_path, mode, content) => {
                let mut reader = Trickle::new(content.as_slice(), 1000);
                encoder
                    .regular_file(&path(file_path), *mode, content.len() as u64, &mut reader)
                    .expect("a trickled file encodes");
            }
            other => {
                write_item(&mut encoder, other).expect("the entry encodes");
            }
        }
    }
    let summary = encoder.finish().expect("the trickled frame finishes");
    assert_eq!(bytes, encoded.bytes);
    assert_eq!(summary, encoded.summary);
}

#[test]
fn control_01_the_canonical_order_is_component_wise() {
    let ordered: [&[u8]; 9] = [
        b"A", b"B", b"a", b"a/b", b"a/b/c", b"a/c", b"a.b", b"a0", b"\xff",
    ];
    for pair in ordered.windows(2) {
        assert!(
            path(pair[0]) < path(pair[1]),
            "{:?} < {:?}",
            String::from_utf8_lossy(pair[0]),
            String::from_utf8_lossy(pair[1])
        );
    }
    // Raw byte order would put `a.b` first, because `.` (0x2e) sorts before `/` (0x2f).
    assert!(b"a.b".as_slice() < b"a/b".as_slice());
    assert!(path(b"a/b") < path(b"a.b"));
    let fixture = round_trip_fixture();
    let paths: Vec<CustodyFramePathV1> = fixture
        .iter()
        .map(|item| match item {
            Item::Dir(bytes, _) | Item::File(bytes, _, _) | Item::Link(bytes, _) => path(bytes),
        })
        .collect();
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
}

// ---------------------------------------------------------------------------------------------
// §5.2 Determinism and binding
// ---------------------------------------------------------------------------------------------

#[test]
fn control_02_the_same_class_generation_and_entries_encode_identically() {
    let items = round_trip_fixture();
    let first = encode(&header(), roomy(), &items).expect("encodes");
    let second = encode(
        &header_for(Class::Worktree, GENERATION),
        budget(1_048_576, 10 * 1024 * 1024 * 1024),
        &items,
    )
    .expect("encodes");
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.summary, second.summary);
    assert_eq!(first.receipts, second.receipts);
}

#[test]
fn control_02_changing_only_the_class_or_the_generation_changes_the_bytes() {
    let items = golden_items();
    let original = encode(&golden_header(), roomy(), &items).expect("encodes");

    let other_class = encode(&header_for(Class::Index, "g1"), roomy(), &items).expect("encodes");
    assert_ne!(other_class.bytes, original.bytes);
    assert_eq!(other_class.bytes[9], 3, "the class byte is Index's code");
    assert_ne!(
        other_class.summary.frame_sha256(),
        original.summary.frame_sha256()
    );
    assert_refused(
        &other_class.bytes,
        &golden_header(),
        roomy(),
        E::ClassMismatch,
        "class changed",
    );

    let other_generation =
        encode(&header_for(Class::Worktree, "g2"), roomy(), &items).expect("encodes");
    assert_ne!(other_generation.bytes, original.bytes);
    assert_ne!(
        other_generation.summary.frame_sha256(),
        original.summary.frame_sha256()
    );
    assert_refused(
        &other_generation.bytes,
        &golden_header(),
        roomy(),
        E::GenerationMismatch,
        "generation changed",
    );
}

#[test]
fn control_02_golden_bytes_receipts_and_summary() {
    let golden = golden();
    let encoded = encode(&golden_header(), roomy(), &golden_items()).expect("encodes");
    assert_eq!(
        hex(&encoded.bytes),
        GOLDEN_FRAME_HEX,
        "the pinned v1 encoding"
    );

    // The receipt: every field against an independent value.
    assert_eq!(encoded.receipts.len(), 1);
    let receipt = encoded.receipts[0];
    assert_eq!(receipt.length(), 2);
    assert_eq!(hex(&receipt.sha256()), GOLDEN_HI_SHA256_HEX);
    assert_eq!(receipt.sha256(), sha256(b"hi"));

    // The summary: every field against an independent value.
    let summary = encoded.summary;
    assert_eq!(summary.entries(), 3);
    assert_eq!(summary.content_bytes(), 2);
    assert_eq!(summary.frame_bytes(), 128);
    assert_eq!(summary.frame_bytes(), golden.len() as u64);
    assert_eq!(hex(&summary.frame_sha256()), GOLDEN_FRAME_SHA256_HEX);
    // `frame_sha256` is the SHA-256 of the complete emitted bytes, trailer digest included ...
    assert_eq!(summary.frame_sha256(), sha256(&golden));
    // ... and the trailer digest covers the magic through `total`.
    let (body, trailer_digest) = golden.split_at(golden.len() - 32);
    assert_eq!(hex(trailer_digest), GOLDEN_FRAME_DIGEST_HEX);
    assert_eq!(trailer_digest, sha256(body));
    assert_ne!(summary.frame_sha256().as_slice(), trailer_digest);
    assert_eq!(&body[body.len() - 17..body.len() - 16], &[0xFF]);

    assert_decodes(&golden, &golden_header(), &golden_items(), "golden");
}

// ---------------------------------------------------------------------------------------------
// §5.3 Refusals: class codes and field boundaries
// ---------------------------------------------------------------------------------------------

#[test]
fn control_03_the_class_code_table_frames_all_thirteen_classes() {
    for (class, code) in CLASS_CODES {
        let header = header_for(class, GENERATION);
        assert_eq!(header.class(), class);
        assert_eq!(header.generation_id(), GENERATION);
        let items = [dir(b"x", 0o755)];
        let encoded = encode(&header, roomy(), &items).expect("a framed class encodes");
        assert_eq!(encoded.bytes[9], code, "{class:?}");
        assert_eq!(
            encoded.bytes,
            raw_frame_of(&items, RawFrame::new(code, GENERATION.as_bytes())),
            "{class:?}"
        );
        assert_decodes(&encoded.bytes, &header, &items, &format!("{class:?}"));
    }
    // The codes follow `CustodyCoverageClassV1::ALL`, 1-based.
    for (index, class) in Class::ALL.iter().enumerate() {
        match CLASS_CODES.iter().find(|(framed, _)| framed == class) {
            Some((_, code)) => assert_eq!(usize::from(*code), index + 1, "{class:?}"),
            None => assert_eq!(*class, Class::ObjectDatabase),
        }
    }
}

#[test]
fn control_03_codes_0_2_15_16_255_and_object_database_are_refused() {
    assert_eq!(
        CustodyFrameHeaderV1::new(Class::ObjectDatabase, GENERATION),
        Err(E::ObjectDatabaseNotFramed)
    );
    for (code, expected) in [
        (0, E::UnknownClass),
        (2, E::ObjectDatabaseNotFramed),
        (15, E::UnknownClass),
        (16, E::UnknownClass),
        (255, E::UnknownClass),
    ] {
        let bytes = RawFrame::new(code, GENERATION.as_bytes()).seal();
        assert_refused(
            &bytes,
            &header(),
            roomy(),
            expected,
            &format!("code {code}"),
        );
    }
}

#[test]
fn control_03_generation_length_boundaries_on_the_constructor_and_the_decoder() {
    // The constructor: 0 / 1 / 1024 / 1025 bytes, counted in bytes, not characters.
    assert_eq!(
        CustodyFrameHeaderV1::new(Class::Worktree, ""),
        Err(E::InvalidGeneration)
    );
    for generation in ["g".to_owned(), "g".repeat(1024), "é".repeat(512)] {
        let header = CustodyFrameHeaderV1::new(Class::Worktree, &generation)
            .expect("an admitted generation");
        assert_eq!(header.generation_id(), generation);
        let encoded = encode(&header, roomy(), &[]).expect("encodes");
        assert_decodes(&encoded.bytes, &header, &[], "constructor max");
    }
    for generation in ["g".repeat(1025), format!("{}g", "é".repeat(512))] {
        assert_eq!(
            CustodyFrameHeaderV1::new(Class::Worktree, &generation),
            Err(E::InvalidGeneration),
            "{} bytes",
            generation.len()
        );
    }

    // The raw decoder: a zero length, and a 1025 length refused before any generation byte is
    // read (the frame ends at the length field).
    let zero = RawFrame::with_header(*b"a2a-cfr1", 1, 4, 0, b"").seal();
    assert_refused(&zero, &header(), roomy(), E::InvalidGeneration, "length 0");
    let over = RawFrame::with_header(*b"a2a-cfr1", 1, 4, 1025, b"").unsealed();
    assert_refused(
        &over,
        &header(),
        roomy(),
        E::InvalidGeneration,
        "length 1025",
    );
    for generation in ["g".to_owned(), "g".repeat(1024)] {
        let bytes = RawFrame::new(4, generation.as_bytes()).seal();
        assert_decodes(
            &bytes,
            &header_for(Class::Worktree, &generation),
            &[],
            &format!("length {}", generation.len()),
        );
    }
    // Not UTF-8: refused even by a decoder expecting the lossy decoding of those bytes.
    let not_utf8 = RawFrame::new(4, b"\xff\xfe").seal();
    for expected in [header_for(Class::Worktree, "\u{FFFD}\u{FFFD}"), header()] {
        assert_refused(
            &not_utf8,
            &expected,
            roomy(),
            E::InvalidGeneration,
            "not UTF-8",
        );
    }
}

#[test]
fn control_03_path_length_boundaries_on_the_constructors_and_the_decoder() {
    // 0 bytes: the implicit root is never an entry.
    let empty = E::InvalidComponent { kind: Fault::Empty };
    assert_eq!(CustodyFramePathV1::from_bytes(b""), Err(empty));
    assert_eq!(
        CustodyFramePathV1::from_components(Vec::<&[u8]>::new()),
        Err(empty)
    );
    // 1 byte.
    assert_eq!(path(b"x").as_bytes(), b"x");
    assert_eq!(
        CustodyFramePathV1::from_components([b"x"]).map(|path| path.as_bytes().to_vec()),
        Ok(b"x".to_vec())
    );
    // 4096 bytes.
    let max = long_components(4096);
    let max_bytes = joined(&max);
    assert_eq!(max_bytes.len(), 4096);
    assert_eq!(path(&max_bytes).as_bytes(), max_bytes.as_slice());
    assert_eq!(
        CustodyFramePathV1::from_components(&max),
        Ok(path(&max_bytes))
    );
    // 4097 bytes.
    let over = long_components(4097);
    let over_bytes = joined(&over);
    assert_eq!(over_bytes.len(), 4097);
    assert_eq!(
        CustodyFramePathV1::from_bytes(&over_bytes),
        Err(E::PathTooLong)
    );
    assert_eq!(
        CustodyFramePathV1::from_components(&over),
        Err(E::PathTooLong)
    );

    // The raw decoder.
    assert_refused(
        &RawFrame::standard().dir(b"", 0o755).seal(),
        &header(),
        roomy(),
        empty,
        "path length 0",
    );
    assert_decodes(
        &RawFrame::standard().dir(b"x", 0o755).seal(),
        &header(),
        &[dir(b"x", 0o755)],
        "path length 1",
    );
    let mut items: Vec<Item> = (1..max.len())
        .map(|depth| dir(&joined(&max[..depth]), 0o755))
        .collect();
    items.push(dir(&max_bytes, 0o700));
    assert_decodes(
        &raw_frame_of(&items, RawFrame::standard()),
        &header(),
        &items,
        "path length 4096",
    );
    assert_eq!(
        encode(&header(), roomy(), &items).map(|encoded| encoded.bytes),
        Ok(raw_frame_of(&items, RawFrame::standard()))
    );
    // A 4097 length is refused before any path byte is read: the frame ends at the length field.
    let over_field = RawFrame::standard()
        .raw(&[1])
        .raw(&4097_u16.to_le_bytes())
        .unsealed();
    assert_refused(
        &over_field,
        &header(),
        roomy(),
        E::PathTooLong,
        "path length 4097",
    );
}

#[test]
fn control_03_component_length_boundaries_on_the_constructors_and_the_decoder() {
    let empty = E::InvalidComponent { kind: Fault::Empty };
    let too_long = E::InvalidComponent {
        kind: Fault::TooLong,
    };
    let c255 = vec![b'c'; 255];
    let c256 = vec![b'c'; 256];

    for bytes in [b"a//b".as_slice(), b"/a", b"a/"] {
        assert_eq!(CustodyFramePathV1::from_bytes(bytes), Err(empty));
    }
    assert_eq!(
        CustodyFramePathV1::from_components([b"a".as_slice(), b""]),
        Err(empty)
    );
    assert_eq!(path(b"a/b").as_bytes(), b"a/b");
    assert_eq!(
        CustodyFramePathV1::from_components([b"a".as_slice(), b"b"]),
        Ok(path(b"a/b"))
    );
    assert_eq!(path(&c255).as_bytes(), c255.as_slice());
    assert_eq!(
        CustodyFramePathV1::from_components([&c255]),
        Ok(path(&c255))
    );
    assert_eq!(CustodyFramePathV1::from_bytes(&c256), Err(too_long));
    assert_eq!(CustodyFramePathV1::from_components([&c256]), Err(too_long));

    assert_refused(
        &RawFrame::standard().dir(b"a/", 0o755).seal(),
        &header(),
        roomy(),
        empty,
        "component length 0",
    );
    assert_decodes(
        &RawFrame::standard()
            .dir(b"a", 0o755)
            .dir(b"a/b", 0o755)
            .seal(),
        &header(),
        &[dir(b"a", 0o755), dir(b"a/b", 0o755)],
        "component length 1",
    );
    assert_decodes(
        &RawFrame::standard().dir(&c255, 0o755).seal(),
        &header(),
        &[dir(&c255, 0o755)],
        "component length 255",
    );
    assert_refused(
        &RawFrame::standard().dir(&c256, 0o755).seal(),
        &header(),
        roomy(),
        too_long,
        "component length 256",
    );
}

#[test]
fn control_03_every_component_fault_is_refused_by_both_constructors_and_the_decoder() {
    let c256 = vec![b'c'; 256];
    // Root-level rows, so the decoder reaches no later guard before the component rule.
    let rows: [(&[u8], Fault); 5] = [
        (b"", Fault::Empty),
        (b".", Fault::Dot),
        (b"..", Fault::DotDot),
        (b"a\0b", Fault::Nul),
        (&c256, Fault::TooLong),
    ];
    for (bytes, kind) in rows {
        let expected = E::InvalidComponent { kind };
        assert_eq!(
            CustodyFramePathV1::from_bytes(bytes),
            Err(expected),
            "{kind:?}"
        );
        assert_eq!(
            CustodyFramePathV1::from_components([bytes]),
            Err(expected),
            "{kind:?}"
        );
        assert_refused(
            &RawFrame::standard().dir(bytes, 0o755).seal(),
            &header(),
            roomy(),
            expected,
            &format!("decoder {kind:?}"),
        );
    }
    // Nested rows.
    for (bytes, kind) in [
        (b"a/./b".as_slice(), Fault::Dot),
        (b"a/..", Fault::DotDot),
        (b"a/b\0", Fault::Nul),
    ] {
        assert_eq!(
            CustodyFramePathV1::from_bytes(bytes),
            Err(E::InvalidComponent { kind })
        );
    }
    // Only a component list can carry a `/` inside one component.
    assert_eq!(
        CustodyFramePathV1::from_components([b"a/b"]),
        Err(E::InvalidComponent { kind: Fault::Slash })
    );
    assert_eq!(
        CustodyFramePathV1::from_components([b"a".as_slice(), b"b/"]),
        Err(E::InvalidComponent { kind: Fault::Slash })
    );
    // Every other byte is kept: backslash, non-UTF-8, controls, and case.
    for bytes in [
        b"back\\slash".as_slice(),
        b"\xff\xfe",
        b"tab\there",
        b"CASE",
        b"case",
        b"...",
        b".hidden",
    ] {
        assert_eq!(path(bytes).as_bytes(), bytes);
    }
}

#[test]
fn control_03_symlink_target_boundaries_and_nul() {
    let t4095 = vec![b't'; 4095];
    let t4096 = vec![b't'; 4096];
    // The encoder.
    let rows: [(&[u8], bool); 6] = [
        (b"", false),
        (b"t", true),
        (&t4095, true),
        (&t4096, false),
        (b"a\0b", false),
        (b"\0", false),
    ];
    for (target, admitted) in rows {
        let items = [link(b"l", target)];
        match encode(&header(), roomy(), &items) {
            Ok(encoded) => {
                assert!(admitted, "a {}-byte target was admitted", target.len());
                assert_decodes(&encoded.bytes, &header(), &items, "encoded target");
            }
            Err(error) => {
                assert!(!admitted, "a {}-byte target was refused", target.len());
                assert_eq!(error, E::InvalidSymlinkTarget);
            }
        }
    }
    // The raw decoder.
    let admitted: [&[u8]; 2] = [b"t", &t4095];
    for target in admitted {
        assert_decodes(
            &RawFrame::standard().link(b"l", target).seal(),
            &header(),
            &[link(b"l", target)],
            "raw target",
        );
    }
    let refused: [&[u8]; 2] = [b"", b"a\0b"];
    for target in refused {
        assert_refused(
            &RawFrame::standard().link(b"l", target).seal(),
            &header(),
            roomy(),
            E::InvalidSymlinkTarget,
            "raw target",
        );
    }
    // A 4096 length is refused before any target byte is read.
    let over_field = RawFrame::standard()
        .entry(3, b"l")
        .raw(&4096_u16.to_le_bytes())
        .unsealed();
    assert_refused(
        &over_field,
        &header(),
        roomy(),
        E::InvalidSymlinkTarget,
        "target length 4096",
    );
}

#[test]
fn control_03_mode_boundaries_on_the_encoder_and_the_decoder() {
    // The encoder: 0o777 and 0 are encoded exactly, for directories and regular files.
    for mode in [0o777, 0, 0o644] {
        let items = [dir(b"d", mode), file(b"f", mode, b"x")];
        let encoded = encode(&header(), roomy(), &items).expect("an admitted mode encodes");
        assert_decodes(&encoded.bytes, &header(), &items, "encoded mode");
    }
    // The encoder refuses every special bit and anything the caller failed to mask.
    for mode in [0o1000, 0o2000, 0o4000, 0o1777, 0o4755, 0o7777, 0o100644] {
        assert_eq!(
            encode(&header(), roomy(), &[dir(b"d", mode)]).map(|encoded| encoded.bytes),
            Err(E::UnsupportedMode),
            "directory {mode:o}"
        );
        assert_eq!(
            encode(&header(), roomy(), &[file(b"f", mode, b"x")]).map(|encoded| encoded.bytes),
            Err(E::UnsupportedMode),
            "regular file {mode:o}"
        );
    }
    // The raw decoder.
    assert_decodes(
        &RawFrame::standard()
            .dir(b"d", 0o777)
            .file(b"f", 0o777, b"x")
            .seal(),
        &header(),
        &[dir(b"d", 0o777), file(b"f", 0o777, b"x")],
        "raw 0o777",
    );
    for mode in [0o1000_u16, 0o4755, 0o7777, u16::MAX] {
        assert_refused(
            &RawFrame::standard().dir(b"d", mode).seal(),
            &header(),
            roomy(),
            E::UnsupportedMode,
            &format!("raw directory {mode:o}"),
        );
        assert_refused(
            &RawFrame::standard().file(b"f", mode, b"x").seal(),
            &header(),
            roomy(),
            E::UnsupportedMode,
            &format!("raw regular file {mode:o}"),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// §5.3 Refusals: format, order, content, trailer, io
// ---------------------------------------------------------------------------------------------

#[test]
fn control_03_bad_magic_and_unsupported_version_are_refused() {
    for magic in [*b"a2a-cfr2", *b"A2A-CFR1", *b"a2a-cfr\0", [0; 8]] {
        let bytes = RawFrame::with_header(magic, 1, 4, 12, GENERATION.as_bytes()).seal();
        assert_refused(&bytes, &header(), roomy(), E::BadMagic, "magic");
    }
    for version in [0, 2, 0xFF] {
        let bytes =
            RawFrame::with_header(*b"a2a-cfr1", version, 4, 12, GENERATION.as_bytes()).seal();
        assert_refused(
            &bytes,
            &header(),
            roomy(),
            E::UnsupportedVersion,
            &format!("version {version}"),
        );
    }
}

#[test]
fn control_03_unknown_tags_are_refused() {
    for tag in [0x00, 0x04, 0x7F, 0xFE] {
        // As a trailer tag whose count, total, and frame digest are otherwise correct.
        let bytes = RawFrame::standard()
            .dir(b"d", 0o755)
            .seal_as(tag, 1, 0, None);
        assert_refused(
            &bytes,
            &header(),
            roomy(),
            E::UnknownTag,
            &format!("trailer {tag:#x}"),
        );
        // In an entry position, before a valid entry.
        let bytes = RawFrame::standard().raw(&[tag]).dir(b"d", 0o755).seal();
        assert_refused(
            &bytes,
            &header(),
            roomy(),
            E::UnknownTag,
            &format!("{tag:#x}"),
        );
    }
}

#[test]
fn control_03_bytes_after_the_trailer_are_refused() {
    let whole = golden();
    let suffixes: [&[u8]; 3] = [b"\0", b"\xff", &whole];
    for suffix in suffixes {
        let mut bytes = golden();
        bytes.extend_from_slice(suffix);
        assert_refused(
            &bytes,
            &golden_header(),
            roomy(),
            E::TrailingBytes,
            &format!("{} trailing bytes", suffix.len()),
        );
    }
}

#[test]
fn control_03_out_of_order_entries_are_refused_by_the_encoder_and_the_decoder() {
    let rows: [(&str, Vec<Item>); 5] = [
        (
            "reversed siblings",
            vec![dir(b"b", 0o755), dir(b"a", 0o755)],
        ),
        ("duplicate", vec![dir(b"a", 0o755), dir(b"a", 0o755)]),
        (
            "duplicate across types",
            vec![dir(b"a", 0o755), file(b"a", 0o644, b"")],
        ),
        (
            "raw byte order",
            vec![
                dir(b"a", 0o755),
                file(b"a.b", 0o644, b""),
                file(b"a/b", 0o644, b""),
            ],
        ),
        ("case", vec![dir(b"a", 0o755), dir(b"A", 0o755)]),
    ];
    for (row, items) in rows {
        assert_eq!(
            encode(&header(), roomy(), &items).map(|encoded| encoded.bytes),
            Err(E::OutOfOrder),
            "encoder: {row}"
        );
        assert_refused(
            &raw_frame_of(&items, RawFrame::standard()),
            &header(),
            roomy(),
            E::OutOfOrder,
            &format!("decoder: {row}"),
        );
    }
}

#[test]
fn control_03_an_entry_without_a_directory_parent_is_refused_by_the_encoder_and_the_decoder() {
    let rows: [(&str, Vec<Item>); 5] = [
        ("absent parent", vec![file(b"a/b", 0o644, b"")]),
        (
            "regular file as parent",
            vec![file(b"a", 0o644, b""), file(b"a/b", 0o644, b"")],
        ),
        (
            "symlink as parent",
            vec![link(b"a", b"t"), dir(b"a/b", 0o755)],
        ),
        (
            "absent intermediate",
            vec![dir(b"a", 0o755), file(b"a/c/d", 0o644, b"")],
        ),
        (
            "closed parent",
            vec![dir(b"a", 0o755), dir(b"a/b", 0o755), dir(b"a/b.c/d", 0o755)],
        ),
    ];
    for (row, items) in rows {
        assert_eq!(
            encode(&header(), roomy(), &items).map(|encoded| encoded.bytes),
            Err(E::MissingParent),
            "encoder: {row}"
        );
        assert_refused(
            &raw_frame_of(&items, RawFrame::standard()),
            &header(),
            roomy(),
            E::MissingParent,
            &format!("decoder: {row}"),
        );
    }
}

#[test]
fn control_03_a_reader_that_disagrees_with_the_declared_length_is_refused() {
    let short = E::ContentLengthMismatch {
        kind: CustodyFrameLengthMismatchV1::Short,
    };
    let long = E::ContentLengthMismatch {
        kind: CustodyFrameLengthMismatchV1::Long,
    };
    let chunk = 64 * 1024;
    for (declared, supplied, expected) in [
        (10, 9, Some(short)),
        (10, 0, Some(short)),
        (chunk + 7, chunk + 6, Some(short)),
        (10, 11, Some(long)),
        (0, 1, Some(long)),
        (chunk, chunk + 1, Some(long)),
        (10, 10, None),
        (0, 0, None),
        (chunk, chunk, None),
    ] {
        let content = vec![0x5A; supplied];
        let mut encoder =
            CustodyFrameEncoderV1::new(Vec::new(), &header(), roomy()).expect("the header fits");
        let outcome =
            encoder.regular_file(&path(b"f"), 0o644, declared as u64, &mut content.as_slice());
        match expected {
            Some(expected) => assert_eq!(outcome, Err(expected), "{declared} vs {supplied}"),
            None => assert_eq!(
                outcome.map(|receipt| receipt.length()),
                Ok(declared as u64),
                "{declared} vs {supplied}"
            ),
        }
    }
}

#[test]
fn control_03_the_read_that_exhausts_corrupted_content_refuses() {
    for (row, stored, described) in [
        ("one byte", b"hello".to_vec(), b"hellp".to_vec()),
        ("zero length", Vec::new(), b"x".to_vec()),
        (
            "multi chunk",
            vec![0x33; 3 * 64 * 1024],
            vec![0x34; 3 * 64 * 1024],
        ),
    ] {
        let bytes = RawFrame::standard()
            .file_with_digest(b"f", 0o644, &stored, sha256(&described))
            .seal();
        let mut decoder =
            CustodyFrameDecoderV1::new(bytes.as_slice(), &header(), roomy()).expect("header");
        let Some(CustodyFrameEntryV1::Regular { mut content, .. }) =
            decoder.next_entry().expect("the entry is yielded")
        else {
            panic!("{row}: not a regular file");
        };
        // Every read before the exhausting one returns data; the exhausting one refuses.
        let mut buffer = vec![0; 64 * 1024];
        let refusal = loop {
            match content.read(&mut buffer) {
                Ok(0) => panic!("{row}: the content ended without a digest refusal"),
                Ok(_) => {}
                Err(error) => break refusal_of(&error),
            }
        };
        assert_eq!(refusal, E::ContentDigestMismatch, "{row}");
        assert_refused(&bytes, &header(), roomy(), E::ContentDigestMismatch, row);
    }
}

#[test]
fn control_03_trailer_count_total_and_frame_digest_mismatches_are_refused() {
    let entries = || {
        RawFrame::standard()
            .dir(b"d", 0o755)
            .file(b"d/f", 0o644, b"hi")
    };
    for (entries_field, total_field, expected) in [
        (1, 2, E::CountMismatch),
        (3, 2, E::CountMismatch),
        (0, 0, E::CountMismatch),
        (2, 1, E::TotalMismatch),
        (2, 3, E::TotalMismatch),
    ] {
        assert_refused(
            &entries().seal_with(entries_field, total_field),
            &header(),
            roomy(),
            expected,
            &format!("entries {entries_field} total {total_field}"),
        );
    }
    assert_refused(
        &entries().seal_as(0xFF, 2, 2, Some([0; 32])),
        &header(),
        roomy(),
        E::FrameDigestMismatch,
        "zero digest",
    );
    // A change that still parses: the directory mode 0o755 becomes 0o754.
    let mut bytes = golden();
    let mode_offset = 14 + 4;
    assert_eq!(&bytes[mode_offset..mode_offset + 2], &[0xed, 0x01]);
    bytes[mode_offset] = 0xec;
    assert_refused(
        &bytes,
        &golden_header(),
        roomy(),
        E::FrameDigestMismatch,
        "a still-valid mode",
    );
}

#[test]
fn control_03_io_failures_are_refused_as_io() {
    let broken = E::Io(io::ErrorKind::BrokenPipe);
    // The encoder's sink fails at the header, mid-record, mid-content, and at the trailer.
    assert!(matches!(
        CustodyFrameEncoderV1::new(FailingSink { accept: 3 }, &header(), roomy()),
        Err(error) if error == broken
    ));
    let content = vec![0x11; 100_000];
    for accept in [24 + 3, 24 + 20, 24 + 60_000, 24 + 48 + 100_000 + 5] {
        let mut encoder = CustodyFrameEncoderV1::new(FailingSink { accept }, &header(), roomy())
            .expect("the header fits");
        let outcome = encoder
            .regular_file(&path(b"f"), 0o644, 100_000, &mut content.as_slice())
            .and_then(|_| encoder.finish());
        assert_eq!(outcome, Err(broken), "sink accepts {accept}");
    }
    // The encoder's content reader fails.
    let mut encoder =
        CustodyFrameEncoderV1::new(Vec::new(), &header(), roomy()).expect("the header fits");
    let mut failing = FailingSource {
        bytes: vec![1, 2, 3],
        offset: 0,
    };
    assert_eq!(
        encoder.regular_file(&path(b"f"), 0o644, 10, &mut failing),
        Err(E::Io(io::ErrorKind::PermissionDenied))
    );
    // The decoder's source fails at every offset: in the header, an entry, content, or trailer.
    let frame = golden();
    for offset in 0..frame.len() {
        for consume in [Consume::Read, Consume::Skip] {
            let source = FailingSource {
                bytes: frame[..offset].to_vec(),
                offset: 0,
            };
            assert_eq!(
                decode_from(source, &golden_header(), roomy(), consume),
                Err(E::Io(io::ErrorKind::PermissionDenied)),
                "source fails at {offset} ({consume:?})"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// §5.4 Truncation sweep and §5.5 flip sweep
// ---------------------------------------------------------------------------------------------

#[test]
fn control_04_every_proper_prefix_is_truncated() {
    let frame = golden();
    assert_decodes(&frame, &golden_header(), &golden_items(), "the whole frame");
    for cut in 0..frame.len() {
        for consume in [Consume::Read, Consume::Skip] {
            match decode(&frame[..cut], &golden_header(), roomy(), consume) {
                Err(E::Truncated) => {}
                other => panic!("prefix of {cut} bytes ({consume:?}): {other:?}"),
            }
        }
    }
}

fn variant_name(error: E) -> String {
    format!("{error:?}")
        .split([' ', '(', '{'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn control_05_every_single_bit_flip_is_refused() {
    let frame = golden();
    // The refusal each flip produced, by byte offset, and in total.
    let mut by_offset: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let mut completed = Vec::new();
    for index in 0..frame.len() {
        for bit in 0..8 {
            let mut flipped = frame.clone();
            flipped[index] ^= 1 << bit;
            for consume in [Consume::Read, Consume::Skip] {
                match decode(&flipped, &golden_header(), roomy(), consume) {
                    Ok(_) => completed.push((index, bit, consume)),
                    Err(error) => {
                        let variant = variant_name(error);
                        *by_offset
                            .entry(index)
                            .or_default()
                            .entry(variant.clone())
                            .or_default() += 1;
                        *outcomes.entry(variant).or_default() += 1;
                    }
                }
            }
        }
    }
    for (index, variants) in &by_offset {
        eprintln!("flip offset {index:3}: {variants:?}");
    }
    eprintln!("flip sweep over {} bytes: {outcomes:?}", frame.len());
    assert!(
        completed.is_empty(),
        "flips that completed decoding: {completed:?}"
    );
    assert_eq!(outcomes.values().sum::<usize>(), frame.len() * 8 * 2);
    assert!(!outcomes.contains_key("Poisoned"));
}

// ---------------------------------------------------------------------------------------------
// §5.6 Budgets
// ---------------------------------------------------------------------------------------------

#[test]
fn control_06_the_entry_limit_admits_max_and_refuses_max_plus_one_on_both_sides() {
    let items = golden_items();
    let frame = encode(&golden_header(), budget(3, 1 << 20), &items)
        .expect("three entries under a limit of three")
        .bytes;
    assert_eq!(frame, golden());
    assert!(decode(&frame, &golden_header(), budget(3, 1 << 20), Consume::Read).is_ok());

    // The encoder refuses the third record before any byte of it is written.
    let sink = SharedSink::default();
    let mut encoder =
        CustodyFrameEncoderV1::new(sink.clone(), &golden_header(), budget(2, 1 << 20))
            .expect("the header fits");
    write_item(&mut encoder, &items[0]).expect("first");
    write_item(&mut encoder, &items[1]).expect("second");
    let before = sink.written();
    assert_eq!(write_item(&mut encoder, &items[2]), Err(E::EntryLimit));
    assert_eq!(sink.written(), before, "no byte of the refused record");
    assert_refused(
        &frame,
        &golden_header(),
        budget(2, 1 << 20),
        E::EntryLimit,
        "decoder max+1",
    );

    // A limit of zero admits the empty frame only.
    let empty = encode(&header(), budget(0, 1 << 20), &[]).expect("zero entries");
    assert_decodes(&empty.bytes, &header(), &[], "empty");
    assert!(decode(&empty.bytes, &header(), budget(0, 1 << 20), Consume::Read).is_ok());
    assert_eq!(
        encode(&header(), budget(0, 1 << 20), &[dir(b"d", 0o755)]).map(|encoded| encoded.bytes),
        Err(E::EntryLimit)
    );
    assert_refused(
        &RawFrame::standard().dir(b"d", 0o755).seal(),
        &header(),
        budget(0, 1 << 20),
        E::EntryLimit,
        "decoder, limit 0",
    );
}

#[test]
fn control_06_frame_bytes_admit_the_exact_size_and_refuse_one_byte_less_on_both_sides() {
    let items = golden_items();
    let size = golden().len() as u64;
    let at_max = encode(&golden_header(), budget(16, size), &items).expect("the exact size");
    assert_eq!(at_max.bytes, golden());
    assert_eq!(at_max.summary.frame_bytes(), size);
    assert!(decode(&golden(), &golden_header(), budget(16, size), Consume::Read).is_ok());

    // The encoder refuses the last record before any byte of it is written: the trailer is
    // reserved, so the frame can never outgrow the budget at `finish`.
    let sink = SharedSink::default();
    let mut encoder =
        CustodyFrameEncoderV1::new(sink.clone(), &golden_header(), budget(16, size - 1))
            .expect("the header fits");
    write_item(&mut encoder, &items[0]).expect("first");
    write_item(&mut encoder, &items[1]).expect("second");
    let before = sink.written();
    assert_eq!(write_item(&mut encoder, &items[2]), Err(E::ByteBudget));
    assert_eq!(sink.written(), before, "no byte of the refused record");
    assert_refused(
        &golden(),
        &golden_header(),
        budget(16, size - 1),
        E::ByteBudget,
        "decoder max+1",
    );

    // The header and trailer alone.
    let empty = encode(&header(), roomy(), &[]).expect("empty").bytes;
    let empty_size = empty.len() as u64;
    assert_eq!(empty_size, 24 + 49);
    assert!(encode(&header(), budget(0, empty_size), &[]).is_ok());
    assert!(decode(&empty, &header(), budget(0, empty_size), Consume::Read).is_ok());
    for limit in [empty_size - 1, 48, 0] {
        let sink = SharedSink::default();
        assert!(
            matches!(
                CustodyFrameEncoderV1::new(sink.clone(), &header(), budget(0, limit)),
                Err(E::ByteBudget)
            ),
            "encoder limit {limit}"
        );
        assert_eq!(sink.written(), 0, "encoder limit {limit} wrote nothing");
        assert_refused(
            &empty,
            &header(),
            budget(0, limit),
            E::ByteBudget,
            &format!("decoder limit {limit}"),
        );
    }
}

#[test]
fn control_06_a_regular_file_reserves_its_declared_length_before_writing_any_byte() {
    // Header 24, directory `d` 6, trailer 49; a file `d/f` of n bytes is 48 + n.
    let limit = 24 + 6 + 49 + 48 + 100;
    let sink = SharedSink::default();
    let mut encoder = CustodyFrameEncoderV1::new(sink.clone(), &header(), budget(16, limit))
        .expect("the header fits");
    encoder.directory(&path(b"d"), 0o755).expect("directory");
    let before = sink.written();
    let mut reader = CountingReader {
        reads: 0,
        available: 101,
    };
    assert_eq!(
        encoder.regular_file(&path(b"d/f"), 0o644, 101, &mut reader),
        Err(E::ByteBudget)
    );
    assert_eq!(sink.written(), before, "no byte of the refused record");
    assert_eq!(reader.reads, 0, "the content reader was never touched");

    let mut encoder =
        CustodyFrameEncoderV1::new(Vec::new(), &header(), budget(16, limit)).expect("header");
    encoder.directory(&path(b"d"), 0o755).expect("directory");
    encoder
        .regular_file(&path(b"d/f"), 0o644, 100, &mut [7_u8; 100].as_slice())
        .expect("the exact fit");
    assert_eq!(encoder.finish().expect("finishes").frame_bytes(), limit);
}

#[test]
fn control_06_declared_lengths_near_u64_max_refuse_without_panic() {
    // Encoder: the record size overflows (MAX, MAX - 31), the running frame size overflows
    // (MAX - 50), or the size is merely over budget (MAX - 1000).
    for declared in [u64::MAX, u64::MAX - 31, u64::MAX - 50, u64::MAX - 1000] {
        let sink = SharedSink::default();
        let mut encoder =
            CustodyFrameEncoderV1::new(sink.clone(), &header(), roomy()).expect("the header fits");
        let before = sink.written();
        let mut reader = CountingReader::default();
        assert_eq!(
            encoder.regular_file(&path(b"f"), 0o644, declared, &mut reader),
            Err(E::ByteBudget),
            "encoder {declared}"
        );
        assert_eq!(sink.written(), before, "encoder {declared} wrote nothing");
        assert_eq!(reader.reads, 0, "encoder {declared} read nothing");
    }
    // Decoder: the content reservation overflows (MAX, MAX - 31), the admitted total overflows
    // (MAX - 40), or the reservation is merely over budget (MAX - 1000). The frame ends at the
    // length field, so anything but a refusal before reading would be `Truncated`.
    for declared in [u64::MAX, u64::MAX - 31, u64::MAX - 40, u64::MAX - 1000] {
        let bytes = RawFrame::standard()
            .entry(2, b"f")
            .raw(&0o644_u16.to_le_bytes())
            .raw(&declared.to_le_bytes())
            .unsealed();
        assert_refused(
            &bytes,
            &header(),
            roomy(),
            E::ByteBudget,
            &format!("decoder {declared}"),
        );
    }
}

#[test]
fn control_06_the_budget_constructor_refuses_values_above_the_v1_ceilings() {
    assert_eq!(MAX_ENTRIES_V1, 1_048_576);
    assert_eq!(MAX_FRAME_BYTES_V1, 10 * (1 << 30));
    let at_ceiling =
        CustodyFrameBudgetV1::new(1_048_576, 10_737_418_240).expect("the ceilings themselves");
    assert_eq!(at_ceiling.max_entries(), 1_048_576);
    assert_eq!(at_ceiling.max_frame_bytes(), 10_737_418_240);
    assert_eq!(
        CustodyFrameBudgetV1::new(1_048_577, 1024),
        Err(E::EntryLimit)
    );
    assert_eq!(
        CustodyFrameBudgetV1::new(u64::MAX, 1024),
        Err(E::EntryLimit)
    );
    assert_eq!(
        CustodyFrameBudgetV1::new(16, 10_737_418_241),
        Err(E::ByteBudget)
    );
    assert_eq!(CustodyFrameBudgetV1::new(16, u64::MAX), Err(E::ByteBudget));
}

// ---------------------------------------------------------------------------------------------
// §5.7 Replay binding
// ---------------------------------------------------------------------------------------------

#[test]
fn control_07_a_frame_is_refused_under_another_class_or_generation() {
    let frame = encode(&header(), roomy(), &golden_items())
        .expect("encodes")
        .bytes;
    for (class, _) in CLASS_CODES {
        if class == Class::Worktree {
            assert_decodes(&frame, &header(), &golden_items(), "the original role");
            continue;
        }
        assert_refused(
            &frame,
            &header_for(class, GENERATION),
            roomy(),
            E::ClassMismatch,
            &format!("{class:?}"),
        );
    }
    for generation in [
        "generation-8",
        "generation-",
        "generation-77",
        "Generation-7",
        "g",
    ] {
        assert_refused(
            &frame,
            &header_for(Class::Worktree, generation),
            roomy(),
            E::GenerationMismatch,
            generation,
        );
    }
    // `object_database` is refused on both sides.
    assert_eq!(
        CustodyFrameHeaderV1::new(Class::ObjectDatabase, GENERATION),
        Err(E::ObjectDatabaseNotFramed)
    );
    let mut object_database = frame.clone();
    object_database[9] = 2;
    assert_refused(
        &object_database,
        &header(),
        roomy(),
        E::ObjectDatabaseNotFramed,
        "code 2",
    );
}

// ---------------------------------------------------------------------------------------------
// §5.8 Poisoning
// ---------------------------------------------------------------------------------------------

#[test]
fn control_08_an_encoder_refusal_poisons_every_later_call() {
    type Trigger = fn(&mut CustodyFrameEncoderV1<Vec<u8>>) -> Result<(), E>;
    let triggers: [(&str, Trigger, E); 5] = [
        (
            "mode",
            |encoder| encoder.directory(&path(b"a"), 0o4755),
            E::UnsupportedMode,
        ),
        (
            "order",
            |encoder| {
                encoder.directory(&path(b"b"), 0o755)?;
                encoder.directory(&path(b"a"), 0o755)
            },
            E::OutOfOrder,
        ),
        (
            "entry limit",
            |encoder| {
                encoder.directory(&path(b"a"), 0o755)?;
                encoder.directory(&path(b"b"), 0o755)
            },
            E::EntryLimit,
        ),
        (
            "short content",
            |encoder| {
                encoder
                    .regular_file(&path(b"a"), 0o644, 5, &mut b"abc".as_slice())
                    .map(|_| ())
            },
            E::ContentLengthMismatch {
                kind: CustodyFrameLengthMismatchV1::Short,
            },
        ),
        (
            "symlink target",
            |encoder| encoder.symlink(&path(b"a"), b""),
            E::InvalidSymlinkTarget,
        ),
    ];
    for (row, trigger, expected) in triggers {
        let mut encoder = CustodyFrameEncoderV1::new(Vec::new(), &header(), budget(1, 1 << 20))
            .expect("the header fits");
        assert_eq!(trigger(&mut encoder), Err(expected), "{row}");
        // Each later call is admissible on its own, and each returns `Poisoned`.
        assert_eq!(
            encoder.directory(&path(b"z"), 0o755),
            Err(E::Poisoned),
            "{row}"
        );
        assert_eq!(
            encoder.regular_file(&path(b"z1"), 0o644, 0, &mut io::empty()),
            Err(E::Poisoned),
            "{row}"
        );
        assert_eq!(
            encoder.symlink(&path(b"z2"), b"t"),
            Err(E::Poisoned),
            "{row}"
        );
        assert_eq!(encoder.finish(), Err(E::Poisoned), "{row}");
    }
}

#[test]
fn control_08_a_decoder_refusal_poisons_every_later_call() {
    // An unknown tag followed by an otherwise valid entry and trailer.
    let bytes = RawFrame::standard().raw(&[0x04]).dir(b"d", 0o755).seal();
    let mut decoder =
        CustodyFrameDecoderV1::new(bytes.as_slice(), &header(), roomy()).expect("header");
    assert_eq!(
        decoder.next_entry().map(|entry| entry.is_some()),
        Err(E::UnknownTag)
    );
    for _ in 0..3 {
        assert_eq!(
            decoder.next_entry().map(|entry| entry.is_some()),
            Err(E::Poisoned)
        );
    }

    // A trailer refusal: a later call never reports the frame complete.
    let bytes = RawFrame::standard().dir(b"d", 0o755).seal_with(2, 0);
    let mut decoder =
        CustodyFrameDecoderV1::new(bytes.as_slice(), &header(), roomy()).expect("header");
    assert!(matches!(
        decoder.next_entry(),
        Ok(Some(CustodyFrameEntryV1::Directory { .. }))
    ));
    assert_eq!(
        decoder.next_entry().map(|entry| entry.is_some()),
        Err(E::CountMismatch)
    );
    assert_eq!(
        decoder.next_entry().map(|entry| entry.is_some()),
        Err(E::Poisoned)
    );

    // A content refusal through the reader poisons the reader and the decoder.
    let bytes = RawFrame::standard()
        .file_with_digest(b"f", 0o644, b"hello", sha256(b"hellp"))
        .dir(b"g", 0o755)
        .seal();
    let mut decoder =
        CustodyFrameDecoderV1::new(bytes.as_slice(), &header(), roomy()).expect("header");
    {
        let Some(CustodyFrameEntryV1::Regular { mut content, .. }) =
            decoder.next_entry().expect("the entry is yielded")
        else {
            panic!("not a regular file");
        };
        let mut buffer = [0; 16];
        let first = content.read(&mut buffer).expect_err("the digest refusal");
        assert_eq!(refusal_of(&first), E::ContentDigestMismatch);
        let second = content.read(&mut buffer).expect_err("poisoned");
        assert_eq!(refusal_of(&second), E::Poisoned);
    }
    assert_eq!(
        decoder.next_entry().map(|entry| entry.is_some()),
        Err(E::Poisoned)
    );
}

// ---------------------------------------------------------------------------------------------
// §5.9 Skip safety
// ---------------------------------------------------------------------------------------------

/// One skip-safety row: the stored content, the content its digest describes, the bytes read
/// before the consumer drops the reader, and whether an entry follows.
struct SkipRow<'a> {
    row: &'a str,
    stored: &'a [u8],
    described: &'a [u8],
    read_first: usize,
    followed: bool,
}

impl<'a> SkipRow<'a> {
    fn new(
        row: &'a str,
        stored: &'a [u8],
        described: &'a [u8],
        read_first: usize,
        followed: bool,
    ) -> Self {
        Self {
            row,
            stored,
            described,
            read_first,
            followed,
        }
    }
}

#[test]
fn control_09_skipped_content_is_still_digest_checked_by_the_next_call() {
    let multi = vec![0x42; 100 * 1024];
    let mut corrupted_multi = multi.clone();
    corrupted_multi[70_000] ^= 0x01;
    let rows = [
        SkipRow::new("skipped entirely", b"hellp", b"hello", 0, true),
        SkipRow::new("partly read", b"hellp", b"hello", 2, true),
        SkipRow::new("zero length", b"", b"x", 0, true),
        SkipRow::new("multi chunk", &corrupted_multi, &multi, 0, true),
        SkipRow::new("last entry", b"hellp", b"hello", 0, false),
    ];
    for SkipRow {
        row,
        stored,
        described,
        read_first,
        followed,
    } in rows
    {
        let mut frame =
            RawFrame::standard().file_with_digest(b"f", 0o644, stored, sha256(described));
        if followed {
            frame = frame.dir(b"g", 0o755);
        }
        let bytes = frame.seal();
        let mut decoder =
            CustodyFrameDecoderV1::new(bytes.as_slice(), &header(), roomy()).expect("header");
        match decoder.next_entry().expect("the entry is yielded") {
            Some(CustodyFrameEntryV1::Regular { mut content, .. }) => {
                let mut prefix = vec![0; read_first];
                content.read_exact(&mut prefix).expect("the prefix reads");
            }
            other => panic!("{row}: {other:?}"),
        }
        assert_eq!(
            decoder.next_entry().map(|entry| entry.is_some()),
            Err(E::ContentDigestMismatch),
            "{row}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// §5.10 Portability
// ---------------------------------------------------------------------------------------------

#[test]
fn control_10_the_frame_module_is_portable_and_effect_free() {
    let source = include_str!("custody_frame.rs");
    for forbidden in [
        "std::os::",
        "cfg(unix)",
        "cfg(windows)",
        "cfg(target_os",
        "libc::",
        "std::fs",
        "std::process",
        "std::net",
        "std::env",
    ] {
        assert!(
            !source.contains(forbidden),
            "custody_frame.rs names `{forbidden}`"
        );
    }
}
