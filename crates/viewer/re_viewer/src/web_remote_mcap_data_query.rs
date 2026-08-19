//! Production-disarmed remote View, Dataframe, and data-UI query boundary.
//!
//! This module is the MCAP-098 query boundary over the narrow
//! [`RecordingConsumerQueryV1`] capability. It deliberately holds no
//! `ViewerContext`, `AppContext`, `StoreHub`, `StoreBundle`, `EntityDb`, or
//! storage engine, and it does not add a privileged fallback path.

#![allow(dead_code)]

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use re_chunk::RangeQuery;
use re_log_types::AbsoluteTimeRange;

use crate::web_remote_mcap_consumer::{RecordingConsumerQueryV1, RecordingQuerySnapshotV1};
use crate::web_remote_mcap_query::{
    CommittedPresentationTimeV1, ConsumerStorageFreeV1, PresentationLeaseUnavailableV1,
    RemoteCanonicalIndexedExtentV1, RemoteRangeQueryUnavailableV1,
};

mod private {
    pub(crate) struct RemoteDataQueryAdapterSealV1;
    pub(crate) struct LocalDataQueryPassthroughSealV1;

    pub(crate) trait SealedRecordingDataQueryConsumerV1 {}
}

/// Storage-free consumer boundary for View/dataframe/data-UI query code.
pub(crate) trait RecordingDataQueryConsumerV1:
    private::SealedRecordingDataQueryConsumerV1 + ConsumerStorageFreeV1
{
    fn latest_at_v1(&self) -> RemoteLatestAtDataQueryResultV1;

    fn range_v1(&self, query: &RangeQuery) -> RemoteRangeDataQueryResultV1;

    fn publish_latest_at_v1(
        &self,
        result: &RemoteRevisionTaggedLatestAtV1,
    ) -> Option<RemoteLatestAtDataQueryV1>;

    fn publish_range_v1(
        &self,
        result: &RemoteRevisionTaggedRangeV1,
    ) -> Option<RemoteRangeDataQueryV1>;
}

/// Counts successful recording-query leases issued through this data-query boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteDataQueryInvocationCountsV1 {
    pub(crate) latest_at_query_invocations: usize,
    pub(crate) range_query_invocations: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RemoteDataQueryInvocationCounterV1 {
    latest_at_query_invocations: Rc<Cell<usize>>,
    range_query_invocations: Rc<Cell<usize>>,
}

impl RemoteDataQueryInvocationCounterV1 {
    pub(crate) fn counts_v1(&self) -> RemoteDataQueryInvocationCountsV1 {
        RemoteDataQueryInvocationCountsV1 {
            latest_at_query_invocations: self.latest_at_query_invocations.get(),
            range_query_invocations: self.range_query_invocations.get(),
        }
    }

    fn record_latest_at_query_invocation_v1(&self) {
        self.latest_at_query_invocations
            .set(self.latest_at_query_invocations.get() + 1);
    }

    fn record_range_query_invocation_v1(&self) {
        self.range_query_invocations
            .set(self.range_query_invocations.get() + 1);
    }
}

/// Latest-at data-query result derived only from a committed presentation lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteLatestAtDataQueryV1 {
    committed_time: Option<CommittedPresentationTimeV1>,
    snapshot: RecordingQuerySnapshotV1,
}

impl RemoteLatestAtDataQueryV1 {
    pub(crate) fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        self.committed_time.clone()
    }

    fn snapshot_v1(&self) -> RecordingQuerySnapshotV1 {
        self.snapshot.clone()
    }
}

/// Complete-range data-query result derived only from a committed presentation lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRangeDataQueryV1 {
    committed_time: Option<CommittedPresentationTimeV1>,
    snapshot: RecordingQuerySnapshotV1,
}

impl RemoteRangeDataQueryV1 {
    pub(crate) fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        self.committed_time.clone()
    }

    fn snapshot_v1(&self) -> RecordingQuerySnapshotV1 {
        self.snapshot.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteLatestAtDataUnavailableV1 {
    RecordingNotForeground,
    PresentationGated,
}

impl From<PresentationLeaseUnavailableV1> for RemoteLatestAtDataUnavailableV1 {
    fn from(value: PresentationLeaseUnavailableV1) -> Self {
        match value {
            PresentationLeaseUnavailableV1::RecordingNotForeground => Self::RecordingNotForeground,
            PresentationLeaseUnavailableV1::PresentationGated => Self::PresentationGated,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRangeDataUnavailableV1 {
    RecordingNotForeground,
    PresentationGated,
    IndexedCoverageIncomplete {
        indexed_extent: RemoteCanonicalIndexedExtentV1,
        loaded_ranges: Vec<AbsoluteTimeRange>,
    },
    NoIndexedMessages {
        loaded_ranges: Vec<AbsoluteTimeRange>,
    },
}

impl RemoteRangeDataUnavailableV1 {
    pub(crate) const fn is_remote_range_not_fully_loaded_v1(&self) -> bool {
        matches!(self, Self::IndexedCoverageIncomplete { .. })
    }

    pub(crate) const fn message_v1(&self) -> &'static str {
        match self {
            Self::IndexedCoverageIncomplete { .. } => "Remote range data not fully loaded",
            Self::NoIndexedMessages { .. } => "Remote range query has no indexed messages",
            Self::RecordingNotForeground => "Remote recording is not foreground",
            Self::PresentationGated => "Remote presentation is gated",
        }
    }
}

impl From<RemoteRangeQueryUnavailableV1> for RemoteRangeDataUnavailableV1 {
    fn from(value: RemoteRangeQueryUnavailableV1) -> Self {
        match value {
            RemoteRangeQueryUnavailableV1::RecordingNotForeground => Self::RecordingNotForeground,
            RemoteRangeQueryUnavailableV1::PresentationGated => Self::PresentationGated,
            RemoteRangeQueryUnavailableV1::IndexedCoverageIncomplete {
                indexed_extent,
                loaded_ranges,
            } => match indexed_extent {
                RemoteCanonicalIndexedExtentV1::NoIndexedMessages => {
                    Self::NoIndexedMessages { loaded_ranges }
                }
                indexed_extent @ RemoteCanonicalIndexedExtentV1::Known(_) => {
                    Self::IndexedCoverageIncomplete {
                        indexed_extent,
                        loaded_ranges,
                    }
                }
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteLatestAtDataQueryResultV1 {
    Unavailable(RemoteLatestAtDataUnavailableV1),
    Available(RemoteLatestAtDataQueryV1),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRangeDataQueryResultV1 {
    Unavailable(RemoteRangeDataUnavailableV1),
    Available(RemoteRangeDataQueryV1),
}

/// Delayed latest-at result carrying the complete opaque lease snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRevisionTaggedLatestAtV1 {
    snapshot: RecordingQuerySnapshotV1,
    value: RemoteLatestAtDataQueryV1,
}

impl RemoteRevisionTaggedLatestAtV1 {
    pub(crate) fn from_result_v1(value: RemoteLatestAtDataQueryV1) -> Self {
        Self {
            snapshot: value.snapshot_v1(),
            value,
        }
    }
}

/// Delayed range result carrying the complete opaque lease snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRevisionTaggedRangeV1 {
    snapshot: RecordingQuerySnapshotV1,
    value: RemoteRangeDataQueryV1,
}

impl RemoteRevisionTaggedRangeV1 {
    pub(crate) fn from_result_v1(value: RemoteRangeDataQueryV1) -> Self {
        Self {
            snapshot: value.snapshot_v1(),
            value,
        }
    }
}

fn latest_at_from_query_v1(
    query: &dyn RecordingConsumerQueryV1,
    counters: &RemoteDataQueryInvocationCounterV1,
) -> RemoteLatestAtDataQueryResultV1 {
    match query.try_lease_v1() {
        Ok(lease) => {
            counters.record_latest_at_query_invocation_v1();
            let committed_time = lease.committed_time_v1();
            let snapshot = lease.snapshot_v1();
            RemoteLatestAtDataQueryResultV1::Available(RemoteLatestAtDataQueryV1 {
                committed_time,
                snapshot,
            })
        }
        Err(error) => RemoteLatestAtDataQueryResultV1::Unavailable(error.into()),
    }
}

fn range_from_query_v1(
    query: &dyn RecordingConsumerQueryV1,
    counters: &RemoteDataQueryInvocationCounterV1,
    range_query: &RangeQuery,
) -> RemoteRangeDataQueryResultV1 {
    match query.try_complete_range_lease_v1(range_query) {
        Ok(lease) => {
            counters.record_range_query_invocation_v1();
            let committed_time = lease.committed_time_v1();
            let snapshot = lease.snapshot_v1();
            RemoteRangeDataQueryResultV1::Available(RemoteRangeDataQueryV1 {
                committed_time,
                snapshot,
            })
        }
        Err(error) => RemoteRangeDataQueryResultV1::Unavailable(error.into()),
    }
}

/// Remote data-query adapter backed only by the sealed recording-query facade.
pub(crate) struct RemoteRecordingDataQueryAdapterV1<'a> {
    query: &'a dyn RecordingConsumerQueryV1,
    counters: RemoteDataQueryInvocationCounterV1,
    _sealed: private::RemoteDataQueryAdapterSealV1,
}

impl<'a> RemoteRecordingDataQueryAdapterV1<'a> {
    pub(crate) fn new_v1(query: &'a dyn RecordingConsumerQueryV1) -> Self {
        Self::with_counter_v1(query, RemoteDataQueryInvocationCounterV1::default())
    }

    pub(crate) fn with_counter_v1(
        query: &'a dyn RecordingConsumerQueryV1,
        counters: RemoteDataQueryInvocationCounterV1,
    ) -> Self {
        Self {
            query,
            counters,
            _sealed: private::RemoteDataQueryAdapterSealV1,
        }
    }

    pub(crate) fn invocation_counts_v1(&self) -> RemoteDataQueryInvocationCountsV1 {
        self.counters.counts_v1()
    }
}

impl private::SealedRecordingDataQueryConsumerV1 for RemoteRecordingDataQueryAdapterV1<'_> {}

impl RecordingDataQueryConsumerV1 for RemoteRecordingDataQueryAdapterV1<'_> {
    fn latest_at_v1(&self) -> RemoteLatestAtDataQueryResultV1 {
        latest_at_from_query_v1(self.query, &self.counters)
    }

    fn range_v1(&self, query: &RangeQuery) -> RemoteRangeDataQueryResultV1 {
        range_from_query_v1(self.query, &self.counters, query)
    }

    fn publish_latest_at_v1(
        &self,
        result: &RemoteRevisionTaggedLatestAtV1,
    ) -> Option<RemoteLatestAtDataQueryV1> {
        self.query
            .is_current_v1(&result.snapshot)
            .then(|| result.value.clone())
    }

    fn publish_range_v1(
        &self,
        result: &RemoteRevisionTaggedRangeV1,
    ) -> Option<RemoteRangeDataQueryV1> {
        self.query
            .is_current_v1(&result.snapshot)
            .then(|| result.value.clone())
    }
}

/// Native/local passthrough over the same sealed recording-query surface.
pub(crate) struct LocalRecordingDataQueryPassthroughV1<'a> {
    query: &'a dyn RecordingConsumerQueryV1,
    counters: RemoteDataQueryInvocationCounterV1,
    _sealed: private::LocalDataQueryPassthroughSealV1,
}

impl<'a> LocalRecordingDataQueryPassthroughV1<'a> {
    pub(crate) fn new_v1(query: &'a dyn RecordingConsumerQueryV1) -> Self {
        Self::with_counter_v1(query, RemoteDataQueryInvocationCounterV1::default())
    }

    pub(crate) fn with_counter_v1(
        query: &'a dyn RecordingConsumerQueryV1,
        counters: RemoteDataQueryInvocationCounterV1,
    ) -> Self {
        Self {
            query,
            counters,
            _sealed: private::LocalDataQueryPassthroughSealV1,
        }
    }

    pub(crate) fn invocation_counts_v1(&self) -> RemoteDataQueryInvocationCountsV1 {
        self.counters.counts_v1()
    }
}

impl private::SealedRecordingDataQueryConsumerV1 for LocalRecordingDataQueryPassthroughV1<'_> {}

impl RecordingDataQueryConsumerV1 for LocalRecordingDataQueryPassthroughV1<'_> {
    fn latest_at_v1(&self) -> RemoteLatestAtDataQueryResultV1 {
        latest_at_from_query_v1(self.query, &self.counters)
    }

    fn range_v1(&self, query: &RangeQuery) -> RemoteRangeDataQueryResultV1 {
        range_from_query_v1(self.query, &self.counters, query)
    }

    fn publish_latest_at_v1(
        &self,
        result: &RemoteRevisionTaggedLatestAtV1,
    ) -> Option<RemoteLatestAtDataQueryV1> {
        self.query
            .is_current_v1(&result.snapshot)
            .then(|| result.value.clone())
    }

    fn publish_range_v1(
        &self,
        result: &RemoteRevisionTaggedRangeV1,
    ) -> Option<RemoteRangeDataQueryV1> {
        self.query
            .is_current_v1(&result.snapshot)
            .then(|| result.value.clone())
    }
}

impl fmt::Debug for RemoteRecordingDataQueryAdapterV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteRecordingDataQueryAdapterV1")
            .field("invocation_counts", &self.invocation_counts_v1())
            .finish()
    }
}

impl fmt::Debug for LocalRecordingDataQueryPassthroughV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalRecordingDataQueryPassthroughV1")
            .field("invocation_counts", &self.invocation_counts_v1())
            .finish()
    }
}

impl ConsumerStorageFreeV1 for private::RemoteDataQueryAdapterSealV1 {}
impl ConsumerStorageFreeV1 for private::LocalDataQueryPassthroughSealV1 {}

impl ConsumerStorageFreeV1 for RemoteDataQueryInvocationCountsV1 where usize: ConsumerStorageFreeV1 {}
impl ConsumerStorageFreeV1 for RemoteDataQueryInvocationCounterV1 {}

impl ConsumerStorageFreeV1 for RemoteLatestAtDataQueryV1
where
    Option<CommittedPresentationTimeV1>: ConsumerStorageFreeV1,
    RecordingQuerySnapshotV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteRangeDataQueryV1
where
    Option<CommittedPresentationTimeV1>: ConsumerStorageFreeV1,
    RecordingQuerySnapshotV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteLatestAtDataUnavailableV1 {}
impl ConsumerStorageFreeV1 for RemoteRangeDataUnavailableV1
where
    RemoteCanonicalIndexedExtentV1: ConsumerStorageFreeV1,
    Vec<AbsoluteTimeRange>: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteLatestAtDataQueryResultV1
where
    RemoteLatestAtDataUnavailableV1: ConsumerStorageFreeV1,
    RemoteLatestAtDataQueryV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteRangeDataQueryResultV1
where
    RemoteRangeDataUnavailableV1: ConsumerStorageFreeV1,
    RemoteRangeDataQueryV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteRevisionTaggedLatestAtV1
where
    RecordingQuerySnapshotV1: ConsumerStorageFreeV1,
    RemoteLatestAtDataQueryV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteRevisionTaggedRangeV1
where
    RecordingQuerySnapshotV1: ConsumerStorageFreeV1,
    RemoteRangeDataQueryV1: ConsumerStorageFreeV1,
{
}

impl<'a> ConsumerStorageFreeV1 for RemoteRecordingDataQueryAdapterV1<'a>
where
    &'a dyn RecordingConsumerQueryV1: ConsumerStorageFreeV1,
    RemoteDataQueryInvocationCounterV1: ConsumerStorageFreeV1,
    private::RemoteDataQueryAdapterSealV1: ConsumerStorageFreeV1,
{
}

impl<'a> ConsumerStorageFreeV1 for LocalRecordingDataQueryPassthroughV1<'a>
where
    &'a dyn RecordingConsumerQueryV1: ConsumerStorageFreeV1,
    RemoteDataQueryInvocationCounterV1: ConsumerStorageFreeV1,
    private::LocalDataQueryPassthroughSealV1: ConsumerStorageFreeV1,
{
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use re_entity_db::{EntityDb, StoreBundle};
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;
    use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
    use crate::web_remote_mcap_consumer::RecordingConsumerContextV1;
    use crate::web_remote_mcap_query::{
        PrivilegedViewerFrameContextV1, RemoteLoadedCoverageV1, RemotePresentationFacadeV1,
    };

    fn store_id() -> re_log_types::StoreId {
        re_log_types::StoreId::recording("data-query-test-app", "data-query-test-recording")
    }

    fn timeline() -> re_chunk::TimelineName {
        re_chunk::TimelineName::log_time()
    }

    fn extent() -> AbsoluteTimeRange {
        AbsoluteTimeRange::new(0i64, 10i64)
    }

    fn committed_time(cursor: i64) -> CommittedPresentationTimeV1 {
        CommittedPresentationTimeV1 {
            timeline: timeline(),
            cursor: re_chunk::TimeInt::new_temporal(cursor),
        }
    }

    fn make_facade(extent: RemoteCanonicalIndexedExtentV1) -> RemotePresentationFacadeV1 {
        RemotePresentationFacadeV1::new_for_test_v1(
            44,
            store_id(),
            RemoteRecordingUseStateV1::Foreground,
            RemoteLoadedCoverageV1::new_v1(extent, false),
            2,
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

    fn make_complete_coverage(extent: AbsoluteTimeRange) -> RemoteLoadedCoverageV1 {
        let mut coverage =
            RemoteLoadedCoverageV1::new_v1(RemoteCanonicalIndexedExtentV1::Known(extent), true);
        coverage.replace_loaded_ranges_v1([extent]);
        coverage
    }

    fn replace_coverage(facade: &RemotePresentationFacadeV1, coverage: RemoteLoadedCoverageV1) {
        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(facade);
        privileged.replace_loaded_coverage_v1(coverage);
    }

    fn assert_trait_surface<T: RecordingDataQueryConsumerV1 + 'static>() {
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
    fn latest_at_uses_committed_snapshot_and_counts_one_query_invocation() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingDataQueryAdapterV1::new_v1(&consumer);

        let result = adapter.latest_at_v1();
        assert_eq!(
            result,
            RemoteLatestAtDataQueryResultV1::Available(RemoteLatestAtDataQueryV1 {
                committed_time: Some(committed_time(3)),
                snapshot: consumer
                    .try_lease_v1()
                    .expect("verification lease")
                    .snapshot_v1(),
            })
        );
        assert_eq!(
            adapter.invocation_counts_v1(),
            RemoteDataQueryInvocationCountsV1 {
                latest_at_query_invocations: 1,
                range_query_invocations: 0,
            }
        );
    }

    #[test]
    fn incomplete_everything_range_does_not_query_resident_subset() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingDataQueryAdapterV1::new_v1(&consumer);

        let result = adapter.range_v1(&RangeQuery::everything(timeline()));
        match result {
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::IndexedCoverageIncomplete {
                    indexed_extent,
                    loaded_ranges,
                },
            ) => {
                assert_eq!(
                    indexed_extent,
                    RemoteCanonicalIndexedExtentV1::Known(extent())
                );
                assert!(loaded_ranges.is_empty());
                assert!(matches!(
                    adapter.range_v1(&RangeQuery::new(timeline(), extent())),
                    RemoteRangeDataQueryResultV1::Unavailable(
                        RemoteRangeDataUnavailableV1::IndexedCoverageIncomplete { .. }
                    )
                ));
            }
            other => panic!("expected incomplete range result, got {other:?}"),
        }

        assert_eq!(
            adapter.invocation_counts_v1(),
            RemoteDataQueryInvocationCountsV1 {
                latest_at_query_invocations: 0,
                range_query_invocations: 0,
            }
        );
        assert_eq!(facade.live_lease_count_v1(), 0);
    }

    #[test]
    fn no_indexed_messages_range_is_not_temporal_success() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::NoIndexedMessages);
        open_facade(&facade);
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingDataQueryAdapterV1::new_v1(&consumer);

        assert_eq!(
            adapter.range_v1(&RangeQuery::everything(timeline())),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::NoIndexedMessages {
                    loaded_ranges: Vec::new(),
                }
            )
        );
        assert_eq!(
            adapter.invocation_counts_v1(),
            RemoteDataQueryInvocationCountsV1 {
                latest_at_query_invocations: 0,
                range_query_invocations: 0,
            }
        );
    }

    #[test]
    fn gated_and_non_foreground_states_have_zero_query_invocations() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingDataQueryAdapterV1::new_v1(&consumer);

        assert_eq!(
            adapter.latest_at_v1(),
            RemoteLatestAtDataQueryResultV1::Unavailable(
                RemoteLatestAtDataUnavailableV1::PresentationGated
            )
        );
        assert_eq!(
            adapter.range_v1(&RangeQuery::everything(timeline())),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::PresentationGated
            )
        );

        open_facade(&facade);
        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);

        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::CatalogOnly)
            .expect("catalog only");
        assert_eq!(
            adapter.latest_at_v1(),
            RemoteLatestAtDataQueryResultV1::Unavailable(
                RemoteLatestAtDataUnavailableV1::RecordingNotForeground
            )
        );
        assert_eq!(
            adapter.range_v1(&RangeQuery::everything(timeline())),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::RecordingNotForeground
            )
        );

        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::Inactive)
            .expect("inactive");
        assert!(matches!(
            adapter.latest_at_v1(),
            RemoteLatestAtDataQueryResultV1::Unavailable(
                RemoteLatestAtDataUnavailableV1::RecordingNotForeground
            )
        ));
        assert!(matches!(
            adapter.range_v1(&RangeQuery::everything(timeline())),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::RecordingNotForeground
            )
        ));

        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::Foreground)
            .expect("foreground");
        privileged
            .begin_query_visible_insertion_v1()
            .expect("close for mutation");
        assert!(matches!(
            adapter.latest_at_v1(),
            RemoteLatestAtDataQueryResultV1::Unavailable(
                RemoteLatestAtDataUnavailableV1::PresentationGated
            )
        ));
        assert!(matches!(
            adapter.range_v1(&RangeQuery::everything(timeline())),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::PresentationGated
            )
        ));
        privileged
            .finish_query_visible_insertion_v1()
            .expect("finish mutation");
        privileged.terminal_gate_v1();
        assert!(matches!(
            adapter.latest_at_v1(),
            RemoteLatestAtDataQueryResultV1::Unavailable(
                RemoteLatestAtDataUnavailableV1::PresentationGated
            )
        ));
        assert!(matches!(
            adapter.range_v1(&RangeQuery::everything(timeline())),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::PresentationGated
            )
        ));

        assert_eq!(
            adapter.invocation_counts_v1(),
            RemoteDataQueryInvocationCountsV1 {
                latest_at_query_invocations: 0,
                range_query_invocations: 0,
            }
        );
    }

    #[test]
    fn delayed_results_are_published_only_against_current_snapshot() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingDataQueryAdapterV1::new_v1(&consumer);

        let latest_at = match adapter.latest_at_v1() {
            RemoteLatestAtDataQueryResultV1::Available(value) => value,
            other @ RemoteLatestAtDataQueryResultV1::Unavailable(_) => {
                panic!("expected available latest-at result, got {other:?}")
            }
        };
        let tagged = RemoteRevisionTaggedLatestAtV1::from_result_v1(latest_at.clone());
        assert_eq!(adapter.publish_latest_at_v1(&tagged), Some(latest_at));

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::CatalogOnly)
            .expect("catalog only");
        assert_eq!(adapter.publish_latest_at_v1(&tagged), None);
    }

    #[test]
    fn remote_and_local_data_query_adapters_classify_identically() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        replace_coverage(&facade, make_complete_coverage(extent()));

        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let remote = RemoteRecordingDataQueryAdapterV1::new_v1(&consumer);
        let local = LocalRecordingDataQueryPassthroughV1::new_v1(&consumer);

        assert_eq!(remote.latest_at_v1(), local.latest_at_v1());
        assert_eq!(
            remote.range_v1(&RangeQuery::everything(timeline())),
            local.range_v1(&RangeQuery::everything(timeline()))
        );

        let no_indexed_facade = make_facade(RemoteCanonicalIndexedExtentV1::NoIndexedMessages);
        open_facade(&no_indexed_facade);
        let no_indexed_consumer =
            RecordingConsumerContextV1::from_remote_facade_v1(&no_indexed_facade);
        let remote = RemoteRecordingDataQueryAdapterV1::new_v1(&no_indexed_consumer);
        let local = LocalRecordingDataQueryPassthroughV1::new_v1(&no_indexed_consumer);
        assert_eq!(
            remote.range_v1(&RangeQuery::everything(timeline())),
            local.range_v1(&RangeQuery::everything(timeline()))
        );
    }

    #[test]
    fn data_query_types_do_not_contain_storage_handles() {
        assert_trait_surface::<RemoteRecordingDataQueryAdapterV1<'static>>();
        assert_trait_surface::<LocalRecordingDataQueryPassthroughV1<'static>>();

        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StoreBundle);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StorageEngine);

        assert_storage_free_v1::<RemoteRecordingDataQueryAdapterV1<'static>>();
        assert_storage_free_v1::<LocalRecordingDataQueryPassthroughV1<'static>>();
        assert_storage_free_v1::<RemoteLatestAtDataQueryV1>();
        assert_storage_free_v1::<RemoteRangeDataQueryV1>();
        assert_storage_free_v1::<RemoteRevisionTaggedLatestAtV1>();
        assert_storage_free_v1::<RemoteRevisionTaggedRangeV1>();
    }
}
