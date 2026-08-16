//! Production-disarmed Web remote-MCAP memory-pressure reclaim handshake.

#![allow(dead_code)]

use re_chunk_store::GarbageCollectionTarget;
use re_log_types::StoreId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteReclaimRequestIdV1(u64);

impl RemoteReclaimRequestIdV1 {
    #[cfg(test)]
    const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteStoreRevisionV1(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteProtectionRevisionV1(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMcapSlotTokenV1(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteProcessMemorySampleV1 {
    revision: u64,
    bytes: u64,
    has_remote_headroom: bool,
}

impl RemoteProcessMemorySampleV1 {
    pub(crate) const fn new_v1(revision: u64, bytes: u64, has_remote_headroom: bool) -> Self {
        Self {
            revision,
            bytes,
            has_remote_headroom,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PendingRemoteReclaimV1 {
    Gc {
        request_id: RemoteReclaimRequestIdV1,
        store_id: StoreId,
        target: GarbageCollectionTarget,
        store_revision: RemoteStoreRevisionV1,
        protection_revision: RemoteProtectionRevisionV1,
    },
    Close {
        request_id: RemoteReclaimRequestIdV1,
        store_id: StoreId,
        slot_token: RemoteMcapSlotTokenV1,
    },
}

impl PendingRemoteReclaimV1 {
    pub(crate) const fn request_id_v1(&self) -> RemoteReclaimRequestIdV1 {
        match self {
            Self::Gc { request_id, .. } | Self::Close { request_id, .. } => *request_id,
        }
    }

    fn store_id_v1(&self) -> &StoreId {
        match self {
            Self::Gc { store_id, .. } | Self::Close { store_id, .. } => store_id,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PurgeOutcomeV1 {
    pub(crate) freed_now_bytes: u64,
    pub(crate) pending_remote_reclaim: Option<PendingRemoteReclaimV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteReclaimSubmissionActionV1 {
    Started,
    Reused,
    Busy,
    SupersededGc(RemoteReclaimRequestIdV1),
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteReclaimSubmissionV1 {
    pub(crate) ticket: PendingRemoteReclaimV1,
    pub(crate) action: RemoteReclaimSubmissionActionV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteReclaimControllerErrorV1 {
    RequestIdExhausted,
    ConflictingRemoteOwner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteReclaimCompletionOutcomeV1 {
    AppliedAwaitingProcessResample,
    Duplicate,
    Stale,
    SupersededByClose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteProcessSampleOutcomeV1 {
    Recorded,
    ReclaimEpisodeFinished,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteReclaimTelemetryV1 {
    store_bytes_before: u64,
    store_bytes_after: u64,
}

pub(crate) struct RemoteCloseCleanupProofV1 {
    request_id: RemoteReclaimRequestIdV1,
    store_id: StoreId,
    slot_token: RemoteMcapSlotTokenV1,
    store_bytes_before: u64,
    store_bytes_after: u64,
}

impl RemoteCloseCleanupProofV1 {
    #[cfg(test)]
    fn for_test_v1(
        request_id: RemoteReclaimRequestIdV1,
        store_id: StoreId,
        slot_token: RemoteMcapSlotTokenV1,
        store_bytes_before: u64,
        store_bytes_after: u64,
    ) -> Self {
        Self {
            request_id,
            store_id,
            slot_token,
            store_bytes_before,
            store_bytes_after,
        }
    }
}

struct SupersededRemoteGcV1 {
    request_id: RemoteReclaimRequestIdV1,
    store_id: StoreId,
    store_revision: RemoteStoreRevisionV1,
    protection_revision: RemoteProtectionRevisionV1,
}

enum RemoteReclaimEpisodeV1 {
    Pending(PendingRemoteReclaimV1),
    AwaitingProcessResample {
        ticket: PendingRemoteReclaimV1,
        completion: RemoteReclaimTelemetryV1,
        minimum_sample_revision: u64,
    },
}

impl RemoteReclaimEpisodeV1 {
    fn ticket_v1(&self) -> &PendingRemoteReclaimV1 {
        match self {
            Self::Pending(ticket) | Self::AwaitingProcessResample { ticket, .. } => ticket,
        }
    }

    const fn is_awaiting_resample_v1(&self) -> bool {
        matches!(self, Self::AwaitingProcessResample { .. })
    }
}

pub(crate) struct WebRemoteMcapMemoryControllerV1 {
    next_request_id: u64,
    latest_sample: RemoteProcessMemorySampleV1,
    episode: Option<RemoteReclaimEpisodeV1>,
    superseded_gc: Option<SupersededRemoteGcV1>,
}

impl WebRemoteMcapMemoryControllerV1 {
    pub(crate) const fn new_disarmed_v1(initial_sample: RemoteProcessMemorySampleV1) -> Self {
        Self {
            next_request_id: 1,
            latest_sample: initial_sample,
            episode: None,
            superseded_gc: None,
        }
    }

    fn allocate_request_id_v1(
        &mut self,
    ) -> Result<RemoteReclaimRequestIdV1, RemoteReclaimControllerErrorV1> {
        let next = self
            .next_request_id
            .checked_add(1)
            .ok_or(RemoteReclaimControllerErrorV1::RequestIdExhausted)?;
        let request_id = RemoteReclaimRequestIdV1(self.next_request_id);
        self.next_request_id = next;
        Ok(request_id)
    }

    pub(crate) fn purge_outcome_v1(&self, freed_now_bytes: u64) -> PurgeOutcomeV1 {
        PurgeOutcomeV1 {
            freed_now_bytes,
            pending_remote_reclaim: self
                .episode
                .as_ref()
                .map(|episode| episode.ticket_v1().clone()),
        }
    }

    pub(crate) const fn remote_growth_admitted_v1(&self) -> bool {
        self.episode.is_none() && self.latest_sample.has_remote_headroom
    }

    pub(crate) fn completed_remote_store_delta_for_telemetry_v1(&self) -> Option<u64> {
        let RemoteReclaimEpisodeV1::AwaitingProcessResample { completion, .. } =
            self.episode.as_ref()?
        else {
            return None;
        };
        Some(
            completion
                .store_bytes_before
                .saturating_sub(completion.store_bytes_after),
        )
    }

    pub(crate) fn request_gc_v1(
        &mut self,
        store_id: StoreId,
        target: GarbageCollectionTarget,
        store_revision: RemoteStoreRevisionV1,
        protection_revision: RemoteProtectionRevisionV1,
    ) -> Result<RemoteReclaimSubmissionV1, RemoteReclaimControllerErrorV1> {
        if let Some(episode) = &self.episode {
            let ticket = episode.ticket_v1();
            if ticket.store_id_v1() != &store_id {
                return Err(RemoteReclaimControllerErrorV1::ConflictingRemoteOwner);
            }
            let action = match ticket {
                PendingRemoteReclaimV1::Gc {
                    target: current_target,
                    store_revision: current_store_revision,
                    protection_revision: current_protection_revision,
                    ..
                } if gc_targets_equal_v1(*current_target, target)
                    && *current_store_revision == store_revision
                    && *current_protection_revision == protection_revision =>
                {
                    RemoteReclaimSubmissionActionV1::Reused
                }
                _ => RemoteReclaimSubmissionActionV1::Busy,
            };
            return Ok(RemoteReclaimSubmissionV1 {
                ticket: ticket.clone(),
                action,
            });
        }

        let ticket = PendingRemoteReclaimV1::Gc {
            request_id: self.allocate_request_id_v1()?,
            store_id,
            target,
            store_revision,
            protection_revision,
        };
        self.episode = Some(RemoteReclaimEpisodeV1::Pending(ticket.clone()));
        Ok(RemoteReclaimSubmissionV1 {
            ticket,
            action: RemoteReclaimSubmissionActionV1::Started,
        })
    }

    pub(crate) fn request_close_v1(
        &mut self,
        store_id: StoreId,
        slot_token: RemoteMcapSlotTokenV1,
    ) -> Result<RemoteReclaimSubmissionV1, RemoteReclaimControllerErrorV1> {
        if let Some(episode) = &self.episode {
            let existing = episode.ticket_v1();
            if existing.store_id_v1() != &store_id {
                return Err(RemoteReclaimControllerErrorV1::ConflictingRemoteOwner);
            }
            match existing {
                PendingRemoteReclaimV1::Close {
                    slot_token: existing_slot,
                    ..
                } if *existing_slot == slot_token => {
                    return Ok(RemoteReclaimSubmissionV1 {
                        ticket: existing.clone(),
                        action: RemoteReclaimSubmissionActionV1::Reused,
                    });
                }
                PendingRemoteReclaimV1::Close { .. } => {
                    return Err(RemoteReclaimControllerErrorV1::ConflictingRemoteOwner);
                }
                PendingRemoteReclaimV1::Gc {
                    request_id,
                    store_revision,
                    protection_revision,
                    ..
                } => {
                    let superseded = *request_id;
                    let superseded_store_id = existing.store_id_v1().clone();
                    let superseded_store_revision = *store_revision;
                    let superseded_protection_revision = *protection_revision;
                    let ticket = PendingRemoteReclaimV1::Close {
                        request_id: self.allocate_request_id_v1()?,
                        store_id,
                        slot_token,
                    };
                    self.superseded_gc = Some(SupersededRemoteGcV1 {
                        request_id: superseded,
                        store_id: superseded_store_id,
                        store_revision: superseded_store_revision,
                        protection_revision: superseded_protection_revision,
                    });
                    self.episode = Some(RemoteReclaimEpisodeV1::Pending(ticket.clone()));
                    return Ok(RemoteReclaimSubmissionV1 {
                        ticket,
                        action: RemoteReclaimSubmissionActionV1::SupersededGc(superseded),
                    });
                }
            }
        }

        let ticket = PendingRemoteReclaimV1::Close {
            request_id: self.allocate_request_id_v1()?,
            store_id,
            slot_token,
        };
        self.episode = Some(RemoteReclaimEpisodeV1::Pending(ticket.clone()));
        Ok(RemoteReclaimSubmissionV1 {
            ticket,
            action: RemoteReclaimSubmissionActionV1::Started,
        })
    }

    pub(crate) fn complete_gc_v1(
        &mut self,
        request_id: RemoteReclaimRequestIdV1,
        store_id: &StoreId,
        store_revision: RemoteStoreRevisionV1,
        protection_revision: RemoteProtectionRevisionV1,
        store_bytes_before: u64,
        store_bytes_after: u64,
    ) -> RemoteReclaimCompletionOutcomeV1 {
        if self.superseded_gc.as_ref().is_some_and(|superseded| {
            superseded.request_id == request_id
                && &superseded.store_id == store_id
                && superseded.store_revision == store_revision
                && superseded.protection_revision == protection_revision
        }) {
            return RemoteReclaimCompletionOutcomeV1::SupersededByClose;
        }
        let Some(episode) = &self.episode else {
            return RemoteReclaimCompletionOutcomeV1::Stale;
        };
        if episode.is_awaiting_resample_v1() && episode.ticket_v1().request_id_v1() == request_id {
            return RemoteReclaimCompletionOutcomeV1::Duplicate;
        }
        let PendingRemoteReclaimV1::Gc {
            request_id: expected_request,
            store_id: expected_store,
            store_revision: expected_store_revision,
            protection_revision: expected_protection_revision,
            ..
        } = episode.ticket_v1()
        else {
            return RemoteReclaimCompletionOutcomeV1::Stale;
        };
        if *expected_request != request_id
            || expected_store != store_id
            || *expected_store_revision != store_revision
            || *expected_protection_revision != protection_revision
        {
            return RemoteReclaimCompletionOutcomeV1::Stale;
        }
        let ticket = episode.ticket_v1().clone();
        self.episode = Some(RemoteReclaimEpisodeV1::AwaitingProcessResample {
            ticket,
            completion: RemoteReclaimTelemetryV1 {
                store_bytes_before,
                store_bytes_after,
            },
            minimum_sample_revision: self.latest_sample.revision,
        });
        RemoteReclaimCompletionOutcomeV1::AppliedAwaitingProcessResample
    }

    pub(crate) fn complete_close_v1(
        &mut self,
        proof: RemoteCloseCleanupProofV1,
    ) -> RemoteReclaimCompletionOutcomeV1 {
        let RemoteCloseCleanupProofV1 {
            request_id,
            store_id,
            slot_token,
            store_bytes_before,
            store_bytes_after,
        } = proof;
        let Some(episode) = &self.episode else {
            return RemoteReclaimCompletionOutcomeV1::Stale;
        };
        if episode.is_awaiting_resample_v1() && episode.ticket_v1().request_id_v1() == request_id {
            return RemoteReclaimCompletionOutcomeV1::Duplicate;
        }
        let PendingRemoteReclaimV1::Close {
            request_id: expected_request,
            store_id: expected_store,
            slot_token: expected_slot,
        } = episode.ticket_v1()
        else {
            return RemoteReclaimCompletionOutcomeV1::Stale;
        };
        if *expected_request != request_id
            || expected_store != &store_id
            || *expected_slot != slot_token
        {
            return RemoteReclaimCompletionOutcomeV1::Stale;
        }
        let ticket = episode.ticket_v1().clone();
        self.episode = Some(RemoteReclaimEpisodeV1::AwaitingProcessResample {
            ticket,
            completion: RemoteReclaimTelemetryV1 {
                store_bytes_before,
                store_bytes_after,
            },
            minimum_sample_revision: self.latest_sample.revision,
        });
        RemoteReclaimCompletionOutcomeV1::AppliedAwaitingProcessResample
    }

    pub(crate) fn observe_process_memory_sample_v1(
        &mut self,
        sample: RemoteProcessMemorySampleV1,
    ) -> RemoteProcessSampleOutcomeV1 {
        if sample.revision <= self.latest_sample.revision {
            return RemoteProcessSampleOutcomeV1::Stale;
        }
        self.latest_sample = sample;
        let finish_episode = self.episode.as_ref().is_some_and(|episode| {
            matches!(
                episode,
                RemoteReclaimEpisodeV1::AwaitingProcessResample {
                    minimum_sample_revision,
                    ..
                } if sample.revision > *minimum_sample_revision
            )
        });
        if finish_episode {
            self.episode = None;
            self.superseded_gc = None;
            RemoteProcessSampleOutcomeV1::ReclaimEpisodeFinished
        } else {
            RemoteProcessSampleOutcomeV1::Recorded
        }
    }
}

fn gc_targets_equal_v1(left: GarbageCollectionTarget, right: GarbageCollectionTarget) -> bool {
    match (left, right) {
        (
            GarbageCollectionTarget::DropAtLeastBytes(left),
            GarbageCollectionTarget::DropAtLeastBytes(right),
        ) => left == right,
        (
            GarbageCollectionTarget::DropAtLeastFraction(left),
            GarbageCollectionTarget::DropAtLeastFraction(right),
        ) => left.to_bits() == right.to_bits(),
        (GarbageCollectionTarget::Everything, GarbageCollectionTarget::Everything) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use re_entity_db::EntityDb;
    use re_viewer_context::StoreHub;

    use super::*;

    fn store_id() -> StoreId {
        StoreId::recording("remote-memory-test", "recording")
    }

    fn controller(has_headroom: bool) -> WebRemoteMcapMemoryControllerV1 {
        WebRemoteMcapMemoryControllerV1::new_disarmed_v1(RemoteProcessMemorySampleV1::new_v1(
            1,
            1_000,
            has_headroom,
        ))
    }

    #[test]
    fn pending_gc_is_deduplicated_and_never_counted_as_freed_now() {
        let mut controller = controller(true);
        let store_id = store_id();
        let first = controller
            .request_gc_v1(
                store_id.clone(),
                GarbageCollectionTarget::DropAtLeastBytes(400),
                RemoteStoreRevisionV1(7),
                RemoteProtectionRevisionV1(9),
            )
            .unwrap();
        assert_eq!(first.action, RemoteReclaimSubmissionActionV1::Started);
        let request_id = first.ticket.request_id_v1();
        let duplicate = controller
            .request_gc_v1(
                store_id.clone(),
                GarbageCollectionTarget::DropAtLeastBytes(400),
                RemoteStoreRevisionV1(7),
                RemoteProtectionRevisionV1(9),
            )
            .unwrap();
        assert_eq!(duplicate.action, RemoteReclaimSubmissionActionV1::Reused);
        assert_eq!(duplicate.ticket.request_id_v1(), request_id);
        let outcome = controller.purge_outcome_v1(37);
        assert_eq!(outcome.freed_now_bytes, 37);
        assert_eq!(
            outcome.pending_remote_reclaim.unwrap().request_id_v1(),
            request_id
        );
        assert!(!controller.remote_growth_admitted_v1());

        assert_eq!(
            controller.complete_gc_v1(
                request_id,
                &store_id,
                RemoteStoreRevisionV1(7),
                RemoteProtectionRevisionV1(9),
                800,
                300,
            ),
            RemoteReclaimCompletionOutcomeV1::AppliedAwaitingProcessResample
        );
        assert_eq!(
            controller.completed_remote_store_delta_for_telemetry_v1(),
            Some(500)
        );
        let after_completion = controller.purge_outcome_v1(0);
        assert_eq!(after_completion.freed_now_bytes, 0);
        assert!(after_completion.pending_remote_reclaim.is_some());
        assert!(!controller.remote_growth_admitted_v1());
        assert_eq!(
            controller.observe_process_memory_sample_v1(RemoteProcessMemorySampleV1::new_v1(
                1, 500, true,
            )),
            RemoteProcessSampleOutcomeV1::Stale
        );
        assert!(!controller.remote_growth_admitted_v1());
        assert_eq!(
            controller.observe_process_memory_sample_v1(RemoteProcessMemorySampleV1::new_v1(
                2, 500, true,
            )),
            RemoteProcessSampleOutcomeV1::ReclaimEpisodeFinished
        );
        assert!(controller.remote_growth_admitted_v1());
    }

    #[test]
    fn gc_to_close_supersedes_once_and_late_gc_cannot_finish_close() {
        let mut controller = controller(true);
        let store_id = store_id();
        let gc = controller
            .request_gc_v1(
                store_id.clone(),
                GarbageCollectionTarget::Everything,
                RemoteStoreRevisionV1(3),
                RemoteProtectionRevisionV1(4),
            )
            .unwrap();
        let gc_id = gc.ticket.request_id_v1();
        let close = controller
            .request_close_v1(store_id.clone(), RemoteMcapSlotTokenV1(11))
            .unwrap();
        assert_eq!(
            close.action,
            RemoteReclaimSubmissionActionV1::SupersededGc(gc_id)
        );
        let close_id = close.ticket.request_id_v1();
        assert_ne!(close_id, gc_id);
        let duplicate = controller
            .request_close_v1(store_id.clone(), RemoteMcapSlotTokenV1(11))
            .unwrap();
        assert_eq!(duplicate.action, RemoteReclaimSubmissionActionV1::Reused);
        assert_eq!(duplicate.ticket.request_id_v1(), close_id);
        assert_eq!(
            controller.complete_gc_v1(
                gc_id,
                &store_id,
                RemoteStoreRevisionV1(3),
                RemoteProtectionRevisionV1(4),
                900,
                600,
            ),
            RemoteReclaimCompletionOutcomeV1::SupersededByClose
        );
        assert!(!controller.remote_growth_admitted_v1());
        assert_eq!(
            controller.complete_close_v1(RemoteCloseCleanupProofV1::for_test_v1(
                close_id,
                store_id.clone(),
                RemoteMcapSlotTokenV1(11),
                600,
                0,
            )),
            RemoteReclaimCompletionOutcomeV1::AppliedAwaitingProcessResample
        );
        assert!(!controller.remote_growth_admitted_v1());
        assert_eq!(
            controller.observe_process_memory_sample_v1(RemoteProcessMemorySampleV1::new_v1(
                2, 950, false,
            )),
            RemoteProcessSampleOutcomeV1::ReclaimEpisodeFinished
        );
        assert!(!controller.remote_growth_admitted_v1());
    }

    #[test]
    fn mismatched_requests_and_completions_do_not_create_or_finish_work() {
        let mut controller = controller(true);
        let store_id = store_id();
        let first = controller
            .request_gc_v1(
                store_id.clone(),
                GarbageCollectionTarget::DropAtLeastFraction(0.5),
                RemoteStoreRevisionV1(1),
                RemoteProtectionRevisionV1(1),
            )
            .unwrap();
        let request_id = first.ticket.request_id_v1();
        let busy = controller
            .request_gc_v1(
                store_id.clone(),
                GarbageCollectionTarget::DropAtLeastFraction(0.75),
                RemoteStoreRevisionV1(1),
                RemoteProtectionRevisionV1(1),
            )
            .unwrap();
        assert_eq!(busy.action, RemoteReclaimSubmissionActionV1::Busy);
        assert_eq!(busy.ticket.request_id_v1(), request_id);
        assert_eq!(
            controller.complete_gc_v1(
                request_id,
                &store_id,
                RemoteStoreRevisionV1(2),
                RemoteProtectionRevisionV1(1),
                500,
                100,
            ),
            RemoteReclaimCompletionOutcomeV1::Stale
        );
        assert!(!controller.remote_growth_admitted_v1());
        assert!(matches!(
            controller.request_close_v1(
                StoreId::recording("other", "recording"),
                RemoteMcapSlotTokenV1(1),
            ),
            Err(RemoteReclaimControllerErrorV1::ConflictingRemoteOwner)
        ));
    }

    #[test]
    fn completed_gc_can_still_be_superseded_by_close_before_resample() {
        let mut controller = controller(true);
        let store_id = store_id();
        let gc = controller
            .request_gc_v1(
                store_id.clone(),
                GarbageCollectionTarget::Everything,
                RemoteStoreRevisionV1(5),
                RemoteProtectionRevisionV1(6),
            )
            .unwrap();
        let gc_id = gc.ticket.request_id_v1();
        assert_eq!(
            controller.complete_gc_v1(
                gc_id,
                &store_id,
                RemoteStoreRevisionV1(5),
                RemoteProtectionRevisionV1(6),
                700,
                500,
            ),
            RemoteReclaimCompletionOutcomeV1::AppliedAwaitingProcessResample
        );
        let close = controller
            .request_close_v1(store_id, RemoteMcapSlotTokenV1(8))
            .unwrap();
        assert_eq!(
            close.action,
            RemoteReclaimSubmissionActionV1::SupersededGc(gc_id)
        );
    }

    #[test]
    fn ordinary_store_hub_purge_remains_synchronous_and_returns_u64() {
        let mut hub = StoreHub::test_hub();
        let store_id = StoreId::recording("ordinary", "recording");
        hub.insert_entity_db(EntityDb::new(store_id.clone()));
        let freed = hub.purge_fraction_of_ram(1.0, None, &|_| None);
        let require_u64 = |_: &u64| {};
        require_u64(&freed);
        assert!(freed > 0);
        assert!(hub.store_bundle().get(&store_id).is_none());
    }

    #[test]
    fn request_ids_are_stable_for_duplicates_and_advance_for_new_episodes() {
        let mut controller = controller(true);
        let store_id = store_id();
        let first = controller
            .request_close_v1(store_id.clone(), RemoteMcapSlotTokenV1(1))
            .unwrap();
        assert_eq!(first.ticket.request_id_v1().get_v1(), 1);
        assert_eq!(
            controller
                .request_close_v1(store_id.clone(), RemoteMcapSlotTokenV1(1))
                .unwrap()
                .ticket
                .request_id_v1()
                .get_v1(),
            1
        );
        assert_eq!(
            controller.complete_close_v1(RemoteCloseCleanupProofV1::for_test_v1(
                first.ticket.request_id_v1(),
                store_id.clone(),
                RemoteMcapSlotTokenV1(1),
                10,
                0,
            )),
            RemoteReclaimCompletionOutcomeV1::AppliedAwaitingProcessResample
        );
        controller
            .observe_process_memory_sample_v1(RemoteProcessMemorySampleV1::new_v1(2, 100, true));
        let second = controller
            .request_close_v1(store_id, RemoteMcapSlotTokenV1(2))
            .unwrap();
        assert_eq!(second.ticket.request_id_v1().get_v1(), 2);
    }

    #[test]
    fn request_id_exhaustion_is_atomic() {
        let mut controller = controller(true);
        controller.next_request_id = u64::MAX;
        assert!(matches!(
            controller.request_gc_v1(
                store_id(),
                GarbageCollectionTarget::Everything,
                RemoteStoreRevisionV1(1),
                RemoteProtectionRevisionV1(1),
            ),
            Err(RemoteReclaimControllerErrorV1::RequestIdExhausted)
        ));
        assert!(controller.episode.is_none());
        assert!(controller.remote_growth_admitted_v1());
    }

    #[test]
    fn process_memory_samples_toggle_remote_growth_admission() {
        let mut controller = controller(true);
        assert!(controller.remote_growth_admitted_v1());
        assert_eq!(
            controller.observe_process_memory_sample_v1(RemoteProcessMemorySampleV1::new_v1(
                2, 1_500, false,
            )),
            RemoteProcessSampleOutcomeV1::Recorded
        );
        assert!(!controller.remote_growth_admitted_v1());
        assert_eq!(
            controller.observe_process_memory_sample_v1(RemoteProcessMemorySampleV1::new_v1(
                3, 900, true,
            )),
            RemoteProcessSampleOutcomeV1::Recorded
        );
        assert!(controller.remote_growth_admitted_v1());
    }
}
