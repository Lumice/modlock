//! Owner-created remote quarantine authority; never serialized or renderer supplied.
use super::{contract::{ClaimId, JobId}, job_store::RemoteStoreLease, preparation_slot::PreparationSlotLease};
use crate::{archive_quarantine::{self, QuarantineLease, RecoveredQuarantine}, native::NativeError};
use std::{path::Path, sync::Arc};

#[derive(Clone)]
pub(crate) struct RemoteQuarantineAuthority {
    pub(crate) store_lease: Arc<RemoteStoreLease>,
    pub(crate) job: JobId,
    pub(crate) nonce: ClaimId,
}
impl RemoteQuarantineAuthority {
    pub(crate) fn new(store_lease: Arc<RemoteStoreLease>, job: JobId, nonce: ClaimId) -> Result<Self, NativeError> {
        store_lease.validate()?;
        Ok(Self { store_lease, job, nonce })
    }
    pub(crate) fn root(&self) -> &Path { self.store_lease.root() }
    pub(crate) fn validate(&self, slot: &PreparationSlotLease) -> Result<(), NativeError> {
        self.store_lease.validate()?;
        slot.validate()?;
        if slot.root() != self.root() || slot.identity() != self.store_lease.identity() {
            return Err(NativeError::new("recoveryRequired", "Preparation ownership does not match this library."));
        }
        Ok(())
    }
    pub(crate) fn create(&self, slot: &PreparationSlotLease) -> Result<Arc<QuarantineLease>, NativeError> {
        self.validate(slot)?;
        archive_quarantine::create_remote(self, slot)
    }
    pub(crate) fn inspect(&self, slot: &PreparationSlotLease) -> Result<Vec<RecoveredQuarantine>, NativeError> {
        self.validate(slot)?;
        archive_quarantine::inspect_remote(self, slot)
    }
    pub(crate) fn recover(&self, slot: &PreparationSlotLease, admitted: &[(String, String)]) -> Result<Vec<RecoveredQuarantine>, NativeError> {
        self.validate(slot)?;
        archive_quarantine::recover_remote(self, slot, admitted)
    }
}
