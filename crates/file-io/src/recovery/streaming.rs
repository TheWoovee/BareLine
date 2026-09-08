// SPDX-License-Identifier: MPL-2.0
//! Streaming payload path. Record metadata remains bounded independently of payload size.
use super::*;
use bareline_document::{DocumentMetadata, paged::SourceEdit};
impl RecoveryWriter {
    pub fn append_source_transaction(&mut self, revision: u64, edits: &[SourceEdit], metadata: &DocumentMetadata, quota: u64, cancel: &Cancellation, platform: &dyn LocalFileSystem) -> io::Result<DurableReceipt> {
        self.append_source_with_faults(revision, edits, metadata, quota, cancel, platform, &mut NoFault)
    }
    pub fn append_source_with_faults(&mut self, revision: u64, edits: &[SourceEdit], metadata: &DocumentMetadata, quota: u64, cancel: &Cancellation, platform: &dyn LocalFileSystem, faults: &mut dyn FaultInjector) -> io::Result<DurableReceipt> {
        self.ensure_writable()?;
        cancelled(cancel)?;
        if edits.is_empty() || edits.len() > 4096 || self.records >= MAX_RECORDS || self.last_durable().is_some_and(|old| old.revision >= revision) { return Err(invalid("invalid streaming recovery transaction")); }
        let mut refs = Vec::new();
        refs.try_reserve_exact(edits.len()).map_err(io::Error::other)?;
        let mut size = 0u64;
        for edit in edits {
            let removed = edit.inverse.range.end.checked_sub(edit.inverse.range.start).ok_or_else(|| invalid("inverse range"))?;
            let inserted = edit.inserted.range.end.checked_sub(edit.inserted.range.start).ok_or_else(|| invalid("inserted range"))?;
            if edit.range.end.0.checked_sub(edit.range.start.0).map(|n|n as u64) != Some(removed) { return Err(invalid("inverse length mismatch")); }
            size = size.checked_add(removed).and_then(|n| n.checked_add(inserted)).ok_or_else(|| invalid("stream payload overflow"))?;
            refs.push(EditRef { offset: edit.range.start.0 as u64, removed, inserted });
        }
        let next_len = edited_len(self.current_len, &refs)?;
        let admitted_receipt=DurableReceipt{revision,protected_unix_ms:now_ms()};
        let record_bound=Record{version:3,metadata:Some(metadata.values().clone()),receipt:admitted_receipt,segment:Blob{name:format!("segment-{revision}.bin"),len:size,sha256:[255;32]},edits:refs.clone()};
        let frame_bound=serde_json::to_vec(&record_bound).map_err(io::Error::other)?.len() as u64+8;
        if frame_bound>MAX_RECORD as u64+8 || self.journal.metadata()?.len().checked_add(frame_bound).is_none_or(|n|n>MAX_JOURNAL){return Err(invalid("recovery journal quota"));}
        let mut checkpoint=self.manifest.clone();checkpoint.durable=Some(admitted_receipt);
        let checkpoint_bound=serde_json::to_vec(&checkpoint).map_err(io::Error::other)?.len() as u64;
        let required=size.checked_add(frame_bound).and_then(|n|n.checked_add(checkpoint_bound)).ok_or_else(||invalid("recovery quota overflow"))?;
        admit_disk(&self.directory,quota,required,platform,cancel)?;
        let result = (|| {
            let name = format!("segment-{revision}.bin");
            let mut segment = OpenOptions::new().create_new(true).write(true).open(self.directory.join(&name))?;
            let mut hash = Sha256::new();
            for edit in edits {
                for text in [&edit.inverse, &edit.inserted] {
                    crate::owned_read::visit_utf8::<io::Error>(&text.source, text.range.clone(), cancel, |chunk| {
                        segment.write_all(chunk.as_bytes())?;
                        hash.update(chunk.as_bytes());
                        Ok(())
                    })?;
                }
            }
            faults.boundary(Boundary::SegmentsWritten)?;
            segment.sync_all()?;
            faults.boundary(Boundary::SegmentsFlushed)?;
            drop(segment);
            let mut _sealed = platform.open_sealed_read(&self.directory.join(&name))?;
            let expected:[u8;32]=hash.clone().finalize().into();
            verify_held(&mut _sealed,size,expected,cancel)?;
            cancelled(cancel)?;
            let receipt = admitted_receipt;
            let record = Record { version: 3, metadata: Some(metadata.values().clone()), receipt, segment: Blob { name, len: size, sha256: hash.finalize().into() }, edits: refs };
            let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
            if bytes.len() > MAX_RECORD || self.journal.metadata()?.len().checked_add(bytes.len() as u64 + 8).is_none_or(|n| n > MAX_JOURNAL) { return Err(invalid("recovery journal quota")); }
            self.journal.write_all(&(bytes.len() as u32).to_le_bytes())?;
            self.journal.write_all(&crc32c(&bytes).to_le_bytes())?;
            self.journal.write_all(&bytes)?;
            faults.boundary(Boundary::JournalWritten)?;
            self.journal.sync_all()?;
            faults.boundary(Boundary::JournalFlushed)?;
            self.manifest.durable = Some(receipt);
            self.records += 1;
            self.current_len = next_len;
            Ok(receipt)
        })();
        if result.is_err() { self.poisoned = true; }
        result
    }
}

struct RetainedSegment {
    file: std::sync::Mutex<File>,
    _guard: std::sync::Arc<dyn Send + Sync>,
}
impl bareline_document::source::OwnedPageLoader for RetainedSegment {
    fn read(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        let mut file = self.file.lock().map_err(|_| io::Error::other("recovery segment lock"))?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(output)
    }
}
/// Revalidate bytes under the same immutable capability retained by page consumers.
fn retained_source(directory: &Path, blob: &Blob, platform: &dyn LocalFileSystem, options: crate::source::SourceOptions, budget: bareline_document::Budget, cancel: &Cancellation) -> io::Result<bareline_document::source::MemorySource> {
    use bareline_document::source::{MemorySource, Generation, SourceKind};
    use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
    let guard = platform.guard_directory(directory)?;
    let mut file = platform.open_sealed_read(&directory.join(&blob.name))?;
    if file.metadata()?.len() != blob.len { return Err(invalid("recovery segment length changed")); }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; CHUNK];
    loop {
        cancelled(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 { break; }
        hash.update(&buffer[..count]);
    }
    if <[u8; 32]>::from(hash.finalize()) != blob.sha256 { return Err(invalid("recovery segment changed")); }
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let (source, _) = MemorySource::new(blob.len, Generation((7u64 << 61) | NEXT.fetch_add(1, Ordering::Relaxed)), SourceKind::Paged, options.page_size_bytes, options.page_cache_bytes, budget).map_err(|error| io::Error::other(format!("{error:?}")))?;
    source.attach_owned_loader(Arc::new(RetainedSegment { file: std::sync::Mutex::new(file), _guard: guard })).map_err(|error| io::Error::other(format!("{error:?}")))?;
    Ok(source)
}
/// Payloads remain source ranges; callback retention is charged by the supplied page budget.
/// The legacy byte-vector visitor deliberately refuses large records.
pub fn replay_source_transactions(directory: &Path, cancel: &Cancellation, platform: &dyn LocalFileSystem, options: crate::source::SourceOptions, budget: bareline_document::Budget, mut visit: impl FnMut(DurableReceipt, Vec<SourceEdit>, Option<DocumentMetadata>) -> io::Result<()>) -> io::Result<RecoveryInspection> {
    use bareline_document::{TextOffset, paged::OwnedTextRange};
    let _guard = platform.guard_directory(directory)?;
    let scanned = scan(directory, cancel)?;
    for record in &scanned.records {
        cancelled(cancel)?;
        let source = retained_source(directory, &record.segment, platform, options, budget.clone(), cancel)?;
        let mut edits = Vec::new();
        edits.try_reserve_exact(record.edits.len()).map_err(io::Error::other)?;
        let mut cursor = 0;
        for edit in &record.edits {
            let removed_end = cursor + edit.removed;
            let inserted_end = removed_end + edit.inserted;
            edits.push(SourceEdit { range: TextOffset(usize::try_from(edit.offset).map_err(|_|invalid("replay offset exceeds address space"))?)..TextOffset(usize::try_from(edit.offset+edit.removed).map_err(|_|invalid("replay offset exceeds address space"))?), inverse: OwnedTextRange { source: source.clone(), range: cursor..removed_end }, inserted: OwnedTextRange { source: source.clone(), range: removed_end..inserted_end } });
            cursor = inserted_end;
        }
        let metadata = record.metadata.clone().map(DocumentMetadata::new).transpose().map_err(|error| io::Error::other(format!("{error:?}")))?;
        visit(record.receipt, edits, metadata)?;
    }
    Ok(scanned.inspection())
}
/// Open a recipe-owned segment under a retained immutable capability.
pub fn open_retained_owned(directory: &Path, name: &str, len: u64, sha256: [u8; 32], platform: &dyn LocalFileSystem, options: crate::source::SourceOptions, budget: bareline_document::Budget, cancel: &Cancellation) -> io::Result<bareline_document::source::MemorySource> {
    if name.len()<=15 || !name.starts_with("root-owned-") || !name.ends_with(".bin") || !name[11..name.len()-4].bytes().all(|b| b.is_ascii_digit()) { return Err(invalid("invalid owned recipe filename")); }
    retained_source(directory, &Blob { name: name.into(), len, sha256 }, platform, options, budget, cancel)
}

impl RecoveryWriter {
    pub(crate) fn append_streams(&mut self, revision:u64, ranges:&[(u64,u64,u64)], metadata:&DocumentMetadata, quota:u64, cancel:&Cancellation, platform:&dyn LocalFileSystem, mut payload:impl FnMut(&mut dyn Write)->io::Result<()>) -> io::Result<DurableReceipt> {
        self.ensure_writable()?;
        cancelled(cancel)?;
        if ranges.len()>4096 || self.records>=MAX_RECORDS || self.last_durable().is_some_and(|old|old.revision>=revision) {return Err(invalid("invalid streaming recovery transaction"));}
        let refs:Vec<_>=ranges.iter().map(|&(offset,removed,inserted)|EditRef{offset,removed,inserted}).collect();
        let size=refs.iter().try_fold(0u64,|total,edit|total.checked_add(edit.removed)?.checked_add(edit.inserted)).ok_or_else(||invalid("stream payload overflow"))?;
        let faults:&mut dyn FaultInjector=&mut NoFault;        let next_len = edited_len(self.current_len, &refs)?;
        let admitted_receipt=DurableReceipt{revision,protected_unix_ms:now_ms()};
        let record_bound=Record{version:3,metadata:Some(metadata.values().clone()),receipt:admitted_receipt,segment:Blob{name:format!("segment-{revision}.bin"),len:size,sha256:[255;32]},edits:refs.clone()};
        let frame_bound=serde_json::to_vec(&record_bound).map_err(io::Error::other)?.len() as u64+8;
        if frame_bound>MAX_RECORD as u64+8 || self.journal.metadata()?.len().checked_add(frame_bound).is_none_or(|n|n>MAX_JOURNAL){return Err(invalid("recovery journal quota"));}
        let mut checkpoint=self.manifest.clone();checkpoint.durable=Some(admitted_receipt);
        let checkpoint_bound=serde_json::to_vec(&checkpoint).map_err(io::Error::other)?.len() as u64;
        let required=size.checked_add(frame_bound).and_then(|n|n.checked_add(checkpoint_bound)).ok_or_else(||invalid("recovery quota overflow"))?;
        admit_disk(&self.directory,quota,required,platform,cancel)?;
        let result = (|| {
            let name = format!("segment-{revision}.bin");
            let mut segment = OpenOptions::new().create_new(true).write(true).open(self.directory.join(&name))?;
            let mut hash = Sha256::new();
            { let mut output=HashedOutput {file:&mut segment,hash:&mut hash,written:0,expected:size,cancel}; payload(&mut output)?; if output.written!=size {return Err(invalid("stream payload length mismatch"));} }
            faults.boundary(Boundary::SegmentsWritten)?;
            segment.sync_all()?;
            faults.boundary(Boundary::SegmentsFlushed)?;
            drop(segment);
            let mut _sealed = platform.open_sealed_read(&self.directory.join(&name))?;
            let expected:[u8;32]=hash.clone().finalize().into();
            verify_held(&mut _sealed,size,expected,cancel)?;
            cancelled(cancel)?;
            let receipt = admitted_receipt;
            let record = Record { version: 3, metadata: Some(metadata.values().clone()), receipt, segment: Blob { name, len: size, sha256: hash.finalize().into() }, edits: refs };
            let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
            if bytes.len() > MAX_RECORD || self.journal.metadata()?.len().checked_add(bytes.len() as u64 + 8).is_none_or(|n| n > MAX_JOURNAL) { return Err(invalid("recovery journal quota")); }
            self.journal.write_all(&(bytes.len() as u32).to_le_bytes())?;
            self.journal.write_all(&crc32c(&bytes).to_le_bytes())?;
            self.journal.write_all(&bytes)?;
            faults.boundary(Boundary::JournalWritten)?;
            self.journal.sync_all()?;
            faults.boundary(Boundary::JournalFlushed)?;
            self.manifest.durable = Some(receipt);
            self.records += 1;
            self.current_len = next_len;
            Ok(receipt)
        })();
        if result.is_err() { self.poisoned = true; }
        result
    }
}
struct HashedOutput<'a> {file:&'a mut File,hash:&'a mut Sha256,written:u64,expected:u64,cancel:&'a Cancellation}
impl Write for HashedOutput<'_> {
    fn write(&mut self, bytes:&[u8])->io::Result<usize> {
        cancelled(self.cancel)?;
        if bytes.len() as u64>self.expected.saturating_sub(self.written) {return Err(invalid("stream payload exceeds declared length"));}
        let count=self.file.write(bytes)?; self.hash.update(&bytes[..count]); self.written+=count as u64; Ok(count)
    }
    fn flush(&mut self)->io::Result<()> {self.file.flush()}
}
fn verify_held(file:&mut File,len:u64,expected:[u8;32],cancel:&Cancellation)->io::Result<()> {
    if file.metadata()?.len()!=len {return Err(invalid("sealed payload length changed"));}
    let mut hash=Sha256::new();let mut buffer=[0u8;CHUNK];
    loop {cancelled(cancel)?;let count=file.read(&mut buffer)?;if count==0 {break;}hash.update(&buffer[..count]);}
    if <[u8;32]>::from(hash.finalize())!=expected {return Err(invalid("payload changed before seal"));}
    Ok(())
}
pub(crate) fn disk_usage(directory:&Path,cancel:&Cancellation)->io::Result<u64> {
    fn walk(path:&Path,depth:usize,count:&mut usize,cancel:&Cancellation)->io::Result<u64> {
        if depth>4 {return Err(invalid("recovery directory depth"));}
        let mut total=0u64;
        for entry in fs::read_dir(path)? {
            cancelled(cancel)?;*count+=1;if *count>500_000 {return Err(invalid("recovery directory entry limit"));}
            let entry=entry?;let metadata=fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {return Err(invalid("recovery directory link"));}
            let size=if metadata.is_dir(){walk(&entry.path(),depth+1,count,cancel)?}else{metadata.len()};
            total=total.checked_add(size).ok_or_else(||invalid("recovery disk accounting overflow"))?;
        }
        Ok(total)
    }
    walk(directory,0,&mut 0,cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc,atomic::{AtomicU64,Ordering}};
    struct Platform;
    impl LocalFileSystem for Platform {
        fn identity(&self,_:&File)->io::Result<bareline_platform::FileIdentity>{Err(io::Error::other("unused"))}
        fn validate_target(&self,_:&Path)->io::Result<()>{Ok(())}
        fn commit(&self,staged:&Path,target:&Path,_:bool)->io::Result<()>{if target.exists(){fs::remove_file(target)?;}fs::rename(staged,target)}
        fn available_space(&self,_:&Path)->io::Result<u64>{Ok(1u64<<40)}
        fn guard_directory(&self,_:&Path)->io::Result<Arc<dyn Send+Sync>>{Ok(Arc::new(()))}
        fn open_sealed_read(&self,path:&Path)->io::Result<File>{File::open(path)}
    }
    struct Temp(PathBuf);
    impl Temp {fn new()->Self{static NEXT:AtomicU64=AtomicU64::new(1);let path=std::env::temp_dir().join(format!("bareline-stream-recovery-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));fs::create_dir(&path).unwrap();Self(path)}}
    impl Drop for Temp{fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
    fn options()->crate::source::SourceOptions{crate::source::SourceOptions{resident_max_bytes:4096,page_size_bytes:65536,page_cache_bytes:262144}}
    fn staged(temp:&Temp,budget:bareline_document::Budget,cancel:Cancellation)->crate::owned_store::StreamingStoreBuilder{crate::owned_store::StreamingStoreBuilder::new(&temp.0,64*1024*1024,Arc::new(Platform),options(),budget,cancel).unwrap()}
    #[test]
    fn large_payload_is_durable_and_replayed_as_bounded_sources() {
        use bareline_document::{Budget,TextOffset,paged::OwnedTextRange};
        let temp=Temp::new();let budget=Budget::new(2*1024*1024);let cancel=Cancellation::default();
        let mut stage=staged(&temp,budget.clone(),cancel.clone());let chunk=[b'x';65536];
        for _ in 0..288 {stage.write_all(&chunk).unwrap();}
        let source=stage.finish().unwrap();let length=18*1024*1024u64;
        let mut writer=RecoveryWriter::create(&temp.0.join("journal"),RecoveryMetadata{original_path:None,source_generation:"test".into(),codec_catalog_version:"utf8-v1".into(),original_len:0},&Platform).unwrap();
        writer.seal_baseline(&mut io::empty(),||Ok(true),&cancel,&Platform).unwrap();
        let edits=[SourceEdit{range:TextOffset(0)..TextOffset(0),inverse:OwnedTextRange{source:source.clone(),range:0..0},inserted:OwnedTextRange{source,range:0..length}}];
        writer.append_source_transaction(1,&edits,&DocumentMetadata::default(),64*1024*1024,&cancel,&Platform).unwrap();writer.checkpoint(&Platform).unwrap();
        assert!(writer.journal.metadata().unwrap().len()<4096);
        assert!(replay_transactions(&writer.directory,&cancel,|_,_|Ok(())).is_err());
        let mut visited=0;
        let inspection=replay_source_transactions(&writer.directory,&cancel,&Platform,options(),budget,|receipt,edits,_|{assert_eq!(receipt.revision,1);assert_eq!(edits[0].inserted.range.end,length);let mut tail=[0u8;8];crate::owned_read::read_exact(&edits[0].inserted.source,length-8,&mut tail,&cancel)?;assert_eq!(&tail,b"xxxxxxxx");visited+=1;Ok(())}).unwrap();
        assert_eq!(visited,1);assert_eq!(inspection.status,RecoveryStatus::Complete);        struct Fail(Boundary);
        impl FaultInjector for Fail{fn boundary(&mut self,at:Boundary)->io::Result<()>{if at==self.0{Err(io::Error::other("interrupted boundary"))}else{Ok(())}}}
        for (name,boundary,committed) in [("before-journal",Boundary::SegmentsFlushed,false),("after-journal",Boundary::JournalFlushed,true)] {
            let path=temp.0.join(name);
            let mut interrupted=RecoveryWriter::create(&path,RecoveryMetadata{original_path:None,source_generation:"test".into(),codec_catalog_version:"utf8-v1".into(),original_len:0},&Platform).unwrap();
            interrupted.seal_baseline(&mut io::empty(),||Ok(true),&cancel,&Platform).unwrap();
            assert!(interrupted.append_source_with_faults(1,&edits,&DocumentMetadata::default(),64*1024*1024,&cancel,&Platform,&mut Fail(boundary)).is_err());
            drop(interrupted);
            assert_eq!(inspect(&path,&cancel).unwrap().last_durable.is_some(),committed);
        }
    }
    #[test]
    fn zero_payload_admits_metadata_and_frame_bytes_before_creating_segment(){
        let temp=Temp::new();let cancel=Cancellation::default();let path=temp.0.join("journal");
        let mut writer=RecoveryWriter::create(&path,RecoveryMetadata{original_path:None,source_generation:"test".into(),codec_catalog_version:"utf8-v1".into(),original_len:0},&Platform).unwrap();
        let usage=disk_usage(&path,&cancel).unwrap();
        for quota in [usage-1,usage,usage+1]{
            let error=writer.append_streams(1,&[],&DocumentMetadata::default(),quota,&cancel,&Platform,|_|Ok(())).unwrap_err();
            assert_eq!(error.kind(),io::ErrorKind::StorageFull);assert!(writer.last_durable().is_none());assert_eq!(writer.journal.metadata().unwrap().len(),0);assert!(!path.join("segment-1.bin").exists());assert_eq!(disk_usage(&path,&cancel).unwrap(),usage);
        }
        writer.append_streams(1,&[],&DocumentMetadata::default(),usage+8192,&cancel,&Platform,|_|Ok(())).unwrap();
        assert!(writer.last_durable().is_some());
    }
    #[test]
    fn staging_utf8_boundary_quota_and_cancel_fail_before_publication() {
        use bareline_document::Budget;
        let temp=Temp::new();let cancel=Cancellation::default();let mut stage=staged(&temp,Budget::new(1024*1024),cancel.clone());
        stage.write_all(&[0xf0,0x9f]).unwrap();stage.write_all(&[0x98,0x80]).unwrap();let source=stage.finish().unwrap();let mut bytes=[0u8;4];crate::owned_read::read_exact(&source,0,&mut bytes,&cancel).unwrap();assert_eq!(std::str::from_utf8(&bytes).unwrap(),"😀");
        let mut incomplete=staged(&temp,Budget::new(1024*1024),cancel.clone());incomplete.write_all(&[0xf0]).unwrap();assert!(incomplete.finish().is_err());
        let mut cancelled_stage=staged(&temp,Budget::new(1024*1024),cancel.clone());cancel.cancel();assert!(cancelled_stage.write_all(b"x").is_err());assert!(cancelled_stage.finish().is_err());
        let mut small=crate::owned_store::StreamingStoreBuilder::new(&temp.0,3,Arc::new(Platform),options(),Budget::new(1024*1024),Cancellation::default()).unwrap();assert_eq!(small.write_all(b"four").unwrap_err().kind(),io::ErrorKind::StorageFull);assert!(small.finish().is_err());
    }
}
impl RecoveryWriter {
    /// Remove only abandoned preparation files for a revision which has never
    /// reached this live writer's durable watermark. Poisoned/uncertain writers
    /// cannot reuse a revision and therefore cannot erase recoverable evidence.
    pub(crate) fn prepare_recipe_revision(&self,revision:u64)->io::Result<()> {
        self.ensure_writable()?;
        if self.last_durable().is_some_and(|receipt|receipt.revision>=revision){return Err(invalid("recipe revision is already durable"));}
        for name in [format!("root-{revision}.json"),format!("root-{revision}.receipt.json"),format!("root-owned-{revision}.bin")] {
            let path=self.directory.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file()&&!metadata.file_type().is_symlink()=>fs::remove_file(path)?,
                Ok(_)=>return Err(invalid("unexpected recipe preparation entry")),
                Err(error) if error.kind()==io::ErrorKind::NotFound=>{},
                Err(error)=>return Err(error),
            }
        }
        Ok(())
    }
}

pub(crate) fn admit_disk(directory:&Path,quota:u64,additional:u64,platform:&dyn LocalFileSystem,cancel:&Cancellation)->io::Result<u64> {
    let remaining=quota.checked_sub(disk_usage(directory,cancel)?).ok_or_else(||io::Error::new(io::ErrorKind::StorageFull,"recovery already exceeds disk quota"))?;
    if additional>remaining || additional>platform.available_space(directory)?/5{return Err(io::Error::new(io::ErrorKind::StorageFull,"recovery disk quota exceeded"));}
    Ok(remaining-additional)
}