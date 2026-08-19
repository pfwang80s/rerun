//! Production-disarmed remote consumer and privileged storage capability boundaries.
//!
//! This module splits MCAP-095's gated recording-query facade into two surfaces:
//! a storage-free recording-consumer capability and a sealed privileged control-plane
//! capability. It deliberately does not import or publish a `StoreHub`, `StoreBundle`,
//! `EntityDb`, or storage engine handle. Real Viewer wiring remains a later work item.

#![allow(dead_code)]

use std::fmt;

use re_chunk::{RangeQuery, TimeInt, TimelineName};
use re_log_types::StoreId;

use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
use crate::web_remote_mcap_query::{
    CommittedPresentationTimeV1, ConsumerStorageFreeV1, GatedRecordingQueryFacadeV1,
    PresentationLeaseUnavailableV1, PresentationQueryLeaseV1, PresentationQuerySnapshotV1,
    PrivilegedViewerFrameContextV1, RemoteLoadedCoverageV1, RemoteMutationLeaseDrainV1,
    RemotePresentationFacadeV1, RemoteRangeQueryUnavailableV1,
};

mod private {
    pub(crate) struct ConsumerContextSealV1;
    pub(crate) struct LocalPassthroughSealV1;
    pub(crate) trait SealedRecordingQueryV1 {}
}

trait RecordingLeaseBackendV1: ConsumerStorageFreeV1 {
    fn try_lease_v1(&self) -> Result<PresentationQueryLeaseV1<'_>, PresentationLeaseUnavailableV1>;

    fn try_complete_range_lease_v1(
        &self,
        query: &RangeQuery,
    ) -> Result<PresentationQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1>;

    fn is_current_snapshot_v1(&self, snapshot: &PresentationQuerySnapshotV1) -> bool;
}

impl RecordingLeaseBackendV1 for RemotePresentationFacadeV1 {
    fn try_lease_v1(&self) -> Result<PresentationQueryLeaseV1<'_>, PresentationLeaseUnavailableV1> {
        GatedRecordingQueryFacadeV1::try_lease_v1(self)
    }

    fn try_complete_range_lease_v1(
        &self,
        query: &RangeQuery,
    ) -> Result<PresentationQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1> {
        GatedRecordingQueryFacadeV1::try_complete_range_lease_v1(self, query)
    }

    fn is_current_snapshot_v1(&self, snapshot: &PresentationQuerySnapshotV1) -> bool {
        GatedRecordingQueryFacadeV1::is_current_v1(self, snapshot)
    }
}

/// Opaque recording-query snapshot. The underlying facade instance, `StoreId`, and epoch stay private.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RecordingQuerySnapshotV1(PresentationQuerySnapshotV1);

impl RecordingQuerySnapshotV1 {
    pub(crate) fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        self.0.committed_time_v1().cloned()
    }

    fn inner_v1(&self) -> &PresentationQuerySnapshotV1 {
        &self.0
    }
}

impl fmt::Debug for RecordingQuerySnapshotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecordingQuerySnapshotV1")
            .field("committed_time", &self.committed_time_v1())
            .finish()
    }
}

/// Short-lived recording-query lease. It is deliberately not `Clone` or `'static`.
pub(crate) struct RecordingQueryLeaseV1<'a> {
    inner: PresentationQueryLeaseV1<'a>,
}

impl RecordingQueryLeaseV1<'_> {
    pub(crate) fn snapshot_v1(&self) -> RecordingQuerySnapshotV1 {
        RecordingQuerySnapshotV1(self.inner.snapshot.clone())
    }

    pub(crate) fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        self.inner.snapshot.committed_time_v1().cloned()
    }
}

impl fmt::Debug for RecordingQueryLeaseV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecordingQueryLeaseV1")
            .field("committed_time", &self.committed_time_v1())
            .finish()
    }
}

pub(crate) trait RecordingConsumerQueryV1: private::SealedRecordingQueryV1 {
    fn try_lease_v1(&self) -> Result<RecordingQueryLeaseV1<'_>, PresentationLeaseUnavailableV1>;

    fn try_complete_range_lease_v1(
        &self,
        query: &RangeQuery,
    ) -> Result<RecordingQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1>;

    fn is_current_v1(&self, snapshot: &RecordingQuerySnapshotV1) -> bool;
}

fn wrap_lease_v1(inner: PresentationQueryLeaseV1<'_>) -> RecordingQueryLeaseV1<'_> {
    RecordingQueryLeaseV1 { inner }
}

/// Storage-free recording consumer for the remote facade.
///
/// It holds only the gated lease backend and a private seal. It does not expose the facade,
/// revision, `StoreId`, or physical Store.
pub(crate) struct RecordingConsumerContextV1<'a> {
    backend: &'a dyn RecordingLeaseBackendV1,
    _sealed: private::ConsumerContextSealV1,
}

impl<'a> RecordingConsumerContextV1<'a> {
    pub(crate) fn from_remote_facade_v1(facade: &'a RemotePresentationFacadeV1) -> Self {
        Self {
            backend: facade,
            _sealed: private::ConsumerContextSealV1,
        }
    }
}

impl private::SealedRecordingQueryV1 for RecordingConsumerContextV1<'_> {}

impl RecordingConsumerQueryV1 for RecordingConsumerContextV1<'_> {
    fn try_lease_v1(&self) -> Result<RecordingQueryLeaseV1<'_>, PresentationLeaseUnavailableV1> {
        Ok(wrap_lease_v1(self.backend.try_lease_v1()?))
    }

    fn try_complete_range_lease_v1(
        &self,
        query: &RangeQuery,
    ) -> Result<RecordingQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1> {
        Ok(wrap_lease_v1(
            self.backend.try_complete_range_lease_v1(query)?,
        ))
    }

    fn is_current_v1(&self, snapshot: &RecordingQuerySnapshotV1) -> bool {
        self.backend.is_current_snapshot_v1(snapshot.inner_v1())
    }
}

/// Production-disarmed native/local passthrough over the same validated query surface.
///
/// A real native adapter would replace the backend with an ordinary local recording query. This
/// implementation intentionally does not construct or hold an `EntityDb`.
pub(crate) struct LocalRecordingPassthroughV1<'a> {
    backend: &'a dyn RecordingLeaseBackendV1,
    _sealed: private::LocalPassthroughSealV1,
}

impl<'a> LocalRecordingPassthroughV1<'a> {
    pub(crate) fn from_facade_v1(facade: &'a RemotePresentationFacadeV1) -> Self {
        Self {
            backend: facade,
            _sealed: private::LocalPassthroughSealV1,
        }
    }
}

impl private::SealedRecordingQueryV1 for LocalRecordingPassthroughV1<'_> {}

impl RecordingConsumerQueryV1 for LocalRecordingPassthroughV1<'_> {
    fn try_lease_v1(&self) -> Result<RecordingQueryLeaseV1<'_>, PresentationLeaseUnavailableV1> {
        Ok(wrap_lease_v1(self.backend.try_lease_v1()?))
    }

    fn try_complete_range_lease_v1(
        &self,
        query: &RangeQuery,
    ) -> Result<RecordingQueryLeaseV1<'_>, RemoteRangeQueryUnavailableV1> {
        Ok(wrap_lease_v1(
            self.backend.try_complete_range_lease_v1(query)?,
        ))
    }

    fn is_current_v1(&self, snapshot: &RecordingQuerySnapshotV1) -> bool {
        self.backend.is_current_snapshot_v1(snapshot.inner_v1())
    }
}

impl ConsumerStorageFreeV1 for private::ConsumerContextSealV1 {}
impl ConsumerStorageFreeV1 for private::LocalPassthroughSealV1 {}

impl ConsumerStorageFreeV1 for RecordingQuerySnapshotV1 where
    PresentationQuerySnapshotV1: ConsumerStorageFreeV1
{
}

impl<'a> ConsumerStorageFreeV1 for RecordingQueryLeaseV1<'a> where
    PresentationQueryLeaseV1<'a>: ConsumerStorageFreeV1
{
}

impl<'a> ConsumerStorageFreeV1 for RecordingConsumerContextV1<'a>
where
    &'a dyn RecordingLeaseBackendV1: ConsumerStorageFreeV1,
    private::ConsumerContextSealV1: ConsumerStorageFreeV1,
{
}

impl<'a> ConsumerStorageFreeV1 for LocalRecordingPassthroughV1<'a>
where
    &'a dyn RecordingLeaseBackendV1: ConsumerStorageFreeV1,
    private::LocalPassthroughSealV1: ConsumerStorageFreeV1,
{
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use re_chunk::RangeQuery;
    use re_entity_db::{EntityDb, StoreBundle};
    use re_log_types::AbsoluteTimeRange;
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;
    use crate::web_remote_mcap_query::{
        ConsumerStorageFreeV1, PrivilegedRemoteStorageCapabilityV1, RemoteCanonicalIndexedExtentV1,
    };

    fn store_id() -> StoreId {
        StoreId::recording("consumer-test-app", "consumer-test-recording")
    }

    fn timeline() -> TimelineName {
        TimelineName::log_time()
    }

    fn extent() -> AbsoluteTimeRange {
        AbsoluteTimeRange::new(0i64, 10i64)
    }

    fn committed_time(cursor: i64) -> CommittedPresentationTimeV1 {
        CommittedPresentationTimeV1 {
            timeline: timeline(),
            cursor: TimeInt::new_temporal(cursor),
        }
    }

    fn facade(max_live_leases: usize) -> RemotePresentationFacadeV1 {
        RemotePresentationFacadeV1::new_for_test_v1(
            42,
            store_id(),
            RemoteRecordingUseStateV1::Foreground,
            RemoteLoadedCoverageV1::new_v1(RemoteCanonicalIndexedExtentV1::Known(extent()), false),
            max_live_leases,
            Rc::new(|| {}),
        )
    }

    fn open_facade(facade: &RemotePresentationFacadeV1) {
        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(facade);
        privileged.set_opening_static_satisfied_v1(true);
        privileged
            .commit_initial_presentation_v1(Some(committed_time(3)))
            .expect("open facade");
    }

    fn assert_consumer_trait_surface<T: RecordingConsumerQueryV1 + 'static>() {
        let type_name = std::any::type_name::<T>();
        assert!(!type_name.contains("EntityDb"));
        assert!(!type_name.contains("StorageEngine"));
        assert!(!type_name.contains("StoreHub"));
        assert!(!type_name.contains("StoreBundle"));
    }

    fn assert_storage_free_v1<T: ConsumerStorageFreeV1>() {}

    macro_rules! assert_not_storage_free_v1 {
        ($type:ty) => {
            const _: fn() = || {
                trait AmbiguousIfImpl<A> {
                    fn some_item() {}
                }

                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}

                struct Invalid;

                impl<T: ?Sized + ConsumerStorageFreeV1> AmbiguousIfImpl<Invalid> for T {}

                let _ = <$type as AmbiguousIfImpl<_>>::some_item;
            };
        };
    }

    #[test]
    fn remote_consumer_and_local_passthrough_share_the_same_facade_snapshot() {
        let facade = facade(2);
        open_facade(&facade);

        let remote = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let local = LocalRecordingPassthroughV1::from_facade_v1(&facade);

        let remote_lease = remote.try_lease_v1().expect("remote lease");
        let local_lease = local.try_lease_v1().expect("local lease");

        assert_eq!(remote_lease.snapshot_v1(), local_lease.snapshot_v1());
        assert_eq!(remote_lease.committed_time_v1(), Some(committed_time(3)));
        assert!(remote.is_current_v1(&remote_lease.snapshot_v1()));
        assert!(local.is_current_v1(&local_lease.snapshot_v1()));
    }

    #[test]
    fn consumer_trait_supports_complete_range_lease_with_revision_check() {
        let facade = facade(1);
        open_facade(&facade);

        let mut coverage =
            RemoteLoadedCoverageV1::new_v1(RemoteCanonicalIndexedExtentV1::Known(extent()), true);
        coverage.replace_loaded_ranges_v1([extent()]);

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged.replace_loaded_coverage_v1(coverage);

        let remote = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let lease = remote
            .try_complete_range_lease_v1(&RangeQuery::new(timeline(), extent()))
            .expect("complete range lease");

        assert_eq!(lease.committed_time_v1(), Some(committed_time(3)));
        assert!(remote.is_current_v1(&lease.snapshot_v1()));
    }

    #[test]
    fn gated_facade_states_reject_consumer_leases() {
        let facade = facade(1);
        let remote = RecordingConsumerContextV1::from_remote_facade_v1(&facade);

        assert!(matches!(
            remote.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::PresentationGated)
        ));
        assert!(matches!(
            remote.try_complete_range_lease_v1(&RangeQuery::new(timeline(), extent())),
            Err(RemoteRangeQueryUnavailableV1::PresentationGated)
        ));

        open_facade(&facade);

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::CatalogOnly)
            .expect("catalog only");
        assert!(matches!(
            remote.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::RecordingNotForeground)
        ));

        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::Inactive)
            .expect("inactive");
        assert!(matches!(
            remote.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::RecordingNotForeground)
        ));

        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::Foreground)
            .expect("foreground");
        let _lease = remote.try_lease_v1().expect("foreground lease");
        privileged
            .begin_query_visible_insertion_v1()
            .expect("close for mutation");
        assert!(matches!(
            remote.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::PresentationGated)
        ));

        drop(_lease);
        privileged
            .finish_query_visible_insertion_v1()
            .expect("finish insertion");
        privileged.terminal_gate_v1();
        assert!(matches!(
            remote.try_lease_v1(),
            Err(PresentationLeaseUnavailableV1::PresentationGated)
        ));
    }

    #[test]
    fn privileged_capability_returns_owned_outcomes_for_lease_drain() {
        let facade = facade(1);
        open_facade(&facade);

        let remote = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let _lease = remote.try_lease_v1().expect("lease");

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);

        assert_eq!(
            privileged.begin_query_visible_insertion_v1(),
            Ok(RemoteMutationLeaseDrainV1::WaitingForLiveLeaseDrain)
        );
        assert!(!privileged.take_lease_drain_ready_v1());

        drop(_lease);
        assert!(privileged.take_lease_drain_ready_v1());
        privileged
            .finish_query_visible_insertion_v1()
            .expect("finish insertion");
    }

    #[test]
    fn consumer_type_surface_does_not_mention_storage_handles() {
        assert_consumer_trait_surface::<RecordingConsumerContextV1<'static>>();
        assert_consumer_trait_surface::<LocalRecordingPassthroughV1<'static>>();
    }

    #[test]
    fn snapshot_and_lease_debug_do_not_leak_presentation_identity() {
        let facade = facade(1);
        open_facade(&facade);

        let remote = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let lease = remote.try_lease_v1().expect("lease");
        let snapshot = lease.snapshot_v1();
        let recording_id = store_id().recording_id().as_str().to_owned();

        for rendered in [format!("{snapshot:?}"), format!("{lease:?}")] {
            assert!(!rendered.contains("PresentationRevision"));
            assert!(!rendered.contains("PresentationQuerySnapshot"));
            assert!(!rendered.contains("StoreId"));
            assert!(!rendered.contains(&recording_id));
            assert!(!rendered.contains("42"));
        }
    }

    #[test]
    fn consumer_and_privileged_types_are_storage_free_by_structure() {
        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StoreBundle);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StorageEngine);

        assert_storage_free_v1::<RecordingConsumerContextV1<'static>>();
        assert_storage_free_v1::<LocalRecordingPassthroughV1<'static>>();
        assert_storage_free_v1::<RecordingQuerySnapshotV1>();
        assert_storage_free_v1::<RecordingQueryLeaseV1<'static>>();
        assert_storage_free_v1::<PrivilegedViewerFrameContextV1>();
        assert_storage_free_v1::<PrivilegedRemoteStorageCapabilityV1<'static>>();
    }
}
