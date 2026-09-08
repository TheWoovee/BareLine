// SPDX-License-Identifier: MPL-2.0
//! Private disk-backed streaming transcode. Quota pauses occur before publishing a
//! batch; raising the quota resumes the exact retained input without rereading it.
use super::{
    Decoder, Encoding, detect,
    state::{EncodingState, EolState},
};
use crate::{
    CodecError, DecodedSink, DecodedSpan, StreamingDecoder,
    cancellation::Cancellation,
    lifecycle::FileInput,
    source::{FileSource, SourceOptions},
};
use bareline_document::{
    Budget, BudgetClaim, DocumentBuilder, DocumentSnapshot,
    paged::{PagedDocument, PagedSnapshot},
};
use bareline_platform::{FileIdentity, LocalFileSystem};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
const CHUNK: usize = 65536;
const RECORD_BYTES: u64 = 49;
const MAGIC: &[u8; 8] = b"BLMAP001";
#[derive(Debug)]
pub enum DiskError {
    At {range: std::ops::Range<usize>, reason: String},
    Io(io::Error),
    Codec(CodecError),
    Budget,
    Changed,
    Cancelled,
    Quota {
        used: u64,
        required: u64,
        limit: u64,
    },
    NotComplete,
    Failed,
}
impl From<io::Error> for DiskError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<CodecError> for DiskError {
    fn from(e: CodecError) -> Self {
        Self::Codec(e)
    }
}
struct Directory(PathBuf, bool);
impl Drop for Directory {
    fn drop(&mut self) {
        if !self.1 { let _ = fs::remove_dir_all(&self.0); }
    }
}
#[derive(Clone)]
struct Record {
    text_start: u64,
    text_end: u64,
    raw_start: u64,
    raw_end: u64,
    text_unit: u64,
    raw_unit: u64,
    opaque: bool,
}
impl Record {
    fn bytes(&self) -> [u8; 49] {
        let mut b = [0; 49];
        for (i, n) in [
            self.text_start,
            self.text_end,
            self.raw_start,
            self.raw_end,
            self.text_unit,
            self.raw_unit,
        ]
        .into_iter()
        .enumerate()
        {
            b[i * 8..i * 8 + 8].copy_from_slice(&n.to_le_bytes());
        }
        b[48] = u8::from(self.opaque);
        b
    }
}
struct Batch {
    text: String,
    records: Vec<Record>,
    text_start: u64,
    invalid_spans: u64,
    invalid_bytes: u64,
    eol: EolState,
}
impl Batch {
    fn new(text_start: u64, eol: EolState) -> Self {
        Self {
            text: String::with_capacity((CHUNK + 4) * 3),
            records: Vec::with_capacity(CHUNK + 4),
            text_start,
            invalid_spans: 0,
            invalid_bytes: 0,
            eol,
        }
    }
}
impl DecodedSink for Batch {
    fn remaining_capacity(&self) -> usize {
        self.text.capacity() - self.text.len()
    }
    fn write(&mut self, s: DecodedSpan<'_>) -> Result<(), CodecError> {
        if s.text.is_empty() {
            return Ok(());
        }
        let start = self.text_start + self.text.len() as u64;
        self.text.push_str(s.text);
        self.eol.push(s.text, false);
        let raw_unit = s.original.end.0 - s.original.start.0;
        let text_unit = s.text.len() as u64;
        if let Some(last) = self.records.last_mut().filter(|last| {
            !last.opaque
                && s.opaque_bytes.is_none()
                && last.raw_unit == raw_unit
                && last.text_unit == text_unit
                && last.raw_end == s.original.start.0
        }) {
            last.raw_end = s.original.end.0;
            last.text_end = start + text_unit;
            return Ok(());
        }
        if let Some(b) = s.opaque_bytes {
            self.invalid_spans += 1;
            self.invalid_bytes += b.len() as u64;
        }
        self.records.push(Record {
            text_start: start,
            text_end: start + text_unit,
            raw_start: s.original.start.0,
            raw_end: s.original.end.0,
            text_unit,
            raw_unit,
            opaque: s.opaque_bytes.is_some(),
        });
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TranscodeProgress {
    pub raw_bytes: u64,
    pub available_text_bytes: u64,
    pub complete: bool,
    pub disk_bytes: u64,
}
pub struct DiskTranscoder {
    hash: Sha256,
    text_hash: Sha256,
    map_hash: Sha256,
    input: File,
    input_path: PathBuf,
    identity: FileIdentity,
    platform: Arc<dyn LocalFileSystem>,
    raw: File,
    text: File,
    map: File,
    store: Arc<Directory>,
    decoder: Decoder,
    pending: Vec<u8>,
    eof: bool,
    complete: bool,
    failed: bool,
    quota: u64,
    used: u64,
    raw_len: u64,
    text_len: u64,
    pub state: EncodingState,
    pub eol: EolState,
    cancellation: Cancellation,
    budget: Budget,
    _scratch: BudgetClaim,
    preview: Option<DocumentSnapshot>,
    preview_published: bool,
}
impl DiskTranscoder {
    /// A continuation segment has no encoding signature, even when its first scalar is U+FEFF.
    pub fn continuation(input: FileInput, platform: Arc<dyn LocalFileSystem>, cache: &Path, options: DiskOptions, budget: Budget, cancellation: Cancellation) -> Result<Self, DiskError> {
        let mut job = Self::new(input, platform, cache, options, budget, cancellation)?;
        job.decoder.start = false;
        job.state.bom = false;
        Ok(job)
    }
    pub fn new(
        input: FileInput,
        platform: Arc<dyn LocalFileSystem>,
        cache: &Path,
        options: DiskOptions,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self, DiskError> {
        cancellation.check().map_err(|_| DiskError::Cancelled)?;
        let identity = platform.identity(&input.file)?;
        let scratch = budget
            .claim((CHUNK + 4) * (std::mem::size_of::<Record>() + 3) + CHUNK)
            .map_err(|_| DiskError::Budget)?;
        if options.temp_quota_bytes < 8 {
            return Err(DiskError::Quota {
                used: 0,
                required: 8,
                limit: options.temp_quota_bytes,
            });
        }
        let mut file = input.file;
        let mut pending = vec![0; CHUNK];
        let mut count = 0;
        while count < CHUNK {
            let n = file.read(&mut pending[count..])?;
            if n == 0 {
                break;
            }
            count += n;
        }
        pending.truncate(count);
        let mut state = EncodingState::new(detect(&pending));
        state.user_override = options.interpret;
        state.save_target = state.interpreted();
        state.bom =
            !state.interpreted().bom().is_empty() && pending.starts_with(state.interpreted().bom());
        static NEXT: AtomicU64 = AtomicU64::new(1);
        fs::create_dir_all(cache)?;
        let store = loop {
            let path = cache.join(format!(
                "bareline-transcode-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break Arc::new(Directory(path, false)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        };
        let open = |name| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(store.0.join(name))
        };
        let raw = open("original.raw")?;
        let text = open("text.utf8")?;
        let mut map = open("provenance.bin")?;
        map.write_all(MAGIC)?;
        Ok(Self {
            hash: Sha256::new(),
            text_hash: Sha256::new(),
            map_hash: { let mut hash = Sha256::new(); hash.update(MAGIC); hash },
            input: file,
            input_path: input.path,
            identity,
            platform,
            raw,
            text,
            map,
            store,
            decoder: Decoder::new(state.interpreted()),
            pending,
            eof: count < CHUNK,
            complete: false,
            failed: false,
            quota: options.temp_quota_bytes,
            used: 8,
            raw_len: 0,
            text_len: 0,
            state,
            eol: EolState::default(),
            cancellation,
            budget,
            _scratch: scratch,
            preview: None,
            preview_published: false,
        })
    }
    pub fn set_quota(&mut self, bytes: u64) {
        self.quota = bytes;
    }
    pub fn set_cancellation(&mut self, cancellation: Cancellation) {
        self.cancellation = cancellation;
    }
    pub fn progress(&self) -> TranscodeProgress {
        TranscodeProgress {
            raw_bytes: self.raw_len,
            available_text_bytes: self.text_len,
            complete: self.complete,
            disk_bytes: self.used,
        }
    }
    pub fn take_preview(&mut self) -> Option<DocumentSnapshot> {
        self.preview.take()
    }
    fn check(&self) -> Result<(), DiskError> {
        self.cancellation
            .check()
            .map_err(|_| DiskError::Cancelled)?;
        if self.identity != self.platform.identity(&self.input)?
            || self.identity != self.platform.identity(&File::open(&self.input_path)?)?
        {
            return Err(DiskError::Changed);
        }
        Ok(())
    }
    pub fn step(&mut self) -> Result<TranscodeProgress, DiskError> {
        if self.failed {
            return Err(DiskError::Failed);
        }
        self.check()?;
        if self.complete {
            return Ok(self.progress());
        }
        if self.pending.is_empty() && !self.eof {
            self.pending.resize(CHUNK, 0);
            let n = self.input.read(&mut self.pending)?;
            self.pending.truncate(n);
            self.eof = n == 0;
        }
        let mut decoder = self.decoder.clone();
        let mut batch = Batch::new(self.text_len, self.eol);
        let p = decoder.push(&self.pending, self.eof, &mut batch)?;
        if p.needs_output || p.consumed != self.pending.len() {
            return Err(DiskError::Failed);
        }
        let required = self.pending.len() as u64
            + batch.text.len() as u64
            + batch.records.len() as u64 * RECORD_BYTES;
        let free = self.platform.available_space(&self.store.0)?;
        // Recompute the effective total quota before every growth batch. Adding our
        // retained bytes avoids progressively charging the same storage twice.
        let effective_quota = self.quota.min(free.saturating_add(self.used) / 5);
        if required > effective_quota.saturating_sub(self.used) {
            return Err(DiskError::Quota {
                used: self.used,
                required,
                limit: effective_quota,
            });
        }
        self.check()?;
        let writes = (|| -> io::Result<()> {
            self.raw.write_all(&self.pending)?;
            self.text.write_all(batch.text.as_bytes())?;
            for record in &batch.records {
                self.map.write_all(&record.bytes())?;
            }
            Ok(())
        })();
        if let Err(e) = writes {
            self.failed = true;
            return Err(e.into());
        }
        self.raw_len += self.pending.len() as u64;
        self.hash.update(&self.pending);
        self.text_hash.update(batch.text.as_bytes());
        for record in &batch.records { self.map_hash.update(record.bytes()); }
        self.text_len += batch.text.len() as u64;
        self.used += required;
        self.decoder = decoder;
        self.eol = batch.eol;
        self.state.invalid_span_count += batch.invalid_spans;
        self.state.invalid_byte_count += batch.invalid_bytes;
        self.state.had_decode_errors |= batch.invalid_spans > 0;
        self.pending.clear();
        if !self.preview_published {
            let mut len = batch.text.len().min(CHUNK);
            while !batch.text.is_char_boundary(len) {
                len -= 1;
            }
            let mut builder = DocumentBuilder::new(self.budget.clone(), Budget::new(0))
                .map_err(|_| DiskError::Budget)?;
            builder
                .append(&batch.text[..len])
                .map_err(|_| DiskError::Budget)?;
            self.preview = Some(builder.prefix());
            self.preview_published = true;
        }
        if self.eof {
            if self.raw_len != self.identity.length {
                return Err(DiskError::Changed);
            }
            self.check()?;
            self.raw.sync_all()?;
            self.text.sync_all()?;
            self.map.sync_all()?;
            self.eol.push("", true);
            self.complete = true;
        }
        Ok(self.progress())
    }
    pub fn finish(self) -> Result<DiskDecoded, DiskError> {
        if !self.complete {
            return Err(DiskError::NotComplete);
        }
        Ok(DiskDecoded {
            _directory_guard: None,
            platform: self.platform.clone(),
            sealed_hashes: [self.hash.clone().finalize().into(), self.text_hash.clone().finalize().into(), self.map_hash.clone().finalize().into()],
            original_encoding: self.state.interpreted(),
            fingerprint: crate::lifecycle::Fingerprint {
                identity: self.identity,
                sha256: self.hash.clone().finalize().into(),
            },
            store: self.store.clone(),
            state: self.state.clone(),
            eol: self.eol,
            text_len: self.text_len,
            raw_len: self.raw_len,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct DiskOptions {
    pub temp_quota_bytes: u64,
    pub interpret: Option<Encoding>,
}
#[derive(Clone)]
pub struct DiskDecoded {
    _directory_guard: Option<Arc<dyn Send + Sync>>,
    platform: Arc<dyn LocalFileSystem>,
    sealed_hashes: [[u8; 32]; 3],
    original_encoding: Encoding,
    pub fingerprint: crate::lifecycle::Fingerprint,
    store: Arc<Directory>,
    pub state: EncodingState,
    pub eol: EolState,
    pub text_len: u64,
    pub raw_len: u64,
}
pub struct SealedStoreRead { file: File, _guards: [File; 3] }
impl Read for SealedStoreRead { fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> { self.file.read(bytes) } }
impl Seek for SealedStoreRead { fn seek(&mut self, position: SeekFrom) -> io::Result<u64> { self.file.seek(position) } }
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedStore {
    version: u32,
    codec_catalog: String,
    original_encoding: Encoding,
    state: EncodingState,
    eol: EolState,
    text_len: u64,
    raw_len: u64,
    identity: [u64; 4],
    original_hash: [u8; 32],
    sealed_hashes: [[u8; 32]; 3],
}
impl DiskDecoded {
    /// Restart at a final opaque decoder unit so a split scalar can complete on append.
    /// Coordinates are relative to this immutable segment's raw and UTF-8 domains.
    pub fn tail_boundary(&self) -> Result<(u64, u64), DiskError> {
        let mut map = File::open(self.provenance_path())?;
        let length = map.metadata()?.len();
        if length < 8 || (length - 8) % RECORD_BYTES != 0 { return Err(DiskError::Failed); }
        if length == 8 { return Ok((self.raw_len, self.text_len)); }
        map.seek(SeekFrom::End(-(RECORD_BYTES as i64)))?;
        let mut bytes = [0; 49]; map.read_exact(&mut bytes)?;
        let number = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        if bytes[48] == 1 && number(24) == self.raw_len {
            Ok((number(16), number(0)))
        } else { Ok((self.raw_len, self.text_len)) }
    }
    pub fn sealed_text_reader(&self, cancel: &Cancellation) -> Result<SealedStoreRead, DiskError> {
        let guards = self.lock_sealed()?;
        self.validate_sealed(cancel)?;
        Ok(SealedStoreRead { file: File::open(self.text_path())?, _guards: guards })
    }
    pub fn sealed_original_reader(&self, cancel: &Cancellation) -> Result<SealedStoreRead, DiskError> {
        let guards = self.lock_sealed()?;
        self.validate_sealed(cancel)?;
        Ok(SealedStoreRead { file: File::open(self.original_path())?, _guards: guards })
    }
    /// Retain all original-byte provenance independently of the transient cache.
    /// The caller owns this fresh directory and records it only after success.
    pub fn retain_recovery(&self, directory: &Path, cancel: &Cancellation) -> Result<Self, DiskError> {
        let _sealed = self.lock_sealed()?;
        self.validate_sealed(cancel)?;
        fs::create_dir(directory)?;
        for (source, name) in [(self.original_path(), "original.raw"), (self.text_path(), "text.utf8"), (self.provenance_path(), "provenance.bin")] {
            let mut input = File::open(source)?;
            let mut output = OpenOptions::new().create_new(true).write(true).open(directory.join(name))?;
            let mut buffer = [0; CHUNK];
            loop { cancel.check().map_err(|_| DiskError::Cancelled)?; let count = input.read(&mut buffer)?; if count == 0 { break; } output.write_all(&buffer[..count])?; }
            output.sync_all()?;
        }
        self.validate_sealed(cancel)?;
        let identity = &self.fingerprint.identity;
        let metadata = RetainedStore { version: 1, codec_catalog: "bareline-codecs-v1".into(), original_encoding: self.original_encoding, state: self.state.clone(), eol: self.eol, text_len: self.text_len, raw_len: self.raw_len, identity: [identity.volume, identity.file, identity.length, identity.modified], original_hash: self.fingerprint.sha256, sealed_hashes: self.sealed_hashes };
        let mut manifest = OpenOptions::new().create_new(true).write(true).open(directory.join("source.json"))?;
        serde_json::to_writer(&mut manifest, &metadata).map_err(io::Error::other)?;
        manifest.sync_all()?;
        Self::open_retained(directory, self.platform.clone(), cancel)
    }
    pub fn open_retained(directory: &Path, platform: Arc<dyn LocalFileSystem>, cancel: &Cancellation) -> Result<Self, DiskError> {
        let directory_guard = platform.guard_directory(directory)?;
        let manifest = platform.open_sealed_read(&directory.join("source.json"))?;
        if manifest.metadata()?.len() > 16384 { return Err(DiskError::Failed); }
        let metadata: RetainedStore = serde_json::from_reader(manifest).map_err(io::Error::other)?;
        if metadata.version != 1 || metadata.codec_catalog != "bareline-codecs-v1" { return Err(DiskError::Failed); }
        let [volume, file, length, modified] = metadata.identity;
        let store = Self { _directory_guard: Some(directory_guard), platform, original_encoding: metadata.original_encoding, fingerprint: crate::lifecycle::Fingerprint { identity: FileIdentity { volume, file, length, modified }, sha256: metadata.original_hash }, store: Arc::new(Directory(directory.into(), true)), sealed_hashes: metadata.sealed_hashes, state: metadata.state, eol: metadata.eol, text_len: metadata.text_len, raw_len: metadata.raw_len };
        let _sealed = store.lock_sealed()?;
        store.validate_sealed(cancel)?;
        Ok(store)
    }
    /// Validate private store content, including same-size/metadata-preserving changes.
    /// This bounded streaming pass brackets export so no corrupted staging file commits.
    fn lock_sealed(&self) -> Result<[File; 3], DiskError> {
        Ok([self.platform.open_sealed_read(&self.original_path())?, self.platform.open_sealed_read(&self.text_path())?, self.platform.open_sealed_read(&self.provenance_path())?])
    }
    fn validate_sealed(&self, cancel: &Cancellation) -> Result<(), DiskError> {
        for (path, expected) in [self.original_path(), self.text_path(), self.provenance_path()].into_iter().zip(self.sealed_hashes) {
            let mut file = File::open(path)?;
            let mut hash = Sha256::new();
            let mut buffer = [0u8; CHUNK];
            loop { cancel.check().map_err(|_| DiskError::Cancelled)?; let count = file.read(&mut buffer)?; if count == 0 { break; } hash.update(&buffer[..count]); }
            let actual: [u8; 32] = hash.finalize().into();
            if actual != expected { return Err(DiskError::Changed); }
        }
        Ok(())
    }

    /// Stream edited source pieces without loading their decoded pages. The sealed
    /// store supplies original bytes and opaque provenance; inserted text encodes.
    pub fn write_snapshot(
        &self,
        snapshot: &PagedSnapshot,
        original_generation: bareline_document::source::Generation,
        target: Encoding,
        bom: bool,
        out: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<(), DiskError> {
        use bareline_document::paged::PagedPiece;
        let policy=super::state::metadata_encoding(snapshot.metadata());
        let (target,bom)=policy.map_or((target,bom),|state|(state.save_target,state.bom));
        let _sealed = self.lock_sealed()?;
        self.validate_sealed(cancel)?;
        if bom {
            out.write_all(target.bom())?;
        }
        let encoder = super::Encoder::new(target, false);
        let mut document_offset=0usize;
        for piece in snapshot.pieces() {
            cancel.check().map_err(|_| DiskError::Cancelled)?;
            let remap=|error:DiskError,source_start:u64|match error {DiskError::At {range,reason}=>DiskError::At {range:document_offset+(range.start as u64-source_start) as usize..document_offset+(range.end as u64-source_start) as usize,reason},error=>error};
            let length=match piece {
                PagedPiece::Original {source,range}|PagedPiece::OriginalOwned {source,range,..}=>{
                    if source.generation()!=original_generation{return Err(DiskError::Changed);}
                    let length=(range.end-range.start) as usize;let start=range.start;
                    self.write_source_range_validated(range,target,out,cancel).map_err(|error|remap(error,start))?;length
                }
                PagedPiece::Inserted(text)=>{
                    let encoded=encoder.encode_text(text).map_err(|error|DiskError::At {range:super::failure::rejected_range(text,target,document_offset),reason:format!("{error:?}")})?;
                    out.write_all(&encoded)?;text.len()
                }
                PagedPiece::OwnedSource {source,range,original}=>{
                    let length=(range.end-range.start) as usize;
                    if let Some((original_source,original_range))=original {
                        if original_source.generation()!=original_generation{return Err(DiskError::Changed);}
                        let start=original_range.start;
                        self.write_source_range_validated(original_range,target,out,cancel).map_err(|error|remap(error,start))?;
                    } else {
                        let mut at=document_offset;
                        crate::owned_read::visit_utf8::<DiskError>(source,range,cancel,|text|{
                            let encoded=encoder.encode_text(text).map_err(|error|DiskError::At {range:super::failure::rejected_range(text,target,at),reason:format!("{error:?}")})?;
                            out.write_all(&encoded)?;at+=text.len();Ok(())
                        })?;
                    }length
                }
            };
            document_offset+=length;
        }
        self.validate_sealed(cancel)
    }
    /// Re-encode a decoded-source range or copy its exact original raw provenance.
    /// Call once per original source piece and encode inserted pieces separately.
    pub fn write_source_range(
        &self,
        range: std::ops::Range<u64>,
        target: Encoding,
        out: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<(), DiskError> {
        let _sealed = self.lock_sealed()?;
        self.validate_sealed(cancel)?;
        self.write_source_range_validated(range, target, out, cancel)?;
        self.validate_sealed(cancel)
    }
    fn write_source_range_validated(&self, range: std::ops::Range<u64>, target: Encoding, out: &mut dyn Write, cancel: &Cancellation) -> Result<(), DiskError> {
        if range.start > range.end || range.end > self.text_len {
            return Err(DiskError::Failed);
        }
        if range.is_empty() {
            return Ok(());
        }
        let mut map = File::open(self.provenance_path())?;
        let size = map.metadata()?.len();
        let mut magic = [0; 8];
        map.read_exact(&mut magic)?;
        if &magic != MAGIC || size < 8 || (size - 8) % RECORD_BYTES != 0 {
            return Err(DiskError::Failed);
        }
        let count = (size - 8) / RECORD_BYTES;
        let record = |map: &mut File, index: u64| -> Result<Record, DiskError> {
            map.seek(SeekFrom::Start(8 + index * RECORD_BYTES))?;
            let mut b = [0; 49];
            map.read_exact(&mut b)?;
            let word = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
            let r = Record {
                text_start: word(0),
                text_end: word(1),
                raw_start: word(2),
                raw_end: word(3),
                text_unit: word(4),
                raw_unit: word(5),
                opaque: b[48] != 0,
            };
            if r.text_start >= r.text_end
                || r.raw_start >= r.raw_end
                || r.text_end > self.text_len
                || r.raw_end > self.raw_len
                || r.text_unit == 0
                || r.raw_unit == 0
                || b[48] > 1
                || (r.text_end - r.text_start) % r.text_unit != 0
                || (r.raw_end - r.raw_start) % r.raw_unit != 0
                || (r.text_end - r.text_start) / r.text_unit
                    != (r.raw_end - r.raw_start) / r.raw_unit
            {
                return Err(DiskError::Failed);
            }
            Ok(r)
        };
        let (mut lo, mut hi) = (0, count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if record(&mut map, mid)?.text_end <= range.start {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut raw = File::open(self.original_path())?;
        let mut text = File::open(self.text_path())?;
        let mut cursor = range.start;
        for index in lo..count {
            cancel.check().map_err(|_| DiskError::Cancelled)?;
            if cursor == range.end {
                break;
            }
            let r = record(&mut map, index)?;
            if r.text_start > cursor || r.text_end <= cursor {
                return Err(DiskError::Failed);
            }
            let end = r.text_end.min(range.end);
            if r.opaque && target != self.original_encoding {
                return Err(DiskError::At {range:cursor as usize..end as usize,reason:"Unresolved original bytes cannot be converted".into()});
            }
            if target == self.original_encoding {
                let a = (r.text_start
                    + (cursor - r.text_start).div_ceil(r.text_unit) * r.text_unit)
                    .min(end);
                let b = (r.text_start + (end - r.text_start) / r.text_unit * r.text_unit).max(a);
                encode_range(&mut text, cursor..a, target, out, cancel)?;
                copy_range(
                    &mut raw,
                    r.raw_start + (a - r.text_start) / r.text_unit * r.raw_unit
                        ..r.raw_start + (b - r.text_start) / r.text_unit * r.raw_unit,
                    out,
                    cancel,
                )?;
                encode_range(&mut text, b..end, target, out, cancel)?;
            } else {
                encode_range(&mut text, cursor..end, target, out, cancel)?;
            }
            cursor = end;
        }
        if cursor != range.end {
            return Err(DiskError::Failed);
        }
        Ok(())
    }
    pub fn text_path(&self) -> PathBuf {
        self.store.0.join("text.utf8")
    }
    pub fn original_path(&self) -> PathBuf {
        self.store.0.join("original.raw")
    }
    pub fn provenance_path(&self) -> PathBuf {
        self.store.0.join("provenance.bin")
    }
    pub fn open_paged(
        &self,
        platform: Arc<dyn LocalFileSystem>,
        mut options: SourceOptions,
        bytes: Budget,
        history: Budget,
        cancellation: Cancellation,
    ) -> Result<PagedTranscoded, DiskError> {
        options.resident_max_bytes = 0;
        let source = FileSource::open(
            &self.text_path(),
            platform,
            options,
            bytes.clone(),
            cancellation,
        )
        .map_err(|e| DiskError::Io(io::Error::other(format!("paged source: {e:?}"))))?;
        let snapshot = PagedSnapshot::utf8(source.source(), 0).map_err(|_| DiskError::Budget)?;
        Ok(PagedTranscoded {
            source,
            document: PagedDocument::new(snapshot, bytes, history),
            store: self.clone(),
        })
    }
    /// Untouched same-encoding export uses sealed original bytes, including BOM.
    /// The destination must be a staged file under the caller's atomic-save policy.
    pub fn copy_original(
        &self,
        out: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<(), DiskError> {
        let _sealed = self.lock_sealed()?;
        self.validate_sealed(cancel)?;
        let mut file = File::open(self.original_path())?;
        let mut buffer = [0; CHUNK];
        loop {
            cancel.check().map_err(|_| DiskError::Cancelled)?;
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            out.write_all(&buffer[..n])?;
        }
        self.validate_sealed(cancel)
    }
}
fn copy_range(
    file: &mut File,
    range: std::ops::Range<u64>,
    out: &mut dyn Write,
    cancel: &Cancellation,
) -> Result<(), DiskError> {
    file.seek(SeekFrom::Start(range.start))?;
    let mut remaining = range.end - range.start;
    let mut bytes = [0; CHUNK];
    while remaining > 0 {
        cancel.check().map_err(|_| DiskError::Cancelled)?;
        let n = file.read(&mut bytes[..remaining.min(CHUNK as u64) as usize])?;
        if n == 0 {
            return Err(DiskError::Failed);
        }
        out.write_all(&bytes[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}
fn encode_range(
    file: &mut File,
    range: std::ops::Range<u64>,
    target: Encoding,
    out: &mut dyn Write,
    cancel: &Cancellation,
) -> Result<(), DiskError> {
    file.seek(SeekFrom::Start(range.start))?;
    let mut remaining = range.end - range.start;
    let mut bytes = [0; CHUNK];
    let mut pending = Vec::with_capacity(CHUNK + 4);
    let encoder = super::Encoder::new(target, false);
    while remaining > 0 {
        cancel.check().map_err(|_| DiskError::Cancelled)?;
        let n = file.read(&mut bytes[..remaining.min(CHUNK as u64) as usize])?;
        if n == 0 {
            return Err(DiskError::Failed);
        }
        remaining -= n as u64;
        pending.extend_from_slice(&bytes[..n]);
        let valid = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(e) if e.error_len().is_none() && remaining > 0 => e.valid_up_to(),
            Err(_) => return Err(DiskError::Codec(CodecError::InvalidSequence)),
        };
        let text=std::str::from_utf8(&pending[..valid]).map_err(|_|DiskError::Codec(CodecError::InvalidSequence))?;
        let offset=(range.end-remaining-pending.len() as u64) as usize;
        let encoded=encoder.encode_text(text).map_err(|error|DiskError::At {range:super::failure::rejected_range(text,target,offset),reason:format!("{error:?}")})?;
        out.write_all(&encoded)?;
        pending.drain(..valid);
    }
    if !pending.is_empty() {
        return Err(DiskError::Codec(CodecError::InvalidSequence));
    }
    Ok(())
}
pub struct PagedTranscoded {
    pub source: FileSource,
    pub document: PagedDocument,
    pub store: DiskDecoded,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{TextOffset, paged::WindowPoll};
    struct Platform {
        logical_size: Option<u64>,
    }
    impl LocalFileSystem for Platform {
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> { Ok(Arc::new(())) }
        fn available_space(&self, _: &Path) -> io::Result<u64> { Ok(u64::MAX) }
        fn open_sealed_read(&self, path: &Path) -> io::Result<File> { File::open(path) }
        fn identity(&self, f: &File) -> io::Result<FileIdentity> {
            let m = f.metadata()?;
            Ok(FileIdentity {
                volume: 1,
                file: 1,
                length: self.logical_size.unwrap_or(m.len()),
                modified: m
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            })
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::Error::other("not used"))
        }
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "bareline-disk-codec-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn job(temp: &Temp, raw: &[u8], quota: u64, budget: Budget) -> DiskTranscoder {
        let path = temp.0.join("input");
        fs::write(&path, raw).unwrap();
        DiskTranscoder::new(
            FileInput {
                file: File::open(&path).unwrap(),
                path,
            },
            Arc::new(Platform { logical_size: None }),
            &temp.0,
            DiskOptions {
                temp_quota_bytes: quota,
                interpret: None,
            },
            budget,
            Cancellation::default(),
        )
        .unwrap()
    }
    #[test]
    fn quota_pause_resumes_exact_raw_and_paged_text() {
        let temp = Temp::new();
        let raw = [255, 254, 65, 0, 13, 0, 10, 0, 0, 0xd8, 66, 0];
        let budget = Budget::new(8 * 1024 * 1024);
        let mut job = job(&temp, &raw, 8, budget.clone());
        assert!(matches!(job.step(), Err(DiskError::Quota { used: 8, .. })));
        assert_eq!(job.progress().raw_bytes, 0);
        assert_eq!(fs::read(temp.0.join("input")).unwrap(), raw);
        job.set_quota(10000);
        assert!(job.step().unwrap().complete);
        let preview = job.take_preview().unwrap();
        assert!(!preview.is_complete());
        assert_eq!(
            preview
                .read(TextOffset(0)..TextOffset(preview.len()), 100)
                .unwrap(),
            "A\r\n�B"
        );
        let store = job.finish().unwrap();
        assert_eq!(store.state.invalid_byte_count, 2);
        let mut copy = vec![];
        store
            .copy_original(&mut copy, &Cancellation::default())
            .unwrap();
        assert_eq!(copy, raw);
        let mut paged = store
            .open_paged(
                Arc::new(Platform { logical_size: None }),
                SourceOptions {
                    resident_max_bytes: 0,
                    page_size_bytes: 4,
                    page_cache_bytes: 32,
                },
                budget.clone(),
                Budget::new(1000),
                Cancellation::default(),
            )
            .unwrap();
        let snap = paged.document.snapshot();
        let mut request = snap
            .begin_read(TextOffset(0)..TextOffset(snap.len()), 100, &budget)
            .unwrap();
        let text = loop {
            match request.poll() {
                WindowPoll::Pending(ticket) => {
                    paged.source.read_page(ticket).unwrap();
                }
                WindowPoll::Ready(text) => break text,
                _ => panic!("unexpected incomplete paged read"),
            }
        };
        assert_eq!(text.text(), "A\r\n�B");
        let generation = paged.source.source().generation();
        paged
            .document
            .apply_materialized(
                bareline_document::EditTransaction {
                    base_revision: snap.revision,
                    edits: vec![bareline_document::Edit {
                        range: TextOffset(0)..TextOffset(1),
                        insert: "Z".into(),
                    }],
                },
                std::slice::from_ref(&text),
            )
            .unwrap();
        let mut edited = vec![];
        store
            .write_snapshot(
                &paged.document.snapshot(),
                generation,
                Encoding::Utf16Le,
                true,
                &mut edited,
                &Cancellation::default(),
            )
            .unwrap();
        let mut expected = raw.to_vec();
        expected[2] = b'Z';
        assert_eq!(edited, expected);
        assert!(matches!(
            store.write_snapshot(
                &paged.document.snapshot(),
                generation,
                Encoding::Utf8,
                false,
                &mut vec![],
                &Cancellation::default()
            ),
            Err(DiskError::At { .. })
        ));
        paged.document.undo().unwrap();
        let mut undone = vec![];
        store
            .write_snapshot(
                &paged.document.snapshot(),
                generation,
                Encoding::Utf16Le,
                true,
                &mut undone,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(undone, raw);
        paged.document.apply_materialized(bareline_document::EditTransaction {base_revision:paged.document.snapshot().revision,edits:vec![bareline_document::Edit {range:TextOffset(0)..TextOffset(0),insert:"prefix".into()}]},std::slice::from_ref(&text)).unwrap();
        let Err(DiskError::At {range,..})=store.write_snapshot(&paged.document.snapshot(),generation,Encoding::Utf8,false,&mut Vec::new(),&Cancellation::default()) else {panic!("expected located opaque conversion failure")};
        assert_eq!(range,9..12);
        drop(text);
        drop(request);
        drop(snap);
        drop(paged);
        drop(store);
        drop(preview);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn giant_logical_source_publishes_only_bounded_prefix_then_refuses_truncated_input() {
        let temp = Temp::new();
        let path = temp.0.join("input");
        let mut raw = vec![0; CHUNK];
        raw[..2].copy_from_slice(&[255, 254]);
        fs::write(&path, raw).unwrap();
        let budget = Budget::new(8 * 1024 * 1024);
        let mut job = DiskTranscoder::new(
            FileInput {
                file: File::open(&path).unwrap(),
                path,
            },
            Arc::new(Platform {
                logical_size: Some(5 * 1024 * 1024 * 1024),
            }),
            &temp.0,
            DiskOptions {
                temp_quota_bytes: 1024 * 1024,
                interpret: None,
            },
            budget.clone(),
            Cancellation::default(),
        )
        .unwrap();
        let p = job.step().unwrap();
        assert_eq!(p.raw_bytes, CHUNK as u64);
        assert!(!p.complete);
        assert!(p.available_text_bytes < CHUNK as u64);
        assert!(budget.used() < 8 * 1024 * 1024);
        assert!(job.take_preview().is_some());
        assert!(matches!(job.step(), Err(DiskError::Changed)));
        drop(job);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn cancelled_paused_job_cleans_private_segments() {
        let temp = Temp::new();
        let budget = Budget::new(8 * 1024 * 1024);
        let mut j = job(&temp, b"abc", 8, budget.clone());
        assert!(matches!(j.step(), Err(DiskError::Quota { .. })));
        j.cancellation.cancel();
        assert!(matches!(j.step(), Err(DiskError::Cancelled)));
        drop(j);
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
        assert_eq!(budget.used(), 0);
    }
}
