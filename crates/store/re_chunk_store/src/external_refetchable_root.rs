//! Production-disarmed Web remote-MCAP external root origins.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use re_chunk::{Chunk, ChunkId};
use re_log_types::StoreId;

use crate::lineage::TrackedDirectChunkLineage;
use crate::{
    ChunkDirectLineage, ChunkDirectLineageReport, ChunkStore, ChunkStoreConfig, ChunkStoreError,
    ChunkStoreEvent,
};

static NEXT_WEB_REMOTE_MCAP_STORE_TOKEN_V1: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, re_byte_size::SizeBytes)]
pub struct ExternalRefetchableRootDescriptorIdentityV1(ChunkId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, re_byte_size::SizeBytes)]
pub struct ExternalRefetchableRootDescriptorV1 {
    root_chunk_id: ChunkId,
    identity: ExternalRefetchableRootDescriptorIdentityV1,
    store_token: u64,
    is_static: bool,
}

impl ExternalRefetchableRootDescriptorV1 {
    pub fn root_chunk_id_v1(self) -> ChunkId {
        self.root_chunk_id
    }

    pub fn identity_v1(self) -> ExternalRefetchableRootDescriptorIdentityV1 {
        self.identity
    }

    pub fn is_static_v1(self) -> bool {
        self.is_static
    }
}

#[derive(Clone, Debug, PartialEq, Eq, re_byte_size::SizeBytes)]
pub struct ExternalRefetchableRootOriginV1 {
    descriptor: ExternalRefetchableRootDescriptorV1,
}

impl ExternalRefetchableRootOriginV1 {
    pub fn descriptor_v1(&self) -> ExternalRefetchableRootDescriptorV1 {
        self.descriptor
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalRefetchableRootExistenceV1 {
    Unloaded,
    Resident,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalRefetchableRootErrorV1 {
    #[error("the Web remote-MCAP Store capability is revoked")]
    CapabilityRevoked,

    #[error("the external root capability belongs to a different Store")]
    WrongStore,

    #[error("the external root origin is not registered")]
    UnknownRoot,

    #[error("the root identity conflicts with an existing origin")]
    OriginConflict,

    #[error("the external root is already resident")]
    AlreadyResident,

    #[error("the physical root does not match its sealed descriptor")]
    ChunkMismatch,

    #[error(transparent)]
    Store(#[from] ChunkStoreError),
}

/// A production-disarmed configuration for a Web remote-MCAP Store.
///
/// The public type deliberately has no public production constructor.
pub struct WebRemoteMcapStoreConfigV1 {
    store_token: u64,
}

impl WebRemoteMcapStoreConfigV1 {
    #[cfg(test)]
    pub(crate) fn for_test_v1() -> Self {
        let store_token = NEXT_WEB_REMOTE_MCAP_STORE_TOKEN_V1
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |token| {
                token.checked_add(1)
            })
            .expect("the test-only Web remote-MCAP Store token space is not exhausted");
        Self { store_token }
    }

    pub fn into_store_v1(self, store_id: StoreId) -> (ChunkStore, WebRemoteMcapRootCapabilityV1) {
        let mut store = ChunkStore::new(store_id.clone(), ChunkStoreConfig::COMPACTION_DISABLED);
        store.web_remote_mcap_store_token_v1 = Some(self.store_token);
        let capability = WebRemoteMcapRootCapabilityV1 {
            store_id,
            store_token: self.store_token,
            active: Arc::new(AtomicBool::new(true)),
        };
        (store, capability)
    }
}

/// Move-only representation authority for external roots owned by one Web remote-MCAP Store.
pub struct WebRemoteMcapRootCapabilityV1 {
    store_id: StoreId,
    store_token: u64,
    active: Arc<AtomicBool>,
}

impl WebRemoteMcapRootCapabilityV1 {
    fn ensure_active_v1(&self) -> Result<(), ExternalRefetchableRootErrorV1> {
        self.active
            .load(Ordering::Acquire)
            .then_some(())
            .ok_or(ExternalRefetchableRootErrorV1::CapabilityRevoked)
    }

    fn ensure_store_v1(&self, store: &ChunkStore) -> Result<(), ExternalRefetchableRootErrorV1> {
        (store.id == self.store_id
            && store.web_remote_mcap_store_token_v1 == Some(self.store_token))
        .then_some(())
        .ok_or(ExternalRefetchableRootErrorV1::WrongStore)
    }

    pub fn register_root_origin_v1(
        &self,
        store: &mut ChunkStore,
        root_chunk_id: ChunkId,
        is_static: bool,
    ) -> Result<ExternalRefetchableRootDescriptorV1, ExternalRefetchableRootErrorV1> {
        self.ensure_active_v1()?;
        self.ensure_store_v1(store)?;
        let descriptor = ExternalRefetchableRootDescriptorV1 {
            root_chunk_id,
            identity: ExternalRefetchableRootDescriptorIdentityV1(root_chunk_id),
            store_token: self.store_token,
            is_static,
        };
        let origin = ExternalRefetchableRootOriginV1 { descriptor };
        match store.chunks_lineage.entry(root_chunk_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(TrackedDirectChunkLineage {
                    lineage: ChunkDirectLineage::RootFromExternalSource(origin),
                    ref_count: 0,
                    descends_from_manifest: false,
                    descends_from_external_source: true,
                });
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                let ChunkDirectLineage::RootFromExternalSource(existing) = &entry.get().lineage
                else {
                    return Err(ExternalRefetchableRootErrorV1::OriginConflict);
                };
                if existing.descriptor != descriptor {
                    return Err(ExternalRefetchableRootErrorV1::OriginConflict);
                }
            }
        }
        Ok(descriptor)
    }

    pub fn issue_refetch_v1(
        &self,
        store: &ChunkStore,
        descriptor: ExternalRefetchableRootDescriptorV1,
    ) -> Result<WebRemoteMcapRootRefetchPermitV1, ExternalRefetchableRootErrorV1> {
        self.ensure_active_v1()?;
        self.ensure_store_v1(store)?;
        if descriptor.store_token != self.store_token {
            return Err(ExternalRefetchableRootErrorV1::WrongStore);
        }
        match store.external_refetchable_root_existence_v1(descriptor)? {
            ExternalRefetchableRootExistenceV1::Unloaded => Ok(WebRemoteMcapRootRefetchPermitV1 {
                descriptor,
                active: Arc::clone(&self.active),
            }),
            ExternalRefetchableRootExistenceV1::Resident => {
                Err(ExternalRefetchableRootErrorV1::AlreadyResident)
            }
        }
    }

    pub fn revoke_v1(self) {
        self.active.store(false, Ordering::Release);
    }
}

impl Drop for WebRemoteMcapRootCapabilityV1 {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

/// One-shot authority to insert one exact external root while its representation remains active.
pub struct WebRemoteMcapRootRefetchPermitV1 {
    descriptor: ExternalRefetchableRootDescriptorV1,
    active: Arc<AtomicBool>,
}

impl ChunkStore {
    pub fn external_refetchable_root_existence_v1(
        &self,
        descriptor: ExternalRefetchableRootDescriptorV1,
    ) -> Result<ExternalRefetchableRootExistenceV1, ExternalRefetchableRootErrorV1> {
        if self.web_remote_mcap_store_token_v1 != Some(descriptor.store_token) {
            return Err(ExternalRefetchableRootErrorV1::WrongStore);
        }
        let Some(lineage) = self.chunks_lineage.get(&descriptor.root_chunk_id) else {
            return Err(ExternalRefetchableRootErrorV1::UnknownRoot);
        };
        let ChunkDirectLineage::RootFromExternalSource(origin) = &lineage.lineage else {
            return Err(ExternalRefetchableRootErrorV1::OriginConflict);
        };
        if origin.descriptor != descriptor {
            return Err(ExternalRefetchableRootErrorV1::WrongStore);
        }
        Ok(
            if self
                .physical_chunks_per_chunk_id
                .contains_key(&descriptor.root_chunk_id)
            {
                ExternalRefetchableRootExistenceV1::Resident
            } else {
                ExternalRefetchableRootExistenceV1::Unloaded
            },
        )
    }

    #[must_use = "The chunk store events should be handled"]
    pub fn insert_external_refetchable_root_v1(
        &mut self,
        permit: WebRemoteMcapRootRefetchPermitV1,
        chunk: &Arc<Chunk>,
    ) -> Result<Vec<ChunkStoreEvent>, ExternalRefetchableRootErrorV1> {
        let WebRemoteMcapRootRefetchPermitV1 { descriptor, active } = permit;
        if !active.load(Ordering::Acquire) {
            return Err(ExternalRefetchableRootErrorV1::CapabilityRevoked);
        }
        if chunk.id() != descriptor.root_chunk_id || chunk.is_static() != descriptor.is_static {
            return Err(ExternalRefetchableRootErrorV1::ChunkMismatch);
        }
        if self.external_refetchable_root_existence_v1(descriptor)?
            != ExternalRefetchableRootExistenceV1::Unloaded
        {
            return Err(ExternalRefetchableRootErrorV1::AlreadyResident);
        }
        let origin = ExternalRefetchableRootOriginV1 { descriptor };
        let diffs = self.insert_chunk_impl(
            chunk,
            ChunkDirectLineageReport::RootFromExternalSource(origin),
        )?;
        Ok(self.finalize_events(diffs))
    }
}

#[cfg(test)]
mod tests {
    use re_chunk::{Chunk, RowId};
    use re_log_types::{StoreKind, Timeline};
    use re_sdk_types::archetypes;

    use crate::{ChunkDirectLineage, GarbageCollectionOptions};

    use super::*;

    fn temporal_chunk(root_chunk_id: ChunkId) -> Arc<Chunk> {
        Arc::new(
            Chunk::builder_with_id(root_chunk_id, "world/points")
                .with_archetype(
                    RowId::new(),
                    [(Timeline::log_tick(), 1)],
                    &archetypes::Points3D::new([[1.0, 2.0, 3.0]]),
                )
                .build()
                .unwrap(),
        )
    }

    fn store_and_capability() -> (ChunkStore, WebRemoteMcapRootCapabilityV1) {
        WebRemoteMcapStoreConfigV1::for_test_v1()
            .into_store_v1(StoreId::random(StoreKind::Recording, "external-root-test"))
    }

    #[test]
    fn root_identity_survives_insert_gc_and_refetch() {
        let (mut store, capability) = store_and_capability();
        let root_chunk_id = ChunkId::new();
        let descriptor = capability
            .register_root_origin_v1(&mut store, root_chunk_id, false)
            .unwrap();
        let duplicate = capability
            .register_root_origin_v1(&mut store, root_chunk_id, false)
            .unwrap();
        assert_eq!(descriptor.identity_v1(), duplicate.identity_v1());
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Unloaded
        );

        let chunk = temporal_chunk(root_chunk_id);
        assert!(store.insert_chunk(&chunk).unwrap().is_empty());
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Unloaded
        );
        let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
        store
            .insert_external_refetchable_root_v1(permit, &chunk)
            .unwrap();
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Resident
        );
        assert!(store.find_root_manifest_chunks(&root_chunk_id).is_empty());
        assert!(matches!(
            store.direct_lineage(&root_chunk_id),
            Some(ChunkDirectLineage::RootFromExternalSource(_))
        ));

        store.gc(&GarbageCollectionOptions::gc_everything());
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Unloaded
        );
        assert_eq!(descriptor.identity_v1(), duplicate.identity_v1());

        let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
        store
            .insert_external_refetchable_root_v1(permit, &chunk)
            .unwrap();
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Resident
        );
    }

    #[test]
    fn revocation_cancels_outstanding_refetch_but_preserves_origin() {
        let (mut store, capability) = store_and_capability();
        let root_chunk_id = ChunkId::new();
        let descriptor = capability
            .register_root_origin_v1(&mut store, root_chunk_id, false)
            .unwrap();
        let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
        capability.revoke_v1();

        assert!(matches!(
            store.insert_external_refetchable_root_v1(permit, &temporal_chunk(root_chunk_id)),
            Err(ExternalRefetchableRootErrorV1::CapabilityRevoked)
        ));
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Unloaded
        );
    }

    #[test]
    fn dropping_capability_cancels_outstanding_refetch_but_preserves_origin() {
        let (mut store, capability) = store_and_capability();
        let root_chunk_id = ChunkId::new();
        let descriptor = capability
            .register_root_origin_v1(&mut store, root_chunk_id, false)
            .unwrap();
        let permit = capability.issue_refetch_v1(&store, descriptor).unwrap();
        drop(capability);

        assert!(matches!(
            store.insert_external_refetchable_root_v1(permit, &temporal_chunk(root_chunk_id)),
            Err(ExternalRefetchableRootErrorV1::CapabilityRevoked)
        ));
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Unloaded
        );
    }

    #[test]
    fn rrd_manifest_cannot_overwrite_external_origin() {
        let (mut store, capability) = store_and_capability();
        let root_chunk_id = ChunkId::new();
        let descriptor = capability
            .register_root_origin_v1(&mut store, root_chunk_id, false)
            .unwrap();
        let chunk = temporal_chunk(root_chunk_id);
        let manifest = re_log_encoding::RrdManifest::build_in_memory_from_chunks(
            store.id(),
            std::iter::once(&*chunk),
        )
        .unwrap();

        assert!(store.insert_rrd_manifest(manifest).is_empty());
        assert!(matches!(
            store.direct_lineage(&root_chunk_id),
            Some(ChunkDirectLineage::RootFromExternalSource(_))
        ));
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Unloaded
        );
        assert!(store.insert_chunk(&chunk).unwrap().is_empty());
        assert_eq!(
            store
                .external_refetchable_root_existence_v1(descriptor)
                .unwrap(),
            ExternalRefetchableRootExistenceV1::Unloaded
        );
    }

    #[test]
    fn descriptors_are_store_bound_but_keep_the_same_stable_identity() {
        let (mut first_store, first_capability) = store_and_capability();
        let (mut second_store, second_capability) = store_and_capability();
        let root_chunk_id = ChunkId::new();
        let first = first_capability
            .register_root_origin_v1(&mut first_store, root_chunk_id, false)
            .unwrap();
        let second = second_capability
            .register_root_origin_v1(&mut second_store, root_chunk_id, false)
            .unwrap();

        assert_eq!(first.identity_v1(), second.identity_v1());
        assert!(matches!(
            second_store.external_refetchable_root_existence_v1(first),
            Err(ExternalRefetchableRootErrorV1::WrongStore)
        ));
    }

    #[test]
    fn capability_cannot_cross_two_store_instances_with_the_same_store_id() {
        let store_id = StoreId::random(StoreKind::Recording, "same-store-id-test");
        let (mut first_store, first_capability) =
            WebRemoteMcapStoreConfigV1::for_test_v1().into_store_v1(store_id.clone());
        let (mut second_store, second_capability) =
            WebRemoteMcapStoreConfigV1::for_test_v1().into_store_v1(store_id);
        let root_chunk_id = ChunkId::new();

        assert!(matches!(
            first_capability.register_root_origin_v1(&mut second_store, root_chunk_id, false),
            Err(ExternalRefetchableRootErrorV1::WrongStore)
        ));
        first_capability
            .register_root_origin_v1(&mut first_store, root_chunk_id, false)
            .unwrap();
        second_capability
            .register_root_origin_v1(&mut second_store, root_chunk_id, false)
            .unwrap();
    }
}
