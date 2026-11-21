//! Dedicated uncached metadata session for one remote owner worker.
use super::contract::SessionId;
use crate::{catalog::transport::MetadataTransport,native::NativeError,paths};
pub(crate) use crate::catalog::transport::{MetadataRequest,TransportError};
use std::{fs::{self,File,OpenOptions},os::{fd::AsRawFd,unix::fs::{MetadataExt,OpenOptionsExt}},path::{Path,PathBuf},sync::{Arc,atomic::AtomicBool},time::Instant};

struct Active {id:String,url:String,requested_at:Instant}
pub(crate) struct MetadataCompletion {
    pub request_id:String,pub request_url:String,pub http_status:Option<u16>,pub final_url:Option<String>,
    pub retry_after_seconds:Option<u32>,pub body:Option<Vec<u8>>,pub error:Option<TransportError>,pub requested_at:Instant,pub session:SessionId,
}
pub(crate) struct RemoteMetadata {
    transport:MetadataTransport,root:PathBuf,root_handle:File,metadata_dir:PathBuf,metadata_handle:File,
    owner_identity:(u64,u64),active:Option<Active>,closing:bool,session:SessionId,
}
fn storage()->NativeError{NativeError::new("transportStorage","Remote metadata ownership or storage is unavailable.")}
fn transport_error(kind:&str)->TransportError{TransportError{kind:kind.into(),message:"Remote metadata request could not be completed.".into()}}
fn dir(path:&Path)->Result<File,NativeError>{Ok(OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(path).map_err(|_|storage())?)}
fn identity(m:&fs::Metadata)->(u64,u64){(m.dev(),m.ino())}
fn generated_route(url:&str)->bool{
    if url=="https://gamebanana.com/apiv11/Util/Fileservers?_nPage=1"{return true;}
    let Some(rest)=url.strip_prefix("https://gamebanana.com/apiv11/")else{return false;};
    let mut parts=rest.split('/');let (Some(model),Some(id),Some(page),None)=(parts.next(),parts.next(),parts.next(),parts.next())else{return false;};
    matches!(model,"Mod"|"Sound"|"Wip")&&matches!(page,"ProfilePage"|"DownloadPage")
        &&crate::catalog::contract::u64_text(&json::JsonValue::from(id)).is_ok_and(|n|n>0)
}
impl RemoteMetadata {
    pub(crate) fn new(root:PathBuf,session:SessionId)->Result<Self,NativeError>{
        paths::reject_symlinks(&root).map_err(|_|storage())?;
        let root=fs::canonicalize(root).map_err(|_|storage())?;let root_handle=dir(&root)?;
        let metadata_dir=root.join("cache/remote-metadata");paths::private_dir(&metadata_dir).map_err(|_|storage())?;
        paths::reject_symlinks(&metadata_dir).map_err(|_|storage())?;
        let metadata_handle=dir(&metadata_dir)?;let meta=metadata_handle.metadata().map_err(|_|storage())?;
        unsafe extern "C"{fn geteuid()->u32;fn flock(fd:i32,operation:i32)->i32;}
        let uid=unsafe{geteuid()};
        if meta.uid()!=uid||(meta.mode()&0o077)!=0{return Err(storage());}
        let owner_path=metadata_dir.join(".owner.lock");
        let owner=OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(0x20000|0x800).open(&owner_path).map_err(|_|storage())?;
        let owner_meta=owner.metadata().map_err(|_|storage())?;let named=fs::symlink_metadata(&owner_path).map_err(|_|storage())?;
        if !owner_meta.is_file()||!named.is_file()||owner_meta.uid()!=uid||owner_meta.nlink()!=1||(owner_meta.mode()&0o077)!=0||identity(&owner_meta)!=identity(&named){return Err(storage());}
        if unsafe{flock(owner.as_raw_fd(),2|4)}!=0{return Err(NativeError::new("remoteBusy","Another remote metadata owner is still active."));}
        let owner_identity=identity(&owner_meta);
        let transport=MetadataTransport::new_remote(metadata_dir.join("spools"),session.as_str().to_owned(),owner)?;
        let metadata=Self{transport,root,root_handle,metadata_dir,metadata_handle,owner_identity,active:None,closing:false,session};
        metadata.validate_owner()?;Ok(metadata)
    }
    fn validate_owner(&self)->Result<(),NativeError>{
        paths::reject_symlinks(&self.root).map_err(|_|storage())?;
        paths::reject_symlinks(&self.metadata_dir).map_err(|_|storage())?;
        let root=fs::metadata(&self.root).map_err(|_|storage())?;let held_root=self.root_handle.metadata().map_err(|_|storage())?;
        let meta=fs::metadata(&self.metadata_dir).map_err(|_|storage())?;let held=self.metadata_handle.metadata().map_err(|_|storage())?;
        let owner=fs::symlink_metadata(self.metadata_dir.join(".owner.lock")).map_err(|_|storage())?;
        if identity(&root)!=identity(&held_root)||identity(&meta)!=identity(&held)||identity(&owner)!=self.owner_identity||!owner.is_file()||owner.uid()!=held.uid()||owner.nlink()!=1||(owner.mode()&0o077)!=0{return Err(storage());}Ok(())
    }
    pub(crate) fn submit(&mut self,request:MetadataRequest)->Result<(),TransportError>{
        if self.closing{return Err(transport_error("remoteBusy"));}
        if self.validate_owner().is_err(){self.begin_close();return Err(transport_error("transportStorage"));}
        if self.active.is_some(){return Err(transport_error("remoteBusy"));}
        if !generated_route(&request.url){return Err(transport_error("transportFraming"));}
        let active=Active{id:request.request_id.clone(),url:request.url.clone(),requested_at:Instant::now()};
        self.transport.submit(request)?;self.active=Some(active);Ok(())
    }
    pub(crate) fn poll_for(&mut self,id:&str)->Option<MetadataCompletion>{
        if self.active.as_ref().is_none_or(|a|a.id!=id){return None;}
        if self.validate_owner().is_err(){self.begin_close();}
        let result=self.transport.poll_for(id)?;let active=self.active.take()?;
        // MetadataBody is a single-live-body lease. Copy its bounded bytes and
        // drop that lease before the next request can be consumed.
        let body=result.body.map(|body|body.to_vec());
        Some(MetadataCompletion{request_id:result.request_id,request_url:active.url,http_status:result.http_status,final_url:result.final_url,
            retry_after_seconds:result.retry_after_seconds,body,error:result.error,requested_at:active.requested_at,session:self.session.clone()})
    }
    pub(crate) fn begin_close(&mut self){if !self.closing{self.closing=true;self.transport.begin_remote_close();}}
    pub(crate) fn is_quiescent(&mut self)->bool{self.closing&&self.transport.remote_is_quiescent()}
    /// The owner may retain this latch across an unexpected worker drop.
    pub(crate) fn retirement_ticket(&self)->Arc<AtomicBool>{self.transport.remote_retirement_ticket().expect("remote lifetime mode")}
}
impl Drop for RemoteMetadata{fn drop(&mut self){self.begin_close();}}
