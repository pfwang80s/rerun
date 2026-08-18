//! Production-disarmed remote-MCAP pagehide/freeze teardown.
//!
//! This module owns only the remote work admitted by the page-control seam. It models the
//! synchronous teardown of remote tokens, Fetch, CPU, mutation, facade, Store capability, secret,
//! and reservation owners when the browser reports `pagehide` or Chrome `freeze`.
//!
//! Viewer, canvas, non-MCAP Stores, receivers, observers, and handlers are deliberately outside
//! this type. A terminated page never revives an old remote token; the host can explicitly reopen
//! remote MCAP with a fresh manager identity.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Upper bound for concurrently admitted remote owners in one page teardown seam.
pub const MAX_REMOTE_PAGE_TEARDOWN_OWNERS_V1: usize = 64;

/// Opaque page-control identity carried by page signals.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RemotePageTeardownTokenV1 {
    slot: u64,
    token: u64,
    execution_epoch: u64,
    issuer: u64,
}

impl fmt::Debug for RemotePageTeardownTokenV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemotePageTeardownTokenV1(..)")
    }
}

/// Opaque owner identity for one admitted remote-MCAP work owner.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RemotePageOwnerTokenV1 {
    slot: u64,
    token: u64,
    execution_epoch: u64,
    issuer: u64,
}

impl fmt::Debug for RemotePageOwnerTokenV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemotePageOwnerTokenV1(..)")
    }
}

/// Remote work owner category covered by page teardown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RemotePageOwnerKindV1 {
    RemoteToken,
    Fetch,
    Cpu,
    Mutation,
    Facade,
    StoreCapability,
    Secret,
    Reservation,
}

/// Browser page signal accepted by the teardown seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePageSignalKindV1 {
    PageHide,
    Freeze,
    PageShow,
    Resume,
}

/// One page-control signal carrying an execution epoch and remote token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemotePageSignalV1 {
    kind: RemotePageSignalKindV1,
    execution_epoch: u64,
    remote_token: RemotePageTeardownTokenV1,
}

impl RemotePageSignalV1 {
    pub const fn kind_v1(&self) -> RemotePageSignalKindV1 {
        self.kind
    }

    pub const fn execution_epoch_v1(&self) -> u64 {
        self.execution_epoch
    }
}

/// One callback result attempting to release a previously admitted remote owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemotePageTeardownCallbackV1 {
    owner: RemotePageOwnerTokenV1,
    execution_epoch: u64,
}

impl RemotePageTeardownCallbackV1 {
    pub const fn execution_epoch_v1(&self) -> u64 {
        self.execution_epoch
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePageSignalOutcomeV1 {
    Terminated {
        torn_down: usize,
        terminal_token: RemotePageTeardownTokenV1,
    },
    RevivalRejected,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePageCallbackOutcomeV1 {
    Released(RemotePageOwnerKindV1),
    RejectedStale,
    RejectedTerminated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePageTeardownErrorV1 {
    AlreadyActive,
    InvalidOwner,
    OwnerLimitExceeded,
    Overflow,
    Terminated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemotePageTeardownPhaseV1 {
    Active,
    Terminated,
}

#[derive(Clone, Copy, Debug)]
struct Owner {
    token: u64,
    execution_epoch: u64,
    kind: RemotePageOwnerKindV1,
}

static NEXT_MANAGER_ISSUER_V1: AtomicU64 = AtomicU64::new(1);

fn next_manager_issuer_v1() -> u64 {
    NEXT_MANAGER_ISSUER_V1
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |issuer| {
            issuer.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("RemotePageTeardownV1 issuer allocator exhausted"))
}

/// Remote-only pagehide/freeze teardown coordinator.
pub struct RemotePageTeardownV1 {
    phase: RemotePageTeardownPhaseV1,
    execution_epoch: u64,
    next_slot: u64,
    next_token: u64,
    issuer: u64,
    control_token: RemotePageTeardownTokenV1,
    terminal_token: Option<RemotePageTeardownTokenV1>,
    owners: BTreeMap<u64, Owner>,
    teardown_count: u64,
    released_signals: u64,
}

impl Default for RemotePageTeardownV1 {
    fn default() -> Self {
        Self::new_v1()
    }
}

impl RemotePageTeardownV1 {
    pub fn new_v1() -> Self {
        let issuer = next_manager_issuer_v1();
        let execution_epoch = 0;
        let next_slot = 1;
        let next_token = 1;
        let control_token = RemotePageTeardownTokenV1 {
            slot: 0,
            token: 0,
            execution_epoch,
            issuer,
        };

        Self {
            phase: RemotePageTeardownPhaseV1::Active,
            execution_epoch,
            next_slot,
            next_token,
            issuer,
            control_token,
            terminal_token: None,
            owners: BTreeMap::new(),
            teardown_count: 0,
            released_signals: 0,
        }
    }

    pub const fn execution_epoch_v1(&self) -> u64 {
        self.execution_epoch
    }

    pub const fn page_token_v1(&self) -> RemotePageTeardownTokenV1 {
        self.control_token
    }

    pub const fn is_terminated_v1(&self) -> bool {
        matches!(self.phase, RemotePageTeardownPhaseV1::Terminated)
    }

    pub const fn terminal_token_v1(&self) -> Option<RemotePageTeardownTokenV1> {
        self.terminal_token
    }

    pub fn owner_count_v1(&self) -> usize {
        self.owners.len()
    }

    pub const fn teardown_count_v1(&self) -> u64 {
        self.teardown_count
    }

    pub const fn released_signals_v1(&self) -> u64 {
        self.released_signals
    }

    /// Creates a current page signal carrying this manager's epoch and page-control token.
    pub fn page_signal_v1(
        &self,
        kind: RemotePageSignalKindV1,
    ) -> Result<RemotePageSignalV1, RemotePageTeardownErrorV1> {
        if self.is_terminated_v1() {
            return Err(RemotePageTeardownErrorV1::Terminated);
        }
        Ok(RemotePageSignalV1 {
            kind,
            execution_epoch: self.execution_epoch,
            remote_token: self.control_token,
        })
    }

    /// Admits one bounded remote work owner while the page is active.
    pub fn register_owner_v1(
        &mut self,
        kind: RemotePageOwnerKindV1,
    ) -> Result<RemotePageOwnerTokenV1, RemotePageTeardownErrorV1> {
        if self.is_terminated_v1() {
            return Err(RemotePageTeardownErrorV1::Terminated);
        }
        if self.owners.len() >= MAX_REMOTE_PAGE_TEARDOWN_OWNERS_V1 {
            return Err(RemotePageTeardownErrorV1::OwnerLimitExceeded);
        }

        let token = self.issue_token_v1(self.execution_epoch)?;
        self.owners.insert(
            token.slot,
            Owner {
                token: token.token,
                execution_epoch: token.execution_epoch,
                kind,
            },
        );
        Ok(RemotePageOwnerTokenV1 {
            slot: token.slot,
            token: token.token,
            execution_epoch: token.execution_epoch,
            issuer: token.issuer,
        })
    }

    pub fn callback_v1(
        &self,
        owner: RemotePageOwnerTokenV1,
    ) -> Result<RemotePageTeardownCallbackV1, RemotePageTeardownErrorV1> {
        if self.is_terminated_v1() {
            return Err(RemotePageTeardownErrorV1::Terminated);
        }
        if !self.owner_token_is_current_v1(owner) {
            return Err(RemotePageTeardownErrorV1::InvalidOwner);
        }
        Ok(RemotePageTeardownCallbackV1 {
            owner,
            execution_epoch: owner.execution_epoch,
        })
    }

    pub fn owner_is_current_v1(&self, owner: RemotePageOwnerTokenV1) -> bool {
        self.owner_token_is_current_v1(owner)
    }

    /// Applies a page signal.
    ///
    /// A matching `PageHide` or `Freeze` terminates every admitted owner exactly once. Stale,
    /// duplicate, and revival signals are no-ops that release their own signal ownership.
    pub fn apply_signal_v1(
        &mut self,
        signal: RemotePageSignalV1,
    ) -> Result<RemotePageSignalOutcomeV1, RemotePageTeardownErrorV1> {
        let RemotePageSignalV1 {
            kind,
            execution_epoch,
            remote_token,
        } = signal;

        if self.is_terminated_v1() {
            self.released_signals = self.released_signals.saturating_add(1);
            return Ok(match kind {
                RemotePageSignalKindV1::PageShow | RemotePageSignalKindV1::Resume => {
                    RemotePageSignalOutcomeV1::RevivalRejected
                }
                RemotePageSignalKindV1::PageHide | RemotePageSignalKindV1::Freeze => {
                    RemotePageSignalOutcomeV1::Stale
                }
            });
        }

        if execution_epoch != self.execution_epoch || remote_token != self.control_token {
            self.released_signals = self.released_signals.saturating_add(1);
            return Ok(RemotePageSignalOutcomeV1::Stale);
        }

        match kind {
            RemotePageSignalKindV1::PageShow | RemotePageSignalKindV1::Resume => {
                self.released_signals = self.released_signals.saturating_add(1);
                Ok(RemotePageSignalOutcomeV1::RevivalRejected)
            }
            RemotePageSignalKindV1::PageHide | RemotePageSignalKindV1::Freeze => self.teardown_v1(),
        }
    }

    /// Completes a remote callback by releasing its matching owner.
    pub fn complete_callback_v1(
        &mut self,
        callback: RemotePageTeardownCallbackV1,
    ) -> RemotePageCallbackOutcomeV1 {
        let RemotePageTeardownCallbackV1 {
            owner,
            execution_epoch,
        } = callback;

        if self.is_terminated_v1() {
            return RemotePageCallbackOutcomeV1::RejectedTerminated;
        }
        if !self.owner_token_is_current_v1(owner) || execution_epoch != self.execution_epoch {
            return RemotePageCallbackOutcomeV1::RejectedStale;
        }

        let Some(owner_state) = self.owners.remove(&owner.slot) else {
            return RemotePageCallbackOutcomeV1::RejectedStale;
        };
        RemotePageCallbackOutcomeV1::Released(owner_state.kind)
    }

    /// Explicitly reopens a terminated remote page with a fresh control identity.
    ///
    /// The old issuer, slot, and token counters are never rewound, so a pre-teardown page signal
    /// or callback cannot target the reopened manager.
    pub fn reopen_v1(&mut self) -> Result<RemotePageTeardownTokenV1, RemotePageTeardownErrorV1> {
        if !self.is_terminated_v1() {
            return Err(RemotePageTeardownErrorV1::AlreadyActive);
        }

        let execution_epoch = self
            .execution_epoch
            .checked_add(1)
            .ok_or(RemotePageTeardownErrorV1::Overflow)?;
        let control_token = self.issue_token_v1(execution_epoch)?;
        self.execution_epoch = execution_epoch;
        self.control_token = control_token;
        self.terminal_token = None;
        self.phase = RemotePageTeardownPhaseV1::Active;
        self.owners.clear();
        Ok(control_token)
    }

    fn teardown_v1(&mut self) -> Result<RemotePageSignalOutcomeV1, RemotePageTeardownErrorV1> {
        let execution_epoch = self
            .execution_epoch
            .checked_add(1)
            .ok_or(RemotePageTeardownErrorV1::Overflow)?;
        let terminal_token = self.issue_token_v1(execution_epoch)?;
        let torn_down = self.owners.len();

        self.execution_epoch = execution_epoch;
        self.control_token = terminal_token;
        self.terminal_token = Some(terminal_token);
        self.phase = RemotePageTeardownPhaseV1::Terminated;
        self.owners.clear();
        self.teardown_count = self.teardown_count.saturating_add(1);

        Ok(RemotePageSignalOutcomeV1::Terminated {
            torn_down,
            terminal_token,
        })
    }

    fn issue_token_v1(
        &mut self,
        execution_epoch: u64,
    ) -> Result<RemotePageTeardownTokenV1, RemotePageTeardownErrorV1> {
        let slot = self.next_slot;
        let next_slot = slot
            .checked_add(1)
            .ok_or(RemotePageTeardownErrorV1::Overflow)?;
        let token = self.next_token;
        let next_token = token
            .checked_add(1)
            .ok_or(RemotePageTeardownErrorV1::Overflow)?;
        self.next_slot = next_slot;
        self.next_token = next_token;

        Ok(RemotePageTeardownTokenV1 {
            slot,
            token,
            execution_epoch,
            issuer: self.issuer,
        })
    }

    fn owner_token_is_current_v1(&self, owner: RemotePageOwnerTokenV1) -> bool {
        if self.is_terminated_v1() || owner.issuer != self.issuer {
            return false;
        }
        let Some(state) = self.owners.get(&owner.slot) else {
            return false;
        };
        state.token == owner.token
            && state.execution_epoch == owner.execution_epoch
            && state.execution_epoch == self.execution_epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_OWNER_KINDS: [RemotePageOwnerKindV1; 8] = [
        RemotePageOwnerKindV1::RemoteToken,
        RemotePageOwnerKindV1::Fetch,
        RemotePageOwnerKindV1::Cpu,
        RemotePageOwnerKindV1::Mutation,
        RemotePageOwnerKindV1::Facade,
        RemotePageOwnerKindV1::StoreCapability,
        RemotePageOwnerKindV1::Secret,
        RemotePageOwnerKindV1::Reservation,
    ];

    fn register_all(manager: &mut RemotePageTeardownV1) -> Vec<RemotePageOwnerTokenV1> {
        ALL_OWNER_KINDS
            .into_iter()
            .map(|kind| manager.register_owner_v1(kind).unwrap())
            .collect()
    }

    #[test]
    fn pagehide_terminates_every_owner_kind_exactly_once() {
        let mut manager = RemotePageTeardownV1::new_v1();
        let owners = register_all(&mut manager);
        let signal = manager
            .page_signal_v1(RemotePageSignalKindV1::PageHide)
            .unwrap();
        assert_eq!(signal.kind_v1(), RemotePageSignalKindV1::PageHide);
        assert_eq!(signal.execution_epoch_v1(), manager.execution_epoch_v1());

        let outcome = manager.apply_signal_v1(signal).unwrap();
        assert!(matches!(
            outcome,
            RemotePageSignalOutcomeV1::Terminated { torn_down: 8, .. }
        ));
        assert!(manager.is_terminated_v1());
        assert_eq!(manager.owner_count_v1(), 0);
        assert_eq!(manager.teardown_count_v1(), 1);
        assert_eq!(manager.released_signals_v1(), 0);
        assert!(manager.terminal_token_v1().is_some());

        assert_eq!(
            manager.apply_signal_v1(signal).unwrap(),
            RemotePageSignalOutcomeV1::Stale
        );
        assert_eq!(manager.teardown_count_v1(), 1);
        assert_eq!(manager.released_signals_v1(), 1);
        for owner in owners {
            assert!(!manager.owner_is_current_v1(owner));
        }
    }

    #[test]
    fn freeze_terminates_and_pageshow_resume_do_not_revive_old_owners() {
        let mut manager = RemotePageTeardownV1::new_v1();
        let owner = manager
            .register_owner_v1(RemotePageOwnerKindV1::Fetch)
            .unwrap();
        let callback = manager.callback_v1(owner).unwrap();
        let freeze = manager
            .page_signal_v1(RemotePageSignalKindV1::Freeze)
            .unwrap();
        assert!(matches!(
            manager.apply_signal_v1(freeze).unwrap(),
            RemotePageSignalOutcomeV1::Terminated { torn_down: 1, .. }
        ));

        assert_eq!(
            manager.complete_callback_v1(callback),
            RemotePageCallbackOutcomeV1::RejectedTerminated
        );
        let pageshow = RemotePageSignalV1 {
            kind: RemotePageSignalKindV1::PageShow,
            execution_epoch: manager.execution_epoch_v1() - 1,
            remote_token: RemotePageTeardownTokenV1 {
                slot: 999,
                token: 999,
                execution_epoch: manager.execution_epoch_v1() - 1,
                issuer: manager.issuer,
            },
        };
        assert_eq!(
            manager.apply_signal_v1(pageshow).unwrap(),
            RemotePageSignalOutcomeV1::RevivalRejected
        );
        assert!(manager.is_terminated_v1());
        assert_eq!(manager.owner_count_v1(), 0);
        assert!(!manager.owner_is_current_v1(owner));

        let resume = RemotePageSignalV1 {
            kind: RemotePageSignalKindV1::Resume,
            execution_epoch: manager.execution_epoch_v1() - 1,
            remote_token: RemotePageTeardownTokenV1 {
                slot: 1000,
                token: 1000,
                execution_epoch: manager.execution_epoch_v1() - 1,
                issuer: manager.issuer,
            },
        };
        assert_eq!(
            manager.apply_signal_v1(resume).unwrap(),
            RemotePageSignalOutcomeV1::RevivalRejected
        );
    }

    #[test]
    fn stale_late_and_duplicate_signals_release_without_transition() {
        let mut manager = RemotePageTeardownV1::new_v1();
        let owner = manager
            .register_owner_v1(RemotePageOwnerKindV1::Cpu)
            .unwrap();
        let stale = RemotePageSignalV1 {
            kind: RemotePageSignalKindV1::PageHide,
            execution_epoch: manager.execution_epoch_v1() + 1,
            remote_token: manager.page_token_v1(),
        };
        assert_eq!(
            manager.apply_signal_v1(stale).unwrap(),
            RemotePageSignalOutcomeV1::Stale
        );
        assert!(!manager.is_terminated_v1());
        assert_eq!(manager.owner_count_v1(), 1);
        assert!(manager.owner_is_current_v1(owner));
        assert_eq!(manager.released_signals_v1(), 1);

        let valid = manager
            .page_signal_v1(RemotePageSignalKindV1::PageHide)
            .unwrap();
        assert!(matches!(
            manager.apply_signal_v1(valid).unwrap(),
            RemotePageSignalOutcomeV1::Terminated { .. }
        ));
        assert_eq!(
            manager.apply_signal_v1(valid).unwrap(),
            RemotePageSignalOutcomeV1::Stale
        );
        assert_eq!(manager.teardown_count_v1(), 1);
        assert_eq!(manager.released_signals_v1(), 2);
    }

    #[test]
    fn reopen_uses_fresh_identity_and_isolates_old_owners() {
        let mut manager = RemotePageTeardownV1::new_v1();
        let old_owner = manager
            .register_owner_v1(RemotePageOwnerKindV1::Mutation)
            .unwrap();
        let old_callback = manager.callback_v1(old_owner).unwrap();
        let old_signal = manager
            .page_signal_v1(RemotePageSignalKindV1::PageHide)
            .unwrap();
        manager.apply_signal_v1(old_signal).unwrap();

        let new_page_token = manager.reopen_v1().unwrap();
        assert!(!manager.is_terminated_v1());
        assert_ne!(new_page_token, old_signal.remote_token);
        assert_eq!(manager.owner_count_v1(), 0);
        assert_eq!(
            manager.apply_signal_v1(old_signal).unwrap(),
            RemotePageSignalOutcomeV1::Stale
        );
        assert_eq!(
            manager.complete_callback_v1(old_callback),
            RemotePageCallbackOutcomeV1::RejectedStale
        );

        let new_owner = manager
            .register_owner_v1(RemotePageOwnerKindV1::StoreCapability)
            .unwrap();
        assert!(manager.owner_is_current_v1(new_owner));
        assert!(!manager.owner_is_current_v1(old_owner));
        assert_eq!(
            manager.complete_callback_v1(manager.callback_v1(new_owner).unwrap()),
            RemotePageCallbackOutcomeV1::Released(RemotePageOwnerKindV1::StoreCapability)
        );
        assert_eq!(manager.owner_count_v1(), 0);
    }

    #[test]
    fn terminated_callbacks_and_registration_fail_closed_until_reopen() {
        let mut manager = RemotePageTeardownV1::new_v1();
        let owner = manager
            .register_owner_v1(RemotePageOwnerKindV1::Reservation)
            .unwrap();
        let callback = manager.callback_v1(owner).unwrap();
        assert_eq!(callback.execution_epoch_v1(), owner.execution_epoch);
        manager
            .apply_signal_v1(
                manager
                    .page_signal_v1(RemotePageSignalKindV1::PageHide)
                    .unwrap(),
            )
            .unwrap();

        assert_eq!(
            manager.register_owner_v1(RemotePageOwnerKindV1::Fetch),
            Err(RemotePageTeardownErrorV1::Terminated)
        );
        assert_eq!(
            manager.complete_callback_v1(callback),
            RemotePageCallbackOutcomeV1::RejectedTerminated
        );
        manager.reopen_v1().unwrap();
        assert!(
            manager
                .register_owner_v1(RemotePageOwnerKindV1::Fetch)
                .is_ok()
        );
    }

    #[test]
    fn owner_capacity_and_identity_overflow_fail_closed() {
        let mut manager = RemotePageTeardownV1::new_v1();
        for _ in 0..MAX_REMOTE_PAGE_TEARDOWN_OWNERS_V1 {
            manager
                .register_owner_v1(RemotePageOwnerKindV1::Cpu)
                .unwrap();
        }
        assert_eq!(
            manager.register_owner_v1(RemotePageOwnerKindV1::Cpu),
            Err(RemotePageTeardownErrorV1::OwnerLimitExceeded)
        );

        let mut manager = RemotePageTeardownV1::new_v1();
        manager.next_slot = u64::MAX;
        assert_eq!(
            manager.register_owner_v1(RemotePageOwnerKindV1::Fetch),
            Err(RemotePageTeardownErrorV1::Overflow)
        );
        assert!(manager.owners.is_empty());

        manager.next_slot = 1;
        manager.next_token = u64::MAX;
        assert_eq!(
            manager.register_owner_v1(RemotePageOwnerKindV1::Fetch),
            Err(RemotePageTeardownErrorV1::Overflow)
        );

        let mut manager = RemotePageTeardownV1::new_v1();
        manager.execution_epoch = u64::MAX;
        let owner = manager
            .register_owner_v1(RemotePageOwnerKindV1::Secret)
            .unwrap();
        let signal = manager
            .page_signal_v1(RemotePageSignalKindV1::PageHide)
            .unwrap();
        assert_eq!(
            manager.apply_signal_v1(signal),
            Err(RemotePageTeardownErrorV1::Overflow)
        );
        assert!(!manager.is_terminated_v1());
        assert_eq!(manager.owner_count_v1(), 1);
        assert!(manager.owner_is_current_v1(owner));
        assert_eq!(manager.teardown_count_v1(), 0);

        let mut manager = RemotePageTeardownV1::new_v1();
        manager
            .apply_signal_v1(
                manager
                    .page_signal_v1(RemotePageSignalKindV1::PageHide)
                    .unwrap(),
            )
            .unwrap();
        manager.execution_epoch = u64::MAX;
        assert_eq!(
            manager.reopen_v1(),
            Err(RemotePageTeardownErrorV1::Overflow)
        );
        assert!(manager.is_terminated_v1());
    }

    #[test]
    fn duplicate_callback_is_rejected_after_release() {
        let mut manager = RemotePageTeardownV1::new_v1();
        let owner = manager
            .register_owner_v1(RemotePageOwnerKindV1::Facade)
            .unwrap();
        let callback = manager.callback_v1(owner).unwrap();
        assert_eq!(
            manager.complete_callback_v1(callback),
            RemotePageCallbackOutcomeV1::Released(RemotePageOwnerKindV1::Facade)
        );
        assert_eq!(
            manager.complete_callback_v1(callback),
            RemotePageCallbackOutcomeV1::RejectedStale
        );
    }

    #[test]
    fn cross_manager_identity_confusion_fails_closed() {
        let mut first = RemotePageTeardownV1::new_v1();
        let mut second = RemotePageTeardownV1::new_v1();
        let first_owner = first
            .register_owner_v1(RemotePageOwnerKindV1::Reservation)
            .unwrap();
        let first_signal = first
            .page_signal_v1(RemotePageSignalKindV1::PageHide)
            .unwrap();

        assert_eq!(
            second.apply_signal_v1(first_signal).unwrap(),
            RemotePageSignalOutcomeV1::Stale
        );
        assert!(!second.is_terminated_v1());
        assert!(matches!(
            second.callback_v1(first_owner),
            Err(RemotePageTeardownErrorV1::InvalidOwner)
        ));
        assert!(first.owner_is_current_v1(first_owner));
    }
}
