//! Page-hidden ownership for remote-MCAP insertion and GC turns.
//!
//! This is a deliberately small, production-disarmed seam.  It models the ownership transfer
//! performed by the remote Store arbiter without changing the native or ordinary Store mutation
//! schedulers.  In particular, hiding a page moves an in-flight safe-point turn into a suspended
//! owner; it never treats the turn as stale work and drops it.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteMutationKindV1 {
    Insertion,
    Gc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteMutationSafePointV1 {
    WaitingForLeases,
    BeforeFirstAddChunk,
    BetweenAddChunks,
    AfterLastAddChunkBeforeAck,
    GcWaitingForLeases,
    AfterGcBeforeReopen,
}

impl RemoteMutationSafePointV1 {
    fn can_suspend(self) -> bool {
        matches!(
            self,
            Self::WaitingForLeases
                | Self::BeforeFirstAddChunk
                | Self::BetweenAddChunks
                | Self::AfterLastAddChunkBeforeAck
                | Self::GcWaitingForLeases
                | Self::AfterGcBeforeReopen
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFacadeSnapshotV1 {
    pub facade_revision: u64,
    pub content_revision: u64,
    pub protection_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteMutationEffectsV1 {
    pub staged_events: u32,
    pub residency_events: u32,
    pub presentation_epoch_effects: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteMutationOwnershipV1 {
    pub kind: RemoteMutationKindV1,
    pub safe_point: RemoteMutationSafePointV1,
    pub frozen_commit_set: Vec<u64>,
    pub facade: RemoteFacadeSnapshotV1,
    pub pins: Vec<u64>,
    pub reservation_bytes: u64,
    pub effects: RemoteMutationEffectsV1,
    physical_mutation_started: bool,
    acked: bool,
}

impl RemoteMutationOwnershipV1 {
    pub fn new(
        kind: RemoteMutationKindV1,
        safe_point: RemoteMutationSafePointV1,
        frozen_commit_set: Vec<u64>,
        facade: RemoteFacadeSnapshotV1,
        pins: Vec<u64>,
        reservation_bytes: u64,
        effects: RemoteMutationEffectsV1,
    ) -> Self {
        Self {
            kind,
            safe_point,
            frozen_commit_set,
            facade,
            pins,
            reservation_bytes,
            effects,
            physical_mutation_started: false,
            acked: false,
        }
    }

    /// Marks the first physical add/delete as having started.
    pub fn mark_physical_mutation_started(&mut self) {
        self.physical_mutation_started = true;
    }

    pub fn physical_mutation_started(&self) -> bool {
        self.physical_mutation_started
    }

    /// A presentation acknowledgement is one-shot, including after a resume.
    pub fn acknowledge_once(&mut self) -> bool {
        if self.acked {
            false
        } else {
            self.acked = true;
            true
        }
    }

    pub fn acknowledged(&self) -> bool {
        self.acked
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RemoteMutationStateV1 {
    Visible {
        epoch: u64,
    },
    Active {
        epoch: u64,
        owner: RemoteMutationOwnershipV1,
    },
    Hidden {
        epoch: u64,
        owner: RemoteMutationOwnershipV1,
        resume_nonce: u64,
    },
    Poisoned {
        epoch: u64,
        owner: RemoteMutationOwnershipV1,
    },
    Terminated {
        epoch: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteMutationResumeResultV1 {
    Rebound,
    Rejected,
    Poisoned,
}

/// Store-scoped page-control seam for one remote mutation turn.
pub struct RemoteMutationSuspensionV1 {
    state: RemoteMutationStateV1,
    next_resume_nonce: u64,
    last_resume_nonce: u64,
}

impl Default for RemoteMutationSuspensionV1 {
    fn default() -> Self {
        Self {
            state: RemoteMutationStateV1::Visible { epoch: 0 },
            next_resume_nonce: 1,
            last_resume_nonce: 0,
        }
    }
}

impl fmt::Debug for RemoteMutationSuspensionV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteMutationSuspensionV1")
            .field("state", &self.state)
            .field("last_resume_nonce", &self.last_resume_nonce)
            .finish()
    }
}

impl RemoteMutationSuspensionV1 {
    pub fn begin_v1(&mut self, owner: RemoteMutationOwnershipV1) -> bool {
        let RemoteMutationStateV1::Visible { epoch } = self.state else {
            return false;
        };
        self.state = RemoteMutationStateV1::Active { epoch, owner };
        true
    }

    pub fn begin_visible_v1(&mut self, owner: RemoteMutationOwnershipV1) -> bool {
        self.begin_v1(owner)
    }

    /// Records that a synchronous physical add/delete has started before a page signal.
    pub fn mark_physical_mutation_started_v1(&mut self) -> bool {
        let owner = match &mut self.state {
            RemoteMutationStateV1::Active { owner, .. }
            | RemoteMutationStateV1::Hidden { owner, .. } => owner,
            _ => return false,
        };
        owner.mark_physical_mutation_started();
        true
    }

    pub fn hide_v1(&mut self, resulting_epoch: u64) -> bool {
        let RemoteMutationStateV1::Active { epoch, .. } = self.state else {
            return false;
        };
        if resulting_epoch != epoch.saturating_add(1) {
            return false;
        }
        true
    }

    /// Move the active owner to page-hidden storage.  Only explicit safe points are suspendable.
    pub fn suspend_v1(&mut self, resulting_epoch: u64) -> bool {
        let state = std::mem::replace(
            &mut self.state,
            RemoteMutationStateV1::Terminated { epoch: 0 },
        );
        let RemoteMutationStateV1::Active { epoch, owner } = state else {
            self.state = state;
            return false;
        };
        if resulting_epoch != epoch.saturating_add(1) || !owner.safe_point.can_suspend() {
            self.state = RemoteMutationStateV1::Active { epoch, owner };
            return false;
        }
        let resume_nonce = self.next_resume_nonce;
        self.next_resume_nonce = self.next_resume_nonce.saturating_add(1);
        self.state = RemoteMutationStateV1::Hidden {
            epoch: resulting_epoch,
            owner,
            resume_nonce,
        };
        true
    }

    pub fn resume_v1(
        &mut self,
        expected_epoch: u64,
        resume_nonce: u64,
        facade: RemoteFacadeSnapshotV1,
    ) -> RemoteMutationResumeResultV1 {
        let state = std::mem::replace(
            &mut self.state,
            RemoteMutationStateV1::Terminated { epoch: 0 },
        );
        let RemoteMutationStateV1::Hidden {
            epoch,
            owner,
            resume_nonce: expected_nonce,
        } = state
        else {
            self.state = state;
            return RemoteMutationResumeResultV1::Rejected;
        };
        if expected_epoch != epoch
            || resume_nonce != expected_nonce
            || resume_nonce <= self.last_resume_nonce
        {
            self.state = RemoteMutationStateV1::Hidden {
                epoch,
                owner,
                resume_nonce: expected_nonce,
            };
            return RemoteMutationResumeResultV1::Rejected;
        }
        self.last_resume_nonce = resume_nonce;
        if owner.facade != facade {
            if owner.physical_mutation_started() {
                self.state = RemoteMutationStateV1::Poisoned { epoch, owner };
                return RemoteMutationResumeResultV1::Poisoned;
            }
            // Before the first physical mutation the staged ownership can be terminally
            // discarded.  We do not reopen the old facade or publish an acknowledgement.
            self.state = RemoteMutationStateV1::Terminated { epoch };
            return RemoteMutationResumeResultV1::Rejected;
        }
        self.state = RemoteMutationStateV1::Visible { epoch };
        RemoteMutationResumeResultV1::Rebound
    }

    pub fn terminate_v1(&mut self, epoch: u64) -> bool {
        let current = match self.state {
            RemoteMutationStateV1::Visible { epoch }
            | RemoteMutationStateV1::Active { epoch, .. }
            | RemoteMutationStateV1::Hidden { epoch, .. }
            | RemoteMutationStateV1::Poisoned { epoch, .. }
            | RemoteMutationStateV1::Terminated { epoch } => epoch,
        };
        if epoch < current {
            return false;
        }
        self.state = RemoteMutationStateV1::Terminated { epoch };
        true
    }

    pub fn is_poisoned_v1(&self) -> bool {
        matches!(self.state, RemoteMutationStateV1::Poisoned { .. })
    }

    pub fn suspended_owner_v1(&self) -> Option<&RemoteMutationOwnershipV1> {
        match &self.state {
            RemoteMutationStateV1::Hidden { owner, .. }
            | RemoteMutationStateV1::Poisoned { owner, .. } => Some(owner),
            _ => None,
        }
    }

    pub fn suspended_resume_nonce_v1(&self) -> Option<u64> {
        match self.state {
            RemoteMutationStateV1::Hidden { resume_nonce, .. } => Some(resume_nonce),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> RemoteMutationOwnershipV1 {
        RemoteMutationOwnershipV1::new(
            RemoteMutationKindV1::Insertion,
            RemoteMutationSafePointV1::BetweenAddChunks,
            vec![1, 2],
            RemoteFacadeSnapshotV1 {
                facade_revision: 3,
                content_revision: 4,
                protection_revision: 5,
            },
            vec![9],
            42,
            RemoteMutationEffectsV1 {
                staged_events: 1,
                residency_events: 2,
                presentation_epoch_effects: 3,
            },
        )
    }

    #[test]
    fn hidden_moves_owner_and_resume_is_one_shot() {
        let mut arbiter = RemoteMutationSuspensionV1::default();
        assert!(arbiter.begin_v1(owner()));
        assert!(arbiter.suspend_v1(1));
        assert!(arbiter.suspended_owner_v1().is_some());
        let facade = arbiter.suspended_owner_v1().unwrap().facade.clone();
        let nonce = arbiter.suspended_resume_nonce_v1().unwrap();
        assert_eq!(
            arbiter.resume_v1(1, nonce, facade.clone()),
            RemoteMutationResumeResultV1::Rebound
        );
        assert_eq!(
            arbiter.resume_v1(1, nonce, facade),
            RemoteMutationResumeResultV1::Rejected
        );
    }

    #[test]
    fn revalidation_after_physical_write_poisoned_without_ack_or_reopen() {
        let mut arbiter = RemoteMutationSuspensionV1::default();
        assert!(arbiter.begin_v1(owner()));
        assert!(arbiter.mark_physical_mutation_started_v1());
        assert!(arbiter.suspend_v1(1));
        // The owner remains move-only and records that a physical write happened before hide.
        // A real arbiter marks this before the transition; this test uses the ownership API.
        let facade = RemoteFacadeSnapshotV1 {
            facade_revision: 99,
            content_revision: 4,
            protection_revision: 5,
        };
        let nonce = arbiter.suspended_resume_nonce_v1().unwrap();
        assert_eq!(
            arbiter.resume_v1(1, nonce, facade),
            RemoteMutationResumeResultV1::Poisoned
        );
        assert!(arbiter.is_poisoned_v1());
    }

    #[test]
    fn duplicate_ack_is_rejected() {
        let mut owner = owner();
        assert!(owner.acknowledge_once());
        assert!(!owner.acknowledge_once());
    }
}
