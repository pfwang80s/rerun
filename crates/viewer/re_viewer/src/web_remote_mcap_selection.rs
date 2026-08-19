//! Production-disarmed remote-MCAP selection authority.
//!
//! This module models the user-navigation/programmatic-selection split required by MCAP-105.
//! It owns only revision-tagged authority and does not construct or publish a Viewer, `StoreHub`,
//! `StoreBundle`, `EntityDb`, navigation reducer, or ordinary transport selection state.

#![allow(dead_code)]

use std::fmt;

/// Remote recording open behavior.
///
/// Only [`RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect`] can acquire initial selection
/// authority. `Open` and `Background` install an entry without selecting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapRecordingOpenBehaviorV1 {
    OpenAndSelect,
    Open,
    Background,
}

impl RemoteMcapRecordingOpenBehaviorV1 {
    pub(crate) const fn acquires_initial_selection_authority_v1(self) -> bool {
        matches!(self, Self::OpenAndSelect)
    }
}

/// Monotonic user navigation revision.
///
/// This is advanced only by user navigation. Programmatic selection activation must not advance
/// it, and stale programmatic intents are invalid once it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UserNavigationRevisionV1(u64);

impl UserNavigationRevisionV1 {
    pub(crate) const fn new_v1(revision: u64) -> Self {
        Self(revision)
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }

    pub(crate) const fn next_v1(self) -> Result<Self, RemoteSelectionErrorV1> {
        match self.0.checked_add(1) {
            Some(next) => Ok(Self(next)),
            None => Err(RemoteSelectionErrorV1::UserNavigationRevisionOverflow),
        }
    }
}

/// Opaque operation identity retained in a programmatic intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RemoteSelectionOperationIdV1(u64);

impl RemoteSelectionOperationIdV1 {
    pub(crate) const fn new_v1(operation_id: u64) -> Option<Self> {
        if operation_id == 0 {
            None
        } else {
            Some(Self(operation_id))
        }
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

/// Batch-local ordinal for one strict atomic batch item.
///
/// The ordinal is intentionally not a global browser-ingress sequence. The checked upper bound
/// makes the no-global-sequence rule explicit without adding another global counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RemoteBatchOrdinalV1(u64);

impl RemoteBatchOrdinalV1 {
    pub(crate) const fn new_v1(ordinal: u64) -> Result<Self, RemoteSelectionErrorV1> {
        if ordinal == u64::MAX {
            Err(RemoteSelectionErrorV1::StrictBatchOrdinalOverflow)
        } else {
            Ok(Self(ordinal))
        }
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

/// Frozen remote-MCAP programmatic selection intent.
///
/// The intent separates the operation behavior from the user navigation revision that was current
/// when the intent was created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteProgrammaticSelectionIntentV1 {
    operation_id: RemoteSelectionOperationIdV1,
    behavior: RemoteMcapRecordingOpenBehaviorV1,
    frozen_user_navigation_revision: UserNavigationRevisionV1,
    batch_local_ordinal: Option<RemoteBatchOrdinalV1>,
}

impl RemoteProgrammaticSelectionIntentV1 {
    pub(crate) const fn new_v1(
        operation_id: RemoteSelectionOperationIdV1,
        behavior: RemoteMcapRecordingOpenBehaviorV1,
        frozen_user_navigation_revision: UserNavigationRevisionV1,
        batch_local_ordinal: Option<RemoteBatchOrdinalV1>,
    ) -> Self {
        Self {
            operation_id,
            behavior,
            frozen_user_navigation_revision,
            batch_local_ordinal,
        }
    }

    pub(crate) const fn operation_id_v1(self) -> RemoteSelectionOperationIdV1 {
        self.operation_id
    }

    pub(crate) const fn behavior_v1(self) -> RemoteMcapRecordingOpenBehaviorV1 {
        self.behavior
    }

    pub(crate) const fn frozen_user_navigation_revision_v1(self) -> UserNavigationRevisionV1 {
        self.frozen_user_navigation_revision
    }

    pub(crate) const fn batch_local_ordinal_v1(self) -> Option<RemoteBatchOrdinalV1> {
        self.batch_local_ordinal
    }

    pub(crate) const fn authority_v1(self) -> RemoteSelectionAuthorityV1 {
        if self.behavior.acquires_initial_selection_authority_v1() {
            RemoteSelectionAuthorityV1::Programmatic(self)
        } else {
            RemoteSelectionAuthorityV1::UserNavigation(self.frozen_user_navigation_revision)
        }
    }

    pub(crate) const fn is_stale_for_v1(self, current_revision: UserNavigationRevisionV1) -> bool {
        self.frozen_user_navigation_revision.get_v1() != current_revision.get_v1()
    }

    pub(crate) fn effect_v1(
        self,
        current_revision: UserNavigationRevisionV1,
    ) -> Result<RemoteSelectionEffectV1, RemoteSelectionErrorV1> {
        match self.behavior {
            RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect
                if self.is_stale_for_v1(current_revision) =>
            {
                Err(RemoteSelectionErrorV1::StaleProgrammaticSelectionIntent)
            }
            RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect => {
                Ok(RemoteSelectionEffectV1::SelectRecording {
                    operation_id: self.operation_id,
                    frozen_user_navigation_revision: self.frozen_user_navigation_revision,
                })
            }
            RemoteMcapRecordingOpenBehaviorV1::Open => {
                Ok(RemoteSelectionEffectV1::InstallPanelEntry {
                    operation_id: self.operation_id,
                })
            }
            RemoteMcapRecordingOpenBehaviorV1::Background => {
                Ok(RemoteSelectionEffectV1::InstallCatalogEntry {
                    operation_id: self.operation_id,
                })
            }
        }
    }
}

/// Selection authority.
///
/// `Programmatic` is present only when an `OpenAndSelect` intent has current selection authority.
/// `UserNavigation` is a non-programmatic fallback and never turns into an automatic selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSelectionAuthorityV1 {
    UserNavigation(UserNavigationRevisionV1),
    Programmatic(RemoteProgrammaticSelectionIntentV1),
}

impl RemoteSelectionAuthorityV1 {
    pub(crate) const fn is_programmatic_v1(self) -> bool {
        matches!(self, Self::Programmatic(_))
    }

    pub(crate) fn effect_v1(
        self,
        current_revision: UserNavigationRevisionV1,
    ) -> Result<RemoteSelectionEffectV1, RemoteSelectionErrorV1> {
        match self {
            Self::UserNavigation(_) => Ok(RemoteSelectionEffectV1::NoRemoteSelectionEffect),
            Self::Programmatic(intent) => intent.effect_v1(current_revision),
        }
    }
}

/// Observable result of applying selection authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSelectionEffectV1 {
    SelectRecording {
        operation_id: RemoteSelectionOperationIdV1,
        frozen_user_navigation_revision: UserNavigationRevisionV1,
    },
    InstallPanelEntry {
        operation_id: RemoteSelectionOperationIdV1,
    },
    InstallCatalogEntry {
        operation_id: RemoteSelectionOperationIdV1,
    },
    NoRemoteSelectionEffect,
}

/// One strict atomic batch item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteStrictBatchItemV1 {
    operation_id: RemoteSelectionOperationIdV1,
    behavior: RemoteMcapRecordingOpenBehaviorV1,
    batch_ordinal: RemoteBatchOrdinalV1,
}

impl RemoteStrictBatchItemV1 {
    pub(crate) const fn new_v1(
        operation_id: RemoteSelectionOperationIdV1,
        behavior: RemoteMcapRecordingOpenBehaviorV1,
        batch_ordinal: RemoteBatchOrdinalV1,
    ) -> Self {
        Self {
            operation_id,
            behavior,
            batch_ordinal,
        }
    }
}

/// Batch-local winner selected from one strict atomic batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteStrictBatchSelectionV1 {
    winner: Option<RemoteProgrammaticSelectionIntentV1>,
    operation_count: usize,
}

impl RemoteStrictBatchSelectionV1 {
    pub(crate) const fn winner_v1(self) -> Option<RemoteProgrammaticSelectionIntentV1> {
        self.winner
    }

    pub(crate) const fn operation_count_v1(self) -> usize {
        self.operation_count
    }
}

/// Authority and effect returned by one compatibility remote operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCompatibilitySelectionOutcomeV1 {
    pub(crate) authority: RemoteSelectionAuthorityV1,
    pub(crate) effect: RemoteSelectionEffectV1,
}

/// Selection state machine failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSelectionErrorV1 {
    UserNavigationRevisionOverflow,
    StrictBatchOrdinalOverflow,
    StrictBatchOrdinalNotMonotonic,
    DuplicateStrictBatchOperationId,
    EmptyStrictBatch,
    StaleProgrammaticSelectionIntent,
    CompatibilityCompletionSequenceOverflow,
}

impl fmt::Display for RemoteSelectionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UserNavigationRevisionOverflow => {
                formatter.write_str("user navigation revision overflow")
            }
            Self::StrictBatchOrdinalOverflow => {
                formatter.write_str("strict batch ordinal overflow")
            }
            Self::StrictBatchOrdinalNotMonotonic => {
                formatter.write_str("strict batch ordinal is not monotonic")
            }
            Self::DuplicateStrictBatchOperationId => {
                formatter.write_str("duplicate strict batch operation id")
            }
            Self::EmptyStrictBatch => formatter.write_str("strict batch is empty"),
            Self::StaleProgrammaticSelectionIntent => {
                formatter.write_str("stale programmatic selection intent")
            }
            Self::CompatibilityCompletionSequenceOverflow => {
                formatter.write_str("compatibility completion sequence overflow")
            }
        }
    }
}

/// Storage-free selection authority state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSelectionStateMachineV1 {
    user_navigation_revision: UserNavigationRevisionV1,
    selected_programmatic_intent: Option<RemoteProgrammaticSelectionIntentV1>,
}

impl RemoteSelectionStateMachineV1 {
    pub(crate) const fn new_v1(user_navigation_revision: UserNavigationRevisionV1) -> Self {
        Self {
            user_navigation_revision,
            selected_programmatic_intent: None,
        }
    }

    pub(crate) const fn user_navigation_revision_v1(&self) -> UserNavigationRevisionV1 {
        self.user_navigation_revision
    }

    pub(crate) const fn selected_programmatic_intent_v1(
        &self,
    ) -> Option<RemoteProgrammaticSelectionIntentV1> {
        self.selected_programmatic_intent
    }

    /// Advance the user navigation revision and invalidate any previous programmatic intent.
    pub(crate) fn record_user_navigation_v1(
        &mut self,
    ) -> Result<UserNavigationRevisionV1, RemoteSelectionErrorV1> {
        let next_revision = self.user_navigation_revision.next_v1()?;
        self.user_navigation_revision = next_revision;
        self.selected_programmatic_intent = None;
        Ok(next_revision)
    }

    /// Freeze one compatibility remote operation at the current synchronous call order.
    ///
    /// This method intentionally does not advance the user navigation revision.
    pub(crate) fn freeze_compatibility_remote_operation_v1(
        &mut self,
        operation_id: RemoteSelectionOperationIdV1,
        behavior: RemoteMcapRecordingOpenBehaviorV1,
    ) -> Result<RemoteCompatibilitySelectionOutcomeV1, RemoteSelectionErrorV1> {
        let intent = RemoteProgrammaticSelectionIntentV1::new_v1(
            operation_id,
            behavior,
            self.user_navigation_revision,
            None,
        );
        let authority = intent.authority_v1();
        let effect = intent.effect_v1(self.user_navigation_revision)?;

        if behavior.acquires_initial_selection_authority_v1() {
            self.selected_programmatic_intent = Some(intent);
        }

        Ok(RemoteCompatibilitySelectionOutcomeV1 { authority, effect })
    }

    /// Reconcile an already frozen authority without advancing user navigation.
    pub(crate) fn reconcile_remote_authority_v1(
        &self,
        authority: RemoteSelectionAuthorityV1,
    ) -> Result<RemoteSelectionEffectV1, RemoteSelectionErrorV1> {
        authority.effect_v1(self.user_navigation_revision)
    }

    /// Apply one strict atomic batch and return only the batch-local winner.
    ///
    /// `Open` and `Background` never become a winner, so a batch without `OpenAndSelect` leaves the
    /// previous selection authority unchanged. The batch is validated before the winner is stored.
    pub(crate) fn apply_strict_atomic_batch_v1(
        &mut self,
        items: &[RemoteStrictBatchItemV1],
    ) -> Result<RemoteStrictBatchSelectionV1, RemoteSelectionErrorV1> {
        let winner = select_strict_atomic_batch_winner_v1(self.user_navigation_revision, items)?;
        if let Some(intent) = winner {
            self.selected_programmatic_intent = Some(intent);
        }

        Ok(RemoteStrictBatchSelectionV1 {
            winner,
            operation_count: items.len(),
        })
    }
}

fn select_strict_atomic_batch_winner_v1(
    current_revision: UserNavigationRevisionV1,
    items: &[RemoteStrictBatchItemV1],
) -> Result<Option<RemoteProgrammaticSelectionIntentV1>, RemoteSelectionErrorV1> {
    if items.is_empty() {
        return Err(RemoteSelectionErrorV1::EmptyStrictBatch);
    }

    let mut previous_ordinal = None;
    let mut winner = None;

    for (index, item) in items.iter().enumerate() {
        if items[..index]
            .iter()
            .any(|earlier| earlier.operation_id == item.operation_id)
        {
            return Err(RemoteSelectionErrorV1::DuplicateStrictBatchOperationId);
        }

        if let Some(previous) = previous_ordinal
            && item.batch_ordinal <= previous
        {
            return Err(RemoteSelectionErrorV1::StrictBatchOrdinalNotMonotonic);
        }
        previous_ordinal = Some(item.batch_ordinal);

        if item.behavior.acquires_initial_selection_authority_v1() {
            winner = Some(RemoteProgrammaticSelectionIntentV1::new_v1(
                item.operation_id,
                item.behavior,
                current_revision,
                Some(item.batch_ordinal),
            ));
        }
    }

    Ok(winner)
}

/// Ingress classifier proving nonremote routes do not enter the remote selection state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSelectionIngressKindV1 {
    Native,
    Legacy,
    GrpcMessageProxy,
    Redap,
    NonRemoteCompatibility,
    RemoteCompatibility,
    RemoteStrictAtomicBatch,
}

impl RemoteSelectionIngressKindV1 {
    pub(crate) const fn participates_in_remote_selection_state_machine_v1(self) -> bool {
        matches!(
            self,
            Self::RemoteCompatibility | Self::RemoteStrictAtomicBatch
        )
    }
}

/// Sequence marker for existing non-MCAP compatibility completion ordering.
///
/// The sequence is supplied by the existing compatibility path; this module does not create a
/// global browser-ingress sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RemoteCompatibilityCompletionSequenceV1(u64);

impl RemoteCompatibilityCompletionSequenceV1 {
    pub(crate) const fn new_v1(sequence: u64) -> Result<Self, RemoteSelectionErrorV1> {
        if sequence == u64::MAX {
            Err(RemoteSelectionErrorV1::CompatibilityCompletionSequenceOverflow)
        } else {
            Ok(Self(sequence))
        }
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

/// Opaque identity for an existing non-MCAP compatibility completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct NonRemoteCompatibilitySelectionIdentityV1(u64);

impl NonRemoteCompatibilitySelectionIdentityV1 {
    pub(crate) const fn new_v1(identity: u64) -> Option<Self> {
        if identity == 0 {
            None
        } else {
            Some(Self(identity))
        }
    }

    pub(crate) const fn get_v1(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NonRemoteCompatibilityCompletionV1 {
    identity: NonRemoteCompatibilitySelectionIdentityV1,
    sequence: RemoteCompatibilityCompletionSequenceV1,
}

/// Isolated last-completion-wins selection for existing non-MCAP compatibility URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NonRemoteCompatibilitySelectionV1 {
    latest_completion: Option<NonRemoteCompatibilityCompletionV1>,
}

impl NonRemoteCompatibilitySelectionV1 {
    pub(crate) const fn new_v1() -> Self {
        Self {
            latest_completion: None,
        }
    }

    pub(crate) const fn selected_v1(&self) -> Option<NonRemoteCompatibilitySelectionIdentityV1> {
        match self.latest_completion {
            Some(completion) => Some(completion.identity),
            None => None,
        }
    }

    pub(crate) fn complete_v1(
        &mut self,
        identity: NonRemoteCompatibilitySelectionIdentityV1,
        sequence: RemoteCompatibilityCompletionSequenceV1,
    ) {
        let replace = match self.latest_completion {
            Some(latest) => sequence >= latest.sequence,
            None => true,
        };

        if replace {
            self.latest_completion =
                Some(NonRemoteCompatibilityCompletionV1 { identity, sequence });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn operation(id: u64) -> RemoteSelectionOperationIdV1 {
        match RemoteSelectionOperationIdV1::new_v1(id) {
            Some(operation) => operation,
            None => panic!("test operation id must be non-zero"),
        }
    }

    const fn ordinal(value: u64) -> RemoteBatchOrdinalV1 {
        match RemoteBatchOrdinalV1::new_v1(value) {
            Ok(ordinal) => ordinal,
            Err(_) => panic!("test batch ordinal must not overflow"),
        }
    }

    const fn sequence(value: u64) -> RemoteCompatibilityCompletionSequenceV1 {
        match RemoteCompatibilityCompletionSequenceV1::new_v1(value) {
            Ok(sequence) => sequence,
            Err(_) => panic!("test completion sequence must not overflow"),
        }
    }

    fn item(
        id: u64,
        behavior: RemoteMcapRecordingOpenBehaviorV1,
        batch_ordinal: u64,
    ) -> RemoteStrictBatchItemV1 {
        RemoteStrictBatchItemV1::new_v1(operation(id), behavior, ordinal(batch_ordinal))
    }

    #[test]
    fn user_navigation_revision_overflow_returns_checked_error() {
        let revision = UserNavigationRevisionV1::new_v1(u64::MAX);
        assert_eq!(
            revision.next_v1(),
            Err(RemoteSelectionErrorV1::UserNavigationRevisionOverflow)
        );

        let penultimate = UserNavigationRevisionV1::new_v1(u64::MAX - 1);
        assert_eq!(
            penultimate.next_v1(),
            Ok(UserNavigationRevisionV1::new_v1(u64::MAX))
        );
    }

    #[test]
    fn remote_user_navigation_invalidates_old_programmatic_intent() {
        let mut state = RemoteSelectionStateMachineV1::new_v1(UserNavigationRevisionV1::new_v1(10));
        let outcome = state
            .freeze_compatibility_remote_operation_v1(
                operation(1),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
            )
            .unwrap();
        assert_eq!(
            outcome.effect,
            RemoteSelectionEffectV1::SelectRecording {
                operation_id: operation(1),
                frozen_user_navigation_revision: UserNavigationRevisionV1::new_v1(10),
            }
        );
        assert!(state.selected_programmatic_intent_v1().is_some());

        assert_eq!(
            state.record_user_navigation_v1(),
            Ok(UserNavigationRevisionV1::new_v1(11))
        );
        assert!(state.selected_programmatic_intent_v1().is_none());
        assert_eq!(
            state.reconcile_remote_authority_v1(outcome.authority),
            Err(RemoteSelectionErrorV1::StaleProgrammaticSelectionIntent)
        );
    }

    #[test]
    fn compatibility_remote_authority_freeze_does_not_advance_user_revision() {
        let mut state = RemoteSelectionStateMachineV1::new_v1(UserNavigationRevisionV1::new_v1(5));

        let outcome = state
            .freeze_compatibility_remote_operation_v1(
                operation(7),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
            )
            .unwrap();

        assert_eq!(
            state.user_navigation_revision_v1(),
            UserNavigationRevisionV1::new_v1(5)
        );
        assert_eq!(
            state.reconcile_remote_authority_v1(outcome.authority),
            Ok(RemoteSelectionEffectV1::SelectRecording {
                operation_id: operation(7),
                frozen_user_navigation_revision: UserNavigationRevisionV1::new_v1(5),
            })
        );
        assert_eq!(
            state.user_navigation_revision_v1(),
            UserNavigationRevisionV1::new_v1(5)
        );
    }

    #[test]
    fn compatibility_open_and_background_do_not_acquire_selection_authority() {
        let mut state = RemoteSelectionStateMachineV1::new_v1(UserNavigationRevisionV1::new_v1(1));

        let open = state
            .freeze_compatibility_remote_operation_v1(
                operation(2),
                RemoteMcapRecordingOpenBehaviorV1::Open,
            )
            .unwrap();
        assert!(!open.authority.is_programmatic_v1());
        assert_eq!(
            open.effect,
            RemoteSelectionEffectV1::InstallPanelEntry {
                operation_id: operation(2),
            }
        );
        assert!(state.selected_programmatic_intent_v1().is_none());

        let background = state
            .freeze_compatibility_remote_operation_v1(
                operation(3),
                RemoteMcapRecordingOpenBehaviorV1::Background,
            )
            .unwrap();
        assert!(!background.authority.is_programmatic_v1());
        assert_eq!(
            background.effect,
            RemoteSelectionEffectV1::InstallCatalogEntry {
                operation_id: operation(3),
            }
        );
        assert!(state.selected_programmatic_intent_v1().is_none());
    }

    #[test]
    fn strict_atomic_batch_uses_last_open_and_select_winner() {
        let mut state = RemoteSelectionStateMachineV1::new_v1(UserNavigationRevisionV1::new_v1(2));
        let items = [
            item(1, RemoteMcapRecordingOpenBehaviorV1::Open, 1),
            item(2, RemoteMcapRecordingOpenBehaviorV1::Background, 2),
            item(3, RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect, 3),
            item(4, RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect, 4),
        ];

        let result = state.apply_strict_atomic_batch_v1(&items).unwrap();
        let winner = result.winner_v1().unwrap();
        assert_eq!(winner.operation_id_v1(), operation(4));
        assert_eq!(winner.batch_local_ordinal_v1(), Some(ordinal(4)));
        assert_eq!(result.operation_count_v1(), 4);
        assert_eq!(state.selected_programmatic_intent_v1(), Some(winner));
    }

    #[test]
    fn strict_atomic_batch_without_open_and_select_has_no_winner() {
        let mut state = RemoteSelectionStateMachineV1::new_v1(UserNavigationRevisionV1::new_v1(3));
        let items = [
            item(5, RemoteMcapRecordingOpenBehaviorV1::Open, 1),
            item(6, RemoteMcapRecordingOpenBehaviorV1::Background, 2),
        ];

        let result = state.apply_strict_atomic_batch_v1(&items).unwrap();
        assert_eq!(result.winner_v1(), None);
        assert_eq!(state.selected_programmatic_intent_v1(), None);
    }

    #[test]
    fn strict_batch_ordinal_overflow_returns_checked_error() {
        assert_eq!(
            RemoteBatchOrdinalV1::new_v1(u64::MAX),
            Err(RemoteSelectionErrorV1::StrictBatchOrdinalOverflow)
        );
    }

    #[test]
    fn nonremote_ingress_kinds_do_not_participate() {
        for kind in [
            RemoteSelectionIngressKindV1::Native,
            RemoteSelectionIngressKindV1::Legacy,
            RemoteSelectionIngressKindV1::GrpcMessageProxy,
            RemoteSelectionIngressKindV1::Redap,
            RemoteSelectionIngressKindV1::NonRemoteCompatibility,
        ] {
            assert!(!kind.participates_in_remote_selection_state_machine_v1());
        }

        assert!(
            RemoteSelectionIngressKindV1::RemoteCompatibility
                .participates_in_remote_selection_state_machine_v1()
        );
        assert!(
            RemoteSelectionIngressKindV1::RemoteStrictAtomicBatch
                .participates_in_remote_selection_state_machine_v1()
        );
    }

    #[test]
    fn non_mcap_compatibility_urls_reverse_completion_still_last_completion_wins() {
        let mut nonremote = NonRemoteCompatibilitySelectionV1::new_v1();
        let Some(original_second) = NonRemoteCompatibilitySelectionIdentityV1::new_v1(2) else {
            panic!("test identity must be non-zero")
        };
        let Some(original_first) = NonRemoteCompatibilitySelectionIdentityV1::new_v1(1) else {
            panic!("test identity must be non-zero")
        };

        nonremote.complete_v1(original_second, sequence(1));
        nonremote.complete_v1(original_first, sequence(2));

        assert_eq!(nonremote.selected_v1(), Some(original_first));
    }
}
