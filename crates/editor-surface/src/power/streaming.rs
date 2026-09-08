// SPDX-License-Identifier: MPL-2.0
//! Worker-only external line sorting. Runs and merge heads stay inside the caller budget.
use std::{cmp::Ordering,fs::{self,File,OpenOptions},io::{self,BufRead,BufReader,BufWriter,Read,Write},path::{Path,PathBuf},sync::atomic::{AtomicU64,Ordering as AtomicOrdering}};
#[derive(Clone,Copy)]
pub struct SortOptions {pub descending:bool,pub case_sensitive:bool,pub numeric:bool}
/// Rotate a selected line block with its captured adjacent line. `pivot` splits
/// the expanded input into the two blocks in original byte order.
pub fn move_lines(mut input:impl Read+std::io::Seek,mut output:impl Write,pivot:u64,down:bool,quota:u64,cancel:impl Fn()->bool)->io::Result<u64> {
    use std::io::SeekFrom;
    let length=input.seek(SeekFrom::End(0))?;
    if pivot==0||pivot>=length{return Err(io::Error::new(io::ErrorKind::InvalidInput,"Move has no adjacent line"));}
    let segments=[0..pivot,pivot..length];let selected=if down{0}else{1};
    let mut trimmed=[0u64;2];let mut eol=Vec::new();
    for index in [selected,1-selected] {
        let range=&segments[index];let tail=3.min(range.end-range.start);input.seek(SeekFrom::Start(range.end-tail))?;let mut bytes=[0u8;3];input.read_exact(&mut bytes[..tail as usize])?;
        let bytes=&bytes[..tail as usize];trimmed[index]=if bytes.ends_with(b"\r\n"){2}else if bytes.ends_with(b"\r")||bytes.ends_with(b"\n"){1}else{0};
        if eol.is_empty(){input.seek(SeekFrom::Start(range.start))?;let mut reader=BufReader::new((&mut input).take(range.end-range.start));let mut found=false;
            loop {interrupted(&cancel)?;let bytes=reader.fill_buf()?;if bytes.is_empty(){break;}let take=bytes.iter().position(|byte|matches!(byte,b'\r'|b'\n'));if let Some(at)=take {let first=bytes[at];reader.consume(at+1);eol.push(first);if first==b'\r'&&reader.fill_buf()?.first()==Some(&b'\n'){eol.push(b'\n');}found=true;}else{let count=bytes.len();reader.consume(count);}if found{break;}}
        }
    }
    if eol.is_empty(){eol.push(b'\n');}let mut written=0;let mut buffer=[0u8;16*1024];
    for (position,index) in [1usize,0].into_iter().enumerate(){let range=&segments[index];input.seek(SeekFrom::Start(range.start))?;let mut remaining=range.end-range.start-trimmed[index];
        while remaining>0 {interrupted(&cancel)?;let count=(remaining as usize).min(buffer.len());input.read_exact(&mut buffer[..count])?;charge(&mut written,count as u64,quota)?;output.write_all(&buffer[..count])?;remaining-=count as u64;}
        if position==0||trimmed[1]>0 {charge(&mut written,eol.len()as u64,quota)?;output.write_all(&eol)?;}
    }
    output.flush()?;Ok(written)
}
struct Row {ordinal:u64,text:String}
fn next_line(input:&mut impl BufRead,max:usize,cancel:&impl Fn()->bool)->io::Result<Option<(String,String)>> {
    let mut value=Vec::new();let mut ending=String::new();
    loop {
        interrupted(cancel)?;
        let available=input.fill_buf()?;if available.is_empty(){break;}
        let end=available.iter().position(|b|matches!(b,b'\r'|b'\n'));
        let take=end.unwrap_or(available.len());
        if value.len().checked_add(take).is_none_or(|n|n>max){return Err(io::Error::new(io::ErrorKind::OutOfMemory,"Individual line exceeds transform budget"));}
        value.extend_from_slice(&available[..take]);input.consume(take);
        if end.is_some(){let first=input.fill_buf()?[0];input.consume(1);ending.push(first as char);if first==b'\r'&&input.fill_buf()?.first()==Some(&b'\n'){input.consume(1);ending.push('\n');}break;}
    }
    if value.is_empty()&&ending.is_empty(){return Ok(None);}
    Ok(Some((String::from_utf8(value).map_err(|_|io::Error::new(io::ErrorKind::InvalidData,"Selected text is not UTF-8"))?,ending)))
}

/// Streaming row-local transforms share the bounded in-memory command semantics.
/// Global ordering/duplication commands are dispatched separately by transform_lines.
fn row_transform(body:&str,action:super::Transform,tab_width:usize,memory:usize)->io::Result<String> {
    // next_line emitted a real empty row with a terminator. An empty temporary
    // document has no rows, so the resident helper alone would skip its indent.
    if body.is_empty() && matches!(action,super::Transform::Indent) {
        return Ok(" ".repeat(tab_width.min(memory/2)));
    }
    let document=bareline_document::Document::from_utf8(body,bareline_document::Budget::new(memory),bareline_document::Budget::new(memory)).map_err(|e|io::Error::other(format!("{e:?}")))?;
    let selection=super::SelectionSet{selections:vec![crate::Selection{anchor:0,caret:body.len()}],primary:0};
    let edit=super::transform(&document.snapshot(),&selection,action,super::Limits{max_bytes:memory/2,tab_width,..super::Limits::default()}).map_err(|e|io::Error::other(format!("{e:?}")))?;
    Ok(edit.transaction.edits.into_iter().next().map_or_else(String::new,|edit|edit.insert))
}

pub fn transform_lines(mut input:impl Read+std::io::Seek,mut output:impl Write,cache:&Path,memory:usize,quota:u64,action:super::Transform,tab_width:usize,cancel:impl Fn()->bool)->io::Result<u64> {
    use super::Transform;
    use std::io::SeekFrom;
    if memory<64*1024 || tab_width==0 || tab_width>memory/16 {return Err(io::Error::new(io::ErrorKind::InvalidInput,"Invalid transform budget or tab width"));}
    if let Transform::Sort{descending,case_sensitive,numeric}=action {return sort_lines(input,output,cache,memory,quota,SortOptions{descending,case_sensitive,numeric},cancel);}
    if matches!(action,Transform::RemoveDuplicates) {return unique_lines(input,output,cache,memory,quota,&cancel);}
    if matches!(action,Transform::MoveUp|Transform::MoveDown) {return Err(io::Error::new(io::ErrorKind::Unsupported,"Move requires the adjacent logical line range"));}
    let mut written=0u64;
    let mut emit=|bytes:&[u8]|->io::Result<()>{interrupted(&cancel)?;charge(&mut written,bytes.len()as u64,quota)?;output.write_all(bytes)};
    if matches!(action,Transform::Duplicate|Transform::DuplicateSelections) {
        let start=input.stream_position()?;let mut buffer=[0u8;16*1024];let mut ending=0u8;let mut first_eol=String::new();
        for pass in 0..2 {
            input.seek(SeekFrom::Start(start))?;
            loop {interrupted(&cancel)?;let count=input.read(&mut buffer)?;if count==0{break;}
                if pass==0 {for byte in &buffer[..count] {if first_eol.is_empty()&&matches!(byte,b'\r'|b'\n'){first_eol.push(*byte as char);}else if first_eol=="\r"&&ending==b'\r'&&*byte==b'\n'{first_eol.push('\n');}ending=*byte;}}
                emit(&buffer[..count])?;
            }
            if pass==0&&matches!(action,Transform::Duplicate)&&!matches!(ending,b'\r'|b'\n') {emit(if first_eol.is_empty(){b"\n"}else{first_eol.as_bytes()})?;}
        }
    } else {
        let mut input=BufReader::with_capacity(16*1024,input);let mut row=next_line(&mut input,memory/16,&cancel)?;
        let mut previous=String::new();let mut has_previous=false;let mut first_eol=String::new();let mut retained=false;let mut trailing=false;
        while let Some((body,ending))=row {
            if first_eol.is_empty()&&!ending.is_empty(){first_eol=ending.clone();}
            trailing=!ending.is_empty();row=next_line(&mut input,memory/16,&cancel)?;
            let filter=matches!(action,Transform::RemoveConsecutiveDuplicates|Transform::RemoveEmpty|Transform::RemoveBlank);
            let keep=match action {Transform::RemoveConsecutiveDuplicates=>!has_previous||previous!=body,Transform::RemoveEmpty=>!body.is_empty(),Transform::RemoveBlank=>!body.trim().is_empty(),_=>true};
            if matches!(action,Transform::RemoveConsecutiveDuplicates){previous=body.clone();has_previous=true;}
            if !keep{continue;}
            if filter {if retained{emit(if first_eol.is_empty(){b"\n"}else{first_eol.as_bytes()})?;}emit(body.as_bytes())?;retained=true;}
            else if matches!(action,Transform::Join){emit(body.as_bytes())?;emit(if row.is_some(){b" "}else{ending.as_bytes()})?;}
            else {let mut transformed=row_transform(&body,action.clone(),tab_width,memory)?;if matches!(action,Transform::Split{..})&&!first_eol.is_empty()&&first_eol!="\n"{transformed=transformed.replace('\n',&first_eol);}emit(transformed.as_bytes())?;emit(ending.as_bytes())?;}
        }
        if retained&&trailing{emit(if first_eol.is_empty(){b"\n"}else{first_eol.as_bytes()})?;}
    }
    output.flush()?;Ok(written)
}
struct Scratch {paths:Vec<PathBuf>,directory:PathBuf}
impl Drop for Scratch {fn drop(&mut self){for path in &self.paths{let _=fs::remove_file(path);}let _=fs::remove_dir(&self.directory);}}
fn compare(a:&Row,b:&Row,options:SortOptions)->Ordering{
    let left;if options.case_sensitive{left=None;}else{left=Some(a.text.to_lowercase());}
    let right;if options.case_sensitive{right=None;}else{right=Some(b.text.to_lowercase());}
    let x=left.as_deref().unwrap_or(&a.text);let y=right.as_deref().unwrap_or(&b.text);
    let order=if options.numeric {match(x.trim().parse::<f64>(),y.trim().parse::<f64>()){(Ok(x),Ok(y))=>x.total_cmp(&y),_=>x.cmp(y)}}else{x.cmp(y)};
    let order=if options.descending{order.reverse()}else{order};order.then(a.ordinal.cmp(&b.ordinal))
}
fn merge_ordered(mut runs:Vec<PathBuf>,scratch:&mut Scratch,used:&mut u64,quota:u64,max_line:usize,ordinal:bool,cancel:&impl Fn()->bool)->io::Result<Option<PathBuf>> {
    let options=SortOptions{descending:false,case_sensitive:true,numeric:false};
    while runs.len()>1 {
        let mut next=Vec::new();
        for chunk in runs.chunks(8) {
            interrupted(cancel)?;
            let path=scratch.directory.join(format!("run-{}",scratch.paths.len()));scratch.paths.push(path.clone());
            let mut writer=BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
            let mut readers=chunk.iter().map(|path|File::open(path).map(BufReader::new)).collect::<io::Result<Vec<_>>>()?;
            let mut heads=readers.iter_mut().map(|reader|read_row(reader,max_line)).collect::<io::Result<Vec<_>>>()?;
            loop {
                interrupted(cancel)?;
                let pick=heads.iter().enumerate().filter_map(|(i,row)|row.as_ref().map(|row|(i,row))).min_by(|(_,a),(_,b)|if ordinal{a.ordinal.cmp(&b.ordinal)}else{compare(a,b,options)}).map(|(i,_)|i);
                let Some(index)=pick else{break;};let row=heads[index].take().unwrap();
                charge(used,16+row.text.len()as u64,quota)?;write_row(&mut writer,&row)?;heads[index]=read_row(&mut readers[index],max_line)?;
            }
            writer.flush()?;drop(writer);drop(readers);
            for old in chunk {let size=fs::metadata(old)?.len();fs::remove_file(old)?;*used=used.saturating_sub(size);}
            next.push(path);
        }
        runs=next;
    }
    Ok(runs.pop())
}
fn unique_lines(input:impl Read,mut output:impl Write,cache:&Path,memory:usize,quota:u64,cancel:&impl Fn()->bool)->io::Result<u64> {
    fs::create_dir_all(cache)?;static NEXT_UNIQUE:AtomicU64=AtomicU64::new(1);
    let directory=cache.join(format!("power-unique-{}-{}",std::process::id(),NEXT_UNIQUE.fetch_add(1,AtomicOrdering::Relaxed)));fs::create_dir(&directory)?;
    let mut scratch=Scratch{paths:Vec::new(),directory};let mut used=0;let mut rows=Vec::new();let mut runs=Vec::new();let mut bytes=0;let mut ordinal=0;
    let options=SortOptions{descending:false,case_sensitive:true,numeric:false};let mut input=BufReader::new(input);let max_line=memory/16;
    let mut eol=String::new();let mut trailing=false;
    while let Some((text,ending))=next_line(&mut input,max_line,cancel)? {
        if eol.is_empty()&&!ending.is_empty(){eol=ending.clone();}trailing=!ending.is_empty();bytes+=text.len()+32;rows.push(Row{ordinal,text});ordinal+=1;
        if bytes>=memory/4 {runs.push(flush_run(&mut rows,&mut scratch,&mut used,quota,options)?);bytes=0;if runs.len()>memory/256{return Err(io::Error::new(io::ErrorKind::OutOfMemory,"Too many transform runs"));}}
    }
    if !rows.is_empty(){runs.push(flush_run(&mut rows,&mut scratch,&mut used,quota,options)?);}
    let Some(sorted)=merge_ordered(runs,&mut scratch,&mut used,quota,max_line,false,cancel)? else{return Ok(0);};
    let mut input=BufReader::new(File::open(&sorted)?);let mut previous=None::<String>;let mut runs=Vec::new();bytes=0;
    let flush_ordinals=|rows:&mut Vec<Row>,scratch:&mut Scratch,used:&mut u64|->io::Result<PathBuf>{
        rows.sort_by_key(|row|row.ordinal);let path=scratch.directory.join(format!("run-{}",scratch.paths.len()));scratch.paths.push(path.clone());
        let mut writer=BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
        for row in rows.drain(..){charge(used,16+row.text.len()as u64,quota)?;write_row(&mut writer,&row)?;}writer.flush()?;Ok(path)
    };
    while let Some(row)=read_row(&mut input,max_line)? {
        interrupted(cancel)?;if previous.as_ref()==Some(&row.text){continue;}previous=Some(row.text.clone());bytes+=row.text.len()+32;rows.push(row);
        if bytes>=memory/4 {runs.push(flush_ordinals(&mut rows,&mut scratch,&mut used)?);bytes=0;if runs.len()>memory/256{return Err(io::Error::new(io::ErrorKind::OutOfMemory,"Too many transform runs"));}}
    }
    if !rows.is_empty(){runs.push(flush_ordinals(&mut rows,&mut scratch,&mut used)?);}
    drop(input);let size=fs::metadata(&sorted)?.len();fs::remove_file(sorted)?;used=used.saturating_sub(size);
    let Some(path)=merge_ordered(runs,&mut scratch,&mut used,quota,max_line,true,cancel)?else{return Ok(0);};
    let mut input=BufReader::new(File::open(path)?);let mut row=read_row(&mut input,max_line)?;let mut written=0;
    let eol=if eol.is_empty(){"\n"}else{&eol};
    while let Some(current)=row {interrupted(cancel)?;row=read_row(&mut input,max_line)?;charge(&mut written,current.text.len()as u64,quota)?;output.write_all(current.text.as_bytes())?;if row.is_some()||trailing{charge(&mut written,eol.len()as u64,quota)?;output.write_all(eol.as_bytes())?;}}
    output.flush()?;Ok(written)
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
    fn streamed_indent_includes_empty_rows_without_inventing_a_trailing_row(){
        for (input,expected) in [("",""),("\n","    \n"),("\r\n\r\nx\r","    \r\n    \r\n    x\r"),("x\n\n","    x\n    \n")] {
            let mut output=Vec::new();
            transform_lines(std::io::Cursor::new(input),&mut output,&std::env::temp_dir(),64*1024,1024,crate::power::Transform::Indent,4,||false).unwrap();
            assert_eq!(output,expected.as_bytes(),"input={input:?}");
        }
    }
    #[test]
    fn streamed_move_preserves_empty_rows_and_final_eol_policy(){
        for (selected,neighbor,down) in [("a\r\n\r\n","b",true),("a\n","b\r\n",false),("a","b\n",false)] {
            let expected=crate::power::move_rows(selected,neighbor,down);
            let (input,pivot)=if down{(format!("{selected}{neighbor}"),selected.len())}else{(format!("{neighbor}{selected}"),neighbor.len())};
            let mut output=Vec::new();move_lines(std::io::Cursor::new(input),&mut output,pivot as u64,down,1024,||false).unwrap();
            assert_eq!(output,expected.as_bytes());
        }
    }
    #[test]
    fn streamed_transforms_match_resident_commands_on_mixed_unicode_rows(){
        use crate::power::{Transform,Limits,SelectionSet};
        let input="  界 a\r\n\r\n  界 a\nBeta\r\nBeta\r\n\tomega  ";
        for action in [Transform::Uppercase,Transform::Lowercase,Transform::Titlecase,Transform::InvertCase,Transform::TrimStart,Transform::TrimEnd,Transform::Trim,Transform::Indent,Transform::Unindent,Transform::TabsToSpaces,Transform::SpacesToTabs,Transform::Duplicate,Transform::DuplicateSelections,Transform::Join,Transform::Split{column:3},Transform::RemoveDuplicates,Transform::RemoveConsecutiveDuplicates,Transform::RemoveEmpty,Transform::RemoveBlank] {
            let document=bareline_document::Document::from_utf8(input,bareline_document::Budget::new(1<<20),bareline_document::Budget::new(1<<20)).unwrap();
            let set=SelectionSet{selections:vec![crate::Selection{anchor:0,caret:input.len()}],primary:0};
            let expected=crate::power::transform(&document.snapshot(),&set,action.clone(),Limits::default()).unwrap().transaction.edits.remove(0).insert;
            let mut output=Vec::new();
            transform_lines(std::io::Cursor::new(input),&mut output,&std::env::temp_dir(),64*1024,8<<20,action.clone(),Limits::default().tab_width,||false).unwrap();
            assert_eq!(String::from_utf8(output).unwrap(),expected,"{action:?}");
        }
    }
    #[test]
    fn external_unique_keeps_first_occurrence_order_across_runs(){
        let input=(0..4000).rev().chain(0..4000).map(|n|format!("{n}\r\n")).collect::<String>();
        let expected=(0..4000).rev().map(|n|format!("{n}\r\n")).collect::<String>();
        let mut output=Vec::new();
        transform_lines(std::io::Cursor::new(input),&mut output,&std::env::temp_dir(),64*1024,8<<20,crate::power::Transform::RemoveDuplicates,4,||false).unwrap();
        assert_eq!(output,expected.as_bytes());
    }
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
