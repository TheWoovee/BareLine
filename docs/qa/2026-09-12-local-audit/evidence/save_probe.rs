use bareline_document::{Document, Budget};
use bareline_file_io::lifecycle::{open_utf8,save_utf8_cancellable};
use bareline_file_io::cancellation::Cancellation;
use bareline_platform::{LocalFileSystem,FileIdentity};
use bareline_platform_windows::WindowsFileSystem;
use std::{path::Path,fs::File,io};
struct ChangeBeforeCommit;
impl LocalFileSystem for ChangeBeforeCommit {
 fn identity(&self,file:&File)->io::Result<FileIdentity>{WindowsFileSystem.identity(file)}
 fn validate_target(&self,path:&Path)->io::Result<()>{WindowsFileSystem.validate_target(path)}
 fn commit(&self,staged:&Path,target:&Path,existed:bool)->io::Result<()> {
  std::fs::write(target,b"another writer changed this after the fingerprint check")?;
  println!("interleaved_writer_bytes={}",std::fs::read_to_string(target)?);
  WindowsFileSystem.commit(staged,target,existed)
 }
}
fn main(){
 let root=std::env::current_dir().unwrap().join("docs/qa/2026-09-12-local-audit/evidence/save-probe-fixture");
 std::fs::create_dir(&root).unwrap();
 let path=root.join("existing.txt");std::fs::write(&path,b"original").unwrap();
 let document=Document::from_utf8("editor replacement",Budget::new(1<<20),Budget::new(1<<20)).unwrap();
 let save_as=save_utf8_cancellable(document.snapshot(),&path,None,false,&WindowsFileSystem,&Cancellation::default());
 println!("save_as_existing_result={:?}; content={}",save_as.err(),std::fs::read_to_string(&path).unwrap());
 let opened=open_utf8(&path,&WindowsFileSystem,Budget::new(1<<20),Budget::new(1<<20)).unwrap();
 let save=save_utf8_cancellable(document.snapshot(),&path,Some(&opened.fingerprint),false,&ChangeBeforeCommit,&Cancellation::default());
 println!("interleaved_save_succeeded={}; final_content={}",save.is_ok(),std::fs::read_to_string(&path).unwrap());
}
