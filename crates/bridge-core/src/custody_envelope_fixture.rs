//! The shared test-only envelope fixture (ADR-0041 slice 2B3a task 1): the deterministic fixture
//! sealer the 2B2 exporter's controls seal with, and its mirror opener, which the 2B3a capsule
//! reader opens with.
//!
//! The envelope is the fixed magic followed by every plaintext byte verbatim. It is a fixture: it
//! makes no confidentiality claim. The sealer moved here from `custody_export_tests.rs`
//! byte-for-byte in behavior; the opener is new.

use crate::custody_capsule::{
    sealed, CustodyCapsuleErrorV1, CustodyEnvelopeChunkSinkV1, CustodyEnvelopeChunkSourceV1,
    CustodyEnvelopeChunkV1, CustodyEnvelopeContextV1, CustodyEnvelopeFormatV1,
    CustodyEnvelopeMetadataV1, CustodyEnvelopeOpenReceiptV1, CustodyEnvelopeOpenRequestV1,
    CustodyEnvelopeOpenerV1, CustodyEnvelopeSealReceiptV1, CustodyEnvelopeSealerV1,
    CustodyEnvelopeSinkValidatorV1,
};
use crate::execution_policy::Sha256HexV1;
use ring::digest;
use std::cell::{Cell, RefCell};

pub(crate) const FIXTURE_ENVELOPE_MAGIC_V1: &[u8; 8] = b"A2AFIX1\n";

/// The one envelope format the fixture sealer's exports carry and the fixture opener accepts.
pub(crate) fn fixture_envelope_format_v1() -> CustodyEnvelopeFormatV1 {
    CustodyEnvelopeFormatV1::new("capsule-v1", "a2a-bridge-2b2-fixture", "0.1.0")
        .expect("the fixture envelope format is valid")
}

// ---------------------------------------------------------------------------------------------
// The deterministic fixture sealer
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SealerFaultV1 {
    /// Read at most this many plaintext chunks before sealing anyway (control 16).
    pub(crate) stop_after_chunks: Option<usize>,
    /// Mint each receipt from the PREVIOUS call's context (control 15).
    pub(crate) swap_receipt_contexts: bool,
}

/// The envelope is the fixed magic chunk followed by every plaintext chunk verbatim. It is a
/// fixture: it makes no confidentiality claim.
pub(crate) struct FixtureSealerV1 {
    fault: SealerFaultV1,
    /// Only artifacts whose name contains this marker are faulted; `None` faults every artifact.
    target: Option<Vec<u8>>,
    previous_context: RefCell<Option<CustodyEnvelopeContextV1>>,
}

impl FixtureSealerV1 {
    pub(crate) fn honest() -> Self {
        Self {
            fault: SealerFaultV1::default(),
            target: None,
            previous_context: RefCell::new(None),
        }
    }

    pub(crate) fn faulted(fault: SealerFaultV1, target: Option<&[u8]>) -> Self {
        Self {
            fault,
            target: target.map(<[u8]>::to_vec),
            previous_context: RefCell::new(None),
        }
    }

    fn applies_to(&self, context: &CustodyEnvelopeContextV1) -> bool {
        match &self.target {
            None => true,
            Some(marker) => context
                .artifact_name()
                .as_bytes()
                .windows(marker.len())
                .any(|window| window == marker.as_slice()),
        }
    }
}

impl sealed::Sealed for FixtureSealerV1 {}

impl CustodyEnvelopeSealerV1 for FixtureSealerV1 {
    fn seal(
        &self,
        context: &CustodyEnvelopeContextV1,
        plaintext: &mut dyn CustodyEnvelopeChunkSourceV1,
        _metadata: &CustodyEnvelopeMetadataV1,
        ciphertext: &mut dyn CustodyEnvelopeChunkSinkV1,
    ) -> Result<CustodyEnvelopeSealReceiptV1, CustodyCapsuleErrorV1> {
        let faulted = self.applies_to(context);
        let limit = if faulted {
            self.fault.stop_after_chunks
        } else {
            None
        };

        let mut body: Vec<Vec<u8>> = Vec::new();
        let mut read = 0_usize;
        while limit.is_none_or(|limit| read < limit) {
            let Some(chunk) = plaintext.next_chunk()? else {
                break;
            };
            read += 1;
            if !chunk.bytes().is_empty() {
                body.push(chunk.bytes().to_vec());
            }
        }

        let mut pieces = vec![FIXTURE_ENVELOPE_MAGIC_V1.to_vec()];
        pieces.extend(body);

        // The sealer keeps its own mirror of the destination's validator so it can mint a receipt
        // without touching the exporter-owned sink validator.
        let mut mirror = CustodyEnvelopeSinkValidatorV1::new(ciphertext.limits());
        let count = pieces.len();
        for (index, piece) in pieces.into_iter().enumerate() {
            let ordinal = u32::try_from(index).map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
            let chunk = CustodyEnvelopeChunkV1::new(ordinal, piece, index + 1 == count)?;
            mirror.accept_chunk(&chunk)?;
            ciphertext.write_chunk(chunk)?;
        }
        let completion = mirror.finish()?;

        let receipt_context = if faulted && self.fault.swap_receipt_contexts {
            self.previous_context
                .borrow()
                .clone()
                .unwrap_or_else(|| context.clone())
        } else {
            context.clone()
        };
        *self.previous_context.borrow_mut() = Some(context.clone());
        CustodyEnvelopeSealReceiptV1::new(&receipt_context, completion)
    }
}

// ---------------------------------------------------------------------------------------------
// The mirror fixture opener
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenerFaultV1 {
    /// Report a plaintext length one byte longer than the bytes written.
    pub(crate) lie_about_receipt_length: bool,
    /// Refuse every open, before reading anything.
    pub(crate) refuse: bool,
    /// Read at most this many ciphertext chunks, then finish the plaintext anyway.
    pub(crate) stop_after_chunks: Option<usize>,
}

/// Opens the fixture envelope: it proves the magic, then forwards every later byte as plaintext.
///
/// The ciphertext is one byte stream. Chunk boundaries are transport, not framing, so the magic is
/// accumulated across as many chunks as it takes, and nothing is written to the plaintext sink
/// before the whole magic is proven.
pub(crate) struct FixtureOpenerV1 {
    fault: OpenerFaultV1,
    calls: Cell<usize>,
}

impl FixtureOpenerV1 {
    pub(crate) fn honest() -> Self {
        Self::faulted(OpenerFaultV1::default())
    }

    pub(crate) fn faulted(fault: OpenerFaultV1) -> Self {
        Self {
            fault,
            calls: Cell::new(0),
        }
    }

    /// Every `open` call made so far, refused ones included.
    pub(crate) fn calls(&self) -> usize {
        self.calls.get()
    }
}

impl sealed::Sealed for FixtureOpenerV1 {}

impl CustodyEnvelopeOpenerV1 for FixtureOpenerV1 {
    fn open(
        &self,
        request: &CustodyEnvelopeOpenRequestV1,
        ciphertext: &mut dyn CustodyEnvelopeChunkSourceV1,
        plaintext: &mut dyn CustodyEnvelopeChunkSinkV1,
    ) -> Result<CustodyEnvelopeOpenReceiptV1, CustodyCapsuleErrorV1> {
        self.calls.set(self.calls.get() + 1);
        if self.fault.refuse || request.context().format() != &fixture_envelope_format_v1() {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }

        let mut writer = PlaintextWriterV1 {
            sink: plaintext,
            held: None,
            ordinal: 0,
            length: 0,
            digest: digest::Context::new(&digest::SHA256),
        };
        let mut magic = Vec::with_capacity(FIXTURE_ENVELOPE_MAGIC_V1.len());
        let mut read = 0_usize;
        while self
            .fault
            .stop_after_chunks
            .is_none_or(|limit| read < limit)
        {
            let Some(chunk) = ciphertext.next_chunk()? else {
                break;
            };
            read += 1;
            let mut bytes = chunk.bytes();
            if magic.len() < FIXTURE_ENVELOPE_MAGIC_V1.len() {
                let take = (FIXTURE_ENVELOPE_MAGIC_V1.len() - magic.len()).min(bytes.len());
                magic.extend_from_slice(&bytes[..take]);
                bytes = &bytes[take..];
                if magic.len() < FIXTURE_ENVELOPE_MAGIC_V1.len() {
                    continue;
                }
                if magic.as_slice() != FIXTURE_ENVELOPE_MAGIC_V1 {
                    return Err(CustodyCapsuleErrorV1::InvalidInput);
                }
            }
            writer.forward(bytes)?;
        }
        if magic.as_slice() != FIXTURE_ENVELOPE_MAGIC_V1 {
            return Err(CustodyCapsuleErrorV1::InvalidInput);
        }

        let (length, sha256) = writer.finish()?;
        let reported = if self.fault.lie_about_receipt_length {
            length + 1
        } else {
            length
        };
        CustodyEnvelopeOpenReceiptV1::new(reported, sha256)
    }
}

/// Holds back the latest non-empty plaintext piece, so the final one can carry the last-chunk
/// flag, and hashes every byte it writes.
struct PlaintextWriterV1<'a> {
    sink: &'a mut dyn CustodyEnvelopeChunkSinkV1,
    held: Option<Vec<u8>>,
    ordinal: u32,
    length: u64,
    digest: digest::Context,
}

impl PlaintextWriterV1<'_> {
    fn forward(&mut self, bytes: &[u8]) -> Result<(), CustodyCapsuleErrorV1> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some(previous) = self.held.replace(bytes.to_vec()) {
            self.write(previous, false)?;
        }
        Ok(())
    }

    /// Writes the held piece as the final chunk; an empty plaintext is one empty final chunk.
    fn finish(mut self) -> Result<(u64, Sha256HexV1), CustodyCapsuleErrorV1> {
        let last = self.held.take().unwrap_or_default();
        self.write(last, true)?;
        let mut hex = String::with_capacity(64);
        for byte in self.digest.finish().as_ref() {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{byte:02x}");
        }
        let sha256 = Sha256HexV1::parse(hex).map_err(|_| CustodyCapsuleErrorV1::InvalidInput)?;
        Ok((self.length, sha256))
    }

    fn write(&mut self, bytes: Vec<u8>, final_chunk: bool) -> Result<(), CustodyCapsuleErrorV1> {
        self.digest.update(&bytes);
        self.length += bytes.len() as u64;
        let chunk = CustodyEnvelopeChunkV1::new(self.ordinal, bytes, final_chunk)?;
        self.ordinal += 1;
        self.sink.write_chunk(chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custody_capsule::{
        CustodyCapsuleSealProofV1, CustodyEnvelopeSourceDescriptorV1, CustodyEnvelopeStreamLimitsV1,
    };
    use crate::custody_inventory::LosslessPathV1;
    use std::collections::{BTreeMap, VecDeque};

    const NAME_V1: &[u8] = b"control/manifest.json.enc";

    fn limits() -> CustodyEnvelopeStreamLimitsV1 {
        CustodyEnvelopeStreamLimitsV1::new(1 << 20, 1 << 20, 64).expect("fixture limits")
    }

    /// An in-memory chunk source over explicit pieces, counting the chunks it served.
    struct MemorySourceV1 {
        descriptor: CustodyEnvelopeSourceDescriptorV1,
        pieces: VecDeque<Vec<u8>>,
        ordinal: u32,
        served: usize,
    }

    impl MemorySourceV1 {
        fn new(pieces: &[&[u8]]) -> Self {
            let total = pieces.iter().map(|piece| piece.len() as u64).sum();
            Self {
                descriptor: CustodyEnvelopeSourceDescriptorV1::new(total, limits())
                    .expect("a fixture source descriptor"),
                pieces: pieces.iter().map(|piece| piece.to_vec()).collect(),
                ordinal: 0,
                served: 0,
            }
        }
    }

    impl sealed::Sealed for MemorySourceV1 {}

    impl CustodyEnvelopeChunkSourceV1 for MemorySourceV1 {
        fn descriptor(&self) -> &CustodyEnvelopeSourceDescriptorV1 {
            &self.descriptor
        }

        fn next_chunk(&mut self) -> Result<Option<CustodyEnvelopeChunkV1>, CustodyCapsuleErrorV1> {
            let Some(piece) = self.pieces.pop_front() else {
                return Ok(None);
            };
            let chunk = CustodyEnvelopeChunkV1::new(self.ordinal, piece, self.pieces.is_empty())?;
            self.ordinal += 1;
            self.served += 1;
            Ok(Some(chunk))
        }
    }

    /// An in-memory sink keeping every chunk it was given.
    #[derive(Default)]
    struct MemorySinkV1 {
        chunks: Vec<CustodyEnvelopeChunkV1>,
    }

    impl MemorySinkV1 {
        fn bytes(&self) -> Vec<u8> {
            self.chunks
                .iter()
                .flat_map(|chunk| chunk.bytes().to_vec())
                .collect()
        }
    }

    impl sealed::Sealed for MemorySinkV1 {}

    impl CustodyEnvelopeChunkSinkV1 for MemorySinkV1 {
        fn limits(&self) -> CustodyEnvelopeStreamLimitsV1 {
            limits()
        }

        fn write_chunk(
            &mut self,
            chunk: CustodyEnvelopeChunkV1,
        ) -> Result<(), CustodyCapsuleErrorV1> {
            self.chunks.push(chunk);
            Ok(())
        }
    }

    fn context(format: CustodyEnvelopeFormatV1) -> CustodyEnvelopeContextV1 {
        CustodyEnvelopeContextV1::new(
            LosslessPathV1::from_bytes(NAME_V1.to_vec()),
            Sha256HexV1::digest(b"manifest"),
            format,
            vec!["fixture-recipient".to_owned()],
        )
        .expect("a fixture context")
    }

    /// Seals `plaintext` (one source chunk per piece) with the honest fixture sealer, and returns
    /// the ciphertext chunks and the open request its receipt derives.
    fn sealed_with(
        format: CustodyEnvelopeFormatV1,
        plaintext: &[&[u8]],
    ) -> (Vec<Vec<u8>>, CustodyEnvelopeOpenRequestV1) {
        let mut source = MemorySourceV1::new(plaintext);
        let mut ciphertext = MemorySinkV1::default();
        let metadata = CustodyEnvelopeMetadataV1::new(BTreeMap::new()).expect("empty metadata");
        let receipt = FixtureSealerV1::honest()
            .seal(&context(format), &mut source, &metadata, &mut ciphertext)
            .expect("the fixture sealer seals");
        let proof = CustodyCapsuleSealProofV1::from_receipts(vec![receipt]).expect("a seal proof");
        let request = CustodyEnvelopeOpenRequestV1::from_seal_artifact(
            &proof,
            LosslessPathV1::from_bytes(NAME_V1.to_vec()),
        )
        .expect("an open request");
        let chunks = ciphertext
            .chunks
            .iter()
            .map(|chunk| chunk.bytes().to_vec())
            .collect();
        (chunks, request)
    }

    fn open(
        opener: &FixtureOpenerV1,
        request: &CustodyEnvelopeOpenRequestV1,
        pieces: &[&[u8]],
    ) -> (
        Result<CustodyEnvelopeOpenReceiptV1, CustodyCapsuleErrorV1>,
        MemorySourceV1,
        MemorySinkV1,
    ) {
        let mut source = MemorySourceV1::new(pieces);
        let mut sink = MemorySinkV1::default();
        let result = opener.open(request, &mut source, &mut sink);
        (result, source, sink)
    }

    const PLAINTEXT_V1: [&[u8]; 3] = [b"first plaintext chunk;", b"second;", b"third and last"];

    fn plaintext() -> Vec<u8> {
        PLAINTEXT_V1.concat()
    }

    #[test]
    fn fixture_opener_round_trips_the_fixture_sealer() {
        let (chunks, request) = sealed_with(fixture_envelope_format_v1(), &PLAINTEXT_V1);
        assert_eq!(chunks.len(), 4, "the magic chunk and three body chunks");
        let pieces: Vec<&[u8]> = chunks.iter().map(Vec::as_slice).collect();
        let opener = FixtureOpenerV1::honest();
        let (result, source, sink) = open(&opener, &request, &pieces);
        let receipt = result.expect("the fixture opener opens");

        assert_eq!(sink.bytes(), plaintext());
        assert_eq!(receipt.plaintext_length(), plaintext().len() as u64);
        assert_eq!(
            receipt.plaintext_sha256(),
            &Sha256HexV1::digest(&plaintext())
        );
        assert_eq!(source.served, 4);
        assert_eq!(opener.calls(), 1);
        let ordinals: Vec<u32> = sink.chunks.iter().map(|chunk| chunk.ordinal()).collect();
        assert_eq!(ordinals, (0..sink.chunks.len() as u32).collect::<Vec<_>>());
        assert!(sink.chunks.last().expect("a plaintext chunk").final_chunk());
        assert!(sink.chunks[..sink.chunks.len() - 1]
            .iter()
            .all(|chunk| !chunk.final_chunk()));
    }

    #[test]
    fn fixture_opener_accepts_split_magic() {
        let (chunks, request) = sealed_with(fixture_envelope_format_v1(), &PLAINTEXT_V1);
        let envelope = chunks.concat();
        let body = &envelope[FIXTURE_ENVELOPE_MAGIC_V1.len()..];
        let second = [&b"FIX1\n"[..], &body[..3]].concat();
        let pieces: [&[u8]; 3] = [b"A2A", &second, &body[3..]];
        let (result, _, sink) = open(&FixtureOpenerV1::honest(), &request, &pieces);

        let receipt = result.expect("a magic split across chunks opens");
        assert_eq!(sink.bytes(), plaintext());
        assert_eq!(receipt.plaintext_length(), plaintext().len() as u64);
        assert!(sink.chunks.last().expect("a plaintext chunk").final_chunk());
    }

    #[test]
    fn fixture_opener_accepts_coalesced_magic() {
        let (chunks, request) = sealed_with(fixture_envelope_format_v1(), &PLAINTEXT_V1);
        let envelope = chunks.concat();
        let (result, _, sink) = open(&FixtureOpenerV1::honest(), &request, &[&envelope]);

        let receipt = result.expect("the whole envelope as one chunk opens");
        assert_eq!(sink.bytes(), plaintext());
        assert_eq!(
            receipt.plaintext_sha256(),
            &Sha256HexV1::digest(&plaintext())
        );
        assert_eq!(sink.chunks.len(), 1);
        assert!(sink.chunks[0].final_chunk());
    }

    #[test]
    fn fixture_opener_refuses_a_wrong_magic() {
        let (_, request) = sealed_with(fixture_envelope_format_v1(), &PLAINTEXT_V1);
        for pieces in [
            vec![&b"NOTMAGIC"[..], b"a body after the wrong magic"],
            vec![&b"NOTMAGICa body in the same chunk"[..]],
            vec![&b"A2AFIX"[..], b"2\na body after a near miss"],
        ] {
            let (result, _, sink) = open(&FixtureOpenerV1::honest(), &request, &pieces);
            assert_eq!(result.unwrap_err(), CustodyCapsuleErrorV1::InvalidInput);
            assert!(sink.chunks.is_empty(), "wrote before the magic was proven");
        }
    }

    #[test]
    fn fixture_opener_refuses_a_short_envelope() {
        let (_, request) = sealed_with(fixture_envelope_format_v1(), &PLAINTEXT_V1);
        for pieces in [vec![], vec![&b"A2AFIX"[..]], vec![&b"A2A"[..], b"FIX"]] {
            let (result, _, sink) = open(&FixtureOpenerV1::honest(), &request, &pieces);
            assert_eq!(result.unwrap_err(), CustodyCapsuleErrorV1::InvalidInput);
            assert!(sink.chunks.is_empty());
        }
    }

    #[test]
    fn fixture_opener_refuses_an_unsupported_format() {
        let other = CustodyEnvelopeFormatV1::new("capsule-v1", "another-sealer", "0.1.0")
            .expect("a format");
        let (chunks, request) = sealed_with(other, &PLAINTEXT_V1);
        let pieces: Vec<&[u8]> = chunks.iter().map(Vec::as_slice).collect();
        let (result, source, sink) = open(&FixtureOpenerV1::honest(), &request, &pieces);

        assert_eq!(result.unwrap_err(), CustodyCapsuleErrorV1::InvalidInput);
        assert_eq!(source.served, 0, "a chunk was read before the format check");
        assert!(sink.chunks.is_empty());
    }
}
