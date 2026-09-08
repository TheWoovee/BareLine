// SPDX-License-Identifier: MPL-2.0
//! Worker-only external line sorting. Runs and merge heads stay inside the caller budget.
use std::{cmp::Ordering,fs::{self,File,OpenOptions},io::{self,BufRead,BufReader,BufWriter,Read,Write},path::{Path,PathBuf},sync::atomic::{AtomicU64,Ordering as AtomicOrdering}};
#[derive(Clone,Copy)]
pub struct SortOptions {pub descending:bool,pub case_sensitive:bool,pub numeric:bool}
struct Row {ordinal:u64,text:String}
struct Scratch {paths:Vec<PathBuf>,directory:PathBuf}
impl Drop for Scratch {fn drop(&mut self){for path in &self.paths{let _=fs::remove_file(path);}let _=fs::remove_dir(&self.directory);}}
fn compare(a:&Row,b:&Row,options:SortOptions)->Ordering{
    let left;if options.case_sensitive{left=None;}else{left=Some(a.text.to_lowercase());}
    let right;if options.case_sensitive{right=None;}else{right=Some(b.text.to_lowercase());}
    let x=left.as_deref().unwrap_or(&a.text);let y=right.as_deref().unwrap_or(&b.text);
    let order=if options.numeric {match(x.trim().parse::<f64>(),y.trim().parse::<f64>()){(Ok(x),Ok(y))=>x.total_cmp(&y),_=>x.cmp(y)}}else{x.cmp(y)};
    let order=if options.descending{order.reverse()}else{order};order.then(a.ordinal.cmp(&b.ordinal))
}
fn write_row(out:&mut impl Write,row:&Row)->io::Result<u64>{out.write_all(&row.ordinal.to_le_bytes())?;out.write_all(&(row.text.len()as u64).to_le_bytes())?;out.write_all(row.text.as_bytes())?;Ok(16+row.text.len()as u64)}
fn read_row(input:&mut impl Read,max:usize)->io::Result<Option<Row>>{
    let mut head=[0u8;16];let count=input.read(&mut head[..1])?;if count==0{return Ok(None);}input.read_exact(&mut head[1..])?;
    let ordinal=u64::from_le_bytes(head[..8].try_into().unwrap());let size=u64::from_le_bytes(head[8..].try_into().unwrap());
    if size>max as u64{return Err(io::Error::new(io::ErrorKind::InvalidData,"external line exceeds budget"));}
    let mut bytes=vec![0;size as usize];input.read_exact(&mut bytes)?;let text=String::from_utf8(bytes).map_err(|_|io::Error::new(io::ErrorKind::InvalidData,"invalid UTF-8 run"))?;Ok(Some(Row{ordinal,text}))
}
fn interrupted(cancel:&impl Fn()->bool)->io::Result<()>{if cancel(){Err(io::Error::new(io::ErrorKind::Interrupted,"power transform cancelled"))}else{Ok(())}}
/// Caller supplies an owned output store and dispatches this on its bounded worker pool.
/// Preserves the first line terminator as the configured output policy, including trailing EOL.
/// Per-line size is separately capped at memory/16; total selected text is not materialized.
pub fn sort_lines(input:impl Read,mut output:impl Write,cache:&Path,memory:usize,quota:u64,options:SortOptions,cancel:impl Fn()->bool)->io::Result<u64>{
    if memory<64*1024{return Err(io::Error::new(io::ErrorKind::InvalidInput,"external sort memory too small"));}
    interrupted(&cancel)?;fs::create_dir_all(cache)?;static NEXT:AtomicU64=AtomicU64::new(1);
    let directory=cache.join(format!("power-sort-{}-{}",std::process::id(),NEXT.fetch_add(1,AtomicOrdering::Relaxed)));fs::create_dir(&directory)?;
    let mut scratch=Scratch{paths:Vec::new(),directory};let mut used=0u64;let max_line=memory/16;let mut input=BufReader::with_capacity(16*1024,input);
    let mut runs=Vec::new();let mut rows=Vec::new();let mut bytes=0usize;let mut ordinal=0u64;let mut eol=String::new();let mut trailing=false;
    loop {
        interrupted(&cancel)?;let mut value=Vec::new();let mut terminator=Vec::new();
        loop{let available=input.fill_buf()?;if available.is_empty(){break;}let end=available.iter().position(|b|*b==b'\n'||*b==b'\r');let take=end.unwrap_or(available.len());
            if value.len().checked_add(take).is_none_or(|n|n>max_line){return Err(io::Error::new(io::ErrorKind::OutOfMemory,"individual line exceeds external sort budget"));}
            value.extend_from_slice(&available[..take]);input.consume(take);
            if end.is_some(){let first=input.fill_buf()?[0];input.consume(1);terminator.push(first);if first==b'\r'&&input.fill_buf()?.first()==Some(&b'\n'){input.consume(1);terminator.push(b'\n');}break;}
        }
        if value.is_empty()&&terminator.is_empty(){break;}
        trailing=!terminator.is_empty();if eol.is_empty()&&trailing{eol=String::from_utf8(terminator).unwrap();}
        let text=String::from_utf8(value).map_err(|_|io::Error::new(io::ErrorKind::InvalidData,"selected text is not UTF-8"))?;bytes+=text.len()+32;rows.push(Row{ordinal,text});ordinal+=1;
        if bytes>=memory/4{runs.push(flush_run(&mut rows,&mut scratch,&mut used,quota,options)?);bytes=0;}
    }
    if !rows.is_empty(){runs.push(flush_run(&mut rows,&mut scratch,&mut used,quota,options)?);}
    while runs.len()>1{
        let mut next=Vec::new();for chunk in runs.chunks(8){interrupted(&cancel)?;let path=scratch.directory.join(format!("run-{}",scratch.paths.len()));scratch.paths.push(path.clone());
            let mut writer=BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);let mut readers:Vec<_>=chunk.iter().map(|path|File::open(path).map(BufReader::new)).collect::<io::Result<_>>()?;
            let mut heads:Vec<_>=readers.iter_mut().map(|reader|read_row(reader,max_line)).collect::<io::Result<_>>()?;
            loop{interrupted(&cancel)?;let pick=heads.iter().enumerate().filter_map(|(index,row)|row.as_ref().map(|row|(index,row))).min_by(|(_,a),(_,b)|compare(a,b,options)).map(|(index,_)|index);let Some(index)=pick else{break;};let row=heads[index].take().unwrap();charge(&mut used,16+row.text.len()as u64,quota)?;write_row(&mut writer,&row)?;heads[index]=read_row(&mut readers[index],max_line)?;}
            writer.flush()?;drop(writer);drop(readers);for old in chunk{let size=fs::metadata(old)?.len();fs::remove_file(old)?;used=used.saturating_sub(size);}next.push(path);
        }runs=next;
    }
    let mut written=0u64;if let Some(path)=runs.first(){let mut reader=BufReader::new(File::open(path)?);let mut row=read_row(&mut reader,max_line)?;let eol=if eol.is_empty(){"\n"}else{&eol};while let Some(current)=row{interrupted(&cancel)?;row=read_row(&mut reader,max_line)?;output.write_all(current.text.as_bytes())?;written+=current.text.len()as u64;if row.is_some()||trailing{output.write_all(eol.as_bytes())?;written+=eol.len()as u64;}}}
    output.flush()?;Ok(written)
}
fn charge(used:&mut u64,amount:u64,quota:u64)->io::Result<()>{*used=used.checked_add(amount).filter(|n|*n<=quota).ok_or_else(||io::Error::new(io::ErrorKind::StorageFull,"external sort quota exceeded"))?;Ok(())}
fn flush_run(rows:&mut Vec<Row>,scratch:&mut Scratch,used:&mut u64,quota:u64,options:SortOptions)->io::Result<PathBuf>{
    rows.sort_by(|a,b|compare(a,b,options));let path=scratch.directory.join(format!("run-{}",scratch.paths.len()));scratch.paths.push(path.clone());let mut file=BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
    for row in rows.drain(..){charge(used,16+row.text.len()as u64,quota)?;write_row(&mut file,&row)?;}file.flush()?;Ok(path)
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn external_runs_sort_numeric_and_keep_first_eol_policy(){
        let input=(0..5000).rev().map(|n|format!("{n}\r\n")).collect::<String>();let mut output=Vec::new();
        sort_lines(input.as_bytes(),&mut output,&std::env::temp_dir(),64*1024,8<<20,SortOptions{descending:false,case_sensitive:true,numeric:true},||false).unwrap();
        let expected=(0..5000).map(|n|format!("{n}\r\n")).collect::<String>();assert_eq!(output,expected.as_bytes());
    }
    #[test]
    fn cancellation_and_quota_do_not_publish_partial_actor_edits(){
        let options=SortOptions{descending:false,case_sensitive:true,numeric:false};let mut output=Vec::new();
        assert_eq!(sort_lines("b\na".as_bytes(),&mut output,&std::env::temp_dir(),64*1024,1,options,||false).unwrap_err().kind(),io::ErrorKind::StorageFull);assert!(output.is_empty());
        assert_eq!(sort_lines("b\na".as_bytes(),&mut output,&std::env::temp_dir(),64*1024,1024,options,||true).unwrap_err().kind(),io::ErrorKind::Interrupted);
    }
}
