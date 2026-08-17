//! Production-disarmed compatibility remote-MCAP ingress.
//!
//! Compatibility `open`/`start` keeps its existing void, per-item dispatcher contract.  This
//! seam only owns the route decision and the singleton admission point for a future remote-MCAP
//! capability.  Until that capability is installed, dispatch falls back to the existing
//! `ViewerOpenUrl` path without retaining URL material or changing any visible behavior.

use std::fmt;

use crate::open_source_client_registry::{
    OpenSourceClientOpeningHandleV1, OpenSourceClientRegistryErrorV1, OpenSourceClientRegistryV1,
};
use crate::open_source_terminal::{OpenSourceStatusOwnerV1, OpenSourceToken};

/// The compatibility ingress route decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityRemoteMcapDispatchV1 {
    /// The remote capability is not installed; use the existing dispatcher unchanged.
    ExistingDispatcher,
    /// The future remote capability accepted this item into the singleton source.
    RemoteAccepted { source_token: OpenSourceToken },
    /// A different remote source is already occupying the singleton slot.
    RemoteSessionLimitReached,
}

/// Production-disarmed singleton admission for compatibility remote-MCAP inputs.
pub struct CompatibilityRemoteMcapSingletonV1 {
    capability_installed: bool,
    registry: OpenSourceClientRegistryV1,
    opening: Option<OpenSourceClientOpeningHandleV1>,
    opening_suspended: bool,
    page: RemotePageManagerV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemotePageStateV1 {
    Visible { epoch: u32, resume_nonce: u32 },
    Hidden { epoch: u32 },
    Terminated { epoch: u32 },
}

#[derive(Debug)]
struct RemotePageManagerV1 {
    state: RemotePageStateV1,
    visible_deadline_ms: Option<u64>,
    last_resume_nonce: u32,
}

impl Default for RemotePageManagerV1 {
    fn default() -> Self {
        Self {
            state: RemotePageStateV1::Visible {
                epoch: 0,
                resume_nonce: 0,
            },
            visible_deadline_ms: None,
            last_resume_nonce: 0,
        }
    }
}

impl CompatibilityRemoteMcapSingletonV1 {
    /// Creates the compatibility seam with the remote capability disabled.
    pub fn new_disarmed_v1() -> Self {
        Self {
            capability_installed: false,
            registry: OpenSourceClientRegistryV1::new_v1(),
            opening: None,
            opening_suspended: false,
            page: RemotePageManagerV1::default(),
        }
    }

    /// Routes one already-classified explicit `.mcap` item.
    ///
    /// The disarmed production path intentionally returns [`ExistingDispatcher`].  The future
    /// capability installation can arm this same owner without changing the compatibility
    /// caller's per-item, non-throwing contract.
    pub fn dispatch_v1(&mut self) -> CompatibilityRemoteMcapDispatchV1 {
        if !self.capability_installed {
            return CompatibilityRemoteMcapDispatchV1::ExistingDispatcher;
        }

        if matches!(self.page.state, RemotePageStateV1::Terminated { .. }) {
            return CompatibilityRemoteMcapDispatchV1::RemoteSessionLimitReached;
        }

        if self.opening.is_some() {
            return CompatibilityRemoteMcapDispatchV1::RemoteSessionLimitReached;
        }

        let owner = OpenSourceStatusOwnerV1::new_fresh_v1();
        let source_token = owner.source_token();
        match self.registry.claim_opening_v1(owner) {
            Ok(opening) => {
                self.opening = Some(opening);
                self.opening_suspended =
                    matches!(self.page.state, RemotePageStateV1::Hidden { .. });
                CompatibilityRemoteMcapDispatchV1::RemoteAccepted { source_token }
            }
            Err(
                OpenSourceClientRegistryErrorV1::Occupied
                | OpenSourceClientRegistryErrorV1::WrongPhase
                | OpenSourceClientRegistryErrorV1::OwnerAlreadyTerminal,
            ) => CompatibilityRemoteMcapDispatchV1::RemoteSessionLimitReached,
        }
    }

    pub fn page_hidden_v1(&mut self, epoch: u32) {
        let RemotePageStateV1::Visible { epoch: current, .. } = self.page.state else {
            return;
        };
        if epoch != current {
            return;
        }
        self.page.state = RemotePageStateV1::Hidden { epoch };
        self.page.visible_deadline_ms = None;
        if self.opening.is_some() {
            self.opening_suspended = true;
        }
    }

    pub fn page_resume_v1(&mut self, from_epoch: u32, to_epoch: u32, resume_nonce: u32) -> bool {
        if self.page.state != (RemotePageStateV1::Hidden { epoch: from_epoch })
            || to_epoch != from_epoch.saturating_add(1)
            || resume_nonce == 0
            || resume_nonce <= self.page.last_resume_nonce
        {
            return false;
        }
        self.page.state = RemotePageStateV1::Visible {
            epoch: to_epoch,
            resume_nonce,
        };
        self.page.visible_deadline_ms = None;
        self.page.last_resume_nonce = resume_nonce;
        self.opening_suspended = false;
        true
    }

    pub fn page_visible_deadline_v1(&mut self, epoch: u32, deadline_ms: Option<u64>) -> bool {
        if !matches!(self.page.state, RemotePageStateV1::Visible { epoch: current, .. } if current == epoch)
        {
            return false;
        }
        self.page.visible_deadline_ms = deadline_ms;
        true
    }

    pub fn page_terminate_v1(&mut self, epoch: u32) {
        let current = match self.page.state {
            RemotePageStateV1::Visible { epoch: current, .. }
            | RemotePageStateV1::Hidden { epoch: current } => current,
            RemotePageStateV1::Terminated { .. } => return,
        };
        if epoch != current {
            return;
        }
        self.page.state = RemotePageStateV1::Terminated { epoch };
        self.page.visible_deadline_ms = None;
        let _ = self.cancel_opening_v1();
    }

    /// Arms the future remote capability after its measured profile is installed.
    ///
    /// This is intentionally crate-private for now; MCAP-088 will provide the sealed production
    /// bridge.  Keeping the state transition here makes the singleton admission testable without
    /// exposing a public capability before the profile is frozen.
    #[cfg(test)]
    fn arm_for_test_v1(&mut self) {
        self.capability_installed = true;
    }

    /// Cancels an opening claim, returning the singleton to `Vacant`.
    pub fn cancel_opening_v1(&mut self) -> bool {
        let Some(opening) = self.opening.take() else {
            return false;
        };
        self.opening_suspended = false;
        opening.cancel_v1(crate::open_source_terminal::OpenSourceTerminalCauseV1::OpeningFailure)
    }
}

impl fmt::Debug for CompatibilityRemoteMcapSingletonV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompatibilityRemoteMcapSingletonV1")
            .field("capability_installed", &self.capability_installed)
            .field("opening", &self.opening.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disarmed_path_is_an_exact_noop_for_existing_dispatcher() {
        let mut singleton = CompatibilityRemoteMcapSingletonV1::new_disarmed_v1();
        assert_eq!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::ExistingDispatcher
        );
        assert!(!singleton.cancel_opening_v1());
    }

    #[test]
    fn page_lifecycle_gates_armed_admission_and_requires_matching_resume() {
        let mut singleton = CompatibilityRemoteMcapSingletonV1::new_disarmed_v1();
        singleton.page_hidden_v1(0);
        singleton.arm_for_test_v1();
        assert!(matches!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::RemoteAccepted { .. }
        ));
        assert!(singleton.opening_suspended);
        assert!(!singleton.page_resume_v1(0, 2, 1));
        assert!(singleton.opening_suspended);
        assert!(singleton.page_resume_v1(0, 1, 1));
        assert!(!singleton.opening_suspended);
        singleton.page_hidden_v1(0); // stale hidden must not suspend epoch 1
        assert!(matches!(
            singleton.page.state,
            RemotePageStateV1::Visible { epoch: 1, .. }
        ));
        assert!(singleton.page_visible_deadline_v1(1, Some(10)));
        assert!(!singleton.page_visible_deadline_v1(0, Some(10)));
        assert!(singleton.cancel_opening_v1());
        assert!(matches!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::RemoteAccepted { .. }
        ));
        singleton.page_terminate_v1(3);
        assert_eq!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::RemoteSessionLimitReached
        );
    }

    #[test]
    fn stale_termination_cannot_cancel_a_new_epoch_owner() {
        let mut singleton = CompatibilityRemoteMcapSingletonV1::new_disarmed_v1();
        singleton.arm_for_test_v1();
        singleton.page_hidden_v1(0);
        assert!(singleton.page_resume_v1(0, 1, 1));
        assert!(matches!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::RemoteAccepted { .. }
        ));
        singleton.page_terminate_v1(0);
        assert!(singleton.opening.is_some());
        singleton.page_terminate_v1(1);
        assert!(singleton.opening.is_none());
    }

    #[test]
    fn disarmed_compatibility_fallback_is_unchanged_in_every_page_state() {
        let mut singleton = CompatibilityRemoteMcapSingletonV1::new_disarmed_v1();
        singleton.page_hidden_v1(1);
        assert_eq!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::ExistingDispatcher
        );
        singleton.page_terminate_v1(2);
        assert_eq!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::ExistingDispatcher
        );
    }

    #[test]
    fn armed_singleton_accepts_one_then_limits_competing_source() {
        let mut singleton = CompatibilityRemoteMcapSingletonV1::new_disarmed_v1();
        singleton.arm_for_test_v1();

        let first = singleton.dispatch_v1();
        assert!(matches!(
            first,
            CompatibilityRemoteMcapDispatchV1::RemoteAccepted { .. }
        ));
        assert_eq!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::RemoteSessionLimitReached
        );

        assert!(singleton.cancel_opening_v1());
        assert!(matches!(
            singleton.dispatch_v1(),
            CompatibilityRemoteMcapDispatchV1::RemoteAccepted { .. }
        ));
    }
}
