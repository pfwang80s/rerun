//! Production-disarmed remote memoizer, transform/view, video-range, and external query boundary.
//!
//! This module is the MCAP-099 revision-checking boundary over the narrow
//! [`RecordingConsumerQueryV1`] capability. It models the cache and publication checks without
//! importing or publishing a `ViewerContext`, `AppContext`, `StoreHub`, `StoreBundle`, `EntityDb`,
//! or storage engine. Real memoizer, video, and transform cache wiring remains a later work item.

#![allow(dead_code)]

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use re_chunk::RangeQuery;

use crate::web_remote_mcap_consumer::{RecordingConsumerQueryV1, RecordingQuerySnapshotV1};
use crate::web_remote_mcap_data_query::{
    RecordingDataQueryConsumerV1 as _, RemoteLatestAtDataQueryResultV1, RemoteLatestAtDataQueryV1,
    RemoteLatestAtDataUnavailableV1, RemoteRangeDataQueryResultV1, RemoteRangeDataQueryV1,
    RemoteRangeDataUnavailableV1, RemoteRecordingDataQueryAdapterV1,
    RemoteRevisionTaggedLatestAtV1, RemoteRevisionTaggedRangeV1,
};
use crate::web_remote_mcap_query::ConsumerStorageFreeV1;

mod private {
    pub(crate) struct RemoteCacheAdapterSealV1;
    pub(crate) struct LocalCachePassthroughSealV1;

    pub(crate) trait SealedRecordingCacheConsumerV1 {}
}

/// Storage-free cache domain covered by the MCAP-099 boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCacheDomainV1 {
    Memoizer,
    TransformView,
    ExternalWeb,
    VideoRange,
}

/// Cache query discriminator. Range queries retain their complete query in the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCacheQueryKindV1 {
    LatestAt,
    Range(RangeQuery),
}

/// Opaque bounded cache key.
///
/// The key always contains the complete [`RecordingQuerySnapshotV1`] presentation revision. A
/// bare epoch, `StoreId`, facade instance, or mutable navigation cursor cannot be used to form it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCacheKeyV1 {
    domain: RemoteCacheDomainV1,
    query: RemoteCacheQueryKindV1,
    snapshot: RecordingQuerySnapshotV1,
}

impl RemoteCacheKeyV1 {
    pub(crate) fn latest_at_v1(
        domain: RemoteCacheDomainV1,
        snapshot: RecordingQuerySnapshotV1,
    ) -> Self {
        Self {
            domain,
            query: RemoteCacheQueryKindV1::LatestAt,
            snapshot,
        }
    }

    pub(crate) fn range_v1(
        domain: RemoteCacheDomainV1,
        query: RangeQuery,
        snapshot: RecordingQuerySnapshotV1,
    ) -> Self {
        Self {
            domain,
            query: RemoteCacheQueryKindV1::Range(query),
            snapshot,
        }
    }

    pub(crate) fn domain_v1(&self) -> RemoteCacheDomainV1 {
        self.domain
    }

    pub(crate) fn query_v1(&self) -> &RemoteCacheQueryKindV1 {
        &self.query
    }

    pub(crate) fn snapshot_v1(&self) -> &RecordingQuerySnapshotV1 {
        &self.snapshot
    }
}

/// Latest-at cache lookup outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteLatestAtCacheLookupV1 {
    Hit(RemoteRevisionTaggedLatestAtV1),
    Miss,
    NotCurrent,
}

/// Range cache lookup outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRangeCacheLookupV1 {
    Hit(RemoteRevisionTaggedRangeV1),
    Miss,
    NotCurrent,
}

/// Cache insertion outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCacheInsertionV1 {
    Inserted { evicted: usize },
    Updated,
    NotCurrent,
}

/// Execution and publication trace for a bounded cache adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteCacheInvocationCountsV1 {
    pub(crate) latest_at_cache_queries: usize,
    pub(crate) range_cache_queries: usize,
    pub(crate) cache_lookups: usize,
    pub(crate) cache_hits: usize,
    pub(crate) cache_misses: usize,
    pub(crate) not_current_lookups: usize,
    pub(crate) cache_inserts: usize,
    pub(crate) cache_evictions: usize,
    pub(crate) not_current_inserts: usize,
    pub(crate) cache_publications: usize,
    pub(crate) rejected_publications: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RemoteCacheEntryV1 {
    LatestAt {
        key: RemoteCacheKeyV1,
        tagged: RemoteRevisionTaggedLatestAtV1,
    },
    Range {
        key: RemoteCacheKeyV1,
        tagged: RemoteRevisionTaggedRangeV1,
    },
}

impl RemoteCacheEntryV1 {
    fn key_v1(&self) -> &RemoteCacheKeyV1 {
        match self {
            Self::LatestAt { key, .. } | Self::Range { key, .. } => key,
        }
    }
}

#[derive(Debug)]
struct RemoteCacheAdapterStateV1 {
    max_entries: usize,
    entries: RefCell<Vec<RemoteCacheEntryV1>>,
    counts: RefCell<RemoteCacheInvocationCountsV1>,
}

impl RemoteCacheAdapterStateV1 {
    fn new_v1(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            entries: RefCell::new(Vec::new()),
            counts: RefCell::new(RemoteCacheInvocationCountsV1::default()),
        }
    }

    fn counts_v1(&self) -> RemoteCacheInvocationCountsV1 {
        *self.counts.borrow()
    }

    fn record_latest_at_cache_query_v1(&self) {
        self.counts.borrow_mut().latest_at_cache_queries += 1;
    }

    fn record_range_cache_query_v1(&self) {
        self.counts.borrow_mut().range_cache_queries += 1;
    }

    fn record_cache_lookup_v1(&self) {
        self.counts.borrow_mut().cache_lookups += 1;
    }

    fn record_cache_hit_v1(&self) {
        self.counts.borrow_mut().cache_hits += 1;
    }

    fn record_cache_miss_v1(&self) {
        self.counts.borrow_mut().cache_misses += 1;
    }

    fn record_not_current_lookup_v1(&self) {
        self.counts.borrow_mut().not_current_lookups += 1;
    }

    fn record_cache_insert_v1(&self) {
        self.counts.borrow_mut().cache_inserts += 1;
    }

    fn record_cache_evictions_v1(&self, evicted: usize) {
        self.counts.borrow_mut().cache_evictions += evicted;
    }

    fn record_not_current_insert_v1(&self) {
        self.counts.borrow_mut().not_current_inserts += 1;
    }

    fn record_cache_publication_v1(&self) {
        self.counts.borrow_mut().cache_publications += 1;
    }

    fn record_rejected_publication_v1(&self) {
        self.counts.borrow_mut().rejected_publications += 1;
    }
}

fn is_current_for_key_v1(query: &dyn RecordingConsumerQueryV1, key: &RemoteCacheKeyV1) -> bool {
    query.is_current_v1(key.snapshot_v1())
}

fn lookup_latest_at_v1_inner(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    key: &RemoteCacheKeyV1,
) -> RemoteLatestAtCacheLookupV1 {
    state.record_cache_lookup_v1();
    if !is_current_for_key_v1(query, key) {
        state.record_not_current_lookup_v1();
        return RemoteLatestAtCacheLookupV1::NotCurrent;
    }

    let tagged = state
        .entries
        .borrow()
        .iter()
        .rev()
        .find_map(|entry| match entry {
            RemoteCacheEntryV1::LatestAt {
                key: entry_key,
                tagged,
            } if entry_key == key => Some(tagged.clone()),
            _ => None,
        });

    if let Some(tagged) = tagged {
        state.record_cache_hit_v1();
        RemoteLatestAtCacheLookupV1::Hit(tagged)
    } else {
        state.record_cache_miss_v1();
        RemoteLatestAtCacheLookupV1::Miss
    }
}

fn lookup_range_v1_inner(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    key: &RemoteCacheKeyV1,
) -> RemoteRangeCacheLookupV1 {
    state.record_cache_lookup_v1();
    if !is_current_for_key_v1(query, key) {
        state.record_not_current_lookup_v1();
        return RemoteRangeCacheLookupV1::NotCurrent;
    }

    let tagged = state
        .entries
        .borrow()
        .iter()
        .rev()
        .find_map(|entry| match entry {
            RemoteCacheEntryV1::Range {
                key: entry_key,
                tagged,
            } if entry_key == key => Some(tagged.clone()),
            _ => None,
        });

    if let Some(tagged) = tagged {
        state.record_cache_hit_v1();
        RemoteRangeCacheLookupV1::Hit(tagged)
    } else {
        state.record_cache_miss_v1();
        RemoteRangeCacheLookupV1::Miss
    }
}

fn replace_or_push_latest_at_v1(
    state: &RemoteCacheAdapterStateV1,
    key: RemoteCacheKeyV1,
    tagged: RemoteRevisionTaggedLatestAtV1,
) -> Option<usize> {
    let mut entries = state.entries.borrow_mut();
    if let Some(index) = entries.iter().position(|entry| {
        entry.key_v1() == &key && matches!(entry, RemoteCacheEntryV1::LatestAt { .. })
    }) {
        entries[index] = RemoteCacheEntryV1::LatestAt { key, tagged };
        return None;
    }

    let evicted = usize::from(entries.len() >= state.max_entries);
    if evicted > 0 {
        entries.remove(0);
    }
    entries.push(RemoteCacheEntryV1::LatestAt { key, tagged });
    Some(evicted)
}

fn replace_or_push_range_v1(
    state: &RemoteCacheAdapterStateV1,
    key: RemoteCacheKeyV1,
    tagged: RemoteRevisionTaggedRangeV1,
) -> Option<usize> {
    let mut entries = state.entries.borrow_mut();
    if let Some(index) = entries.iter().position(|entry| {
        entry.key_v1() == &key && matches!(entry, RemoteCacheEntryV1::Range { .. })
    }) {
        entries[index] = RemoteCacheEntryV1::Range { key, tagged };
        return None;
    }

    let evicted = usize::from(entries.len() >= state.max_entries);
    if evicted > 0 {
        entries.remove(0);
    }
    entries.push(RemoteCacheEntryV1::Range { key, tagged });
    Some(evicted)
}

fn insert_latest_at_v1_inner(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    key: &RemoteCacheKeyV1,
    tagged: &RemoteRevisionTaggedLatestAtV1,
) -> RemoteCacheInsertionV1 {
    if !is_current_for_key_v1(query, key) {
        state.record_not_current_insert_v1();
        return RemoteCacheInsertionV1::NotCurrent;
    }

    state.record_cache_insert_v1();
    match replace_or_push_latest_at_v1(state, key.clone(), tagged.clone()) {
        Some(evicted) => {
            state.record_cache_evictions_v1(evicted);
            RemoteCacheInsertionV1::Inserted { evicted }
        }
        None => RemoteCacheInsertionV1::Updated,
    }
}

fn insert_range_v1_inner(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    key: &RemoteCacheKeyV1,
    tagged: &RemoteRevisionTaggedRangeV1,
) -> RemoteCacheInsertionV1 {
    if !is_current_for_key_v1(query, key) {
        state.record_not_current_insert_v1();
        return RemoteCacheInsertionV1::NotCurrent;
    }

    state.record_cache_insert_v1();
    match replace_or_push_range_v1(state, key.clone(), tagged.clone()) {
        Some(evicted) => {
            state.record_cache_evictions_v1(evicted);
            RemoteCacheInsertionV1::Inserted { evicted }
        }
        None => RemoteCacheInsertionV1::Updated,
    }
}

fn publish_latest_at_v1_inner(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    key: &RemoteCacheKeyV1,
    tagged: &RemoteRevisionTaggedLatestAtV1,
) -> Option<RemoteLatestAtDataQueryV1> {
    if !is_current_for_key_v1(query, key) {
        state.record_rejected_publication_v1();
        return None;
    }

    let data_query = RemoteRecordingDataQueryAdapterV1::new_v1(query);
    if let Some(value) = data_query.publish_latest_at_v1(tagged) {
        state.record_cache_publication_v1();
        Some(value)
    } else {
        state.record_rejected_publication_v1();
        None
    }
}

fn publish_range_v1_inner(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    key: &RemoteCacheKeyV1,
    tagged: &RemoteRevisionTaggedRangeV1,
) -> Option<RemoteRangeDataQueryV1> {
    if !is_current_for_key_v1(query, key) {
        state.record_rejected_publication_v1();
        return None;
    }

    let data_query = RemoteRecordingDataQueryAdapterV1::new_v1(query);
    if let Some(value) = data_query.publish_range_v1(tagged) {
        state.record_cache_publication_v1();
        Some(value)
    } else {
        state.record_rejected_publication_v1();
        None
    }
}

fn latest_at_cache_query_v1(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    domain: RemoteCacheDomainV1,
) -> RemoteLatestAtDataQueryResultV1 {
    state.record_latest_at_cache_query_v1();

    let key = {
        let lease = match query.try_lease_v1() {
            Ok(lease) => lease,
            Err(error) => {
                return RemoteLatestAtDataQueryResultV1::Unavailable(
                    RemoteLatestAtDataUnavailableV1::from(error),
                );
            }
        };
        RemoteCacheKeyV1::latest_at_v1(domain, lease.snapshot_v1())
    };

    if let RemoteLatestAtCacheLookupV1::Hit(tagged) = lookup_latest_at_v1_inner(query, state, &key)
        && let Some(value) = publish_latest_at_v1_inner(query, state, &key, &tagged)
    {
        return RemoteLatestAtDataQueryResultV1::Available(value);
    }

    let data_query = RemoteRecordingDataQueryAdapterV1::new_v1(query);
    match data_query.latest_at_v1() {
        RemoteLatestAtDataQueryResultV1::Available(value) => {
            let tagged = RemoteRevisionTaggedLatestAtV1::from_result_v1(value);
            let _ = insert_latest_at_v1_inner(query, state, &key, &tagged);
            match publish_latest_at_v1_inner(query, state, &key, &tagged) {
                Some(value) => RemoteLatestAtDataQueryResultV1::Available(value),
                None => RemoteLatestAtDataQueryResultV1::Unavailable(
                    RemoteLatestAtDataUnavailableV1::PresentationGated,
                ),
            }
        }
        result @ RemoteLatestAtDataQueryResultV1::Unavailable(_) => result,
    }
}

fn range_cache_query_v1(
    query: &dyn RecordingConsumerQueryV1,
    state: &RemoteCacheAdapterStateV1,
    domain: RemoteCacheDomainV1,
    range_query: &RangeQuery,
) -> RemoteRangeDataQueryResultV1 {
    state.record_range_cache_query_v1();

    let key = {
        let lease = match query.try_complete_range_lease_v1(range_query) {
            Ok(lease) => lease,
            Err(error) => {
                return RemoteRangeDataQueryResultV1::Unavailable(
                    RemoteRangeDataUnavailableV1::from(error),
                );
            }
        };
        RemoteCacheKeyV1::range_v1(domain, range_query.clone(), lease.snapshot_v1())
    };

    if let RemoteRangeCacheLookupV1::Hit(tagged) = lookup_range_v1_inner(query, state, &key)
        && let Some(value) = publish_range_v1_inner(query, state, &key, &tagged)
    {
        return RemoteRangeDataQueryResultV1::Available(value);
    }

    let data_query = RemoteRecordingDataQueryAdapterV1::new_v1(query);
    match data_query.range_v1(range_query) {
        RemoteRangeDataQueryResultV1::Available(value) => {
            let tagged = RemoteRevisionTaggedRangeV1::from_result_v1(value);
            let _ = insert_range_v1_inner(query, state, &key, &tagged);
            match publish_range_v1_inner(query, state, &key, &tagged) {
                Some(value) => RemoteRangeDataQueryResultV1::Available(value),
                None => RemoteRangeDataQueryResultV1::Unavailable(
                    RemoteRangeDataUnavailableV1::PresentationGated,
                ),
            }
        }
        result @ RemoteRangeDataQueryResultV1::Unavailable(_) => result,
    }
}

/// Sealed storage-free consumer boundary for memoizer, transform/view, video, and external query
/// cache code.
pub(crate) trait RecordingCacheConsumerV1:
    private::SealedRecordingCacheConsumerV1 + ConsumerStorageFreeV1
{
    fn latest_at_v1(&self, domain: RemoteCacheDomainV1) -> RemoteLatestAtDataQueryResultV1;

    fn range_v1(
        &self,
        domain: RemoteCacheDomainV1,
        query: &RangeQuery,
    ) -> RemoteRangeDataQueryResultV1;

    fn lookup_latest_at_v1(&self, key: &RemoteCacheKeyV1) -> RemoteLatestAtCacheLookupV1;

    fn insert_latest_at_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedLatestAtV1,
    ) -> RemoteCacheInsertionV1;

    fn publish_latest_at_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedLatestAtV1,
    ) -> Option<RemoteLatestAtDataQueryV1>;

    fn lookup_range_v1(&self, key: &RemoteCacheKeyV1) -> RemoteRangeCacheLookupV1;

    fn insert_range_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedRangeV1,
    ) -> RemoteCacheInsertionV1;

    fn publish_range_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedRangeV1,
    ) -> Option<RemoteRangeDataQueryV1>;

    fn invocation_counts_v1(&self) -> RemoteCacheInvocationCountsV1;
}

/// Remote cache adapter backed only by the sealed recording-query facade.
pub(crate) struct RemoteRecordingCacheAdapterV1<'a> {
    query: &'a dyn RecordingConsumerQueryV1,
    state: Rc<RemoteCacheAdapterStateV1>,
    _sealed: private::RemoteCacheAdapterSealV1,
}

impl<'a> RemoteRecordingCacheAdapterV1<'a> {
    pub(crate) fn new_v1(query: &'a dyn RecordingConsumerQueryV1, max_entries: usize) -> Self {
        Self {
            query,
            state: Rc::new(RemoteCacheAdapterStateV1::new_v1(max_entries)),
            _sealed: private::RemoteCacheAdapterSealV1,
        }
    }

    pub(crate) fn invocation_counts_v1(&self) -> RemoteCacheInvocationCountsV1 {
        self.state.counts_v1()
    }
}

impl private::SealedRecordingCacheConsumerV1 for RemoteRecordingCacheAdapterV1<'_> {}

impl RecordingCacheConsumerV1 for RemoteRecordingCacheAdapterV1<'_> {
    fn latest_at_v1(&self, domain: RemoteCacheDomainV1) -> RemoteLatestAtDataQueryResultV1 {
        latest_at_cache_query_v1(self.query, self.state.as_ref(), domain)
    }

    fn range_v1(
        &self,
        domain: RemoteCacheDomainV1,
        query: &RangeQuery,
    ) -> RemoteRangeDataQueryResultV1 {
        range_cache_query_v1(self.query, self.state.as_ref(), domain, query)
    }

    fn lookup_latest_at_v1(&self, key: &RemoteCacheKeyV1) -> RemoteLatestAtCacheLookupV1 {
        lookup_latest_at_v1_inner(self.query, self.state.as_ref(), key)
    }

    fn insert_latest_at_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedLatestAtV1,
    ) -> RemoteCacheInsertionV1 {
        insert_latest_at_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn publish_latest_at_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedLatestAtV1,
    ) -> Option<RemoteLatestAtDataQueryV1> {
        publish_latest_at_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn lookup_range_v1(&self, key: &RemoteCacheKeyV1) -> RemoteRangeCacheLookupV1 {
        lookup_range_v1_inner(self.query, self.state.as_ref(), key)
    }

    fn insert_range_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedRangeV1,
    ) -> RemoteCacheInsertionV1 {
        insert_range_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn publish_range_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedRangeV1,
    ) -> Option<RemoteRangeDataQueryV1> {
        publish_range_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn invocation_counts_v1(&self) -> RemoteCacheInvocationCountsV1 {
        self.invocation_counts_v1()
    }
}

/// Native/local passthrough over the same cache validation and publication path.
pub(crate) struct LocalRecordingCachePassthroughV1<'a> {
    query: &'a dyn RecordingConsumerQueryV1,
    state: Rc<RemoteCacheAdapterStateV1>,
    _sealed: private::LocalCachePassthroughSealV1,
}

impl<'a> LocalRecordingCachePassthroughV1<'a> {
    pub(crate) fn new_v1(query: &'a dyn RecordingConsumerQueryV1, max_entries: usize) -> Self {
        Self {
            query,
            state: Rc::new(RemoteCacheAdapterStateV1::new_v1(max_entries)),
            _sealed: private::LocalCachePassthroughSealV1,
        }
    }

    pub(crate) fn invocation_counts_v1(&self) -> RemoteCacheInvocationCountsV1 {
        self.state.counts_v1()
    }
}

impl private::SealedRecordingCacheConsumerV1 for LocalRecordingCachePassthroughV1<'_> {}

impl RecordingCacheConsumerV1 for LocalRecordingCachePassthroughV1<'_> {
    fn latest_at_v1(&self, domain: RemoteCacheDomainV1) -> RemoteLatestAtDataQueryResultV1 {
        latest_at_cache_query_v1(self.query, self.state.as_ref(), domain)
    }

    fn range_v1(
        &self,
        domain: RemoteCacheDomainV1,
        query: &RangeQuery,
    ) -> RemoteRangeDataQueryResultV1 {
        range_cache_query_v1(self.query, self.state.as_ref(), domain, query)
    }

    fn lookup_latest_at_v1(&self, key: &RemoteCacheKeyV1) -> RemoteLatestAtCacheLookupV1 {
        lookup_latest_at_v1_inner(self.query, self.state.as_ref(), key)
    }

    fn insert_latest_at_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedLatestAtV1,
    ) -> RemoteCacheInsertionV1 {
        insert_latest_at_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn publish_latest_at_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedLatestAtV1,
    ) -> Option<RemoteLatestAtDataQueryV1> {
        publish_latest_at_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn lookup_range_v1(&self, key: &RemoteCacheKeyV1) -> RemoteRangeCacheLookupV1 {
        lookup_range_v1_inner(self.query, self.state.as_ref(), key)
    }

    fn insert_range_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedRangeV1,
    ) -> RemoteCacheInsertionV1 {
        insert_range_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn publish_range_v1(
        &self,
        key: &RemoteCacheKeyV1,
        tagged: &RemoteRevisionTaggedRangeV1,
    ) -> Option<RemoteRangeDataQueryV1> {
        publish_range_v1_inner(self.query, self.state.as_ref(), key, tagged)
    }

    fn invocation_counts_v1(&self) -> RemoteCacheInvocationCountsV1 {
        self.invocation_counts_v1()
    }
}

impl fmt::Debug for RemoteRecordingCacheAdapterV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteRecordingCacheAdapterV1")
            .field("invocation_counts", &self.invocation_counts_v1())
            .finish()
    }
}

impl fmt::Debug for LocalRecordingCachePassthroughV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalRecordingCachePassthroughV1")
            .field("invocation_counts", &self.invocation_counts_v1())
            .finish()
    }
}

impl ConsumerStorageFreeV1 for RemoteCacheDomainV1 {}
impl ConsumerStorageFreeV1 for RemoteCacheQueryKindV1 where RangeQuery: ConsumerStorageFreeV1 {}
impl ConsumerStorageFreeV1 for RemoteCacheKeyV1
where
    RemoteCacheDomainV1: ConsumerStorageFreeV1,
    RemoteCacheQueryKindV1: ConsumerStorageFreeV1,
    RecordingQuerySnapshotV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteLatestAtCacheLookupV1 where
    RemoteRevisionTaggedLatestAtV1: ConsumerStorageFreeV1
{
}

impl ConsumerStorageFreeV1 for RemoteRangeCacheLookupV1 where
    RemoteRevisionTaggedRangeV1: ConsumerStorageFreeV1
{
}

impl ConsumerStorageFreeV1 for RemoteCacheInsertionV1 {}
impl ConsumerStorageFreeV1 for RemoteCacheInvocationCountsV1 {}

impl ConsumerStorageFreeV1 for RemoteCacheEntryV1
where
    RemoteCacheKeyV1: ConsumerStorageFreeV1,
    RemoteRevisionTaggedLatestAtV1: ConsumerStorageFreeV1,
    RemoteRevisionTaggedRangeV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteCacheAdapterStateV1
where
    RefCell<Vec<RemoteCacheEntryV1>>: ConsumerStorageFreeV1,
    RefCell<RemoteCacheInvocationCountsV1>: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for private::RemoteCacheAdapterSealV1 {}
impl ConsumerStorageFreeV1 for private::LocalCachePassthroughSealV1 {}

impl<'a> ConsumerStorageFreeV1 for RemoteRecordingCacheAdapterV1<'a>
where
    &'a dyn RecordingConsumerQueryV1: ConsumerStorageFreeV1,
    Rc<RemoteCacheAdapterStateV1>: ConsumerStorageFreeV1,
    private::RemoteCacheAdapterSealV1: ConsumerStorageFreeV1,
{
}

impl<'a> ConsumerStorageFreeV1 for LocalRecordingCachePassthroughV1<'a>
where
    &'a dyn RecordingConsumerQueryV1: ConsumerStorageFreeV1,
    Rc<RemoteCacheAdapterStateV1>: ConsumerStorageFreeV1,
    private::LocalCachePassthroughSealV1: ConsumerStorageFreeV1,
{
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use re_chunk::{RangeQuery, TimeInt, TimelineName};
    use re_entity_db::{EntityDb, StoreBundle};
    use re_log_types::{AbsoluteTimeRange, StoreId};
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;
    use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
    use crate::web_remote_mcap_consumer::RecordingConsumerContextV1;
    use crate::web_remote_mcap_query::{
        CommittedPresentationTimeV1, PrivilegedViewerFrameContextV1,
        RemoteCanonicalIndexedExtentV1, RemoteLoadedCoverageV1, RemotePresentationFacadeV1,
    };

    fn store_id() -> StoreId {
        StoreId::recording("cache-query-test-app", "cache-query-test-recording")
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

    fn make_facade_with_instance(
        facade_instance: u64,
        extent: RemoteCanonicalIndexedExtentV1,
    ) -> RemotePresentationFacadeV1 {
        RemotePresentationFacadeV1::new_for_test_v1(
            facade_instance,
            store_id(),
            RemoteRecordingUseStateV1::Foreground,
            RemoteLoadedCoverageV1::new_v1(extent, false),
            8,
            Rc::new(|| {}),
        )
    }

    fn make_facade(extent: RemoteCanonicalIndexedExtentV1) -> RemotePresentationFacadeV1 {
        make_facade_with_instance(46, extent)
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

    fn latest_key(
        consumer: &RecordingConsumerContextV1<'_>,
        domain: RemoteCacheDomainV1,
    ) -> RemoteCacheKeyV1 {
        let lease = consumer.try_lease_v1().expect("latest-at key lease");
        RemoteCacheKeyV1::latest_at_v1(domain, lease.snapshot_v1())
    }

    fn range_key(
        consumer: &RecordingConsumerContextV1<'_>,
        domain: RemoteCacheDomainV1,
        query: &RangeQuery,
    ) -> RemoteCacheKeyV1 {
        let lease = consumer
            .try_complete_range_lease_v1(query)
            .expect("range key lease");
        RemoteCacheKeyV1::range_v1(domain, query.clone(), lease.snapshot_v1())
    }

    fn available_latest_at(
        consumer: &RecordingConsumerContextV1<'_>,
    ) -> (RemoteLatestAtDataQueryV1, RemoteRevisionTaggedLatestAtV1) {
        let data_query = RemoteRecordingDataQueryAdapterV1::new_v1(consumer);
        let value = match data_query.latest_at_v1() {
            RemoteLatestAtDataQueryResultV1::Available(value) => value,
            other @ RemoteLatestAtDataQueryResultV1::Unavailable(_) => {
                panic!("expected available latest-at result, got {other:?}")
            }
        };
        let tagged = RemoteRevisionTaggedLatestAtV1::from_result_v1(value.clone());
        (value, tagged)
    }

    fn available_range(
        consumer: &RecordingConsumerContextV1<'_>,
        query: &RangeQuery,
    ) -> (RemoteRangeDataQueryV1, RemoteRevisionTaggedRangeV1) {
        let data_query = RemoteRecordingDataQueryAdapterV1::new_v1(consumer);
        let value = match data_query.range_v1(query) {
            RemoteRangeDataQueryResultV1::Available(value) => value,
            other @ RemoteRangeDataQueryResultV1::Unavailable(_) => {
                panic!("expected available range result, got {other:?}")
            }
        };
        let tagged = RemoteRevisionTaggedRangeV1::from_result_v1(value.clone());
        (value, tagged)
    }

    fn assert_cache_trait_surface<T: RecordingCacheConsumerV1 + 'static>() {
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
    fn cache_hit_insert_and_publish_all_require_current_snapshot() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingCacheAdapterV1::new_v1(&consumer, 2);
        let domain = RemoteCacheDomainV1::Memoizer;
        let key = latest_key(&consumer, domain);

        assert_eq!(
            adapter.lookup_latest_at_v1(&key),
            RemoteLatestAtCacheLookupV1::Miss
        );

        let (value, tagged) = available_latest_at(&consumer);
        assert_eq!(
            adapter.insert_latest_at_v1(&key, &tagged),
            RemoteCacheInsertionV1::Inserted { evicted: 0 }
        );
        assert_eq!(adapter.publish_latest_at_v1(&key, &tagged), Some(value));
        assert!(matches!(
            adapter.lookup_latest_at_v1(&key),
            RemoteLatestAtCacheLookupV1::Hit(_)
        ));

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged
            .begin_query_visible_insertion_v1()
            .expect("close facade");

        assert_eq!(
            adapter.lookup_latest_at_v1(&key),
            RemoteLatestAtCacheLookupV1::NotCurrent
        );
        assert_eq!(
            adapter.insert_latest_at_v1(&key, &tagged),
            RemoteCacheInsertionV1::NotCurrent
        );
        assert_eq!(adapter.publish_latest_at_v1(&key, &tagged), None);

        let counts = adapter.invocation_counts_v1();
        assert_eq!(counts.cache_lookups, 3);
        assert_eq!(counts.cache_hits, 1);
        assert_eq!(counts.cache_misses, 1);
        assert_eq!(counts.not_current_lookups, 1);
        assert_eq!(counts.cache_inserts, 1);
        assert_eq!(counts.not_current_inserts, 1);
        assert_eq!(counts.cache_publications, 1);
        assert_eq!(counts.rejected_publications, 1);
    }

    #[test]
    fn closed_facade_does_not_serve_an_old_cache_hit() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingCacheAdapterV1::new_v1(&consumer, 2);
        let domain = RemoteCacheDomainV1::ExternalWeb;

        assert!(matches!(
            adapter.latest_at_v1(domain),
            RemoteLatestAtDataQueryResultV1::Available(_)
        ));
        let before_close = adapter.invocation_counts_v1();
        assert_eq!(before_close.cache_hits, 0);
        assert_eq!(before_close.cache_misses, 1);
        assert_eq!(before_close.cache_inserts, 1);
        assert_eq!(before_close.cache_publications, 1);

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged
            .begin_query_visible_insertion_v1()
            .expect("close facade");

        assert_eq!(
            adapter.latest_at_v1(domain),
            RemoteLatestAtDataQueryResultV1::Unavailable(
                RemoteLatestAtDataUnavailableV1::PresentationGated
            )
        );
        let after_close = adapter.invocation_counts_v1();
        assert_eq!(after_close.cache_hits, before_close.cache_hits);
        assert_eq!(after_close.cache_misses, before_close.cache_misses);
        assert_eq!(after_close.cache_inserts, before_close.cache_inserts);
        assert_eq!(
            after_close.cache_publications,
            before_close.cache_publications
        );
        assert_eq!(
            after_close.not_current_lookups,
            before_close.not_current_lookups
        );
    }

    #[test]
    fn close_and_reopen_rejects_latest_at_and_range_delayed_results() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        replace_coverage(&facade, make_complete_coverage(extent()));
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingCacheAdapterV1::new_v1(&consumer, 4);
        let latest_domain = RemoteCacheDomainV1::TransformView;
        let range_domain = RemoteCacheDomainV1::VideoRange;
        let range_query = RangeQuery::everything(timeline());

        let old_latest_key = latest_key(&consumer, latest_domain);
        let old_range_key = range_key(&consumer, range_domain, &range_query);
        let (_, old_latest_tagged) = available_latest_at(&consumer);
        let (_, old_range_tagged) = available_range(&consumer, &range_query);
        assert_eq!(
            adapter.insert_latest_at_v1(&old_latest_key, &old_latest_tagged),
            RemoteCacheInsertionV1::Inserted { evicted: 0 }
        );
        assert_eq!(
            adapter.insert_range_v1(&old_range_key, &old_range_tagged),
            RemoteCacheInsertionV1::Inserted { evicted: 0 }
        );

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged
            .begin_query_visible_insertion_v1()
            .expect("close facade");
        privileged
            .finish_query_visible_insertion_v1()
            .expect("reopen facade");

        assert_ne!(
            old_latest_key.snapshot_v1(),
            latest_key(&consumer, latest_domain).snapshot_v1()
        );
        assert_ne!(
            old_range_key.snapshot_v1(),
            range_key(&consumer, range_domain, &range_query).snapshot_v1()
        );
        assert_eq!(
            adapter.lookup_latest_at_v1(&old_latest_key),
            RemoteLatestAtCacheLookupV1::NotCurrent
        );
        assert_eq!(
            adapter.insert_latest_at_v1(&old_latest_key, &old_latest_tagged),
            RemoteCacheInsertionV1::NotCurrent
        );
        assert_eq!(
            adapter.publish_latest_at_v1(&old_latest_key, &old_latest_tagged),
            None
        );
        assert_eq!(
            adapter.lookup_range_v1(&old_range_key),
            RemoteRangeCacheLookupV1::NotCurrent
        );
        assert_eq!(
            adapter.insert_range_v1(&old_range_key, &old_range_tagged),
            RemoteCacheInsertionV1::NotCurrent
        );
        assert_eq!(
            adapter.publish_range_v1(&old_range_key, &old_range_tagged),
            None
        );

        assert!(matches!(
            adapter.latest_at_v1(latest_domain),
            RemoteLatestAtDataQueryResultV1::Available(_)
        ));
        assert!(matches!(
            adapter.range_v1(range_domain, &range_query),
            RemoteRangeDataQueryResultV1::Available(_)
        ));
    }

    #[test]
    fn same_epoch_facade_swap_rejects_latest_at_and_range_delayed_results() {
        let facade_a =
            make_facade_with_instance(111, RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade_a);
        replace_coverage(&facade_a, make_complete_coverage(extent()));
        let consumer_a = RecordingConsumerContextV1::from_remote_facade_v1(&facade_a);

        let latest_key_a = latest_key(&consumer_a, RemoteCacheDomainV1::TransformView);
        let (_, latest_tagged_a) = available_latest_at(&consumer_a);
        let range_query = RangeQuery::everything(timeline());
        let range_key_a = range_key(&consumer_a, RemoteCacheDomainV1::VideoRange, &range_query);
        let (_, range_tagged_a) = available_range(&consumer_a, &range_query);

        let facade_b =
            make_facade_with_instance(112, RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade_b);
        replace_coverage(&facade_b, make_complete_coverage(extent()));
        let consumer_b = RecordingConsumerContextV1::from_remote_facade_v1(&facade_b);
        let adapter_b = RemoteRecordingCacheAdapterV1::new_v1(&consumer_b, 4);

        let latest_key_b = latest_key(&consumer_b, RemoteCacheDomainV1::TransformView);
        let range_key_b = range_key(&consumer_b, RemoteCacheDomainV1::VideoRange, &range_query);

        // Both facades are first presentations with the same numeric epoch, but their opaque
        // snapshots still differ because facade instance is part of the publication revision.
        assert_ne!(latest_key_a.snapshot_v1(), latest_key_b.snapshot_v1());
        assert_ne!(range_key_a.snapshot_v1(), range_key_b.snapshot_v1());
        assert!(consumer_a.is_current_v1(latest_key_a.snapshot_v1()));
        assert!(consumer_a.is_current_v1(range_key_a.snapshot_v1()));
        assert!(consumer_b.is_current_v1(latest_key_b.snapshot_v1()));
        assert!(consumer_b.is_current_v1(range_key_b.snapshot_v1()));
        assert!(!consumer_b.is_current_v1(latest_key_a.snapshot_v1()));
        assert!(!consumer_b.is_current_v1(range_key_a.snapshot_v1()));

        assert_eq!(
            adapter_b.lookup_latest_at_v1(&latest_key_a),
            RemoteLatestAtCacheLookupV1::NotCurrent
        );
        assert_eq!(
            adapter_b.insert_latest_at_v1(&latest_key_a, &latest_tagged_a),
            RemoteCacheInsertionV1::NotCurrent
        );
        assert_eq!(
            adapter_b.publish_latest_at_v1(&latest_key_a, &latest_tagged_a),
            None
        );
        assert_eq!(
            adapter_b.lookup_range_v1(&range_key_a),
            RemoteRangeCacheLookupV1::NotCurrent
        );
        assert_eq!(
            adapter_b.insert_range_v1(&range_key_a, &range_tagged_a),
            RemoteCacheInsertionV1::NotCurrent
        );
        assert_eq!(
            adapter_b.publish_range_v1(&range_key_a, &range_tagged_a),
            None
        );
    }

    #[test]
    fn range_unavailable_classifications_are_structured_and_do_not_query_resident_subset() {
        let gated_facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        let gated_consumer = RecordingConsumerContextV1::from_remote_facade_v1(&gated_facade);
        let gated_adapter = RemoteRecordingCacheAdapterV1::new_v1(&gated_consumer, 2);
        assert!(matches!(
            gated_adapter.range_v1(
                RemoteCacheDomainV1::VideoRange,
                &RangeQuery::everything(timeline())
            ),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::PresentationGated
            )
        ));

        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let adapter = RemoteRecordingCacheAdapterV1::new_v1(&consumer, 2);
        let domain = RemoteCacheDomainV1::VideoRange;
        let query = RangeQuery::everything(timeline());

        assert!(matches!(
            adapter.range_v1(domain, &query),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::IndexedCoverageIncomplete { .. }
            )
        ));
        assert_eq!(facade.live_lease_count_v1(), 0);

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged
            .set_use_state_v1(RemoteRecordingUseStateV1::CatalogOnly)
            .expect("catalog only");
        assert!(matches!(
            adapter.range_v1(domain, &query),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::RecordingNotForeground
            )
        ));

        let no_indexed_facade = make_facade(RemoteCanonicalIndexedExtentV1::NoIndexedMessages);
        open_facade(&no_indexed_facade);
        let no_indexed_consumer =
            RecordingConsumerContextV1::from_remote_facade_v1(&no_indexed_facade);
        let no_indexed_adapter = RemoteRecordingCacheAdapterV1::new_v1(&no_indexed_consumer, 2);
        assert!(matches!(
            no_indexed_adapter.range_v1(domain, &query),
            RemoteRangeDataQueryResultV1::Unavailable(
                RemoteRangeDataUnavailableV1::NoIndexedMessages { .. }
            )
        ));
        assert_eq!(no_indexed_adapter.invocation_counts_v1().cache_inserts, 0);
        assert_eq!(
            no_indexed_adapter.invocation_counts_v1().cache_publications,
            0
        );
    }

    #[test]
    fn remote_and_local_cache_adapters_have_identical_trace_and_classification() {
        let facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&facade);
        replace_coverage(&facade, make_complete_coverage(extent()));
        let consumer = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let remote = RemoteRecordingCacheAdapterV1::new_v1(&consumer, 4);
        let local = LocalRecordingCachePassthroughV1::new_v1(&consumer, 4);
        let latest_domain = RemoteCacheDomainV1::ExternalWeb;
        let range_domain = RemoteCacheDomainV1::VideoRange;
        let range_query = RangeQuery::everything(timeline());

        assert_eq!(
            remote.latest_at_v1(latest_domain),
            local.latest_at_v1(latest_domain)
        );
        assert_eq!(
            remote.latest_at_v1(latest_domain),
            local.latest_at_v1(latest_domain)
        );
        assert_eq!(
            remote.range_v1(range_domain, &range_query),
            local.range_v1(range_domain, &range_query)
        );
        assert_eq!(
            remote.range_v1(range_domain, &range_query),
            local.range_v1(range_domain, &range_query)
        );
        assert_eq!(remote.invocation_counts_v1(), local.invocation_counts_v1());

        let remote_counts = remote.invocation_counts_v1();
        assert_eq!(remote_counts.latest_at_cache_queries, 2);
        assert_eq!(remote_counts.range_cache_queries, 2);
        assert_eq!(remote_counts.cache_hits, 2);
        assert_eq!(remote_counts.cache_misses, 2);
        assert_eq!(remote_counts.cache_inserts, 2);
        assert_eq!(remote_counts.cache_publications, 4);
    }

    #[test]
    fn remote_and_local_cache_adapters_classify_unavailable_identically() {
        let gated_facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        let gated_consumer = RecordingConsumerContextV1::from_remote_facade_v1(&gated_facade);
        let remote_gated = RemoteRecordingCacheAdapterV1::new_v1(&gated_consumer, 2);
        let local_gated = LocalRecordingCachePassthroughV1::new_v1(&gated_consumer, 2);

        assert_eq!(
            remote_gated.latest_at_v1(RemoteCacheDomainV1::Memoizer),
            local_gated.latest_at_v1(RemoteCacheDomainV1::Memoizer)
        );
        assert_eq!(
            remote_gated.range_v1(
                RemoteCacheDomainV1::VideoRange,
                &RangeQuery::everything(timeline())
            ),
            local_gated.range_v1(
                RemoteCacheDomainV1::VideoRange,
                &RangeQuery::everything(timeline())
            )
        );
        assert_eq!(
            remote_gated.invocation_counts_v1(),
            local_gated.invocation_counts_v1()
        );

        let incomplete_facade = make_facade(RemoteCanonicalIndexedExtentV1::Known(extent()));
        open_facade(&incomplete_facade);
        let incomplete_consumer =
            RecordingConsumerContextV1::from_remote_facade_v1(&incomplete_facade);
        let remote_incomplete = RemoteRecordingCacheAdapterV1::new_v1(&incomplete_consumer, 2);
        let local_incomplete = LocalRecordingCachePassthroughV1::new_v1(&incomplete_consumer, 2);
        let range_query = RangeQuery::everything(timeline());

        assert_eq!(
            remote_incomplete.range_v1(RemoteCacheDomainV1::VideoRange, &range_query),
            local_incomplete.range_v1(RemoteCacheDomainV1::VideoRange, &range_query)
        );
        assert_eq!(
            remote_incomplete.invocation_counts_v1(),
            local_incomplete.invocation_counts_v1()
        );
    }

    #[test]
    fn cache_types_are_storage_free_by_structure() {
        assert_cache_trait_surface::<RemoteRecordingCacheAdapterV1<'static>>();
        assert_cache_trait_surface::<LocalRecordingCachePassthroughV1<'static>>();

        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StoreBundle);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StorageEngine);

        assert_storage_free_v1::<RemoteRecordingCacheAdapterV1<'static>>();
        assert_storage_free_v1::<LocalRecordingCachePassthroughV1<'static>>();
        assert_storage_free_v1::<RemoteCacheKeyV1>();
        assert_storage_free_v1::<RemoteLatestAtCacheLookupV1>();
        assert_storage_free_v1::<RemoteRangeCacheLookupV1>();
        assert_storage_free_v1::<RemoteCacheInsertionV1>();
        assert_storage_free_v1::<RemoteCacheInvocationCountsV1>();
        assert_storage_free_v1::<RemoteCacheEntryV1>();
        assert_storage_free_v1::<RemoteCacheAdapterStateV1>();
    }
}
