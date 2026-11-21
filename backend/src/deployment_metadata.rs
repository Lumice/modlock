//! Persisted checkpoint metadata, deliberately separate from tree ownership.
use crate::{deployment_intent, generation, native::NativeError, package};
use json::{object, JsonValue};
use std::path::Path;

fn invalid() -> NativeError {
    NativeError::new("recoveryRequired", "Deployment checkpoint metadata is inconsistent; preserve the library for recovery.")
}
fn checkpoints(doc: &JsonValue) -> impl Iterator<Item=&JsonValue> {
    std::iter::once(&doc["applied"]).chain(std::iter::once(&doc["previous"]["applied"]))
        .chain(doc["retiredGenerations"].members())
}
pub(crate) fn retained_shards(doc: &JsonValue) -> bool {
    checkpoints(doc).any(|a| a["manifest"]["version"] == 3)
}
/// Pure persisted-document admission. All captured identities are required for
/// generation3; prospective game staging must use its own explicit contract.
pub(crate) fn validate(doc: &JsonValue) -> Result<(), NativeError> {
    deployment_intent::validate_retained_proof(doc)?;
    let layout = package::layout_version(doc)?;
    for a in checkpoints(doc).filter(|a| !a.is_null()) {
        if a["manifest"]["version"] != 3 { continue; }
        let fields = ["deploymentIntent","desiredSignature","profileId","enabled","generation","manifest",
            "gameinfo","gameinfoSha256","identities","directoryIdentity","shardIdentities"];
        if !a.is_object() || a.len()!=fields.len()+usize::from(a.has_key("mountedSubset")) || fields.iter().any(|k|!a.has_key(k))
            || doc["storeVersion"]!=4 { return Err(invalid()); }
        let view=generation::decode(&a["manifest"])?;
        if view.package_version!=layout { return Err(invalid()); }
        generation::validate_tree_identities(&view,&a["directoryIdentity"],&a["shardIdentities"],&a["identities"])?;
        if a["identities"]["manifest.json"]["size"].as_u64()!=Some(a["manifest"].dump().len() as u64) {
            return Err(invalid());
        }
    }
    Ok(())
}
/// Bind every retained generation to the concrete owning Store, without I/O.
pub(crate) fn admit(root: &Path, doc: &JsonValue) -> Result<(), NativeError> {
    validate(doc)?;
    let layout=package::layout_version(doc)?;
    for a in checkpoints(doc).filter(|a| !a.is_null()) {
        let view=generation::decode(&a["manifest"])?;
        if a["manifest"]["storeRoot"].as_str()!=root.to_str() || view.generation!=a["generation"].as_str().ok_or_else(invalid)?
            || view.package_version!=layout { return Err(invalid()); }
    }
    Ok(())
}
/// Selective new outer formats carry retained3. Historical formats remain flat.
pub(crate) fn domain(doc: &JsonValue, shard_outer: bool) -> Result<(),NativeError> {
    if shard_outer {
        validate(doc)?;
        if doc["storeVersion"]!=4 || !retained_shards(doc) { return Err(invalid()); }
    } else { generation::require_flat_document(doc)?; }
    Ok(())
}

/// Size reservation only, never ownership evidence. The input is the exact
/// immutable generation3 checkpoint before capture. Consumers must admit the
/// containing before/after/requested documents and journal/outcome envelopes.
pub fn reserve_shard_checkpoint(checkpoint: &JsonValue) -> Result<JsonValue,NativeError> {
    let fields=["deploymentIntent","desiredSignature","profileId","enabled","generation","manifest","gameinfo","gameinfoSha256"];
    if !checkpoint.is_object() || checkpoint.len()!=fields.len()+usize::from(checkpoint.has_key("mountedSubset"))
        || fields.iter().any(|k|!checkpoint.has_key(k)) {return Err(invalid());}
    deployment_intent::applied_intent(checkpoint)?.ok_or_else(invalid)?;
    let view=generation::decode(&checkpoint["manifest"])?;
    if view.version!=3 {return Err(invalid());}
    let identity=|size: u64| object!{size:size,device:u64::MAX,inode:u64::MAX,
        modifiedSeconds:i64::MIN,modifiedNanos:999_999_999,fsid:"ffffffffffffffff"};
    let mut reserved=checkpoint.clone();
    reserved["directoryIdentity"]=identity(u64::MAX);
    let mut directories=JsonValue::new_object();
    for directory in view.directories.iter().skip(1) {
        directories[&directory.relative_path]=identity(u64::MAX);
    }
    let mut files=JsonValue::new_object();
    for file in &view.files {files[&file.name]=identity(file.payload.size);}
    files["manifest.json"]=identity(checkpoint["manifest"].dump().len() as u64);
    generation::validate_tree_identities(&view,&reserved["directoryIdentity"],&directories,&files)?;
    reserved["shardIdentities"]=directories;
    reserved["identities"]=files;
    crate::store::ensure_internal(&reserved)?;
    Ok(reserved)
}
