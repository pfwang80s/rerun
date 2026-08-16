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
}

impl CompatibilityRemoteMcapSingletonV1 {
    /// Creates the compatibility seam with the remote capability disabled.
    pub fn new_disarmed_v1() -> Self {
        Self {
            capability_installed: false,
            registry: OpenSourceClientRegistryV1::new_v1(),
            opening: None,
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

        if self.opening.is_some() {
            return CompatibilityRemoteMcapDispatchV1::RemoteSessionLimitReached;
        }

        let owner = OpenSourceStatusOwnerV1::new_fresh_v1();
        let source_token = owner.source_token();
        match self.registry.claim_opening_v1(owner) {
            Ok(opening) => {
                self.opening = Some(opening);
                CompatibilityRemoteMcapDispatchV1::RemoteAccepted { source_token }
            }
            Err(
                OpenSourceClientRegistryErrorV1::Occupied
                | OpenSourceClientRegistryErrorV1::WrongPhase
                | OpenSourceClientRegistryErrorV1::OwnerAlreadyTerminal,
            ) => CompatibilityRemoteMcapDispatchV1::RemoteSessionLimitReached,
        }
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
