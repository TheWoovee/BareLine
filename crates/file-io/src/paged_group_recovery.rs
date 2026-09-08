// SPDX-License-Identifier: MPL-2.0
//! One durable publication point for all roots in an editor transfer.
use super::*;
use serde::{Serialize,Deserialize};
use sha2::{Digest,Sha256};
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {directory:String, revision:u64, receipt_hash:[u8;32]}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {version:u32,id:u64,members:Vec<Member>}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Pointer {version:u32,id:u64,revision:u64,#[serde(default)] previous:Option<Box<Pointer>>}
fn read_small(path:&Path,platform:&dyn LocalFileSystem)->Result<Vec<u8>,String>{
    use std::io::Read;let mut file=platform.open_sealed_read(path).map_err(|e|e.to_string())?;
    if file.metadata().map_err(|e|e.to_string())?.len()>65536{return Err("Group metadata limit".into());}
    let mut bytes=Vec::new();file.read_to_end(&mut bytes).map_err(|e|e.to_string())?;Ok(bytes)
}
pub enum GroupEdits<'a>{Source(&'a [bareline_document::paged::SourceEdit]),History(&'a [bareline_document::paged::HistorySourceEdit])}
/// All snapshots are the future roots held by an exclusive core group lease.
/// An error before the marker leaves the entire group unpublished.
pub fn commit(journals:&mut [&mut PagedRecovery],snapshots:&[bareline_document::paged::PagedSnapshot],edits:&[GroupEdits<'_>],id:u64,quota:u64,cancel:&Cancellation)->Result<(),String>{
    if journals.len()<2||journals.len()>100||journals.len()!=snapshots.len()||edits.len()!=snapshots.len(){return Err("Invalid recovery group".into());}
    let parent=journals[0].directory.parent().ok_or("Missing recovery root")?.to_path_buf();
    if journals.iter().any(|journal|journal.directory.parent()!=Some(parent.as_path())){return Err("Recovery group roots differ".into());}
    let platform=journals[0].platform.clone();let _parent=platform.guard_directory(&parent).map_err(|e|e.to_string())?;
    let mut marker=Marker{version:1,id,members:Vec::with_capacity(journals.len())};let mut roots=Vec::with_capacity(journals.len());
    for (journal,snapshot) in journals.iter_mut().zip(snapshots){
        cancel.check().map_err(|_|"Transfer cancelled")?;
        journal.cancellation.check().map_err(|_|"Transfer cancelled")?;
        // Baseline worker may still be copying; do not certify an incomplete source.
        loop {let state=journal.status.lock().map_err(|_|"Recovery state stopped")?.clone();if state.complete{break;}if let Some(error)=state.error{return Err(error);}journal.cancellation.check().map_err(|_|"Transfer cancelled")?;cancel.check().map_err(|_|"Transfer cancelled")?;std::thread::yield_now();}
        journal.writer.lock().map_err(|_|"Recovery writer stopped")?.prepare_recipe_revision(snapshot.revision.0).map_err(|e|e.to_string())?;
        crate::recovery::admit_disk(&journal.directory,quota,131072,platform.as_ref(),&journal.cancellation).map_err(|e|e.to_string())?;
        let caller=cancel.clone();let lifecycle=journal.cancellation.clone();let preparation_cancel=Cancellation::with_check(Arc::new(move||caller.check().is_err()||lifecycle.check().is_err()));
        let root=prepare_root(&journal.directory,snapshot,platform.as_ref(),&preparation_cancel,quota.checked_sub(131072).ok_or("Group quota")?,Some(&journal.store)).map_err(|e|e.to_string())?;
        let bytes=serde_json::to_vec(&root).map_err(|e|e.to_string())?;
        let directory=journal.directory.file_name().and_then(|v|v.to_str()).ok_or("Invalid group directory")?.to_owned();
        let intent=journal.directory.join(format!("group-{id}.intent.json"));
        crate::session::publish_json(&intent,&bytes,platform.as_ref()).map_err(|e|e.to_string())?;
        let previous_path=journal.directory.join("paged-group.json");
        let previous=if previous_path.exists(){
            let mut previous:Pointer=serde_json::from_slice(&read_small(&previous_path,platform.as_ref())?).map_err(|e|e.to_string())?;
            if parent.join(format!("group-{}.commit.json",previous.id)).exists(){previous.previous=None;Some(Box::new(previous))}else{previous.previous}
        }else{None};
        let pointer=serde_json::to_vec(&Pointer{version:1,id,revision:root.revision,previous}).map_err(|e|e.to_string())?;
        crate::session::publish_json(&journal.directory.join("paged-group.json"),&pointer,platform.as_ref()).map_err(|e|e.to_string())?;
        marker.members.push(Member{directory,revision:root.revision,receipt_hash:Sha256::digest(&bytes).into()});roots.push(root);
    }
    let bytes=serde_json::to_vec(&marker).map_err(|e|e.to_string())?;
    if bytes.len()>65536{return Err("Recovery group marker limit".into());}
    cancel.check().map_err(|_|"Transfer cancelled")?;
    for journal in journals.iter(){journal.cancellation.check().map_err(|_|"Transfer cancelled")?;}
    // This single atomic local file publication is the transaction commit point.
    crate::session::publish_json(&parent.join(format!("group-{id}.commit.json")),&bytes,platform.as_ref()).map_err(|e|e.to_string())?;
    for (((journal,root),snapshot),edits) in journals.iter_mut().zip(roots).zip(snapshots).zip(edits){
        // Continue each ordinary journal after the authoritative group marker.
        // Failure is durability degradation, never a rejection of committed roots.
        let maintenance=(||->std::io::Result<()>{
            let mut writer=journal.writer.lock().map_err(|_|std::io::Error::other("Recovery writer stopped"))?;
            match edits {
                GroupEdits::Source(edits)=>{writer.append_source_transaction(root.revision,edits,snapshot.metadata(),quota,&journal.cancellation,platform.as_ref())?;},
                GroupEdits::History(edits)=>{
                    let ranges:Vec<_>=edits.iter().map(|edit|(edit.range.start.0 as u64,edit.removed.len() as u64,edit.inserted.len() as u64)).collect();
                    writer.append_streams(root.revision,&ranges,snapshot.metadata(),quota,&journal.cancellation,platform.as_ref(),|output|{
                        for edit in *edits {for snapshot in [&edit.removed,&edit.inserted]{let mut original=journal.store.sealed_text_reader(&journal.cancellation).map_err(|e|std::io::Error::other(format!("{e:?}")))?;stream_snapshot(snapshot,&journal.store,&mut original,&journal.cancellation,output)?;}}Ok(())
                    })?;
                }
            }
            publish_root(&journal.directory,&root,platform.as_ref())?;writer.checkpoint(platform.as_ref())
        })();
        if maintenance.is_err(){if let Ok(mut writer)=journal.writer.lock(){writer.break_continuity();}}
        if let Ok(mut status)=journal.status.lock(){status.durable=Some(DurableReceipt{revision:root.revision,protected_unix_ms:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64});status.error=maintenance.err().map(|e|e.to_string());}
    }
    Ok(())
}
pub(super) fn committed_root(directory:&Path,platform:&dyn LocalFileSystem,cancel:&Cancellation)->Result<Option<RootReceipt>,String>{
    let path=directory.join("paged-group.json");if !path.exists(){return Ok(None);}
    let mut pointer:Pointer=serde_json::from_slice(&read_small(&path,platform)?).map_err(|e|e.to_string())?;
    if pointer.version!=1{return Err("Unsupported group pointer".into());}
    let parent=directory.parent().ok_or("Missing recovery root")?;let _guard=platform.guard_directory(parent).map_err(|e|e.to_string())?;
    let mut path=parent.join(format!("group-{}.commit.json",pointer.id));
    if !path.exists(){let Some(previous)=pointer.previous.take()else{return Ok(None);};if previous.previous.is_some(){return Err("Group fallback depth limit".into());}pointer=*previous;path=parent.join(format!("group-{}.commit.json",pointer.id));if !path.exists(){return Err("Previous committed group is missing".into());}}
    let marker:Marker=serde_json::from_slice(&read_small(&path,platform)?).map_err(|e|e.to_string())?;
    if marker.version!=1||marker.id!=pointer.id||marker.members.len()<2||marker.members.len()>100{return Err("Invalid group commit marker".into());}
    let mut selected=None;let mut names=std::collections::BTreeSet::new();
    for member in marker.members {
        if !member.directory.starts_with("paged-")||!member.directory.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')||!names.insert(member.directory.clone()){return Err("Invalid group member".into());}
        let member_path=parent.join(&member.directory);let _guard=platform.guard_directory(&member_path).map_err(|e|e.to_string())?;
        let bytes=read_small(&member_path.join(format!("group-{}.intent.json",pointer.id)),platform)?;
        if <[u8;32]>::from(Sha256::digest(&bytes))!=member.receipt_hash{return Err("Group member receipt changed".into());}
        let receipt:RootReceipt=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
        if receipt.revision!=member.revision{return Err("Group member revision changed".into());}
        if !matches!(receipt.version,1|2)||receipt.file!=format!("root-{}.json",receipt.revision){return Err("Invalid group recipe".into());}
        verify_asset(&member_path.join(&receipt.file),None,receipt.sha256,platform,cancel)?;
        if let Some(owned)=&receipt.owned {if owned.name!=format!("root-owned-{}.bin",receipt.revision){return Err("Invalid group owned payload".into());}verify_asset(&member_path.join(&owned.name),Some(owned.len),owned.sha256,platform,cancel)?;}

        if member_path==directory {if receipt.revision!=pointer.revision{return Err("Group pointer revision changed".into());}selected=Some(receipt);}
    }
    selected.map(Some).ok_or_else(||"Document missing from recovery group".into())
}

fn verify_asset(path:&Path,length:Option<u64>,expected:[u8;32],platform:&dyn LocalFileSystem,cancel:&Cancellation)->Result<(),String>{
    use std::io::Read;let mut input=platform.open_sealed_read(path).map_err(|e|e.to_string())?;
    if length.is_some_and(|length|input.metadata().map_or(true,|metadata|metadata.len()!=length)){return Err("Group payload length changed".into());}
    let mut hash=Sha256::new();let mut buffer=[0u8;65536];loop{cancel.check().map_err(|_|"Recovery cancelled")?;let count=input.read(&mut buffer).map_err(|e|e.to_string())?;if count==0{break;}hash.update(&buffer[..count]);}
    if <[u8;32]>::from(hash.finalize())!=expected{return Err("Group payload changed".into());}Ok(())
}
