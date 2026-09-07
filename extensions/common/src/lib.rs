// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared reference-client mechanics. All authority remains in the editor broker.
use bareline_extension_sdk::*;
use std::{cell::RefCell,io::{Read,Write},rc::Rc};
pub trait Transport {fn request(&mut self,message:Envelope)->Result<BrokerValue,String>;}
pub struct Client<T:Transport>{pub invocation:Invocation,transport:T,next:u64}
impl<T:Transport> Client<T>{
 pub fn new(invocation:Invocation,transport:T)->Self{Self{invocation,transport,next:1}}
 pub fn call(&mut self,request:Request)->Result<BrokerValue,String>{let(capability,scope)=match &request{Request::ReadTextRange{document,..}|Request::ReadOriginalBytes{document,..}=>(Capability::DocumentRead,Scope::Document(*document)),Request::Panel{..}=>(Capability::UiPanel,Scope::Extension),_=>(Capability::DocumentEdit,Scope::Document(self.invocation.document))};let id=self.next;self.next=self.next.checked_add(1).ok_or("request ID exhausted")?;self.transport.request(Envelope{protocol:PROTOCOL_VERSION,request_id:id,extension_id:self.invocation.extension_id.clone(),context:CapabilityContext{capability,scope,grant_generation:self.invocation.grant_generation},request})}
 pub fn panel(&mut self,panel:&str,text:String)->Result<(),String>{if text.len()>1024*1024{return Err("panel output limit".into());}self.call(Request::Panel{panel:panel.into(),text})?;Ok(())}
 pub fn begin(&mut self)->Result<u64,String>{let value=self.call(Request::BeginEdits{document:self.invocation.document,revision:self.invocation.revision})?;if let BrokerValue::Transaction(id)=value{Ok(id)}else{Err("unexpected BeginEdits response".into())}}
 pub fn commit(&mut self,id:u64)->Result<(),String>{self.call(Request::CommitEdits{transaction:id,revision:self.invocation.revision})?;Ok(())}
}
pub struct Snapshot<T:Transport>{client:Rc<RefCell<Client<T>>>,offset:u64,end:u64,buffer:Vec<u8>,cursor:usize}
impl<T:Transport> Snapshot<T>{pub fn new(client:Rc<RefCell<Client<T>>>)->Self{let end=client.borrow().invocation.text_length;Self{client,offset:0,end,buffer:Vec::new(),cursor:0}}}
impl<T:Transport> Read for Snapshot<T>{fn read(&mut self,out:&mut[u8])->std::io::Result<usize>{if out.is_empty(){return Ok(0);}if self.cursor==self.buffer.len(){if self.offset==self.end{return Ok(0);}let end=self.offset.saturating_add(65536).min(self.end);let mut client=self.client.borrow_mut();let document=client.invocation.document;let revision=client.invocation.revision;let value=client.call(Request::ReadTextRange{document,revision,range:TextRange{start:self.offset,end}}).map_err(std::io::Error::other)?;self.buffer=match value{BrokerValue::Bytes(bytes)=>bytes,BrokerValue::Text(text)=>text.into_bytes(),_=>return Err(std::io::Error::other("unexpected text response"))};if self.buffer.len()as u64!=end-self.offset{return Err(std::io::Error::other("snapshot range length mismatch"));}self.offset=end;self.cursor=0;}let count=out.len().min(self.buffer.len()-self.cursor);out[..count].copy_from_slice(&self.buffer[self.cursor..self.cursor+count]);self.cursor+=count;Ok(count)}}
#[derive(Default)]pub struct Counter(pub u64);
impl Write for Counter{fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{self.0=self.0.checked_add(bytes.len()as u64).ok_or_else(||std::io::Error::other("output length overflow"))?;Ok(bytes.len())}fn flush(&mut self)->std::io::Result<()>{Ok(())}}
pub struct Staged<T:Transport>{client:Rc<RefCell<Client<T>>>,transaction:u64,buffer:Vec<u8>}
impl<T:Transport> Staged<T>{
 pub fn new(client:Rc<RefCell<Client<T>>>,output_bytes:u64)->Result<Self,String>{// PR016 staging currently has a64MiB total cap: reject before opening a transaction.
 if output_bytes>60*1024*1024{return Err("formatter output exceeds current 60 MiB staging budget".into());}let transaction=client.borrow_mut().begin()?;let length=client.borrow().invocation.text_length;let mut value=Self{client,transaction,buffer:Vec::with_capacity(65536)};// postcard Vec length, TextRange, replacement string length; subsequent bytes are UTF8.
 let prefix=postcard::to_stdvec(&(1usize,0u64,length,output_bytes)).map_err(|e|e.to_string())?;value.write_all(&prefix).map_err(|e|e.to_string())?;Ok(value)}
 pub fn finish(mut self)->Result<(),String>{self.flush().map_err(|e|e.to_string())?;self.client.borrow_mut().commit(self.transaction)}
}
impl<T:Transport> Write for Staged<T>{fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{let count=bytes.len().min(65536-self.buffer.len());self.buffer.extend_from_slice(&bytes[..count]);if self.buffer.len()==65536{self.flush()?;}Ok(count)}fn flush(&mut self)->std::io::Result<()>{if !self.buffer.is_empty(){let chunk=std::mem::replace(&mut self.buffer,Vec::with_capacity(65536));self.client.borrow_mut().call(Request::AppendChunk{transaction:self.transaction,chunk}).map_err(std::io::Error::other)?;}Ok(())}}
#[cfg(target_arch="wasm32")]pub struct WasmTransport;
#[cfg(target_arch="wasm32")]impl Transport for WasmTransport{fn request(&mut self,message:Envelope)->Result<BrokerValue,String>{let mut frame=Vec::new();write_frame(&mut frame,&message).map_err(|e|format!("{e:?}"))?;let bytes=bareline_extension_sdk::bindings::bareline::extension::broker::request(&frame)?;let reply:BrokerResponse=postcard::from_bytes(&bytes).map_err(|e|e.to_string())?;if reply.request_id!=message.request_id{return Err("broker reply ID mismatch".into());}reply.result}}
#[cfg(target_arch="wasm32")]pub fn wasm_client()->Result<Rc<RefCell<Client<WasmTransport>>>,String>{let bytes=bareline_extension_sdk::bindings::bareline::extension::broker::invocation();if bytes.len()>16384{return Err("invocation limit".into());}let invocation:Invocation=postcard::from_bytes(&bytes).map_err(|e|e.to_string())?;Ok(Rc::new(RefCell::new(Client::new(invocation,WasmTransport))))}

