//! Production-disarmed remote-MCAP activation seam.
//!
//! This module models the `Opening -> Active` transition required before remote `StoreHub`
//! publication can be wired. It owns only the activation bundle and reservation; it does not
//! create or publish an ordinary Store before activation and does not touch native, local,
//! compatibility, `LogChannel`, gRPC, or Redap paths.

#![allow(dead_code)]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use re_chunk::{TimeInt, TimelineName};
use re_log_types::StoreId;

/// Orthogonal remote recording use state.
///
/// `Active` in [`RemoteMcapSlotV1`] only means the remote resources are installed; the selected
/// foreground/background classification is carried by this field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRecordingUseStateV1 {
    Foreground,
    CatalogOnly,
    Inactive,
}

/// Opaque source identity for a remote opening attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteSourceTokenV1(u64);

impl RemoteSourceTokenV1 {
    pub(crate) const fn new_v1(token: u64) -> Option<Self> {
        if token == 0 { None } else { Some(Self(token)) }
    }

    #[cfg(test)]
    pub(crate) const fn new_for_test_v1(token: u64) -> Self {
        Self(token)
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

/// A token issued when the singleton remote slot enters `Opening`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMcapOpeningTokenV1 {
    slot: u64,
    source: RemoteSourceTokenV1,
}

impl RemoteMcapOpeningTokenV1 {
    pub(crate) const fn source_token_v1(self) -> RemoteSourceTokenV1 {
        self.source
    }

    pub(crate) const fn slot_v1(self) -> u64 {
        self.slot
    }
}

/// Placeholder manifest identity.
///
/// The real immutable manifest is still sealed inside `re_mcap`; this seam only requires the
/// opening and active bundles to retain the same `Arc`, so later replacement can happen without
/// weakening the ownership contract.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RemoteMcapManifestV1 {
    session_identity: u128,
    source_generation: u64,
}

impl RemoteMcapManifestV1 {
    pub(crate) const fn new_v1(session_identity: u128, source_generation: u64) -> Self {
        Self {
            session_identity,
            source_generation,
        }
    }

    pub(crate) const fn identity_v1(&self) -> (u128, u64) {
        (self.session_identity, self.source_generation)
    }
}

/// Opaque controller identity carried into the active bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteMcapControllerV1 {
    identity: u64,
}

impl RemoteMcapControllerV1 {
    pub(crate) const fn new_v1(identity: u64) -> Self {
        Self { identity }
    }

    pub(crate) const fn identity_v1(self) -> u64 {
        self.identity
    }
}

/// Metadata projection installed at activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteStoreProjectionV1 {
    pub(crate) store_id: StoreId,
    pub(crate) canonical_timeline: TimelineName,
}

/// Canonical navigation state installed with the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCanonicalNavigationV1 {
    pub(crate) timeline: TimelineName,
    pub(crate) committed_cursor: Option<TimeInt>,
    pub(crate) play_state: RemotePlayStateV1,
}

/// Remote playback states allowed by the canonical adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemotePlayStateV1 {
    Paused,
    Playing,
}

/// Initial facade state installed with the remote Store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteInitialPresentationGateV1 {
    pub(crate) facade_instance: u64,
    pub(crate) presentation_epoch: u64,
    pub(crate) committed_time: Option<TimeInt>,
    pub(crate) live_lease_count: u64,
}

/// Bounded reservation ledger used to prove failed activations do not leak reservations.
pub(crate) struct RemoteActivationReservationLedgerV1 {
    state: Arc<RemoteActivationReservationLedgerStateV1>,
}

impl RemoteActivationReservationLedgerV1 {
    pub(crate) fn new_v1(max_bytes: u64) -> Self {
        Self {
            state: Arc::new(RemoteActivationReservationLedgerStateV1 {
                max_bytes,
                active_bytes: AtomicU64::new(0),
            }),
        }
    }

    pub(crate) fn active_bytes_v1(&self) -> u64 {
        self.state.active_bytes.load(Ordering::Acquire)
    }

    pub(crate) fn acquire_v1(
        &self,
        bytes: u64,
    ) -> Result<RemoteActivationReservationV1, RemoteMcapActivationErrorV1> {
        self.state
            .active_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                active
                    .checked_add(bytes)
                    .filter(|next| *next <= self.state.max_bytes)
            })
            .map_err(|_active| RemoteMcapActivationErrorV1::ReservationLimitExceeded)?;
        Ok(RemoteActivationReservationV1 {
            ledger: Arc::clone(&self.state),
            bytes,
        })
    }
}

struct RemoteActivationReservationLedgerStateV1 {
    max_bytes: u64,
    active_bytes: AtomicU64,
}

pub(crate) struct RemoteActivationReservationV1 {
    ledger: Arc<RemoteActivationReservationLedgerStateV1>,
    bytes: u64,
}

impl RemoteActivationReservationV1 {
    pub(crate) const fn bytes_v1(&self) -> u64 {
        self.bytes
    }
}

impl Drop for RemoteActivationReservationV1 {
    fn drop(&mut self) {
        self.ledger
            .active_bytes
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Complete pre-Store activation bundle.
///
/// This intentionally does not contain a public Store. The frame driver calls [`RemoteMcapSessionV1::activate_v1`]
/// once, after which the resulting [`ActiveRemoteMcapBundle`] owns every installed piece.
pub(crate) struct OpenedRemoteMcap {
    manifest: Arc<RemoteMcapManifestV1>,
    store_projection: RemoteStoreProjectionV1,
    navigation: RemoteCanonicalNavigationV1,
    initial_gate: RemoteInitialPresentationGateV1,
    controller: RemoteMcapControllerV1,
    use_state: RemoteRecordingUseStateV1,
    reservation: RemoteActivationReservationV1,
}

impl OpenedRemoteMcap {
    pub(crate) fn new_v1(
        manifest: Arc<RemoteMcapManifestV1>,
        store_projection: RemoteStoreProjectionV1,
        navigation: RemoteCanonicalNavigationV1,
        initial_gate: RemoteInitialPresentationGateV1,
        controller: RemoteMcapControllerV1,
        use_state: RemoteRecordingUseStateV1,
        reservation: RemoteActivationReservationV1,
    ) -> Self {
        Self {
            manifest,
            store_projection,
            navigation,
            initial_gate,
            controller,
            use_state,
            reservation,
        }
    }

    pub(crate) fn manifest_arc_v1(&self) -> &Arc<RemoteMcapManifestV1> {
        &self.manifest
    }

    pub(crate) fn store_projection_v1(&self) -> &RemoteStoreProjectionV1 {
        &self.store_projection
    }

    pub(crate) fn controller_identity_v1(&self) -> RemoteMcapControllerV1 {
        self.controller
    }

    pub(crate) const fn use_state_v1(&self) -> RemoteRecordingUseStateV1 {
        self.use_state
    }
}

impl fmt::Debug for OpenedRemoteMcap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenedRemoteMcap")
            .field("manifest", &"<opaque Arc>")
            .field("store_projection", &self.store_projection)
            .field("navigation", &self.navigation)
            .field("initial_gate", &self.initial_gate)
            .field("controller", &self.controller)
            .field("use_state", &self.use_state)
            .field("reservation_bytes", &self.reservation.bytes_v1())
            .finish()
    }
}

/// The public Store installed by the activation closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublicRemoteStoreV1 {
    store_id: StoreId,
    generation: u64,
}

impl PublicRemoteStoreV1 {
    pub(crate) const fn new_v1(store_id: StoreId, generation: u64) -> Self {
        Self {
            store_id,
            generation,
        }
    }

    pub(crate) const fn store_id_v1(&self) -> &StoreId {
        &self.store_id
    }

    pub(crate) const fn generation_v1(&self) -> u64 {
        self.generation
    }
}

/// Fully installed remote bundle observable only after successful activation.
pub(crate) struct ActiveRemoteMcapBundle {
    manifest: Arc<RemoteMcapManifestV1>,
    store_projection: RemoteStoreProjectionV1,
    navigation: RemoteCanonicalNavigationV1,
    initial_gate: RemoteInitialPresentationGateV1,
    controller: RemoteMcapControllerV1,
    use_state: RemoteRecordingUseStateV1,
    store: PublicRemoteStoreV1,
    reservation: RemoteActivationReservationV1,
}

impl ActiveRemoteMcapBundle {
    pub(crate) fn manifest_arc_v1(&self) -> &Arc<RemoteMcapManifestV1> {
        &self.manifest
    }

    pub(crate) fn store_projection_v1(&self) -> &RemoteStoreProjectionV1 {
        &self.store_projection
    }

    pub(crate) const fn controller_identity_v1(&self) -> RemoteMcapControllerV1 {
        self.controller
    }

    pub(crate) const fn use_state_v1(&self) -> RemoteRecordingUseStateV1 {
        self.use_state
    }

    pub(crate) const fn store_v1(&self) -> &PublicRemoteStoreV1 {
        &self.store
    }
}

impl fmt::Debug for ActiveRemoteMcapBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveRemoteMcapBundle")
            .field("manifest", &"<opaque Arc>")
            .field("store_projection", &self.store_projection)
            .field("navigation", &self.navigation)
            .field("initial_gate", &self.initial_gate)
            .field("controller", &self.controller)
            .field("use_state", &self.use_state)
            .field("store", &self.store)
            .field("reservation_bytes", &self.reservation.bytes_v1())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapActivationErrorV1 {
    SlotOccupied,
    NotOpening,
    StaleActivation,
    StoreCreationFailed,
    OpeningTokenExhausted,
    ReservationLimitExceeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapSlotPhaseV1 {
    Vacant,
    Opening,
    Active,
}

pub(crate) enum RemoteMcapSlotV1 {
    Vacant,
    Opening {
        opening_token: RemoteMcapOpeningTokenV1,
        source_token: RemoteSourceTokenV1,
    },
    Active {
        opening_token: RemoteMcapOpeningTokenV1,
        source_token: RemoteSourceTokenV1,
        bundle: Box<ActiveRemoteMcapBundle>,
    },
}

impl fmt::Debug for RemoteMcapSlotV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vacant => formatter.write_str("Vacant"),
            Self::Opening {
                opening_token,
                source_token,
            } => formatter
                .debug_struct("Opening")
                .field("opening_token", opening_token)
                .field("source_token", source_token)
                .finish(),
            Self::Active {
                opening_token,
                source_token,
                bundle,
            } => formatter
                .debug_struct("Active")
                .field("opening_token", opening_token)
                .field("source_token", source_token)
                .field("bundle", bundle)
                .finish(),
        }
    }
}

static NEXT_REMOTE_MCAP_OPENING_SLOT: AtomicU64 = AtomicU64::new(1);

fn allocate_opening_token_v1(
    source_token: RemoteSourceTokenV1,
) -> Result<RemoteMcapOpeningTokenV1, RemoteMcapActivationErrorV1> {
    let slot = NEXT_REMOTE_MCAP_OPENING_SLOT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |slot| {
            slot.checked_add(1)
        })
        .map_err(|_slot| RemoteMcapActivationErrorV1::OpeningTokenExhausted)?;
    Ok(RemoteMcapOpeningTokenV1 {
        slot,
        source: source_token,
    })
}

/// Singleton remote slot for the activation state machine.
pub(crate) struct RemoteMcapSessionV1 {
    slot: RemoteMcapSlotV1,
}

impl Default for RemoteMcapSessionV1 {
    fn default() -> Self {
        Self::new_v1()
    }
}

impl RemoteMcapSessionV1 {
    pub(crate) const fn new_v1() -> Self {
        Self {
            slot: RemoteMcapSlotV1::Vacant,
        }
    }

    pub(crate) const fn slot_phase_v1(&self) -> RemoteMcapSlotPhaseV1 {
        match self.slot {
            RemoteMcapSlotV1::Vacant => RemoteMcapSlotPhaseV1::Vacant,
            RemoteMcapSlotV1::Opening { .. } => RemoteMcapSlotPhaseV1::Opening,
            RemoteMcapSlotV1::Active { .. } => RemoteMcapSlotPhaseV1::Active,
        }
    }

    pub(crate) fn active_bundle_v1(&self) -> Option<&ActiveRemoteMcapBundle> {
        match &self.slot {
            RemoteMcapSlotV1::Active { bundle, .. } => Some(bundle.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn begin_opening_v1(
        &mut self,
        source_token: RemoteSourceTokenV1,
    ) -> Result<RemoteMcapOpeningTokenV1, RemoteMcapActivationErrorV1> {
        if !matches!(self.slot, RemoteMcapSlotV1::Vacant) {
            return Err(RemoteMcapActivationErrorV1::SlotOccupied);
        }
        let opening_token = allocate_opening_token_v1(source_token)?;
        self.slot = RemoteMcapSlotV1::Opening {
            opening_token,
            source_token,
        };
        Ok(opening_token)
    }

    pub(crate) fn activate_v1(
        &mut self,
        opening_token: RemoteMcapOpeningTokenV1,
        opened: OpenedRemoteMcap,
        install_store: impl FnOnce(
            &RemoteStoreProjectionV1,
        ) -> Result<PublicRemoteStoreV1, RemoteMcapActivationErrorV1>,
    ) -> Result<&ActiveRemoteMcapBundle, RemoteMcapActivationErrorV1> {
        let (expected_token, expected_source) = match &self.slot {
            RemoteMcapSlotV1::Opening {
                opening_token,
                source_token,
            } => (*opening_token, *source_token),
            _ => return Err(RemoteMcapActivationErrorV1::NotOpening),
        };
        if opening_token != expected_token || opening_token.source_token_v1() != expected_source {
            return Err(RemoteMcapActivationErrorV1::StaleActivation);
        }

        // The public Store is the first allocation/effect of activation and is created only after
        // the token and phase checks above.
        let store = install_store(opened.store_projection_v1())
            .map_err(|_error| RemoteMcapActivationErrorV1::StoreCreationFailed)?;

        let OpenedRemoteMcap {
            manifest,
            store_projection,
            navigation,
            initial_gate,
            controller,
            use_state,
            reservation,
        } = opened;

        self.slot = RemoteMcapSlotV1::Active {
            opening_token: expected_token,
            source_token: expected_source,
            bundle: Box::new(ActiveRemoteMcapBundle {
                manifest,
                store_projection,
                navigation,
                initial_gate,
                controller,
                use_state,
                store,
                reservation,
            }),
        };

        Ok(self
            .active_bundle_v1()
            .expect("the slot was just replaced with an Active bundle"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(token: u64) -> RemoteSourceTokenV1 {
        RemoteSourceTokenV1::new_for_test_v1(token)
    }

    fn manifest(session: u128, generation: u64) -> Arc<RemoteMcapManifestV1> {
        Arc::new(RemoteMcapManifestV1::new_v1(session, generation))
    }

    fn store_projection() -> RemoteStoreProjectionV1 {
        RemoteStoreProjectionV1 {
            store_id: StoreId::recording("activation-test", "recording"),
            canonical_timeline: TimelineName::log_time(),
        }
    }

    fn navigation() -> RemoteCanonicalNavigationV1 {
        RemoteCanonicalNavigationV1 {
            timeline: TimelineName::log_time(),
            committed_cursor: None,
            play_state: RemotePlayStateV1::Paused,
        }
    }

    fn initial_gate() -> RemoteInitialPresentationGateV1 {
        RemoteInitialPresentationGateV1 {
            facade_instance: 1,
            presentation_epoch: 1,
            committed_time: None,
            live_lease_count: 0,
        }
    }

    fn ledger() -> RemoteActivationReservationLedgerV1 {
        RemoteActivationReservationLedgerV1::new_v1(1_024)
    }

    #[test]
    fn opening_does_not_create_a_public_store() {
        let mut session = RemoteMcapSessionV1::new_v1();
        let token = session.begin_opening_v1(source(1)).unwrap();
        assert_eq!(session.slot_phase_v1(), RemoteMcapSlotPhaseV1::Opening);
        assert!(session.active_bundle_v1().is_none());
        assert_eq!(token.source_token_v1(), source(1));
    }

    #[test]
    fn successful_activation_installs_the_same_manifest_and_controller() {
        let mut session = RemoteMcapSessionV1::new_v1();
        let token = session.begin_opening_v1(source(1)).unwrap();
        let manifest_owner = manifest(0x1234_5678_9abc_def0, 4);
        let controller = RemoteMcapControllerV1::new_v1(99);
        let ledger = ledger();
        let reservation = ledger.acquire_v1(40).unwrap();
        let opened = OpenedRemoteMcap::new_v1(
            Arc::clone(&manifest_owner),
            store_projection(),
            navigation(),
            initial_gate(),
            controller,
            RemoteRecordingUseStateV1::Foreground,
            reservation,
        );

        {
            let active = session
                .activate_v1(token, opened, |projection| {
                    Ok(PublicRemoteStoreV1::new_v1(projection.store_id.clone(), 7))
                })
                .unwrap();

            assert!(Arc::ptr_eq(active.manifest_arc_v1(), &manifest_owner));
            assert_eq!(active.controller_identity_v1(), controller);
            assert_eq!(active.use_state_v1(), RemoteRecordingUseStateV1::Foreground);
            assert_eq!(active.store_v1().generation_v1(), 7);
        }

        assert_eq!(session.slot_phase_v1(), RemoteMcapSlotPhaseV1::Active);
        assert_eq!(ledger.active_bytes_v1(), 40);
    }

    #[test]
    fn active_slot_is_orthogonal_to_foreground() {
        let mut session = RemoteMcapSessionV1::new_v1();
        let token = session.begin_opening_v1(source(1)).unwrap();
        let ledger = ledger();
        let reservation = ledger.acquire_v1(16).unwrap();
        let opened = OpenedRemoteMcap::new_v1(
            manifest(1, 2),
            store_projection(),
            navigation(),
            initial_gate(),
            RemoteMcapControllerV1::new_v1(1),
            RemoteRecordingUseStateV1::CatalogOnly,
            reservation,
        );
        let active = session
            .activate_v1(token, opened, |projection| {
                Ok(PublicRemoteStoreV1::new_v1(projection.store_id.clone(), 8))
            })
            .unwrap();

        assert!(active.manifest_arc_v1().identity_v1().0 > 0);
        assert_eq!(
            active.use_state_v1(),
            RemoteRecordingUseStateV1::CatalogOnly
        );
    }

    #[test]
    fn stale_activation_does_not_install_a_store_or_leak_reservation() {
        let mut session = RemoteMcapSessionV1::new_v1();
        let token = session.begin_opening_v1(source(1)).unwrap();
        let ledger = ledger();
        let reservation = ledger.acquire_v1(32).unwrap();
        let opened = OpenedRemoteMcap::new_v1(
            manifest(2, 3),
            store_projection(),
            navigation(),
            initial_gate(),
            RemoteMcapControllerV1::new_v1(1),
            RemoteRecordingUseStateV1::Inactive,
            reservation,
        );
        let stale = RemoteMcapOpeningTokenV1 {
            slot: token.slot_v1() + 1,
            source: source(1),
        };

        let mut store_installations = 0_u64;
        let result = session.activate_v1(stale, opened, |_projection| {
            store_installations += 1;
            Ok(PublicRemoteStoreV1::new_v1(
                StoreId::recording("activation-test", "stale"),
                9,
            ))
        });

        assert_eq!(
            result.err(),
            Some(RemoteMcapActivationErrorV1::StaleActivation)
        );
        assert_eq!(store_installations, 0);
        assert_eq!(session.slot_phase_v1(), RemoteMcapSlotPhaseV1::Opening);
        assert!(session.active_bundle_v1().is_none());
        assert_eq!(ledger.active_bytes_v1(), 0);
    }

    #[test]
    fn store_creation_failure_drops_the_uninstalled_bundle() {
        let mut session = RemoteMcapSessionV1::new_v1();
        let token = session.begin_opening_v1(source(1)).unwrap();
        let ledger = ledger();
        let reservation = ledger.acquire_v1(48).unwrap();
        let opened = OpenedRemoteMcap::new_v1(
            manifest(3, 4),
            store_projection(),
            navigation(),
            initial_gate(),
            RemoteMcapControllerV1::new_v1(2),
            RemoteRecordingUseStateV1::Inactive,
            reservation,
        );

        assert_eq!(
            session
                .activate_v1(token, opened, |_projection| Err(
                    RemoteMcapActivationErrorV1::StoreCreationFailed
                ))
                .err(),
            Some(RemoteMcapActivationErrorV1::StoreCreationFailed)
        );
        assert_eq!(session.slot_phase_v1(), RemoteMcapSlotPhaseV1::Opening);
        assert!(session.active_bundle_v1().is_none());
        assert_eq!(ledger.active_bytes_v1(), 0);
    }
}
