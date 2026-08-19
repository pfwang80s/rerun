//! Production-disarmed Web remote-MCAP memory-pressure reclaim handshake.

#![allow(dead_code)]

use re_chunk_store::GarbageCollectionTarget;
use re_log_types::StoreId;

use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
use crate::web_remote_mcap_mutation_arbiter::{
    RemoteMutationAddChunkOutcomeV1, RemoteMutationArbiterErrorV1, RemoteMutationCloseProgressV1,
    RemoteMutationRequestIdV1, RemoteMutationSubstateV1, RemoteMutationTurnV1,
    RemoteStoreMutationArbiterV1,
};
use crate::web_remote_mcap_query::{
    CommittedPresentationTimeV1, ConsumerStorageFreeV1, PrivilegedViewerFrameContextV1,
    RemotePresentationFacadeV1,
};

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

impl RemoteStoreRevisionV1 {
    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteProtectionRevisionV1(u64);

impl RemoteProtectionRevisionV1 {
    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMcapSlotTokenV1(u64);

impl RemoteMcapSlotTokenV1 {
    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

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

#[derive(Clone, Debug)]
pub(crate) struct RemoteMutationReclaimV1 {
    freed_now_bytes: u64,
    pending_remote_reclaim: Option<PendingRemoteReclaimV1>,
}

impl RemoteMutationReclaimV1 {
    pub(crate) fn from_purge_outcome_v1(outcome: PurgeOutcomeV1) -> Self {
        Self {
            freed_now_bytes: outcome.freed_now_bytes,
            pending_remote_reclaim: outcome.pending_remote_reclaim,
        }
    }

    pub(crate) const fn freed_now_bytes_v1(&self) -> u64 {
        self.freed_now_bytes
    }

    pub(crate) fn pending_remote_reclaim_v1(&self) -> Option<&PendingRemoteReclaimV1> {
        self.pending_remote_reclaim.as_ref()
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RemoteRootIdentityV1(u64);

impl RemoteRootIdentityV1 {
    pub(crate) const fn new_v1(identity: u64) -> Self {
        Self(identity)
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRootResidencyV1 {
    Unloaded,
    InTransit,
    FullyLoaded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRootProtectionClassV1 {
    current_closure: bool,
    staging: bool,
    pin: bool,
    opening_static: bool,
}

impl RemoteRootProtectionClassV1 {
    pub(crate) const fn unprotected_v1() -> Self {
        Self {
            current_closure: false,
            staging: false,
            pin: false,
            opening_static: false,
        }
    }

    pub(crate) const fn current_closure_v1() -> Self {
        Self {
            current_closure: true,
            staging: false,
            pin: false,
            opening_static: false,
        }
    }

    pub(crate) const fn staging_v1() -> Self {
        Self {
            current_closure: false,
            staging: true,
            pin: false,
            opening_static: false,
        }
    }

    pub(crate) const fn pin_v1() -> Self {
        Self {
            current_closure: false,
            staging: false,
            pin: true,
            opening_static: false,
        }
    }

    pub(crate) const fn opening_static_v1() -> Self {
        Self {
            current_closure: false,
            staging: false,
            pin: false,
            opening_static: true,
        }
    }

    pub(crate) const fn is_protected_v1(self) -> bool {
        self.current_closure || self.staging || self.pin || self.opening_static
    }

    pub(crate) const fn is_current_closure_v1(self) -> bool {
        self.current_closure
    }

    pub(crate) const fn is_staging_v1(self) -> bool {
        self.staging
    }

    pub(crate) const fn is_pin_v1(self) -> bool {
        self.pin
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteResidentRootV1 {
    identity: RemoteRootIdentityV1,
    residency: RemoteRootResidencyV1,
    cursor_distance: Option<u64>,
    protection: RemoteRootProtectionClassV1,
}

impl RemoteResidentRootV1 {
    pub(crate) const fn new_v1(
        identity: RemoteRootIdentityV1,
        residency: RemoteRootResidencyV1,
        cursor_distance: Option<u64>,
        protection: RemoteRootProtectionClassV1,
    ) -> Self {
        Self {
            identity,
            residency,
            cursor_distance,
            protection,
        }
    }

    pub(crate) const fn identity_v1(&self) -> RemoteRootIdentityV1 {
        self.identity
    }

    pub(crate) const fn residency_v1(&self) -> RemoteRootResidencyV1 {
        self.residency
    }

    pub(crate) const fn cursor_distance_v1(&self) -> Option<u64> {
        self.cursor_distance
    }

    pub(crate) const fn protection_v1(&self) -> RemoteRootProtectionClassV1 {
        self.protection
    }

    const fn is_physically_resident_v1(&self) -> bool {
        !matches!(self.residency, RemoteRootResidencyV1::Unloaded)
    }

    const fn is_gc_eligible_v1(&self) -> bool {
        matches!(self.residency, RemoteRootResidencyV1::FullyLoaded)
            && !self.protection.is_protected_v1()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RemoteGcTargetV1 {
    max_session_resident_physical_roots: usize,
    target: GarbageCollectionTarget,
}

impl RemoteGcTargetV1 {
    pub(crate) const fn new_v1(
        max_session_resident_physical_roots: usize,
        target: GarbageCollectionTarget,
    ) -> Result<Self, RemoteMemoryArbiterErrorV1> {
        if max_session_resident_physical_roots == 0 {
            return Err(RemoteMemoryArbiterErrorV1::ResidentRootCapExhausted);
        }
        Ok(Self {
            max_session_resident_physical_roots,
            target,
        })
    }

    pub(crate) const fn target_v1(self) -> GarbageCollectionTarget {
        self.target
    }

    pub(crate) const fn max_session_resident_physical_roots_v1(self) -> usize {
        self.max_session_resident_physical_roots
    }

    pub(crate) fn checked_resident_root_count_v1(
        self,
        current_resident_roots: usize,
        new_roots: usize,
    ) -> Result<usize, RemoteMemoryArbiterErrorV1> {
        let total_roots = current_resident_roots
            .checked_add(new_roots)
            .ok_or(RemoteMemoryArbiterErrorV1::RootCountOverflow)?;
        if total_roots > self.max_session_resident_physical_roots {
            return Err(RemoteMemoryArbiterErrorV1::ResidentRootCapExceeded {
                resident_roots: total_roots,
                max_session_resident_physical_roots: self.max_session_resident_physical_roots,
            });
        }
        Ok(total_roots)
    }

    pub(crate) fn required_drop_root_count_v1(
        self,
        current_resident_roots: usize,
        reserved_new_roots: usize,
    ) -> Result<usize, RemoteMemoryArbiterErrorV1> {
        let target_resident_roots = self
            .max_session_resident_physical_roots
            .checked_sub(reserved_new_roots)
            .ok_or(RemoteMemoryArbiterErrorV1::RootCountOverflow)?;
        if current_resident_roots <= target_resident_roots {
            Ok(0)
        } else {
            current_resident_roots
                .checked_sub(target_resident_roots)
                .ok_or(RemoteMemoryArbiterErrorV1::RootCountOverflow)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteGcPlanV1 {
    roots: Vec<RemoteResidentRootV1>,
    required_roots: usize,
}

impl RemoteGcPlanV1 {
    pub(crate) fn roots_v1(&self) -> &[RemoteResidentRootV1] {
        &self.roots
    }

    pub(crate) const fn required_roots_v1(&self) -> usize {
        self.required_roots
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RemoteNoOpGcBackoffV1 {
    store_revision: RemoteStoreRevisionV1,
    protection_revision: RemoteProtectionRevisionV1,
    target: GarbageCollectionTarget,
    next_allowed_frame: u64,
}

impl RemoteNoOpGcBackoffV1 {
    pub(crate) const fn store_revision_v1(self) -> RemoteStoreRevisionV1 {
        self.store_revision
    }

    pub(crate) const fn protection_revision_v1(self) -> RemoteProtectionRevisionV1 {
        self.protection_revision
    }

    pub(crate) const fn target_v1(self) -> GarbageCollectionTarget {
        self.target
    }

    pub(crate) const fn next_allowed_frame_v1(self) -> u64 {
        self.next_allowed_frame
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMemoryArbiterErrorV1 {
    ResidentRootCapExhausted,
    ResidentRootCapExceeded {
        resident_roots: usize,
        max_session_resident_physical_roots: usize,
    },
    RootCountOverflow,
    RootTargetUnreachable {
        required_roots: usize,
        eligible_roots: usize,
    },
    GcNotAllowedForInactive,
    GcBackoffDeferred {
        next_allowed_frame: u64,
    },
    CloseNotAllowedForForeground,
    UseStateMismatch {
        facade: RemoteRecordingUseStateV1,
        requested: RemoteRecordingUseStateV1,
    },
    NoCloseRequested,
    GcTurnCountOverflow,
    FrameCountOverflow,
    Reclaim(RemoteReclaimControllerErrorV1),
    Mutation(RemoteMutationArbiterErrorV1),
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteMemoryGcTurnV1 {
    mutation_turn: RemoteMutationTurnV1,
    plan: RemoteGcPlanV1,
    target: RemoteGcTargetV1,
}

impl RemoteMemoryGcTurnV1 {
    pub(crate) fn mutation_turn_v1(&self) -> &RemoteMutationTurnV1 {
        &self.mutation_turn
    }

    pub(crate) fn plan_v1(&self) -> &RemoteGcPlanV1 {
        &self.plan
    }

    pub(crate) const fn target_v1(&self) -> RemoteGcTargetV1 {
        self.target
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteMemoryCloseSubmissionV1 {
    pub(crate) ticket: PendingRemoteReclaimV1,
    pub(crate) mutation_close_id: RemoteMutationRequestIdV1,
}

struct RemoteMemoryArbiterStateV1 {
    resident_roots: Vec<RemoteResidentRootV1>,
    store_revision: RemoteStoreRevisionV1,
    protection_revision: RemoteProtectionRevisionV1,
    current_frame: u64,
    no_op_backoff: Option<RemoteNoOpGcBackoffV1>,
    gc_turn_invocations: u64,
    close_mutation_requested: bool,
    close_mutation_request_id: Option<RemoteMutationRequestIdV1>,
    session_ownership_released: bool,
}

impl RemoteMemoryArbiterStateV1 {
    const fn new_v1() -> Self {
        Self {
            resident_roots: Vec::new(),
            store_revision: RemoteStoreRevisionV1(0),
            protection_revision: RemoteProtectionRevisionV1(0),
            current_frame: 0,
            no_op_backoff: None,
            gc_turn_invocations: 0,
            close_mutation_requested: false,
            close_mutation_request_id: None,
            session_ownership_released: false,
        }
    }
}

/// Remote memory-pressure boundary that owns root selection, no-op backoff, and inactive Close
/// supersede. It never owns or publishes a physical Store handle.
pub(crate) struct RemoteMemoryArbiterV1<'a> {
    mutation: RemoteStoreMutationArbiterV1<'a>,
    facade: &'a RemotePresentationFacadeV1,
    state: RemoteMemoryArbiterStateV1,
}

impl<'a> RemoteMemoryArbiterV1<'a> {
    pub(crate) fn new_disarmed_v1(
        frame: PrivilegedViewerFrameContextV1,
        facade: &'a RemotePresentationFacadeV1,
    ) -> Self {
        let mutation = RemoteStoreMutationArbiterV1::new_disarmed_v1(frame, facade);
        Self {
            mutation,
            facade,
            state: RemoteMemoryArbiterStateV1::new_v1(),
        }
    }

    pub(crate) fn replace_resident_roots_v1(
        &mut self,
        roots: impl IntoIterator<Item = RemoteResidentRootV1>,
    ) {
        self.state.resident_roots = roots.into_iter().collect();
    }

    pub(crate) fn resident_roots_v1(&self) -> &[RemoteResidentRootV1] {
        &self.state.resident_roots
    }

    pub(crate) fn set_store_revision_v1(&mut self, revision: RemoteStoreRevisionV1) {
        self.state.store_revision = revision;
    }

    pub(crate) fn set_protection_revision_v1(&mut self, revision: RemoteProtectionRevisionV1) {
        self.state.protection_revision = revision;
    }

    pub(crate) fn set_frame_v1(&mut self, frame: u64) {
        self.state.current_frame = frame;
    }

    pub(crate) const fn store_revision_v1(&self) -> RemoteStoreRevisionV1 {
        self.state.store_revision
    }

    pub(crate) const fn protection_revision_v1(&self) -> RemoteProtectionRevisionV1 {
        self.state.protection_revision
    }

    pub(crate) const fn current_frame_v1(&self) -> u64 {
        self.state.current_frame
    }

    pub(crate) const fn no_op_backoff_v1(&self) -> Option<RemoteNoOpGcBackoffV1> {
        self.state.no_op_backoff
    }

    pub(crate) const fn gc_turn_invocations_v1(&self) -> u64 {
        self.state.gc_turn_invocations
    }

    pub(crate) const fn session_ownership_released_v1(&self) -> bool {
        self.state.session_ownership_released
    }

    pub(crate) fn plan_gc_v1(
        &self,
        target: RemoteGcTargetV1,
        reserved_new_roots: usize,
    ) -> Result<RemoteGcPlanV1, RemoteMemoryArbiterErrorV1> {
        let current_resident_roots = self
            .state
            .resident_roots
            .iter()
            .filter(|root| root.is_physically_resident_v1())
            .count();
        let required_roots =
            target.required_drop_root_count_v1(current_resident_roots, reserved_new_roots)?;

        let mut candidates = self
            .state
            .resident_roots
            .iter()
            .filter(|root| root.is_gc_eligible_v1())
            .cloned()
            .collect::<Vec<_>>();
        if candidates.len() < required_roots {
            return Err(RemoteMemoryArbiterErrorV1::RootTargetUnreachable {
                required_roots,
                eligible_roots: candidates.len(),
            });
        }

        candidates.sort_by(|left, right| {
            right
                .cursor_distance_v1()
                .cmp(&left.cursor_distance_v1())
                .then_with(|| left.identity_v1().cmp(&right.identity_v1()))
        });
        candidates.truncate(required_roots);
        Ok(RemoteGcPlanV1 {
            roots: candidates,
            required_roots,
        })
    }

    pub(crate) fn request_foreground_gc_v1(
        &mut self,
        target: RemoteGcTargetV1,
        reserved_new_roots: usize,
    ) -> Result<RemoteMemoryGcTurnV1, RemoteMemoryArbiterErrorV1> {
        self.ensure_foreground_gc_allowed_v1(target)?;

        let plan = self.plan_gc_v1(target, reserved_new_roots)?;
        let mutation_turn = self
            .mutation
            .request_garbage_collection_v1()
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)?;
        let next_invocations = self
            .state
            .gc_turn_invocations
            .checked_add(1)
            .ok_or(RemoteMemoryArbiterErrorV1::GcTurnCountOverflow)?;
        self.state.gc_turn_invocations = next_invocations;
        Ok(RemoteMemoryGcTurnV1 {
            mutation_turn,
            plan,
            target,
        })
    }

    pub(crate) fn complete_foreground_gc_v1(
        &mut self,
        turn: &RemoteMemoryGcTurnV1,
        query_visible_deletion: bool,
        no_op_backoff_frames: u64,
    ) -> Result<(), RemoteMemoryArbiterErrorV1> {
        self.mutation
            .complete_garbage_collection_v1(
                turn.mutation_turn.request_id_v1(),
                turn.mutation_turn.generation_v1(),
                query_visible_deletion,
            )
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)?;

        if query_visible_deletion {
            self.state.no_op_backoff = None;
        } else {
            let next_allowed_frame = self
                .state
                .current_frame
                .checked_add(no_op_backoff_frames)
                .ok_or(RemoteMemoryArbiterErrorV1::FrameCountOverflow)?;
            self.state.no_op_backoff = Some(RemoteNoOpGcBackoffV1 {
                store_revision: self.state.store_revision,
                protection_revision: self.state.protection_revision,
                target: turn.target_v1().target_v1(),
                next_allowed_frame,
            });
        }
        Ok(())
    }

    pub(crate) fn request_query_visible_insertion_v1(
        &mut self,
    ) -> Result<RemoteMutationTurnV1, RemoteMemoryArbiterErrorV1> {
        let turn = self
            .mutation
            .request_query_visible_insertion_v1()
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)?;
        self.state.no_op_backoff = None;
        Ok(turn)
    }

    pub(crate) fn begin_add_chunk_v1(
        &mut self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
    ) -> Result<RemoteMutationSubstateV1, RemoteMemoryArbiterErrorV1> {
        self.mutation
            .begin_add_chunk_v1(request_id, generation)
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)
    }

    pub(crate) fn complete_add_chunk_v1(
        &mut self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
        outcome: RemoteMutationAddChunkOutcomeV1,
    ) -> Result<RemoteMutationSubstateV1, RemoteMemoryArbiterErrorV1> {
        self.mutation
            .complete_add_chunk_v1(request_id, generation, outcome)
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)
    }

    pub(crate) fn acknowledge_partitions_resident_v1(
        &self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
    ) -> Result<(), RemoteMemoryArbiterErrorV1> {
        self.mutation
            .acknowledge_partitions_resident_v1(request_id, generation)
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)
    }

    pub(crate) fn complete_query_visible_insertion_v1(
        &mut self,
        request_id: RemoteMutationRequestIdV1,
        generation: u64,
    ) -> Result<(), RemoteMemoryArbiterErrorV1> {
        self.mutation
            .complete_query_visible_insertion_v1(request_id, generation)
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)
    }

    pub(crate) fn commit_presentation_v1(
        &self,
        committed_time: Option<CommittedPresentationTimeV1>,
    ) -> Result<(), RemoteMemoryArbiterErrorV1> {
        self.mutation
            .commit_presentation_v1(committed_time)
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)
    }

    pub(crate) fn request_inactive_pressure_close_v1(
        &mut self,
        use_state: RemoteRecordingUseStateV1,
    ) -> Result<RemoteMutationRequestIdV1, RemoteMemoryArbiterErrorV1> {
        self.ensure_inactive_close_latch_v1(use_state)
    }

    pub(crate) fn request_inactive_tokenized_pressure_close_v1(
        &mut self,
        controller: &mut WebRemoteMcapMemoryControllerV1,
        store_id: StoreId,
        slot_token: RemoteMcapSlotTokenV1,
        use_state: RemoteRecordingUseStateV1,
    ) -> Result<RemoteMemoryCloseSubmissionV1, RemoteMemoryArbiterErrorV1> {
        let mutation_close_id = self.ensure_inactive_close_latch_v1(use_state)?;
        let reclaim = controller
            .request_close_v1(store_id, slot_token)
            .map_err(RemoteMemoryArbiterErrorV1::Reclaim)?;
        Ok(RemoteMemoryCloseSubmissionV1 {
            ticket: reclaim.ticket,
            mutation_close_id,
        })
    }

    fn ensure_inactive_close_latch_v1(
        &mut self,
        use_state: RemoteRecordingUseStateV1,
    ) -> Result<RemoteMutationRequestIdV1, RemoteMemoryArbiterErrorV1> {
        let facade_use_state = self.facade.use_state_v1();
        if facade_use_state == RemoteRecordingUseStateV1::Foreground {
            return Err(RemoteMemoryArbiterErrorV1::CloseNotAllowedForForeground);
        }
        if use_state != facade_use_state {
            return Err(RemoteMemoryArbiterErrorV1::UseStateMismatch {
                facade: facade_use_state,
                requested: use_state,
            });
        }
        if self.state.close_mutation_requested {
            return Ok(self
                .state
                .close_mutation_request_id
                .expect("a requested close always has a matching mutation request id"));
        }

        let request_id = self
            .mutation
            .request_close_v1()
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)?;
        self.state.close_mutation_requested = true;
        self.state.close_mutation_request_id = Some(request_id);
        self.release_session_ownership_v1();
        Ok(request_id)
    }

    pub(crate) fn drive_inactive_pressure_close_v1(
        &mut self,
    ) -> Result<RemoteMutationCloseProgressV1, RemoteMemoryArbiterErrorV1> {
        if !self.state.close_mutation_requested {
            return Err(RemoteMemoryArbiterErrorV1::NoCloseRequested);
        }
        let progress = self
            .mutation
            .drive_close_v1()
            .map_err(RemoteMemoryArbiterErrorV1::Mutation)?;
        if progress == RemoteMutationCloseProgressV1::Complete {
            self.state.close_mutation_requested = false;
            self.state.close_mutation_request_id = None;
        }
        Ok(progress)
    }

    fn ensure_foreground_gc_allowed_v1(
        &mut self,
        target: RemoteGcTargetV1,
    ) -> Result<(), RemoteMemoryArbiterErrorV1> {
        if self.facade.use_state_v1() != RemoteRecordingUseStateV1::Foreground {
            return Err(RemoteMemoryArbiterErrorV1::GcNotAllowedForInactive);
        }

        let Some(backoff) = self.state.no_op_backoff else {
            return Ok(());
        };
        if backoff.store_revision != self.state.store_revision
            || backoff.protection_revision != self.state.protection_revision
            || !gc_targets_equal_v1(backoff.target, target.target_v1())
        {
            self.state.no_op_backoff = None;
            return Ok(());
        }
        if self.state.current_frame < backoff.next_allowed_frame {
            return Err(RemoteMemoryArbiterErrorV1::GcBackoffDeferred {
                next_allowed_frame: backoff.next_allowed_frame,
            });
        }
        self.state.no_op_backoff = None;
        Ok(())
    }

    fn release_session_ownership_v1(&mut self) {
        self.state.resident_roots.clear();
        self.state.no_op_backoff = None;
        self.state.session_ownership_released = true;
    }
}

impl ConsumerStorageFreeV1 for GarbageCollectionTarget {}
impl ConsumerStorageFreeV1 for RemoteReclaimRequestIdV1 {}
impl ConsumerStorageFreeV1 for RemoteStoreRevisionV1 {}
impl ConsumerStorageFreeV1 for RemoteProtectionRevisionV1 {}
impl ConsumerStorageFreeV1 for RemoteMcapSlotTokenV1 {}
impl ConsumerStorageFreeV1 for RemoteReclaimControllerErrorV1 {}
impl ConsumerStorageFreeV1 for RemoteProcessMemorySampleV1
where
    u64: ConsumerStorageFreeV1,
    bool: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for PendingRemoteReclaimV1
where
    RemoteReclaimRequestIdV1: ConsumerStorageFreeV1,
    StoreId: ConsumerStorageFreeV1,
    GarbageCollectionTarget: ConsumerStorageFreeV1,
    RemoteStoreRevisionV1: ConsumerStorageFreeV1,
    RemoteProtectionRevisionV1: ConsumerStorageFreeV1,
    RemoteMcapSlotTokenV1: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for PurgeOutcomeV1
where
    u64: ConsumerStorageFreeV1,
    Option<PendingRemoteReclaimV1>: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteMutationReclaimV1
where
    u64: ConsumerStorageFreeV1,
    Option<PendingRemoteReclaimV1>: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteRootIdentityV1 {}
impl ConsumerStorageFreeV1 for RemoteRootResidencyV1 {}
impl ConsumerStorageFreeV1 for RemoteRootProtectionClassV1 where bool: ConsumerStorageFreeV1 {}
impl ConsumerStorageFreeV1 for RemoteResidentRootV1
where
    RemoteRootIdentityV1: ConsumerStorageFreeV1,
    RemoteRootResidencyV1: ConsumerStorageFreeV1,
    Option<u64>: ConsumerStorageFreeV1,
    RemoteRootProtectionClassV1: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteGcTargetV1
where
    usize: ConsumerStorageFreeV1,
    GarbageCollectionTarget: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteGcPlanV1
where
    Vec<RemoteResidentRootV1>: ConsumerStorageFreeV1,
    usize: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteNoOpGcBackoffV1
where
    RemoteStoreRevisionV1: ConsumerStorageFreeV1,
    RemoteProtectionRevisionV1: ConsumerStorageFreeV1,
    GarbageCollectionTarget: ConsumerStorageFreeV1,
    u64: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteMemoryArbiterErrorV1
where
    usize: ConsumerStorageFreeV1,
    u64: ConsumerStorageFreeV1,
    RemoteReclaimControllerErrorV1: ConsumerStorageFreeV1,
    RemoteMutationArbiterErrorV1: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteMemoryGcTurnV1
where
    RemoteMutationTurnV1: ConsumerStorageFreeV1,
    RemoteGcPlanV1: ConsumerStorageFreeV1,
    RemoteGcTargetV1: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteMemoryCloseSubmissionV1
where
    PendingRemoteReclaimV1: ConsumerStorageFreeV1,
    RemoteMutationRequestIdV1: ConsumerStorageFreeV1,
{
}
impl ConsumerStorageFreeV1 for RemoteMemoryArbiterStateV1
where
    Vec<RemoteResidentRootV1>: ConsumerStorageFreeV1,
    RemoteStoreRevisionV1: ConsumerStorageFreeV1,
    RemoteProtectionRevisionV1: ConsumerStorageFreeV1,
    u64: ConsumerStorageFreeV1,
    Option<RemoteNoOpGcBackoffV1>: ConsumerStorageFreeV1,
    bool: ConsumerStorageFreeV1,
    Option<RemoteMutationRequestIdV1>: ConsumerStorageFreeV1,
{
}
impl<'a> ConsumerStorageFreeV1 for RemoteMemoryArbiterV1<'a>
where
    RemoteStoreMutationArbiterV1<'a>: ConsumerStorageFreeV1,
    &'a RemotePresentationFacadeV1: ConsumerStorageFreeV1,
    RemoteMemoryArbiterStateV1: ConsumerStorageFreeV1,
{
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
    use std::rc::Rc;

    use re_chunk::{TimeInt, TimelineName};
    use re_entity_db::{EntityDb, StoreBundle};
    use re_log_types::AbsoluteTimeRange;
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;
    use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
    use crate::web_remote_mcap_query::{
        GatedRecordingQueryFacadeV1 as _, RemoteCanonicalIndexedExtentV1, RemoteLoadedCoverageV1,
    };

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

    fn open_facade(use_state: RemoteRecordingUseStateV1) -> RemotePresentationFacadeV1 {
        let facade = RemotePresentationFacadeV1::new_for_test_v1(
            71,
            store_id(),
            use_state,
            RemoteLoadedCoverageV1::new_v1(RemoteCanonicalIndexedExtentV1::Known(extent()), false),
            1,
            Rc::new(|| {}),
        );
        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged.set_opening_static_satisfied_v1(true);
        privileged
            .commit_initial_presentation_v1(Some(committed_time(3)))
            .expect("open facade");
        facade
    }

    fn memory_arbiter(facade: &RemotePresentationFacadeV1) -> RemoteMemoryArbiterV1<'_> {
        RemoteMemoryArbiterV1::new_disarmed_v1(
            PrivilegedViewerFrameContextV1::new_for_test_v1(),
            facade,
        )
    }

    fn root(identity: u64, distance: u64) -> RemoteResidentRootV1 {
        RemoteResidentRootV1::new_v1(
            RemoteRootIdentityV1::new_v1(identity),
            RemoteRootResidencyV1::FullyLoaded,
            Some(distance),
            RemoteRootProtectionClassV1::unprotected_v1(),
        )
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
        let reclaim = RemoteMutationReclaimV1::from_purge_outcome_v1(outcome.clone());
        assert_eq!(reclaim.freed_now_bytes_v1(), 37);
        assert_eq!(
            reclaim
                .pending_remote_reclaim_v1()
                .expect("pending gc")
                .request_id_v1(),
            request_id
        );
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

    #[test]
    fn resident_root_cap_plan_selects_complete_furthest_roots() {
        let facade = open_facade(RemoteRecordingUseStateV1::Foreground);
        let arbiter = memory_arbiter(&facade);

        let current_closure = RemoteResidentRootV1::new_v1(
            RemoteRootIdentityV1::new_v1(4),
            RemoteRootResidencyV1::FullyLoaded,
            Some(50),
            RemoteRootProtectionClassV1::current_closure_v1(),
        );
        let in_transit = RemoteResidentRootV1::new_v1(
            RemoteRootIdentityV1::new_v1(2),
            RemoteRootResidencyV1::InTransit,
            Some(100),
            RemoteRootProtectionClassV1::unprotected_v1(),
        );
        let mut arbiter = arbiter;
        arbiter.replace_resident_roots_v1([
            root(1, 1),
            in_transit,
            root(3, 9),
            current_closure,
            RemoteResidentRootV1::new_v1(
                RemoteRootIdentityV1::new_v1(5),
                RemoteRootResidencyV1::FullyLoaded,
                Some(70),
                RemoteRootProtectionClassV1::pin_v1(),
            ),
        ]);

        let target =
            RemoteGcTargetV1::new_v1(5, GarbageCollectionTarget::Everything).expect("valid cap");
        let plan = arbiter.plan_gc_v1(target, 1).expect("one eligible root");
        assert_eq!(plan.required_roots_v1(), 1);
        assert_eq!(
            plan.roots_v1()
                .iter()
                .map(RemoteResidentRootV1::identity_v1)
                .collect::<Vec<_>>(),
            vec![RemoteRootIdentityV1::new_v1(3)]
        );

        let unreachable = arbiter
            .plan_gc_v1(target, 3)
            .expect_err("only two roots are gc eligible");
        assert_eq!(
            unreachable,
            RemoteMemoryArbiterErrorV1::RootTargetUnreachable {
                required_roots: 3,
                eligible_roots: 2,
            }
        );
    }

    #[test]
    fn resident_root_cap_checked_preflight_rejects_overflow_and_limit() {
        let target = RemoteGcTargetV1::new_v1(2, GarbageCollectionTarget::DropAtLeastBytes(10))
            .expect("valid cap");
        assert_eq!(
            target.checked_resident_root_count_v1(usize::MAX, 1),
            Err(RemoteMemoryArbiterErrorV1::RootCountOverflow)
        );
        assert_eq!(
            target.checked_resident_root_count_v1(2, 1),
            Err(RemoteMemoryArbiterErrorV1::ResidentRootCapExceeded {
                resident_roots: 3,
                max_session_resident_physical_roots: 2,
            })
        );
        assert_eq!(
            RemoteGcTargetV1::new_v1(0, GarbageCollectionTarget::Everything).err(),
            Some(RemoteMemoryArbiterErrorV1::ResidentRootCapExhausted)
        );
    }

    #[test]
    fn no_op_gc_keeps_epoch_and_uses_bound_backoff() {
        let facade = open_facade(RemoteRecordingUseStateV1::Foreground);
        let before_lease = facade.try_lease_v1().expect("open lease");
        let before_snapshot = before_lease.snapshot.clone();
        drop(before_lease);

        let mut arbiter = memory_arbiter(&facade);
        arbiter.replace_resident_roots_v1([root(1, 1)]);
        arbiter.set_store_revision_v1(RemoteStoreRevisionV1(7));
        arbiter.set_protection_revision_v1(RemoteProtectionRevisionV1(9));
        arbiter.set_frame_v1(10);

        let target =
            RemoteGcTargetV1::new_v1(2, GarbageCollectionTarget::Everything).expect("valid cap");
        let turn = arbiter
            .request_foreground_gc_v1(target, 0)
            .expect("initial no-op gc");
        assert_eq!(arbiter.gc_turn_invocations_v1(), 1);
        arbiter
            .complete_foreground_gc_v1(&turn, false, 5)
            .expect("no-op reopen");
        assert_eq!(
            arbiter.no_op_backoff_v1().unwrap().next_allowed_frame_v1(),
            15
        );
        assert!(facade.is_current_v1(&before_snapshot));

        arbiter.set_frame_v1(14);
        assert_eq!(
            arbiter.request_foreground_gc_v1(target, 0).err(),
            Some(RemoteMemoryArbiterErrorV1::GcBackoffDeferred {
                next_allowed_frame: 15,
            })
        );
        assert_eq!(arbiter.gc_turn_invocations_v1(), 1);

        arbiter.set_frame_v1(15);
        let deletion_turn = arbiter
            .request_foreground_gc_v1(target, 0)
            .expect("backoff elapsed");
        assert_eq!(arbiter.gc_turn_invocations_v1(), 2);
        arbiter
            .complete_foreground_gc_v1(&deletion_turn, true, 0)
            .expect("deletion reopen");
        assert!(!facade.is_current_v1(&before_snapshot));
        assert!(arbiter.no_op_backoff_v1().is_none());
    }

    #[test]
    fn inactive_memory_pressure_can_only_close_and_never_invokes_gc() {
        let facade = open_facade(RemoteRecordingUseStateV1::Inactive);
        let mut arbiter = memory_arbiter(&facade);
        arbiter.replace_resident_roots_v1([root(1, 1)]);
        arbiter.set_store_revision_v1(RemoteStoreRevisionV1(2));
        arbiter.set_protection_revision_v1(RemoteProtectionRevisionV1(3));

        let target =
            RemoteGcTargetV1::new_v1(3, GarbageCollectionTarget::Everything).expect("valid cap");
        assert_eq!(
            arbiter.request_foreground_gc_v1(target, 0).err(),
            Some(RemoteMemoryArbiterErrorV1::GcNotAllowedForInactive)
        );
        assert_eq!(arbiter.gc_turn_invocations_v1(), 0);

        let close_id = arbiter
            .request_inactive_pressure_close_v1(RemoteRecordingUseStateV1::Inactive)
            .expect("inactive close");
        assert_eq!(
            arbiter.request_inactive_pressure_close_v1(RemoteRecordingUseStateV1::Inactive),
            Ok(close_id)
        );
        assert!(arbiter.session_ownership_released_v1());
        assert!(arbiter.resident_roots_v1().is_empty());
        assert_eq!(
            arbiter.drive_inactive_pressure_close_v1(),
            Ok(RemoteMutationCloseProgressV1::TerminalGateStarted)
        );
        assert_eq!(
            arbiter.drive_inactive_pressure_close_v1(),
            Ok(RemoteMutationCloseProgressV1::Complete)
        );
        assert_eq!(arbiter.gc_turn_invocations_v1(), 0);
    }

    #[test]
    fn inactive_tokenized_close_reuses_controller_ticket_and_mutation_latch() {
        let facade = open_facade(RemoteRecordingUseStateV1::CatalogOnly);
        let mut arbiter = memory_arbiter(&facade);
        let mut controller = controller(true);
        let store_id = store_id();
        let slot_token = RemoteMcapSlotTokenV1(17);

        let first = arbiter
            .request_inactive_tokenized_pressure_close_v1(
                &mut controller,
                store_id.clone(),
                slot_token,
                RemoteRecordingUseStateV1::CatalogOnly,
            )
            .expect("first close");
        let second = arbiter
            .request_inactive_tokenized_pressure_close_v1(
                &mut controller,
                store_id,
                slot_token,
                RemoteRecordingUseStateV1::CatalogOnly,
            )
            .expect("reused close");

        assert_eq!(first.ticket.request_id_v1(), second.ticket.request_id_v1());
        assert_eq!(first.mutation_close_id, second.mutation_close_id);
        assert!(arbiter.session_ownership_released_v1());
        assert_eq!(
            arbiter.drive_inactive_pressure_close_v1(),
            Ok(RemoteMutationCloseProgressV1::TerminalGateStarted)
        );
        assert_eq!(
            arbiter.drive_inactive_pressure_close_v1(),
            Ok(RemoteMutationCloseProgressV1::Complete)
        );
    }

    #[test]
    fn inactive_pressure_close_rejects_foreground_facade_atomically() {
        let facade = open_facade(RemoteRecordingUseStateV1::Foreground);
        let mut arbiter = memory_arbiter(&facade);
        let mut controller = controller(true);

        let result = arbiter.request_inactive_tokenized_pressure_close_v1(
            &mut controller,
            store_id(),
            RemoteMcapSlotTokenV1(17),
            RemoteRecordingUseStateV1::Inactive,
        );

        assert_eq!(
            result.err(),
            Some(RemoteMemoryArbiterErrorV1::CloseNotAllowedForForeground)
        );
        assert!(
            controller
                .purge_outcome_v1(0)
                .pending_remote_reclaim
                .is_none()
        );
        assert!(!arbiter.session_ownership_released_v1());

        assert_eq!(
            arbiter
                .request_inactive_pressure_close_v1(RemoteRecordingUseStateV1::CatalogOnly)
                .err(),
            Some(RemoteMemoryArbiterErrorV1::CloseNotAllowedForForeground)
        );
        assert!(
            controller
                .purge_outcome_v1(0)
                .pending_remote_reclaim
                .is_none()
        );
        assert!(!arbiter.session_ownership_released_v1());
    }

    #[test]
    fn inactive_pressure_close_rejects_facade_use_state_mismatch_without_mutation() {
        let facade = open_facade(RemoteRecordingUseStateV1::CatalogOnly);
        let mut arbiter = memory_arbiter(&facade);
        let mut controller = controller(true);

        let result = arbiter.request_inactive_tokenized_pressure_close_v1(
            &mut controller,
            store_id(),
            RemoteMcapSlotTokenV1(17),
            RemoteRecordingUseStateV1::Inactive,
        );

        assert_eq!(
            result.err(),
            Some(RemoteMemoryArbiterErrorV1::UseStateMismatch {
                facade: RemoteRecordingUseStateV1::CatalogOnly,
                requested: RemoteRecordingUseStateV1::Inactive,
            })
        );
        assert!(
            controller
                .purge_outcome_v1(0)
                .pending_remote_reclaim
                .is_none()
        );
        assert!(!arbiter.session_ownership_released_v1());
        assert_eq!(
            arbiter.drive_inactive_pressure_close_v1().err(),
            Some(RemoteMemoryArbiterErrorV1::NoCloseRequested)
        );
    }

    #[test]
    fn inactive_close_supersedes_suspended_insertion_and_blocks_reopen_work() {
        let facade = open_facade(RemoteRecordingUseStateV1::Inactive);
        let mut arbiter = memory_arbiter(&facade);
        let insertion = arbiter
            .request_query_visible_insertion_v1()
            .expect("insertion turn");
        let request_id = insertion.request_id_v1();
        let generation = insertion.generation_v1();

        let close_id = arbiter
            .request_inactive_pressure_close_v1(RemoteRecordingUseStateV1::Inactive)
            .expect("inactive close supersede");
        assert!(arbiter.session_ownership_released_v1());
        assert_eq!(
            arbiter.request_inactive_pressure_close_v1(RemoteRecordingUseStateV1::Inactive),
            Ok(close_id)
        );
        assert_eq!(
            arbiter.begin_add_chunk_v1(request_id, generation),
            Err(RemoteMemoryArbiterErrorV1::Mutation(
                RemoteMutationArbiterErrorV1::CloseAlreadyRequested
            ))
        );
        assert_eq!(
            arbiter.acknowledge_partitions_resident_v1(request_id, generation),
            Err(RemoteMemoryArbiterErrorV1::Mutation(
                RemoteMutationArbiterErrorV1::CloseAlreadyRequested
            ))
        );
        assert_eq!(
            arbiter.complete_query_visible_insertion_v1(request_id, generation),
            Err(RemoteMemoryArbiterErrorV1::Mutation(
                RemoteMutationArbiterErrorV1::CloseAlreadyRequested
            ))
        );
        assert_eq!(
            arbiter.commit_presentation_v1(Some(committed_time(9))),
            Err(RemoteMemoryArbiterErrorV1::Mutation(
                RemoteMutationArbiterErrorV1::CloseAlreadyRequested
            ))
        );

        assert_eq!(
            arbiter.drive_inactive_pressure_close_v1(),
            Ok(RemoteMutationCloseProgressV1::TerminalGateStarted)
        );
        assert_eq!(
            arbiter.drive_inactive_pressure_close_v1(),
            Ok(RemoteMutationCloseProgressV1::Complete)
        );
        assert!(matches!(
            facade.try_lease_v1(),
            Err(crate::web_remote_mcap_query::PresentationLeaseUnavailableV1::RecordingNotForeground)
        ));
    }

    #[test]
    fn remote_memory_arbiter_graph_is_storage_free_by_structure() {
        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StoreBundle);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StorageEngine);

        assert_storage_free_v1::<RemoteMemoryArbiterV1<'static>>();
        assert_storage_free_v1::<RemoteMutationReclaimV1>();
        assert_storage_free_v1::<RemoteResidentRootV1>();
        assert_storage_free_v1::<RemoteGcTargetV1>();
        assert_storage_free_v1::<RemoteGcPlanV1>();
        assert_storage_free_v1::<RemoteMemoryGcTurnV1>();
        assert_storage_free_v1::<RemoteMemoryCloseSubmissionV1>();
        assert_storage_free_v1::<RemoteMemoryArbiterErrorV1>();
    }
}
