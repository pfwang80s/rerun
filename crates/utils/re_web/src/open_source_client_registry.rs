//! Production-disarmed open-source client registry and close-once ownership.
//!
//! This module models the Web-only `Vacant → Opening → Active → Closing → Vacant` lifecycle
//! required by future strict open handoffs.
//! It does not hook into the native viewer or the existing compatibility `open()` / `close()`
//! paths.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::{Rc, Weak};

use crate::open_source_terminal::{
    OpenSourceStatusOwnerV1, OpenSourceStatusV1, OpenSourceTerminalCauseV1, OpenSourceToken,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenSourceClientPhaseV1 {
    Vacant,
    Opening,
    Active,
    Closing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenSourceClientRegistryErrorV1 {
    Occupied,
    WrongPhase,
    OwnerAlreadyTerminal,
}

pub struct OpenSourceClientOpeningHandleV1 {
    source_token: OpenSourceToken,
    registry: Weak<RefCell<OpenSourceClientRegistryInnerV1>>,
    active: bool,
}

impl OpenSourceClientOpeningHandleV1 {
    pub const fn source_token(&self) -> OpenSourceToken {
        self.source_token
    }

    pub fn cancel_v1(self, cause: OpenSourceTerminalCauseV1) -> bool {
        let Some(registry) = self.registry.upgrade() else {
            return false;
        };
        let registry = OpenSourceClientRegistryV1 { inner: registry };
        registry.close_v1(self.source_token, cause)
    }
}

impl Drop for OpenSourceClientOpeningHandleV1 {
    fn drop(&mut self) {
        if self.active {
            return;
        }
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let registry = OpenSourceClientRegistryV1 { inner: registry };
        let _ = registry.close_v1(self.source_token, OpenSourceTerminalCauseV1::OpeningFailure);
    }
}

#[derive(Clone)]
pub struct RemoteCloseOnceV1 {
    closed: Rc<Cell<bool>>,
}

impl RemoteCloseOnceV1 {
    pub fn new_v1() -> Self {
        Self {
            closed: Rc::new(Cell::new(false)),
        }
    }

    pub fn close_v1(&self) -> bool {
        if self.closed.get() {
            return false;
        }
        self.closed.set(true);
        true
    }

    pub fn is_closed_v1(&self) -> bool {
        self.closed.get()
    }
}

impl fmt::Debug for RemoteCloseOnceV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RemoteCloseOnceV1(<opaque>)")
    }
}

struct OpenSourceClientEntryV1 {
    owner: OpenSourceStatusOwnerV1,
    phase: OpenSourceClientPhaseV1,
    close_once: RemoteCloseOnceV1,
}

impl OpenSourceClientEntryV1 {
    fn new_v1(owner: OpenSourceStatusOwnerV1) -> Self {
        Self {
            owner,
            phase: OpenSourceClientPhaseV1::Opening,
            close_once: RemoteCloseOnceV1::new_v1(),
        }
    }
}

struct OpenSourceClientRegistryInnerV1 {
    entries: HashMap<OpenSourceToken, OpenSourceClientEntryV1>,
}

impl OpenSourceClientRegistryInnerV1 {
    fn new_v1() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

pub struct OpenSourceClientRegistryV1 {
    inner: Rc<RefCell<OpenSourceClientRegistryInnerV1>>,
}

impl OpenSourceClientRegistryV1 {
    pub fn new_v1() -> Self {
        Self {
            inner: Rc::new(RefCell::new(OpenSourceClientRegistryInnerV1::new_v1())),
        }
    }

    fn lock_inner_v1(&self) -> std::cell::RefMut<'_, OpenSourceClientRegistryInnerV1> {
        self.inner.borrow_mut()
    }

    pub fn state_v1(&self, source_token: OpenSourceToken) -> OpenSourceClientPhaseV1 {
        self.lock_inner_v1()
            .entries
            .get(&source_token)
            .map(|entry| entry.phase)
            .unwrap_or(OpenSourceClientPhaseV1::Vacant)
    }

    pub fn owner_status_v1(&self, source_token: OpenSourceToken) -> Option<OpenSourceStatusV1> {
        self.lock_inner_v1()
            .entries
            .get(&source_token)
            .map(|entry| entry.owner.status())
    }

    pub fn claim_opening_v1(
        &self,
        owner: OpenSourceStatusOwnerV1,
    ) -> Result<OpenSourceClientOpeningHandleV1, OpenSourceClientRegistryErrorV1> {
        if owner.status() != OpenSourceStatusV1::Live {
            return Err(OpenSourceClientRegistryErrorV1::OwnerAlreadyTerminal);
        }

        let source_token = owner.source_token();
        let mut inner = self.lock_inner_v1();
        if inner.entries.contains_key(&source_token) {
            return Err(OpenSourceClientRegistryErrorV1::Occupied);
        }

        inner
            .entries
            .insert(source_token, OpenSourceClientEntryV1::new_v1(owner));
        Ok(OpenSourceClientOpeningHandleV1 {
            source_token,
            registry: Rc::downgrade(&self.inner),
            active: false,
        })
    }

    pub fn activate_v1(
        &self,
        opening: OpenSourceClientOpeningHandleV1,
    ) -> Result<OpenSourceClientGuardV1, OpenSourceClientRegistryErrorV1> {
        let source_token = opening.source_token;

        let mut inner = self.lock_inner_v1();
        let Some(entry) = inner.entries.get_mut(&source_token) else {
            return Err(OpenSourceClientRegistryErrorV1::WrongPhase);
        };
        if entry.phase != OpenSourceClientPhaseV1::Opening {
            return Err(OpenSourceClientRegistryErrorV1::WrongPhase);
        }

        entry.phase = OpenSourceClientPhaseV1::Active;
        drop(inner);
        let mut opening = opening;
        opening.active = true;
        Ok(OpenSourceClientGuardV1 {
            source_token,
            registry: Rc::downgrade(&self.inner),
        })
    }

    pub fn request_close_v1(
        &self,
        source_token: OpenSourceToken,
        cause: OpenSourceTerminalCauseV1,
    ) -> bool {
        let mut inner = self.lock_inner_v1();
        let Some(entry) = inner.entries.get_mut(&source_token) else {
            return false;
        };
        if !entry.close_once.close_v1() {
            return false;
        }

        entry.owner.latch_terminal_v1(cause);
        entry.phase = OpenSourceClientPhaseV1::Closing;
        true
    }

    pub fn finalize_vacant_v1(&self, source_token: OpenSourceToken) -> bool {
        let mut inner = self.lock_inner_v1();
        let should_remove = match inner.entries.get(&source_token) {
            Some(entry) => {
                entry.phase == OpenSourceClientPhaseV1::Closing && entry.close_once.is_closed_v1()
            }
            None => return false,
        };
        if !should_remove {
            return false;
        }

        inner.entries.remove(&source_token);
        true
    }

    pub fn close_v1(
        &self,
        source_token: OpenSourceToken,
        cause: OpenSourceTerminalCauseV1,
    ) -> bool {
        let requested = self.request_close_v1(source_token, cause);
        if requested {
            self.finalize_vacant_v1(source_token);
        }
        requested
    }
}

impl fmt::Debug for OpenSourceClientRegistryV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenSourceClientRegistryV1")
            .field("entries", &self.lock_inner_v1().entries.len())
            .finish()
    }
}

pub struct OpenSourceClientGuardV1 {
    source_token: OpenSourceToken,
    registry: Weak<RefCell<OpenSourceClientRegistryInnerV1>>,
}

impl OpenSourceClientGuardV1 {
    pub const fn source_token(&self) -> OpenSourceToken {
        self.source_token
    }

    pub fn close_v1(&self, cause: OpenSourceTerminalCauseV1) -> bool {
        let Some(registry) = self.registry.upgrade() else {
            return false;
        };
        let registry = OpenSourceClientRegistryV1 { inner: registry };
        registry.close_v1(self.source_token, cause)
    }
}

impl Drop for OpenSourceClientGuardV1 {
    fn drop(&mut self) {
        let _ = self.close_v1(OpenSourceTerminalCauseV1::ExplicitClose);
    }
}

impl fmt::Debug for OpenSourceClientGuardV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenSourceClientGuardV1(<opaque>)")
    }
}

impl fmt::Debug for OpenSourceClientOpeningHandleV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenSourceClientOpeningHandleV1(<opaque>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(v: u64) -> OpenSourceToken {
        OpenSourceToken::new_for_test_v1(v)
    }

    #[test]
    fn opening_active_closing_and_vacant_follow_the_expected_sequence() {
        let registry = OpenSourceClientRegistryV1::new_v1();
        let owner = OpenSourceStatusOwnerV1::new_with_token_v1(token(1));

        let opening = registry.claim_opening_v1(owner).expect("opening claim");
        assert_eq!(
            registry.state_v1(token(1)),
            OpenSourceClientPhaseV1::Opening
        );

        let guard = registry.activate_v1(opening).expect("activation");
        assert_eq!(registry.state_v1(token(1)), OpenSourceClientPhaseV1::Active);

        assert!(guard.close_v1(OpenSourceTerminalCauseV1::ExplicitClose));
        assert_eq!(registry.state_v1(token(1)), OpenSourceClientPhaseV1::Vacant);
        assert!(registry.owner_status_v1(token(1)).is_none());
    }

    #[test]
    fn opening_handle_drop_closes_unactivated_entry() {
        let registry = OpenSourceClientRegistryV1::new_v1();
        let owner = OpenSourceStatusOwnerV1::new_with_token_v1(token(2));

        let opening = registry.claim_opening_v1(owner).expect("opening claim");
        assert_eq!(
            registry.state_v1(token(2)),
            OpenSourceClientPhaseV1::Opening
        );
        drop(opening);

        assert_eq!(registry.state_v1(token(2)), OpenSourceClientPhaseV1::Vacant);
    }

    #[test]
    fn stale_close_cannot_affect_a_fresh_token() {
        let registry = OpenSourceClientRegistryV1::new_v1();
        let first_owner = OpenSourceStatusOwnerV1::new_with_token_v1(token(3));
        let first_opening = registry
            .claim_opening_v1(first_owner)
            .expect("opening claim");
        let first_guard = registry.activate_v1(first_opening).expect("activation");

        let second_owner = OpenSourceStatusOwnerV1::new_with_token_v1(token(4));
        let second_opening = registry
            .claim_opening_v1(second_owner)
            .expect("second opening claim");
        let second_guard = registry
            .activate_v1(second_opening)
            .expect("second activation");

        assert!(first_guard.close_v1(OpenSourceTerminalCauseV1::ExplicitClose));
        assert_eq!(registry.state_v1(token(3)), OpenSourceClientPhaseV1::Vacant);
        assert_eq!(registry.state_v1(token(4)), OpenSourceClientPhaseV1::Active);

        assert!(second_guard.close_v1(OpenSourceTerminalCauseV1::MemoryPressure));
        assert_eq!(registry.state_v1(token(4)), OpenSourceClientPhaseV1::Vacant);
    }

    #[test]
    fn close_once_is_idempotent_and_finalize_requires_closing() {
        let registry = OpenSourceClientRegistryV1::new_v1();
        let owner = OpenSourceStatusOwnerV1::new_with_token_v1(token(5));

        let opening = registry.claim_opening_v1(owner).expect("opening claim");
        let guard = registry.activate_v1(opening).expect("activation");

        assert!(registry.request_close_v1(token(5), OpenSourceTerminalCauseV1::BackgroundPolicy));
        assert_eq!(
            registry.state_v1(token(5)),
            OpenSourceClientPhaseV1::Closing
        );
        assert!(!registry.request_close_v1(token(5), OpenSourceTerminalCauseV1::ExplicitClose));
        assert!(registry.finalize_vacant_v1(token(5)));
        assert_eq!(registry.state_v1(token(5)), OpenSourceClientPhaseV1::Vacant);

        drop(guard);
    }

    #[test]
    fn terminal_owner_cannot_claim_opening_again() {
        let registry = OpenSourceClientRegistryV1::new_v1();
        let mut owner = OpenSourceStatusOwnerV1::new_with_token_v1(token(6));
        assert!(owner.latch_terminal_v1(OpenSourceTerminalCauseV1::ExplicitClose));

        assert!(matches!(
            registry.claim_opening_v1(owner),
            Err(OpenSourceClientRegistryErrorV1::OwnerAlreadyTerminal)
        ));
    }
}
