//! Generation-checked remote-MCAP resume revalidation and bounded frame work.
//!
//! This is a page-control seam: it owns only remote work admitted by the page manager. Native
//! and ordinary Viewer scheduling is deliberately outside this type. A resume creates a fresh
//! execution generation; every slot, callback and retry must prove that generation before it can
//! run.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Upper bound for the number of slots, queued entries, and snapshots admitted in one resume.
pub const MAX_REMOTE_RESUME_SLOTS_V1: usize = 256;
pub const MAX_REMOTE_RESUME_SNAPSHOTS_V1: usize = 256;

/// Opaque identity of one slot managed by [`RemoteResumeRevalidationV1`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RemoteResumeSlotTokenV1 {
    slot: u64,
    token: u64,
    execution_generation: u64,
    issuer: u64,
}

impl fmt::Debug for RemoteResumeSlotTokenV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteResumeSlotTokenV1(..)")
    }
}

/// Opaque resume snapshot produced by [`RemoteResumeRevalidationV1`] from a live token.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RemoteResumeSlotSnapshotV1 {
    slot: u64,
    token: u64,
    execution_generation: u64,
    issuer: u64,
    kind: RemoteResumeWorkKindV1,
    facts: RemoteResumeSlotFactsV1,
}

impl fmt::Debug for RemoteResumeSlotSnapshotV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RemoteResumeSlotSnapshotV1(..)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumeWorkKindV1 {
    Completion,
    Opening,
    Decoder,
    RetryPending,
    Mutation,
}

impl RemoteResumeWorkKindV1 {
    const fn retry_pending(self) -> bool {
        matches!(self, Self::RetryPending)
    }

    const fn mutation_owned(self) -> bool {
        matches!(self, Self::Mutation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumeValidationFactV1 {
    Valid(u64),
    Invalid(u64),
}

impl RemoteResumeValidationFactV1 {
    const fn revision(self) -> u64 {
        match self {
            Self::Valid(revision) | Self::Invalid(revision) => revision,
        }
    }

    const fn valid(self) -> bool {
        matches!(self, Self::Valid(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumeReservationFactV1 {
    Valid,
    Invalid,
}

impl RemoteResumeReservationFactV1 {
    const fn valid(self) -> bool {
        matches!(self, Self::Valid)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteResumeSlotFactsV1 {
    validator_revision: u64,
    body_reader_revision: u64,
    validator_valid: bool,
    body_reader_valid: bool,
    mutation_owned: bool,
    reservation_valid: bool,
}

impl Default for RemoteResumeSlotFactsV1 {
    fn default() -> Self {
        Self {
            validator_revision: 0,
            body_reader_revision: 0,
            validator_valid: false,
            body_reader_valid: false,
            mutation_owned: false,
            reservation_valid: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumePhaseV1 {
    Hidden,
    Revalidating,
    Visible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteFrameSliceV1 {
    pub remote_started: u32,
    pub retry_started: u32,
    pub viewer_turns: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumeRevalidationResultV1 {
    Rebound(RemoteResumeSlotTokenV1),
    Dropped,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumeRevalidationErrorV1 {
    NotVisible,
    NotHidden,
    InvalidToken,
    Terminated,
    Overflow,
    SlotLimitExceeded,
    SnapshotLimitExceeded,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    token: u64,
    generation: u64,
    kind: RemoteResumeWorkKindV1,
    facts: RemoteResumeSlotFactsV1,
}

static NEXT_MANAGER_ISSUER_V1: AtomicU64 = AtomicU64::new(1);

fn next_manager_issuer_v1() -> u64 {
    NEXT_MANAGER_ISSUER_V1
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |issuer| {
            issuer.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("RemoteResumeRevalidationV1 issuer allocator exhausted"))
}

/// Remote-only resume coordinator with a bounded per-frame quantum.
pub struct RemoteResumeRevalidationV1 {
    generation: u64,
    phase: RemoteResumePhaseV1,
    terminated: bool,
    next_slot: u64,
    next_token: u64,
    issuer: u64,
    slots: BTreeMap<u64, Slot>,
    queue: VecDeque<u64>,
}

impl Default for RemoteResumeRevalidationV1 {
    fn default() -> Self {
        Self::new_v1()
    }
}

impl RemoteResumeRevalidationV1 {
    pub fn new_v1() -> Self {
        Self {
            generation: 0,
            phase: RemoteResumePhaseV1::Visible,
            terminated: false,
            next_slot: 1,
            next_token: 1,
            issuer: next_manager_issuer_v1(),
            slots: BTreeMap::new(),
            queue: VecDeque::new(),
        }
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn phase_v1(&self) -> RemoteResumePhaseV1 {
        self.phase
    }

    pub fn register_v1(
        &mut self,
        kind: RemoteResumeWorkKindV1,
    ) -> Result<RemoteResumeSlotTokenV1, RemoteResumeRevalidationErrorV1> {
        if self.terminated {
            return Err(RemoteResumeRevalidationErrorV1::Terminated);
        }
        if self.phase != RemoteResumePhaseV1::Visible {
            return Err(RemoteResumeRevalidationErrorV1::NotVisible);
        }
        if self.slots.len() >= MAX_REMOTE_RESUME_SLOTS_V1
            || self.queue.len() >= MAX_REMOTE_RESUME_SLOTS_V1
        {
            return Err(RemoteResumeRevalidationErrorV1::SlotLimitExceeded);
        }

        let slot = self.next_slot;
        let next_slot = slot
            .checked_add(1)
            .ok_or(RemoteResumeRevalidationErrorV1::Overflow)?;
        let token = self.next_token;
        let next_token = token
            .checked_add(1)
            .ok_or(RemoteResumeRevalidationErrorV1::Overflow)?;

        self.slots.insert(
            slot,
            Slot {
                token,
                generation: self.generation,
                kind,
                facts: RemoteResumeSlotFactsV1 {
                    mutation_owned: kind.mutation_owned(),
                    ..RemoteResumeSlotFactsV1::default()
                },
            },
        );
        self.queue.push_back(slot);
        self.next_slot = next_slot;
        self.next_token = next_token;

        Ok(RemoteResumeSlotTokenV1 {
            slot,
            token,
            execution_generation: self.generation,
            issuer: self.issuer,
        })
    }

    pub fn update_facts_v1(
        &mut self,
        token: RemoteResumeSlotTokenV1,
        validator: RemoteResumeValidationFactV1,
        body_reader: RemoteResumeValidationFactV1,
        reservation: RemoteResumeReservationFactV1,
    ) -> Result<(), RemoteResumeRevalidationErrorV1> {
        if self.terminated {
            return Err(RemoteResumeRevalidationErrorV1::Terminated);
        }
        if self.phase != RemoteResumePhaseV1::Visible {
            return Err(RemoteResumeRevalidationErrorV1::InvalidToken);
        }
        let kind = self
            .slot_for_token_identity(token)
            .ok_or(RemoteResumeRevalidationErrorV1::InvalidToken)?
            .kind;
        let slot = self
            .slots
            .get_mut(&token.slot)
            .expect("slot identity check returned a live slot");
        slot.facts = RemoteResumeSlotFactsV1 {
            validator_revision: validator.revision(),
            body_reader_revision: body_reader.revision(),
            validator_valid: validator.valid(),
            body_reader_valid: body_reader.valid(),
            mutation_owned: kind.mutation_owned(),
            reservation_valid: reservation.valid(),
        };
        Ok(())
    }

    pub fn snapshot_v1(
        &self,
        token: RemoteResumeSlotTokenV1,
    ) -> Result<RemoteResumeSlotSnapshotV1, RemoteResumeRevalidationErrorV1> {
        if self.terminated {
            return Err(RemoteResumeRevalidationErrorV1::Terminated);
        }
        if self.phase != RemoteResumePhaseV1::Hidden {
            return Err(RemoteResumeRevalidationErrorV1::NotHidden);
        }
        let slot = self
            .slot_for_token_identity(token)
            .ok_or(RemoteResumeRevalidationErrorV1::InvalidToken)?;
        let execution_generation = self
            .generation
            .checked_add(1)
            .ok_or(RemoteResumeRevalidationErrorV1::Overflow)?;

        Ok(RemoteResumeSlotSnapshotV1 {
            slot: token.slot,
            token: slot.token,
            execution_generation,
            issuer: self.issuer,
            kind: slot.kind,
            facts: slot.facts,
        })
    }

    pub fn hidden_v1(&mut self) -> Result<(), RemoteResumeRevalidationErrorV1> {
        if self.terminated {
            return Ok(());
        }
        if self.phase != RemoteResumePhaseV1::Visible {
            return Ok(());
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(RemoteResumeRevalidationErrorV1::Overflow)?;
        self.generation = generation;
        self.phase = RemoteResumePhaseV1::Hidden;
        self.queue.clear();
        Ok(())
    }

    /// Rebinds only slots whose token, generation, validator/body reader and ownership facts all
    /// match the fresh page baseline. Omitted and invalid slots are pruned.
    pub fn resume_v1(
        &mut self,
        snapshots: impl IntoIterator<Item = RemoteResumeSlotSnapshotV1>,
    ) -> Result<Vec<(u64, RemoteResumeRevalidationResultV1)>, RemoteResumeRevalidationErrorV1> {
        if self.terminated {
            return Err(RemoteResumeRevalidationErrorV1::Terminated);
        }
        if self.phase != RemoteResumePhaseV1::Hidden {
            return Err(RemoteResumeRevalidationErrorV1::NotHidden);
        }

        let snapshots: Vec<_> = snapshots
            .into_iter()
            .take(MAX_REMOTE_RESUME_SNAPSHOTS_V1 + 1)
            .collect();
        if snapshots.len() > MAX_REMOTE_RESUME_SNAPSHOTS_V1 {
            return Err(RemoteResumeRevalidationErrorV1::SnapshotLimitExceeded);
        }
        let next_generation = self
            .generation
            .checked_add(1)
            .ok_or(RemoteResumeRevalidationErrorV1::Overflow)?;

        self.phase = RemoteResumePhaseV1::Revalidating;
        let mut seen_slots = BTreeSet::new();
        let mut accepted_slots = BTreeSet::new();
        let mut outcomes = Vec::with_capacity(snapshots.len());

        for snapshot in snapshots {
            if !seen_slots.insert(snapshot.slot) {
                outcomes.push((snapshot.slot, RemoteResumeRevalidationResultV1::Stale));
                continue;
            }

            let Some(slot) = self.slots.get(&snapshot.slot).copied() else {
                outcomes.push((snapshot.slot, RemoteResumeRevalidationResultV1::Stale));
                continue;
            };

            let valid = snapshot.issuer == self.issuer
                && snapshot.token == slot.token
                && snapshot.execution_generation == next_generation
                && snapshot.kind == slot.kind
                && snapshot.facts == slot.facts
                && snapshot.facts.validator_valid
                && snapshot.facts.body_reader_valid
                && snapshot.facts.reservation_valid
                && snapshot.facts.mutation_owned == snapshot.kind.mutation_owned();
            if !valid {
                outcomes.push((snapshot.slot, RemoteResumeRevalidationResultV1::Dropped));
                continue;
            }

            accepted_slots.insert(snapshot.slot);
            outcomes.push((
                snapshot.slot,
                RemoteResumeRevalidationResultV1::Rebound(RemoteResumeSlotTokenV1 {
                    slot: snapshot.slot,
                    token: slot.token,
                    execution_generation: next_generation,
                    issuer: self.issuer,
                }),
            ));
        }

        self.slots.retain(|slot, _| accepted_slots.contains(slot));
        for slot in &accepted_slots {
            if let Some(slot_state) = self.slots.get_mut(slot) {
                slot_state.generation = next_generation;
            }
        }
        self.queue.clear();
        self.queue.extend(&accepted_slots);
        self.generation = next_generation;
        self.phase = RemoteResumePhaseV1::Visible;

        Ok(outcomes)
    }

    pub fn terminate_v1(&mut self) -> Result<(), RemoteResumeRevalidationErrorV1> {
        if self.terminated {
            return Ok(());
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(RemoteResumeRevalidationErrorV1::Overflow)?;
        self.generation = generation;
        self.terminated = true;
        self.phase = RemoteResumePhaseV1::Hidden;
        self.queue.clear();
        self.slots.clear();
        Ok(())
    }

    pub fn callback_is_current_v1(&self, token: RemoteResumeSlotTokenV1) -> bool {
        !self.terminated
            && self.phase == RemoteResumePhaseV1::Visible
            && token.issuer == self.issuer
            && token.execution_generation == self.generation
            && self
                .slots
                .get(&token.slot)
                .is_some_and(|slot| slot.token == token.token && slot.generation == self.generation)
    }

    /// Runs a bounded frame quantum.
    ///
    /// `max_remote_work` is the total work budget for the frame, including the optional viewer
    /// turn. At most one retry starts in a turn; if viewer work is available it consumes one unit
    /// before the remaining remote budget is dispatched.
    pub fn pump_frame_v1(
        &mut self,
        max_remote_work: u32,
        viewer_work_available: bool,
    ) -> RemoteFrameSliceV1 {
        let empty_frame = RemoteFrameSliceV1 {
            remote_started: 0,
            retry_started: 0,
            viewer_turns: viewer_work_available as u32,
        };
        if self.terminated || self.phase != RemoteResumePhaseV1::Visible || max_remote_work == 0 {
            return empty_frame;
        }

        let remote_budget = if viewer_work_available {
            max_remote_work.saturating_sub(1)
        } else {
            max_remote_work
        };
        let scan_budget = max_remote_work;
        let mut remote_started = 0;
        let mut retry_started = 0;
        let mut scanned = 0;
        let mut deferred = VecDeque::new();

        while remote_started < remote_budget && scanned < scan_budget {
            scanned += 1;
            let Some(slot_id) = self.queue.pop_front() else {
                break;
            };
            let Some(slot) = self.slots.get(&slot_id).copied() else {
                continue;
            };
            if slot.generation != self.generation {
                self.slots.remove(&slot_id);
                continue;
            }
            if slot.kind.retry_pending() && retry_started >= 1 {
                deferred.push_back(slot_id);
                continue;
            }

            remote_started += 1;
            if slot.kind.retry_pending() {
                retry_started += 1;
            }
            self.slots.remove(&slot_id);
        }

        self.queue.extend(deferred);
        RemoteFrameSliceV1 {
            remote_started,
            retry_started,
            viewer_turns: viewer_work_available as u32,
        }
    }

    fn slot_for_token_identity(&self, token: RemoteResumeSlotTokenV1) -> Option<&Slot> {
        if self.terminated || token.issuer != self.issuer {
            return None;
        }
        let slot = self.slots.get(&token.slot)?;
        (slot.token == token.token && slot.generation == token.execution_generation).then_some(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rebound_token(outcome: &(u64, RemoteResumeRevalidationResultV1)) -> RemoteResumeSlotTokenV1 {
        match outcome.1 {
            RemoteResumeRevalidationResultV1::Rebound(token) => token,
            _ => panic!("expected rebound outcome"),
        }
    }

    fn valid_facts(mutation_owned: bool) -> RemoteResumeSlotFactsV1 {
        RemoteResumeSlotFactsV1 {
            validator_revision: 7,
            body_reader_revision: 11,
            validator_valid: true,
            body_reader_valid: true,
            mutation_owned,
            reservation_valid: true,
        }
    }

    fn update_valid_facts(
        manager: &mut RemoteResumeRevalidationV1,
        token: RemoteResumeSlotTokenV1,
    ) {
        manager
            .update_facts_v1(
                token,
                RemoteResumeValidationFactV1::Valid(7),
                RemoteResumeValidationFactV1::Valid(11),
                RemoteResumeReservationFactV1::Valid,
            )
            .unwrap();
    }

    #[test]
    fn register_derives_retry_and_is_current_while_visible() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Opening)
            .unwrap();
        assert_eq!(manager.phase_v1(), RemoteResumePhaseV1::Visible);
        assert!(manager.callback_is_current_v1(token));
    }

    #[test]
    fn managers_have_distinct_identity_and_reject_old_capabilities() {
        let mut old_manager = RemoteResumeRevalidationV1::new_v1();
        let old_token = old_manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        update_valid_facts(&mut old_manager, old_token);
        old_manager.hidden_v1().unwrap();
        let old_snapshot = old_manager.snapshot_v1(old_token).unwrap();

        let mut new_manager = RemoteResumeRevalidationV1::new_v1();
        let new_token = new_manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        update_valid_facts(&mut new_manager, new_token);
        assert_eq!(
            new_manager.update_facts_v1(
                old_token,
                RemoteResumeValidationFactV1::Valid(7),
                RemoteResumeValidationFactV1::Valid(11),
                RemoteResumeReservationFactV1::Valid,
            ),
            Err(RemoteResumeRevalidationErrorV1::InvalidToken)
        );

        new_manager.hidden_v1().unwrap();
        let new_snapshot = new_manager.snapshot_v1(new_token).unwrap();
        let new_outcomes = new_manager.resume_v1([old_snapshot]).unwrap();
        assert_eq!(new_outcomes[0].1, RemoteResumeRevalidationResultV1::Dropped);
        assert!(!new_manager.callback_is_current_v1(old_token));
        assert!(!new_manager.callback_is_current_v1(new_token));

        let old_outcomes = old_manager.resume_v1([new_snapshot]).unwrap();
        assert_eq!(old_outcomes[0].1, RemoteResumeRevalidationResultV1::Dropped);
        assert!(!old_manager.callback_is_current_v1(new_token));
    }

    #[test]
    fn phase_and_token_errors_are_explicit() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();

        assert_eq!(
            manager.resume_v1([]),
            Err(RemoteResumeRevalidationErrorV1::NotHidden)
        );
        assert_eq!(
            manager.snapshot_v1(token),
            Err(RemoteResumeRevalidationErrorV1::NotHidden)
        );

        manager.hidden_v1().unwrap();
        assert_eq!(
            manager.register_v1(RemoteResumeWorkKindV1::Completion),
            Err(RemoteResumeRevalidationErrorV1::NotVisible)
        );

        let forged_token = RemoteResumeSlotTokenV1 {
            slot: 999,
            token: 1,
            execution_generation: manager.generation(),
            issuer: manager.issuer,
        };
        assert_eq!(
            manager.update_facts_v1(
                forged_token,
                RemoteResumeValidationFactV1::Valid(7),
                RemoteResumeValidationFactV1::Valid(11),
                RemoteResumeReservationFactV1::Valid,
            ),
            Err(RemoteResumeRevalidationErrorV1::InvalidToken)
        );
        assert_eq!(
            manager.snapshot_v1(forged_token),
            Err(RemoteResumeRevalidationErrorV1::InvalidToken)
        );
    }

    #[test]
    fn hidden_facts_cannot_be_updated_after_hide() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        update_valid_facts(&mut manager, token);

        manager.hidden_v1().unwrap();
        assert!(!manager.callback_is_current_v1(token));
        assert_eq!(
            manager.update_facts_v1(
                token,
                RemoteResumeValidationFactV1::Valid(99),
                RemoteResumeValidationFactV1::Valid(100),
                RemoteResumeReservationFactV1::Valid,
            ),
            Err(RemoteResumeRevalidationErrorV1::InvalidToken)
        );

        let snapshot = manager.snapshot_v1(token).unwrap();
        assert_eq!(snapshot.facts, valid_facts(false));

        let outcomes = manager.resume_v1([snapshot]).unwrap();
        assert_eq!(outcomes.len(), 1);
        let fresh = rebound_token(&outcomes[0]);
        assert!(manager.callback_is_current_v1(fresh));
        let slot = &manager.slots[&outcomes[0].0];
        assert_eq!(slot.facts, valid_facts(false));
        assert_eq!(slot.generation, fresh.execution_generation);
    }

    #[test]
    fn resume_requires_hidden_and_returns_fresh_opaque_token() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        update_valid_facts(&mut manager, token);

        assert_eq!(
            manager.snapshot_v1(token),
            Err(RemoteResumeRevalidationErrorV1::NotHidden)
        );
        manager.hidden_v1().unwrap();
        assert!(!manager.callback_is_current_v1(token));

        let snapshot = manager.snapshot_v1(token).unwrap();
        let outcomes = manager.resume_v1([snapshot]).unwrap();
        assert_eq!(outcomes.len(), 1);
        let fresh = rebound_token(&outcomes[0]);
        assert_eq!(manager.phase_v1(), RemoteResumePhaseV1::Visible);
        assert!(manager.callback_is_current_v1(fresh));
        assert!(!manager.callback_is_current_v1(token));
    }

    #[test]
    fn hide_hide_is_idempotent() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        manager.hidden_v1().unwrap();
        let generation = manager.generation();
        manager.hidden_v1().unwrap();
        assert_eq!(manager.generation(), generation);
        assert_eq!(manager.phase_v1(), RemoteResumePhaseV1::Hidden);
        assert!(manager.snapshot_v1(token).is_ok());
    }

    #[test]
    fn terminate_before_resume_fails_closed() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        manager.hidden_v1().unwrap();
        manager.terminate_v1().unwrap();
        let snapshot = RemoteResumeSlotSnapshotV1 {
            slot: 0,
            token: 0,
            execution_generation: 0,
            issuer: 0,
            kind: RemoteResumeWorkKindV1::Completion,
            facts: valid_facts(false),
        };
        assert_eq!(
            manager.resume_v1([snapshot]),
            Err(RemoteResumeRevalidationErrorV1::Terminated)
        );
        assert_eq!(
            manager.register_v1(RemoteResumeWorkKindV1::Completion),
            Err(RemoteResumeRevalidationErrorV1::Terminated)
        );
        assert!(!manager.callback_is_current_v1(token));
    }

    #[test]
    fn invalid_facts_are_dropped_and_valid_revisions_rebind() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        manager
            .update_facts_v1(
                token,
                RemoteResumeValidationFactV1::Valid(7),
                RemoteResumeValidationFactV1::Invalid(11),
                RemoteResumeReservationFactV1::Valid,
            )
            .unwrap();
        manager.hidden_v1().unwrap();
        let snapshot = manager.snapshot_v1(token).unwrap();
        assert_eq!(
            manager.resume_v1([snapshot]).unwrap()[0].1,
            RemoteResumeRevalidationResultV1::Dropped
        );
        assert!(manager.slots.is_empty());
        assert!(manager.queue.is_empty());
    }

    #[test]
    fn invalid_reservation_is_dropped_before_rebind() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        manager
            .update_facts_v1(
                token,
                RemoteResumeValidationFactV1::Valid(7),
                RemoteResumeValidationFactV1::Valid(11),
                RemoteResumeReservationFactV1::Invalid,
            )
            .unwrap();
        manager.hidden_v1().unwrap();
        let snapshot = manager.snapshot_v1(token).unwrap();

        let outcomes = manager.resume_v1([snapshot]).unwrap();
        assert_eq!(outcomes[0].1, RemoteResumeRevalidationResultV1::Dropped);
        assert!(manager.slots.is_empty());
        assert!(manager.queue.is_empty());
    }

    #[test]
    fn mutation_ownership_and_kind_mismatch_are_dropped() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Mutation)
            .unwrap();
        update_valid_facts(&mut manager, token);
        manager.hidden_v1().unwrap();
        let mut wrong_owner = manager.snapshot_v1(token).unwrap();
        wrong_owner.facts.mutation_owned = false;

        let outcomes = manager.resume_v1([wrong_owner]).unwrap();
        assert_eq!(outcomes[0].1, RemoteResumeRevalidationResultV1::Dropped);
        assert!(manager.slots.is_empty());

        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Mutation)
            .unwrap();
        update_valid_facts(&mut manager, token);
        manager.hidden_v1().unwrap();
        let mut wrong_kind = manager.snapshot_v1(token).unwrap();
        wrong_kind.kind = RemoteResumeWorkKindV1::Completion;

        let outcomes = manager.resume_v1([wrong_kind]).unwrap();
        assert_eq!(outcomes[0].1, RemoteResumeRevalidationResultV1::Dropped);
        assert!(manager.slots.is_empty());
    }

    #[test]
    fn duplicate_snapshot_is_deduplicated() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        update_valid_facts(&mut manager, token);
        manager.hidden_v1().unwrap();
        let snapshot = manager.snapshot_v1(token).unwrap();

        let outcomes = manager.resume_v1([snapshot, snapshot]).unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(
            outcomes[0].1,
            RemoteResumeRevalidationResultV1::Rebound(_)
        ));
        assert_eq!(outcomes[1].1, RemoteResumeRevalidationResultV1::Stale);
        assert_eq!(manager.queue.len(), 1);
    }

    #[test]
    fn omitted_snapshots_are_pruned() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let first = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        let second = manager
            .register_v1(RemoteResumeWorkKindV1::Decoder)
            .unwrap();
        update_valid_facts(&mut manager, first);
        update_valid_facts(&mut manager, second);
        manager.hidden_v1().unwrap();
        let snapshot = manager.snapshot_v1(first).unwrap();

        let outcomes = manager.resume_v1([snapshot]).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes[0].1,
            RemoteResumeRevalidationResultV1::Rebound(_)
        ));
        assert_eq!(manager.slots.len(), 1);
        assert_eq!(manager.queue.len(), 1);
        assert!(manager.callback_is_current_v1(rebound_token(&outcomes[0])));
    }

    #[test]
    fn mutation_kind_preserves_ownership_and_reservation_facts() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Mutation)
            .unwrap();
        update_valid_facts(&mut manager, token);
        manager.hidden_v1().unwrap();
        let snapshot = manager.snapshot_v1(token).unwrap();

        let outcomes = manager.resume_v1([snapshot]).unwrap();
        let fresh = rebound_token(&outcomes[0]);
        assert!(manager.callback_is_current_v1(fresh));
        let slot = &manager.slots[&outcomes[0].0];
        assert!(slot.facts.mutation_owned);
        assert!(slot.facts.reservation_valid);
        assert_eq!(slot.kind, RemoteResumeWorkKindV1::Mutation);
    }

    #[test]
    fn callbacks_are_stale_after_hide_resume_and_terminate() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        update_valid_facts(&mut manager, token);
        assert!(manager.callback_is_current_v1(token));

        manager.hidden_v1().unwrap();
        assert!(!manager.callback_is_current_v1(token));

        let snapshot = manager.snapshot_v1(token).unwrap();
        let outcomes = manager.resume_v1([snapshot]).unwrap();
        let fresh = rebound_token(&outcomes[0]);
        assert!(!manager.callback_is_current_v1(token));
        assert!(manager.callback_is_current_v1(fresh));

        manager.hidden_v1().unwrap();
        assert!(!manager.callback_is_current_v1(fresh));
        manager.terminate_v1().unwrap();
        assert!(!manager.callback_is_current_v1(fresh));
    }

    #[test]
    fn slot_token_and_generation_overflow_fail_closed() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        manager.next_slot = u64::MAX;
        manager.next_token = 1;
        assert_eq!(
            manager.register_v1(RemoteResumeWorkKindV1::Completion),
            Err(RemoteResumeRevalidationErrorV1::Overflow)
        );
        assert!(manager.slots.is_empty());
        assert_eq!(manager.next_slot, u64::MAX);

        manager.next_slot = 1;
        manager.next_token = u64::MAX;
        assert_eq!(
            manager.register_v1(RemoteResumeWorkKindV1::Completion),
            Err(RemoteResumeRevalidationErrorV1::Overflow)
        );

        manager.generation = u64::MAX;
        assert_eq!(
            manager.hidden_v1(),
            Err(RemoteResumeRevalidationErrorV1::Overflow)
        );
        assert_eq!(manager.phase_v1(), RemoteResumePhaseV1::Visible);
        assert_eq!(manager.generation(), u64::MAX);

        manager.phase = RemoteResumePhaseV1::Hidden;
        assert_eq!(
            manager.resume_v1([]),
            Err(RemoteResumeRevalidationErrorV1::Overflow)
        );
        assert_eq!(manager.phase_v1(), RemoteResumePhaseV1::Hidden);
        assert_eq!(manager.generation(), u64::MAX);

        manager.phase = RemoteResumePhaseV1::Visible;
        assert_eq!(
            manager.terminate_v1(),
            Err(RemoteResumeRevalidationErrorV1::Overflow)
        );
        assert!(!manager.terminated);
    }

    #[test]
    fn slot_and_snapshot_caps_are_checked() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        for _ in 0..MAX_REMOTE_RESUME_SLOTS_V1 {
            manager
                .register_v1(RemoteResumeWorkKindV1::Completion)
                .unwrap();
        }
        assert_eq!(
            manager.register_v1(RemoteResumeWorkKindV1::Completion),
            Err(RemoteResumeRevalidationErrorV1::SlotLimitExceeded)
        );

        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();
        update_valid_facts(&mut manager, token);
        manager.hidden_v1().unwrap();
        let snapshot = manager.snapshot_v1(token).unwrap();
        let snapshots = vec![snapshot; MAX_REMOTE_RESUME_SNAPSHOTS_V1 + 1];
        assert_eq!(
            manager.resume_v1(snapshots),
            Err(RemoteResumeRevalidationErrorV1::SnapshotLimitExceeded)
        );
        assert_eq!(manager.phase_v1(), RemoteResumePhaseV1::Hidden);
    }

    #[test]
    fn pump_reserves_viewer_turn_and_bounds_invalid_queue_scan() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        for _ in 0..4 {
            manager
                .register_v1(RemoteResumeWorkKindV1::Completion)
                .unwrap();
        }
        let frame = manager.pump_frame_v1(3, true);
        assert_eq!(frame.remote_started, 2);
        assert_eq!(frame.retry_started, 0);
        assert_eq!(frame.viewer_turns, 1);
        assert_eq!(manager.queue.len(), 2);

        manager.slots.clear();
        manager.queue = (100..110).collect();
        let frame = manager.pump_frame_v1(2, false);
        assert_eq!(frame.remote_started, 0);
        assert_eq!(frame.retry_started, 0);
        assert_eq!(manager.queue.len(), 8);
    }

    #[test]
    fn pump_preserves_viewer_turn_when_budget_is_one() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        manager
            .register_v1(RemoteResumeWorkKindV1::Completion)
            .unwrap();

        let frame = manager.pump_frame_v1(1, true);
        assert_eq!(frame.remote_started, 0);
        assert_eq!(frame.retry_started, 0);
        assert_eq!(frame.viewer_turns, 1);
        assert_eq!(manager.queue.len(), 1);

        let frame = manager.pump_frame_v1(1, false);
        assert_eq!(frame.remote_started, 1);
        assert_eq!(frame.retry_started, 0);
        assert_eq!(frame.viewer_turns, 0);
        assert!(manager.queue.is_empty());
    }

    #[test]
    fn retry_is_bounded_and_viewer_keeps_a_turn() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        manager
            .register_v1(RemoteResumeWorkKindV1::RetryPending)
            .unwrap();
        manager
            .register_v1(RemoteResumeWorkKindV1::RetryPending)
            .unwrap();
        let frame = manager.pump_frame_v1(8, true);
        assert_eq!(frame.retry_started, 1);
        assert_eq!(frame.viewer_turns, 1);
    }
}
