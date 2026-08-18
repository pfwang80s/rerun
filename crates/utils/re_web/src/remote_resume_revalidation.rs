//! Generation-checked remote-MCAP resume revalidation and bounded frame work.
//!
//! This is a page-control seam: it owns only remote work admitted by the page manager.  Native
//! and ordinary Viewer scheduling is deliberately outside this type.  A resume creates a fresh
//! execution generation; every slot, callback and retry must prove that generation before it can
//! run.

use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumeWorkKindV1 {
    Completion,
    Opening,
    Decoder,
    RetryPending,
    Mutation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteResumeSlotSnapshotV1 {
    pub slot: u64,
    pub token: u64,
    pub execution_generation: u64,
    pub validator_revision: u64,
    pub body_reader_revision: u64,
    pub validator_valid: bool,
    pub body_reader_valid: bool,
    pub mutation_owned: bool,
    pub reservation_valid: bool,
    pub kind: RemoteResumeWorkKindV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteResumeSlotTokenV1 {
    pub slot: u64,
    pub token: u64,
    pub execution_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteFrameSliceV1 {
    pub remote_started: u32,
    pub retry_started: u32,
    pub viewer_turns: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteResumeRevalidationResultV1 {
    Rebound,
    Dropped,
    Stale,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    token: u64,
    generation: u64,
    validator_revision: u64,
    body_reader_revision: u64,
    kind: RemoteResumeWorkKindV1,
    retry_pending: bool,
    mutation_owned: bool,
    reservation_valid: bool,
}

/// Remote-only resume coordinator with a bounded per-frame quantum.
pub struct RemoteResumeRevalidationV1 {
    generation: u64,
    visible: bool,
    terminated: bool,
    next_token: u64,
    slots: BTreeMap<u64, Slot>,
    queue: VecDeque<u64>,
}

impl Default for RemoteResumeRevalidationV1 {
    fn default() -> Self {
        Self::new_v1()
    }
}

impl RemoteResumeRevalidationV1 {
    pub const fn new_v1() -> Self {
        Self {
            generation: 0,
            visible: true,
            terminated: false,
            next_token: 1,
            slots: BTreeMap::new(),
            queue: VecDeque::new(),
        }
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn register_v1(
        &mut self,
        kind: RemoteResumeWorkKindV1,
        retry_pending: bool,
    ) -> Option<RemoteResumeSlotTokenV1> {
        if !self.visible || self.terminated {
            return None;
        }
        let slot = self
            .slots
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .unwrap_or(1);
        let token = self.next_token;
        self.next_token = self.next_token.checked_add(1).unwrap_or(1);
        self.slots.insert(
            slot,
            Slot {
                token,
                generation: self.generation,
                validator_revision: 0,
                body_reader_revision: 0,
                kind,
                retry_pending,
                mutation_owned: matches!(kind, RemoteResumeWorkKindV1::Mutation),
                reservation_valid: true,
            },
        );
        self.queue.push_back(slot);
        Some(RemoteResumeSlotTokenV1 {
            slot,
            token,
            execution_generation: self.generation,
        })
    }

    pub fn hidden_v1(&mut self) {
        if self.terminated || !self.visible {
            return;
        }
        self.visible = false;
        self.generation = self.generation.saturating_add(1);
        self.queue.clear();
        for slot in self.slots.values_mut() {
            slot.generation = self.generation;
        }
    }

    /// Rebinds only slots whose token, generation, validator/body reader and ownership facts all
    /// match the fresh page baseline.  Invalid slots are dropped and never retried.
    pub fn resume_v1(
        &mut self,
        snapshots: impl IntoIterator<Item = RemoteResumeSlotSnapshotV1>,
    ) -> Vec<(u64, RemoteResumeRevalidationResultV1)> {
        // Duplicate pageshow/resume signals while already visible cannot create a second
        // baseline or restart queued work.
        if self.terminated || self.visible {
            return Vec::new();
        }
        self.visible = true;
        self.generation = self.generation.saturating_add(1);
        self.queue.clear();
        let mut result = Vec::new();
        for snapshot in snapshots {
            let Some(slot) = self.slots.get_mut(&snapshot.slot) else {
                result.push((snapshot.slot, RemoteResumeRevalidationResultV1::Stale));
                continue;
            };
            let valid = slot.token == snapshot.token
                && snapshot.execution_generation == self.generation
                && snapshot.validator_valid
                && snapshot.body_reader_valid
                && snapshot.mutation_owned == slot.mutation_owned
                && snapshot.reservation_valid == slot.reservation_valid
                && snapshot.validator_revision == slot.validator_revision
                && snapshot.body_reader_revision == slot.body_reader_revision;
            if !valid {
                self.slots.remove(&snapshot.slot);
                result.push((snapshot.slot, RemoteResumeRevalidationResultV1::Dropped));
                continue;
            }
            slot.generation = self.generation;
            slot.kind = snapshot.kind;
            slot.retry_pending = matches!(snapshot.kind, RemoteResumeWorkKindV1::RetryPending);
            self.queue.push_back(snapshot.slot);
            result.push((snapshot.slot, RemoteResumeRevalidationResultV1::Rebound));
        }
        result
    }

    pub fn terminate_v1(&mut self) {
        self.terminated = true;
        self.visible = false;
        self.generation = self.generation.saturating_add(1);
        self.queue.clear();
        self.slots.clear();
    }

    pub fn callback_is_current_v1(&self, token: RemoteResumeSlotTokenV1) -> bool {
        !self.terminated
            && self.visible
            && token.execution_generation == self.generation
            && self
                .slots
                .get(&token.slot)
                .is_some_and(|slot| slot.token == token.token && slot.generation == self.generation)
    }

    /// Runs a bounded remote quantum.  At most one retry starts in a turn; viewer work receives
    /// its normal opportunity independently of remote backlog.
    pub fn pump_frame_v1(
        &mut self,
        max_remote_work: u32,
        viewer_work_available: bool,
    ) -> RemoteFrameSliceV1 {
        if !self.visible || self.terminated || max_remote_work == 0 {
            return RemoteFrameSliceV1 {
                remote_started: 0,
                retry_started: 0,
                viewer_turns: viewer_work_available as u32,
            };
        }
        let mut remote_started = 0;
        let mut retry_started = 0;
        let mut deferred = VecDeque::new();
        while remote_started < max_remote_work {
            let Some(slot_id) = self.queue.pop_front() else {
                break;
            };
            let Some(slot) = self.slots.get(&slot_id).copied() else {
                continue;
            };
            if slot.generation != self.generation {
                continue;
            }
            if slot.retry_pending && retry_started >= 1 {
                deferred.push_back(slot_id);
                continue;
            }
            remote_started += 1;
            if slot.retry_pending {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        token: RemoteResumeSlotTokenV1,
        kind: RemoteResumeWorkKindV1,
    ) -> RemoteResumeSlotSnapshotV1 {
        RemoteResumeSlotSnapshotV1 {
            slot: token.slot,
            token: token.token,
            execution_generation: token.execution_generation + 2,
            validator_revision: 0,
            body_reader_revision: 0,
            validator_valid: true,
            body_reader_valid: true,
            mutation_owned: matches!(kind, RemoteResumeWorkKindV1::Mutation),
            reservation_valid: true,
            kind,
        }
    }

    #[test]
    fn stale_resume_and_callbacks_are_dropped() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        let token = manager
            .register_v1(RemoteResumeWorkKindV1::Completion, false)
            .unwrap();
        manager.hidden_v1();
        assert!(!manager.callback_is_current_v1(token));
        let mut fresh = snapshot(token, RemoteResumeWorkKindV1::Completion);
        fresh.execution_generation = manager.generation() + 1;
        assert_eq!(
            manager.resume_v1([fresh])[0].1,
            RemoteResumeRevalidationResultV1::Rebound
        );
        assert!(manager.callback_is_current_v1(RemoteResumeSlotTokenV1 {
            execution_generation: manager.generation(),
            ..token
        }));
    }

    #[test]
    fn retry_is_bounded_and_viewer_keeps_a_turn() {
        let mut manager = RemoteResumeRevalidationV1::new_v1();
        manager.register_v1(RemoteResumeWorkKindV1::RetryPending, true);
        manager.register_v1(RemoteResumeWorkKindV1::RetryPending, true);
        let frame = manager.pump_frame_v1(8, true);
        assert_eq!(frame.retry_started, 1);
        assert_eq!(frame.viewer_turns, 1);
    }
}
