// SPDX-License-Identifier: MPL-2.0
//! Paged recovery owns the original codec triplet and journals bounded UTF-8 edits.
//! Baseline copying runs on a separate bounded worker without locking the edit actor.
use crate::{
    cancellation::Cancellation,
    codecs::disk::DiskDecoded,
    recovery::{DurableReceipt, RecoveryEdit, RecoveryMetadata, RecoveryWriter},
};
use bareline_platform::LocalFileSystem;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, SyncSender},
    },
};
#[derive(Clone, Default, Debug)]
pub struct PagedRecoveryStatus {
    pub directory: Option<PathBuf>,
    pub durable: Option<DurableReceipt>,
    pub complete: bool,
    pub error: Option<String>,
}
type Job = Box<dyn FnOnce() + Send>;
fn baseline_worker() -> &'static SyncSender<Job> {
    static WORKER: OnceLock<SyncSender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(16);
        std::thread::Builder::new()
            .name("recovery-baselines".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job();
                }
            })
            .expect("recovery worker");
        tx
    })
}
pub struct PagedRecovery {
    writer: Arc<Mutex<RecoveryWriter>>,
    pub status: Arc<Mutex<PagedRecoveryStatus>>,
    directory: PathBuf,
    store: DiskDecoded,
    baseline: bareline_document::paged::PagedSnapshot,
    platform: Arc<dyn LocalFileSystem>,
    notify: Arc<dyn Fn() + Send + Sync>,
    attempt: u64,
    cancellation: Cancellation,
}
impl PagedRecovery {
    pub fn create(
        root: &Path,
        store: DiskDecoded,
        original_path: Option<PathBuf>,
        baseline: bareline_document::paged::PagedSnapshot,
        platform: Arc<dyn LocalFileSystem>,
        status: Arc<Mutex<PagedRecoveryStatus>>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let directory = root.join(format!(
            "paged-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let writer = RecoveryWriter::create(
            &directory,
            RecoveryMetadata {
                original_path,
                source_generation: format!("{:x?}", store.fingerprint.sha256),
                codec_catalog_version: "bareline-codecs-v1".into(),
                original_len: baseline.len() as u64,
            },
            platform.as_ref(),
        )
        .map_err(|e| e.to_string())?;
        *status.lock().map_err(|_| "Recovery state stopped")? = PagedRecoveryStatus {
            directory: Some(directory.clone()),
            ..Default::default()
        };
        let mut recovery = Self {
            writer: Arc::new(Mutex::new(writer)),
            status,
            directory,
            store,
            baseline,
            platform,
            notify,
            attempt: 0,
            cancellation: Cancellation::default(),
        };
        if let Err(error) = recovery.prepare_baseline() {
            recovery
                .status
                .lock()
                .map_err(|_| "Recovery state stopped")?
                .error = Some(error);
        }
        Ok(recovery)
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn retire(self) -> Result<PathBuf, String> {
        self.cancellation.cancel();
        let _writer = self.writer.lock().map_err(|_| "Recovery writer stopped")?;
        crate::recovery::discard(&self.directory, self.platform.as_ref())
            .map_err(|e| e.to_string())?;
        Ok(self.directory.clone())
    }
    pub fn prepare_baseline(&mut self) -> Result<(), String> {
        let preparation = self
            .writer
            .lock()
            .map_err(|_| "Recovery writer stopped")?
            .prepare_baseline()
            .map_err(|e| e.to_string())?;
        self.attempt += 1;
        let name = format!("source-{}", self.attempt);
        let source_path = self.directory.join(&name);
        let directory = self.directory.clone();
        let store = self.store.clone();
        let writer = self.writer.clone();
        let status = self.status.clone();
        let platform = self.platform.clone();
        let notify = self.notify.clone();
        let cancel = self.cancellation.clone();
        let baseline = self.baseline.clone();
        baseline_worker()
            .try_send(Box::new(move || {
                let result = (|| -> Result<(), String> {
                    let retained = store
                        .retain_recovery(&source_path, &cancel)
                        .map_err(|e| format!("{e:?}"))?;
                    let text = retained
                        .sealed_text_reader(&cancel)
                        .map_err(|e| format!("{e:?}"))?;
                    let mut text = SnapshotRead {
                        source: text,
                        snapshot: baseline,
                        offset: 0,
                        cancellation: cancel.clone(),
                    };
                    let prepared = preparation
                        .copy(&mut text, || Ok(true), &cancel)
                        .map_err(|e| e.to_string())?;
                    crate::session::publish_json(
                        &directory.join("paged-source.json"),
                        &serde_json::to_vec(&serde_json::json!({"version":1,"source":name}))
                            .map_err(|e| e.to_string())?,
                        platform.as_ref(),
                    )
                    .map_err(|e| e.to_string())?;
                    writer
                        .lock()
                        .map_err(|_| "Recovery writer stopped")?
                        .attach_baseline(prepared, platform.as_ref())
                        .map_err(|e| e.to_string())?;
                    Ok(())
                })();
                if let Ok(mut state) = status.lock() {
                    match result {
                        Ok(()) => {
                            state.complete = true;
                        }
                        Err(error) => state.error = Some(error),
                    }
                }
                notify();
            }))
            .map_err(|_| "Recovery baseline queue is full; retry.".to_owned())
    }
    pub fn append(
        &mut self,
        snapshot: &bareline_document::paged::PagedSnapshot,
        edits: &[RecoveryEdit],
    ) -> Result<(), String> {
        let revision = snapshot.revision.0;
        let result = (|| {
            let mut writer = self
                .writer
                .lock()
                .map_err(|_| "Recovery writer stopped".to_owned())?;
            let receipt = if edits.is_empty() {writer.append_metadata(revision,snapshot.metadata())} else {writer.append(revision,edits)}.map_err(|e|e.to_string())?;
            write_root(&self.directory, snapshot, self.platform.as_ref(), &self.cancellation)
                .map_err(|e| e.to_string())?;
            writer
                .checkpoint(self.platform.as_ref())
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(receipt)
        })();
        let mut status = self.status.lock().map_err(|_| "Recovery state stopped")?;
        match result {
            Ok(receipt) => {
                status.durable = Some(receipt);
                Ok(())
            }
            Err(error) => {
                status.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

impl Drop for PagedRecovery {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum RootPiece {
    Original { start: u64, end: u64 },
    Inserted { text: String },
    Owned { start: u64, end: u64 },
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(Clone)]
struct RootOwned { name: String, len: u64, sha256: [u8;32] }
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RootReceipt {
    version: u32,
    revision: u64,
    file: String,
    sha256: [u8; 32],
    #[serde(default)]
    metadata: std::collections::BTreeMap<String,String>,
    #[serde(default)]
    owned: Option<RootOwned>,
}
fn prepare_root(
    directory: &Path,
    snapshot: &bareline_document::paged::PagedSnapshot,
    platform: &dyn LocalFileSystem,
    cancel: &Cancellation,
    quota: u64,
) -> std::io::Result<RootReceipt> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    let receipt_bound=RootReceipt{version:2,revision:snapshot.revision.0,file:format!("root-{}.json",snapshot.revision.0),sha256:[255;32],metadata:snapshot.metadata().values().clone(),owned:Some(RootOwned{name:format!("root-owned-{}.bin",snapshot.revision.0),len:u64::MAX,sha256:[255;32]})};
    let receipt_bytes=serde_json::to_vec(&receipt_bound).map_err(std::io::Error::other)?.len() as u64;
    // Historical receipt plus the future atomic latest-pointer staging file.
    let remaining_quota=crate::recovery::admit_disk(directory,quota,receipt_bytes.checked_mul(2).ok_or_else(||std::io::Error::other("Recovery quota overflow"))?,platform,cancel)?;
    let remaining=std::rc::Rc::new(std::cell::Cell::new(remaining_quota));
    let mut cleanup=RecipeCleanup{directory:directory.into(),revision:snapshot.revision.0,preserve:false,owned:false,json:false,receipt:false};
    let owned_name = format!("root-owned-{}.bin", snapshot.revision.0);
    let mut owned = RecipeQuotaFile{file:std::fs::OpenOptions::new().create_new(true).write(true).open(directory.join(&owned_name))?,remaining:remaining.clone(),limit:remaining_quota,written:0,directory,platform,cancel};
    cleanup.owned=true;
    let mut owned_hash = Sha256::new();
    let mut owned_len = 0u64;
    let mut store_owned = |text: &str| -> std::io::Result<std::ops::Range<u64>> {
        cancel.check().map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted,"Recovery cancelled"))?;
        let start = owned_len;
        if text.len() as u64 > remaining.get().min(platform.available_space(directory)? / 5) { return Err(std::io::Error::new(std::io::ErrorKind::StorageFull,"Recovery owned quota")); }
        for chunk in text.as_bytes().chunks(65536) {
            cancel.check().map_err(|_| std::io::Error::new(std::io::ErrorKind::Interrupted,"Recovery cancelled"))?;
            owned.write_all(chunk)?; owned_hash.update(chunk); owned_len += chunk.len() as u64;
        }
        Ok(start..owned_len)
    };
    let name = format!("root-{}.json", snapshot.revision.0);
    let mut file = RecipeQuotaFile{file:std::fs::OpenOptions::new().create_new(true).write(true).open(directory.join(&name))?,remaining:remaining.clone(),limit:128*1024*1024,written:0,directory,platform,cancel};
    cleanup.json=true;
    file.write_all(b"[")?;
    let mut first = true;
    let mut count=0usize;
    let mut emit=|piece:RootPiece|->std::io::Result<()> {
        cancel.check().map_err(|_|std::io::Error::new(std::io::ErrorKind::Interrupted,"Recovery cancelled"))?;
        if count>=65536 {return Err(std::io::Error::other("Recovery piece limit"));}
        count+=1;
        if !first {file.write_all(b",")?;}first=false;
        serde_json::to_writer(&mut file,&piece).map_err(std::io::Error::other)?;
        if file.metadata()?.len()>128*1024*1024 {return Err(std::io::Error::other("Recovery recipe size limit"));}
        Ok(())
    };
    for piece in snapshot.pieces() {
        use bareline_document::paged::PagedPiece;
        match piece {
            PagedPiece::Original {range,..}|PagedPiece::OriginalOwned {range,..}=>emit(RootPiece::Original{start:range.start,end:range.end})?,
            PagedPiece::Inserted(text)=>{ let range=store_owned(text)?; emit(RootPiece::Owned{start:range.start,end:range.end})?; },
            PagedPiece::OwnedSource {source,range,original}=>{
                if let Some((_,original_range))=original {emit(RootPiece::Original{start:original_range.start,end:original_range.end})?;}
                else { let mut stored:Option<std::ops::Range<u64>>=None; crate::owned_read::visit_utf8::<std::io::Error>(source,range,cancel,|text| { let next=store_owned(text)?; if let Some(previous)=stored.as_mut() {previous.end=next.end;} else {stored=Some(next);} Ok(()) })?; if let Some(range)=stored {emit(RootPiece::Owned{start:range.start,end:range.end})?;} }
            }
        }
    }
    drop(store_owned); owned.sync_all()?; drop(owned);
    let mut _owned_seal=platform.open_sealed_read(&directory.join(&owned_name))?;
    let mut sealed_hash=Sha256::new();let mut sealed_buffer=[0u8;65536];
    loop{cancel.check().map_err(|_|std::io::Error::new(std::io::ErrorKind::Interrupted,"Recovery cancelled"))?;let count=_owned_seal.read(&mut sealed_buffer)?;if count==0{break;}sealed_hash.update(&sealed_buffer[..count]);}
    if sealed_hash.finalize()!=owned_hash.clone().finalize(){return Err(std::io::Error::other("Recovery owned bytes changed before seal"));}
    file.write_all(b"]")?;
    if file.metadata()?.len() > 128 * 1024 * 1024 {
        return Err(std::io::Error::other("Recovery recipe size limit"));
    }
    file.sync_all()?;
    drop(file);
    let mut file = std::fs::File::open(directory.join(&name))?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    let receipt = RootReceipt {
        version: 2,
        revision: snapshot.revision.0,
        file: name,
        sha256: hash.finalize().into(),
        metadata: snapshot.metadata().values().clone(),
        owned: Some(RootOwned{name:owned_name,len:owned_len,sha256:owned_hash.finalize().into()}),
    };
    if directory.join(format!("root-{}.receipt.json",receipt.revision)).try_exists()?{return Err(std::io::Error::other("Recovery receipt already exists"));}
    cleanup.receipt=true;
    crate::session::publish_json(
        &directory.join(format!("root-{}.receipt.json", receipt.revision)),
        &serde_json::to_vec(&receipt).map_err(std::io::Error::other)?,
        platform,
    )?;
    cleanup.preserve=true;
    Ok(receipt)
}
fn publish_root(directory: &Path, receipt: &RootReceipt, platform: &dyn LocalFileSystem) -> std::io::Result<()> {
    crate::session::publish_json(&directory.join("paged-root.json"), &serde_json::to_vec(receipt).map_err(std::io::Error::other)?, platform)
}
fn write_root(directory: &Path, snapshot: &bareline_document::paged::PagedSnapshot, platform: &dyn LocalFileSystem, cancel: &Cancellation) -> std::io::Result<()> {
    let receipt = prepare_root(directory,snapshot,platform,cancel,20 * 1024 * 1024 * 1024)?;
    publish_root(directory,&receipt,platform)
}
/// Reconstruct the primary paged document with unchanged raw provenance intact.
/// Never writes to the original source file.
pub fn restore(
    directory: &Path,
    platform: Arc<dyn LocalFileSystem>,
    bytes: bareline_document::Budget,
    history: bareline_document::Budget,
    cancel: &Cancellation,
) -> Result<crate::lifecycle::PagedOpened, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let _directory_guard = platform
        .guard_directory(directory)
        .map_err(|e| e.to_string())?;
    let read_small = |name: &str| -> Result<Vec<u8>, String> {
        let file = platform
            .open_sealed_read(&directory.join(name))
            .map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() > 524288 {
            return Err("Recovery metadata limit".into());
        }
        let mut value = Vec::new();
        file.take(524288)
            .read_to_end(&mut value)
            .map_err(|e| e.to_string())?;
        Ok(value)
    };
    let source: serde_json::Value =
        serde_json::from_slice(&read_small("paged-source.json")?).map_err(|e| e.to_string())?;
    let name = source
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or("Missing recovery source")?;
    if source.get("version").and_then(|v| v.as_u64()) != Some(1)
        || !name.starts_with("source-")
        || !name[7..].bytes().all(|b| b.is_ascii_digit())
    {
        return Err("Invalid recovery source".into());
    }
    let mut root: RootReceipt =
        serde_json::from_slice(&read_small("paged-root.json")?).map_err(|e| e.to_string())?;
    if !matches!(root.version,1|2) || root.file != format!("root-{}.json", root.revision) {
        return Err("Invalid recovery root".into());
    }
    let inspection = crate::recovery::inspect(directory, cancel).map_err(|e| e.to_string())?;
    if inspection.status == crate::recovery::RecoveryStatus::Discarded {
        return Err("Recovery checkpoint was discarded".into());
    }
    if inspection.status == crate::recovery::RecoveryStatus::CorruptTail
        && let Some(validated) = inspection.last_durable
        && validated.revision < root.revision
    {
        root = serde_json::from_slice(&read_small(&format!(
            "root-{}.receipt.json",
            validated.revision
        ))?)
        .map_err(|e| e.to_string())?;
        if !matches!(root.version,1|2)
            || root.revision != validated.revision
            || root.file != format!("root-{}.json", validated.revision)
        {
            return Err("Invalid validated-prefix recovery root".into());
        }
    }
    // The historical recipe is flushed before a streamed journal commit. A
    // failed latest-pointer update must not hide that acknowledged revision.
    if let Some(durable) = inspection.last_durable
        && durable.revision != root.revision
    {
        let candidate: RootReceipt = serde_json::from_slice(&read_small(&format!("root-{}.receipt.json",durable.revision))?).map_err(|e|e.to_string())?;
        if !matches!(candidate.version,1|2) || candidate.revision != durable.revision || candidate.file != format!("root-{}.json",durable.revision) { return Err("Invalid durable recovery root".into()); }
        root=candidate;
    }
    if !inspection.complete_baseline
        || inspection
            .last_durable
            .is_none_or(|r| r.revision != root.revision)
    {
        return Err("Recovery root is stale or not durable; inspect/export protected edits".into());
    }
    let file = platform
        .open_sealed_read(&directory.join(&root.file))
        .map_err(|e| e.to_string())?;
    let length = usize::try_from(file.metadata().map_err(|e| e.to_string())?.len())
        .map_err(|_| "Recovery recipe limit")?;
    if length > 128 * 1024 * 1024 {
        return Err("Recovery recipe limit".into());
    }
    let charge = length
        .checked_mul(2)
        .and_then(|n| {
            n.checked_add(
                65536
                    * (std::mem::size_of::<RootPiece>()
                        + std::mem::size_of::<bareline_document::paged::RestoredPiece>()),
            )
        })
        .ok_or("Recovery recipe memory limit")?;
    let _scratch = bytes
        .claim(charge)
        .map_err(|_| "Recovery recipe memory limit")?;
    let mut data = Vec::with_capacity(length);
    file.take(length as u64)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    let actual: [u8; 32] = Sha256::digest(&data).into();
    if actual != root.sha256 {
        return Err("Recovery root hash mismatch".into());
    }
    let pieces = read_pieces(&data).map_err(|e| e.to_string())?;
    let store = DiskDecoded::open_retained(&directory.join(name), platform.clone(), cancel)
        .map_err(|e| format!("{e:?}"))?;
    let mut transcoded = store
        .open_paged(
            platform.clone(),
            crate::source::SourceOptions::default(),
            bytes.clone(),
            history.clone(),
            cancel.clone(),
        )
        .map_err(|e| format!("{e:?}"))?;
    let owned_source = root.owned.as_ref().map(|owned| crate::recovery::open_retained_owned(directory,&owned.name,owned.len,owned.sha256,platform.as_ref(),crate::source::SourceOptions::default(),bytes.clone(),cancel)).transpose().map_err(|e|e.to_string())?;
    let pieces = pieces
        .into_iter()
        .map(|piece| match piece {
            RootPiece::Original { start, end } => {
                Ok(bareline_document::paged::RestoredPiece::Original(start..end))
            }
            RootPiece::Inserted { text } => Ok(bareline_document::paged::RestoredPiece::Inserted(text)),
            RootPiece::Owned {start,end} => Ok(bareline_document::paged::RestoredPiece::OwnedSource {source:owned_source.clone().ok_or("Missing recovery owned source")?,range:start..end,original:None}),
        })
        .collect::<Result<Vec<_>,String>>()?;
    transcoded.document = bareline_document::paged::PagedDocument::restore_pieces(
        transcoded.source.source(),
        pieces,
        bytes,
        history,
        bareline_document::Revision(root.revision),
    )
    .map_err(|e| format!("{e:?}"))?;
    transcoded.document.restore_metadata(bareline_document::DocumentMetadata::new(root.metadata).map_err(|e|format!("{e:?}"))?).map_err(|e|format!("{e:?}"))?;
    Ok(crate::lifecycle::PagedOpened {
        recovery_origin: Some(directory.into()),
        fingerprint: store.fingerprint.clone(),
        path: directory.join("Recovered document"),
        transcoded,
    })
}

fn read_pieces(bytes: &[u8]) -> Result<Vec<RootPiece>, serde_json::Error> {
    struct Bounded;
    impl<'de> serde::de::Visitor<'de> for Bounded {
        type Value = Vec<RootPiece>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("at most 65536 recovery pieces")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut pieces = Vec::new();
            while let Some(piece) = sequence.next_element::<RootPiece>()? {
                if pieces.len() >= 65536 {
                    return Err(serde::de::Error::custom("Recovery piece limit"));
                }
                pieces.push(piece);
            }
            Ok(pieces)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let pieces = serde::de::Deserializer::deserialize_seq(&mut deserializer, Bounded)?;
    deserializer.end()?;
    Ok(pieces)
}

struct SnapshotRead {
    source: crate::codecs::disk::SealedStoreRead,
    snapshot: bareline_document::paged::PagedSnapshot,
    offset: usize,
    cancellation: Cancellation,
}
impl std::io::Read for SnapshotRead {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{Seek, SeekFrom};
        self.cancellation.check().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Interrupted, "Recovery cancelled")
        })?;
        if out.is_empty() || self.offset == self.snapshot.len() {
            return Ok(0);
        }
        let mut start = 0;
        for piece in self.snapshot.pieces() {
            let length = match &piece {
                bareline_document::paged::PagedPiece::OwnedSource {range,..}=>(range.end-range.start) as usize,
                bareline_document::paged::PagedPiece::Original { range, .. }
                | bareline_document::paged::PagedPiece::OriginalOwned { range, .. } => {
                    (range.end - range.start) as usize
                }
                bareline_document::paged::PagedPiece::Inserted(text) => text.len(),
            };
            if self.offset >= start + length {
                start += length;
                continue;
            }
            let local = self.offset - start;
            let count = (length - local).min(out.len());
            match piece {
                bareline_document::paged::PagedPiece::OwnedSource {source,range,..}=>crate::owned_read::read_exact(source,range.start+local as u64,&mut out[..count],&self.cancellation)?,
                bareline_document::paged::PagedPiece::Original { range, .. }
                | bareline_document::paged::PagedPiece::OriginalOwned { range, .. } => {
                    self.source
                        .seek(SeekFrom::Start(range.start + local as u64))?;
                    self.source.read_exact(&mut out[..count])?;
                }
                bareline_document::paged::PagedPiece::Inserted(text) => {
                    out[..count].copy_from_slice(&text.as_bytes()[local..local + count])
                }
            }
            self.offset += count;
            return Ok(count);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "Recovery snapshot ended early",
        ))
    }
}

impl PagedRecovery {
    /// Called while the core commit lease holds exclusive actor ownership. The
    /// entire provenance recipe is prepared before the journal becomes durable.
    pub fn append_sources(&mut self, snapshot: &bareline_document::paged::PagedSnapshot, edits: &[bareline_document::paged::SourceEdit], quota: u64) -> Result<(), String> {
        let result: Result<(),String> = (|| {
            self.writer.lock().map_err(|_|"Recovery writer stopped")?.prepare_recipe_revision(snapshot.revision.0).map_err(|e|e.to_string())?;
            let root = prepare_root(&self.directory,snapshot,self.platform.as_ref(),&self.cancellation,quota).map_err(|e|e.to_string())?;
            let journal_quota=quota.checked_sub(serde_json::to_vec(&root).map_err(|e|e.to_string())?.len() as u64).ok_or("Recovery pointer quota")?;
            let mut writer=self.writer.lock().map_err(|_|"Recovery writer stopped".to_owned())?;
            let receipt=writer.append_source_transaction(snapshot.revision.0,edits,snapshot.metadata(),journal_quota,&self.cancellation,self.platform.as_ref()).map_err(|e|e.to_string())?;
            // The revision recipe is already durable. Pointer/checkpoint failures
            // must not turn a durable transaction into an in-memory rejection.
            let maintenance=publish_root(&self.directory,&root,self.platform.as_ref()).and_then(|_|writer.checkpoint(self.platform.as_ref()));
            if let Ok(mut status)=self.status.lock() {
                status.durable=Some(receipt);
                status.error=maintenance.err().map(|e|e.to_string());
            }
            Ok(())
        })();
        if let Err(error)=&result {if let Ok(writer)=self.writer.lock(){let _=writer.prepare_recipe_revision(snapshot.revision.0);}if let Ok(mut status)=self.status.lock(){status.error=Some(error.clone());}}
        result
    }
}
impl PagedRecovery {
    pub fn append_source_history(&mut self,snapshot:&bareline_document::paged::PagedSnapshot,edits:&[bareline_document::paged::HistorySourceEdit],quota:u64)->Result<(),String> {
        let result: Result<(),String>=(|| {
            self.writer.lock().map_err(|_|"Recovery writer stopped")?.prepare_recipe_revision(snapshot.revision.0).map_err(|e|e.to_string())?;
            let root=prepare_root(&self.directory,snapshot,self.platform.as_ref(),&self.cancellation,quota).map_err(|e|e.to_string())?;
            let ranges:Vec<_>=edits.iter().map(|edit|(edit.range.start.0 as u64,edit.removed.len() as u64,edit.inserted.len() as u64)).collect();
            let journal_quota=quota.checked_sub(serde_json::to_vec(&root).map_err(|e|e.to_string())?.len() as u64).ok_or("Recovery pointer quota")?;
            let mut writer=self.writer.lock().map_err(|_|"Recovery writer stopped".to_owned())?;
            let receipt=writer.append_streams(snapshot.revision.0,&ranges,snapshot.metadata(),journal_quota,&self.cancellation,self.platform.as_ref(),|output| {
                for edit in edits {for captured in [&edit.removed,&edit.inserted] {
                    let mut original=self.store.sealed_text_reader(&self.cancellation).map_err(|e|std::io::Error::other(format!("{e:?}")))?;
                    stream_snapshot(captured,&mut original,&self.cancellation,output)?;
                }}
                Ok(())
            }).map_err(|e|e.to_string())?;
            let maintenance=publish_root(&self.directory,&root,self.platform.as_ref()).and_then(|_|writer.checkpoint(self.platform.as_ref()));
            if let Ok(mut status)=self.status.lock(){status.durable=Some(receipt);status.error=maintenance.err().map(|e|e.to_string());}
            Ok(())
        })();
        if let Err(error)=&result{if let Ok(writer)=self.writer.lock(){let _=writer.prepare_recipe_revision(snapshot.revision.0);}if let Ok(mut status)=self.status.lock(){status.error=Some(error.clone());}}
        result
    }
}

fn stream_snapshot(snapshot:&bareline_document::paged::PagedSnapshot,original:&mut crate::codecs::disk::SealedStoreRead,cancel:&Cancellation,output:&mut dyn std::io::Write)->std::io::Result<()> {
    use std::io::{Read,Seek,SeekFrom};
    use bareline_document::paged::PagedPiece;
    let mut buffer=[0u8;65536];
    for piece in snapshot.pieces(){
        match piece {
            PagedPiece::Inserted(text)=>for chunk in text.as_bytes().chunks(65536){cancel.check().map_err(|_|std::io::Error::new(std::io::ErrorKind::Interrupted,"Recovery cancelled"))?;output.write_all(chunk)?;},
            PagedPiece::OwnedSource{source,range,..}=>crate::owned_read::visit_utf8::<std::io::Error>(source,range,cancel,|text|output.write_all(text.as_bytes()))?,
            PagedPiece::Original{range,..}|PagedPiece::OriginalOwned{range,..}=>{
                original.seek(SeekFrom::Start(range.start))?;let mut remaining=range.end-range.start;
                while remaining>0{cancel.check().map_err(|_|std::io::Error::new(std::io::ErrorKind::Interrupted,"Recovery cancelled"))?;let count=remaining.min(buffer.len() as u64) as usize;original.read_exact(&mut buffer[..count])?;output.write_all(&buffer[..count])?;remaining-=count as u64;}
            }
        }
    }
    Ok(())
}
struct RecipeQuotaFile<'a>{file:std::fs::File,remaining:std::rc::Rc<std::cell::Cell<u64>>,limit:u64,written:u64,directory:&'a Path,platform:&'a dyn LocalFileSystem,cancel:&'a Cancellation}
impl std::ops::Deref for RecipeQuotaFile<'_>{type Target=std::fs::File;fn deref(&self)->&Self::Target{&self.file}}
impl std::io::Write for RecipeQuotaFile<'_>{
    fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{
        self.cancel.check().map_err(|_|std::io::Error::new(std::io::ErrorKind::Interrupted,"Recovery cancelled"))?;
        let length=bytes.len() as u64;
        if length>self.remaining.get()||length>self.limit.saturating_sub(self.written)||length>self.platform.available_space(self.directory)?/5{return Err(std::io::Error::new(std::io::ErrorKind::StorageFull,"Recovery recipe disk quota"));}
        let count=self.file.write(bytes)?;self.remaining.set(self.remaining.get()-count as u64);self.written+=count as u64;Ok(count)
    }
    fn flush(&mut self)->std::io::Result<()>{std::io::Write::flush(&mut self.file)}
}
struct RecipeCleanup{directory:PathBuf,revision:u64,preserve:bool,owned:bool,json:bool,receipt:bool}
impl Drop for RecipeCleanup{fn drop(&mut self){if !self.preserve{for (created,name) in [(self.json,format!("root-{}.json",self.revision)),(self.receipt,format!("root-{}.receipt.json",self.revision)),(self.owned,format!("root-owned-{}.bin",self.revision))]{if created{let _=std::fs::remove_file(self.directory.join(name));}}}}}
#[cfg(test)]
mod quota_tests{
    use super::*;
    use std::{fs,io};
    struct Platform;
    impl LocalFileSystem for Platform{
        fn identity(&self,_:&fs::File)->io::Result<bareline_platform::FileIdentity>{Err(io::Error::other("unused"))}
        fn validate_target(&self,_:&Path)->io::Result<()>{Ok(())}
        fn commit(&self,staged:&Path,target:&Path,_:bool)->io::Result<()>{fs::rename(staged,target)}
        fn available_space(&self,_:&Path)->io::Result<u64>{Ok(1<<40)}
        fn open_sealed_read(&self,path:&Path)->io::Result<fs::File>{fs::File::open(path)}
    }
    #[test]
    fn empty_owned_recipe_still_consumes_quota_and_failed_prepare_cleans_files(){
        use bareline_document::{Budget,source::{MemorySource,Generation,SourceKind},paged::PagedSnapshot};
        let path=std::env::temp_dir().join(format!("bareline-recipe-quota-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&path).unwrap();
        let (source,_)=MemorySource::new(0,Generation(1),SourceKind::Paged,4096,4096,Budget::new(65536)).unwrap();let snapshot=PagedSnapshot::utf8(source,0).unwrap();let cancel=Cancellation::default();
        let root=prepare_root(&path,&snapshot,&Platform,&cancel,8192).unwrap();assert_eq!(root.owned.as_ref().unwrap().len,0);let physical=crate::recovery::disk_usage(&path,&cancel).unwrap();assert!(physical>2);
        fs::remove_file(path.join(root.file)).unwrap();fs::remove_file(path.join("root-0.receipt.json")).unwrap();fs::remove_file(path.join("root-owned-0.bin")).unwrap();
        assert!(prepare_root(&path,&snapshot,&Platform,&cancel,physical-1).is_err());assert_eq!(fs::read_dir(&path).unwrap().count(),0);
        fs::write(path.join("retained.bin"),b"existing").unwrap();assert!(prepare_root(&path,&snapshot,&Platform,&cancel,0).is_err());assert_eq!(fs::read(path.join("retained.bin")).unwrap(),b"existing");fs::remove_dir_all(path).unwrap();
    }
}
