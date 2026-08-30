//! Production-disarmed public lifecycle sequencer for strict Web open.
//!
//! This module freezes the normative public event ordering for strict Web MCAP open.
//! It does not dispatch events, touch `TypeScript`, or change the existing compatibility
//! `recording_open` event path.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::open_source_terminal::OpenSourceTerminalCauseV1;
use crate::strict_open_wire::{OpenOperationIdentity, PublicRecordingIdentity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicRecordingBehaviorEffectV1 {
    ForegroundSelected,
    PanelOpened,
    CatalogOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicRecordingRemovedReasonV1 {
    Closed,
    SourceFailed,
    MemoryPressure,
    BackgroundPolicy,
    ViewerStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicLifecycleEventV1 {
    Accepted {
        operation_id: OpenOperationIdentity,
    },
    RecordingActivated {
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
    },
    BehaviorEffect {
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
        effect: PublicRecordingBehaviorEffectV1,
    },
    RequestReady {
        operation_id: OpenOperationIdentity,
    },
    RecordingPresentationReady {
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
    },
    Terminal {
        operation_id: OpenOperationIdentity,
        cause: OpenSourceTerminalCauseV1,
    },
    RecordingRemoved {
        recording_id: PublicRecordingIdentity,
        reason: PublicRecordingRemovedReasonV1,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicLifecycleSequencerErrorV1 {
    DuplicateOperation,
    DuplicateRecording,
    UnknownOperation,
    UnknownRecording,
    OperationAlreadyReady,
    OperationAlreadyTerminal,
    RecordingAlreadyHasBehaviorEffect,
    RecordingAlreadyPresentationReady,
    RecordingAlreadyRemoved,
    MissingRecordingActivation,
    MissingBehaviorEffect,
    MissingRequestReady,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationLifecyclePhaseV1 {
    Accepted,
    Ready,
    Terminal,
}

struct OperationLifecycleStateV1 {
    phase: OperationLifecyclePhaseV1,
    recordings: BTreeSet<PublicRecordingIdentity>,
}

struct RecordingLifecycleSequenceStateV1 {
    operation_id: OpenOperationIdentity,
    activated: bool,
    behavior_effect: Option<PublicRecordingBehaviorEffectV1>,
    presentation_ready: bool,
    removed: bool,
}

pub struct PublicLifecycleSequencerV1 {
    operations: BTreeMap<OpenOperationIdentity, OperationLifecycleStateV1>,
    recordings: BTreeMap<PublicRecordingIdentity, RecordingLifecycleSequenceStateV1>,
    events: Vec<PublicLifecycleEventV1>,
}

impl PublicLifecycleSequencerV1 {
    pub fn new_v1() -> Self {
        Self {
            operations: BTreeMap::new(),
            recordings: BTreeMap::new(),
            events: Vec::new(),
        }
    }

    pub fn events_v1(&self) -> &[PublicLifecycleEventV1] {
        &self.events
    }

    pub fn accept_operation_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
    ) -> Result<(), PublicLifecycleSequencerErrorV1> {
        if self.operations.contains_key(&operation_id) {
            return Err(PublicLifecycleSequencerErrorV1::DuplicateOperation);
        }
        self.operations.insert(
            operation_id,
            OperationLifecycleStateV1 {
                phase: OperationLifecyclePhaseV1::Accepted,
                recordings: BTreeSet::new(),
            },
        );
        self.events
            .push(PublicLifecycleEventV1::Accepted { operation_id });
        Ok(())
    }

    pub fn activate_recording_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
    ) -> Result<(), PublicLifecycleSequencerErrorV1> {
        if self.recordings.contains_key(&recording_id) {
            return Err(PublicLifecycleSequencerErrorV1::DuplicateRecording);
        }
        let operation = self.live_operation_mut_v1(operation_id)?;
        if operation.phase == OperationLifecyclePhaseV1::Ready {
            return Err(PublicLifecycleSequencerErrorV1::OperationAlreadyReady);
        }
        operation.recordings.insert(recording_id);
        self.recordings.insert(
            recording_id,
            RecordingLifecycleSequenceStateV1 {
                operation_id,
                activated: true,
                behavior_effect: None,
                presentation_ready: false,
                removed: false,
            },
        );
        self.events
            .push(PublicLifecycleEventV1::RecordingActivated {
                operation_id,
                recording_id,
            });
        Ok(())
    }

    pub fn apply_behavior_effect_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
        effect: PublicRecordingBehaviorEffectV1,
    ) -> Result<(), PublicLifecycleSequencerErrorV1> {
        let operation = self.live_operation_mut_v1(operation_id)?;
        if operation.phase == OperationLifecyclePhaseV1::Ready {
            return Err(PublicLifecycleSequencerErrorV1::OperationAlreadyReady);
        }
        let recording = self.recording_for_operation_mut_v1(operation_id, recording_id)?;
        if !recording.activated {
            return Err(PublicLifecycleSequencerErrorV1::MissingRecordingActivation);
        }
        if recording.behavior_effect.is_some() {
            return Err(PublicLifecycleSequencerErrorV1::RecordingAlreadyHasBehaviorEffect);
        }
        recording.behavior_effect = Some(effect);
        self.events.push(PublicLifecycleEventV1::BehaviorEffect {
            operation_id,
            recording_id,
            effect,
        });
        Ok(())
    }

    pub fn mark_request_ready_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
    ) -> Result<(), PublicLifecycleSequencerErrorV1> {
        let recording_ids = {
            let operation = self.live_operation_mut_v1(operation_id)?;
            if operation.phase == OperationLifecyclePhaseV1::Ready {
                return Err(PublicLifecycleSequencerErrorV1::OperationAlreadyReady);
            }
            operation.recordings.iter().copied().collect::<Vec<_>>()
        };
        for recording_id in recording_ids {
            let recording = self
                .recordings
                .get(&recording_id)
                .ok_or(PublicLifecycleSequencerErrorV1::UnknownRecording)?;
            if recording.behavior_effect.is_none() {
                return Err(PublicLifecycleSequencerErrorV1::MissingBehaviorEffect);
            }
        }
        self.operations
            .get_mut(&operation_id)
            .expect("operation still exists")
            .phase = OperationLifecyclePhaseV1::Ready;
        self.events
            .push(PublicLifecycleEventV1::RequestReady { operation_id });
        Ok(())
    }

    pub fn mark_presentation_ready_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
    ) -> Result<(), PublicLifecycleSequencerErrorV1> {
        let operation = self
            .operations
            .get(&operation_id)
            .ok_or(PublicLifecycleSequencerErrorV1::UnknownOperation)?;
        match operation.phase {
            OperationLifecyclePhaseV1::Accepted => {
                return Err(PublicLifecycleSequencerErrorV1::MissingRequestReady);
            }
            OperationLifecyclePhaseV1::Terminal => {
                return Err(PublicLifecycleSequencerErrorV1::OperationAlreadyTerminal);
            }
            OperationLifecyclePhaseV1::Ready => {}
        }
        let recording = self.recording_for_operation_mut_v1(operation_id, recording_id)?;
        if recording.behavior_effect.is_none() {
            return Err(PublicLifecycleSequencerErrorV1::MissingBehaviorEffect);
        }
        if recording.presentation_ready {
            return Err(PublicLifecycleSequencerErrorV1::RecordingAlreadyPresentationReady);
        }
        recording.presentation_ready = true;
        self.events
            .push(PublicLifecycleEventV1::RecordingPresentationReady {
                operation_id,
                recording_id,
            });
        Ok(())
    }

    pub fn terminal_operation_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
        cause: OpenSourceTerminalCauseV1,
    ) -> Result<(), PublicLifecycleSequencerErrorV1> {
        let operation = self
            .operations
            .get_mut(&operation_id)
            .ok_or(PublicLifecycleSequencerErrorV1::UnknownOperation)?;
        if operation.phase == OperationLifecyclePhaseV1::Terminal {
            return Err(PublicLifecycleSequencerErrorV1::OperationAlreadyTerminal);
        }
        operation.phase = OperationLifecyclePhaseV1::Terminal;
        self.events.push(PublicLifecycleEventV1::Terminal {
            operation_id,
            cause,
        });
        Ok(())
    }

    pub fn remove_recording_v1(
        &mut self,
        recording_id: PublicRecordingIdentity,
        reason: PublicRecordingRemovedReasonV1,
    ) -> Result<(), PublicLifecycleSequencerErrorV1> {
        let recording = self
            .recordings
            .get_mut(&recording_id)
            .ok_or(PublicLifecycleSequencerErrorV1::UnknownRecording)?;
        if recording.removed {
            return Err(PublicLifecycleSequencerErrorV1::RecordingAlreadyRemoved);
        }
        recording.removed = true;
        self.events.push(PublicLifecycleEventV1::RecordingRemoved {
            recording_id,
            reason,
        });
        Ok(())
    }

    fn live_operation_mut_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
    ) -> Result<&mut OperationLifecycleStateV1, PublicLifecycleSequencerErrorV1> {
        let operation = self
            .operations
            .get_mut(&operation_id)
            .ok_or(PublicLifecycleSequencerErrorV1::UnknownOperation)?;
        if operation.phase == OperationLifecyclePhaseV1::Terminal {
            return Err(PublicLifecycleSequencerErrorV1::OperationAlreadyTerminal);
        }
        Ok(operation)
    }

    fn recording_for_operation_mut_v1(
        &mut self,
        operation_id: OpenOperationIdentity,
        recording_id: PublicRecordingIdentity,
    ) -> Result<&mut RecordingLifecycleSequenceStateV1, PublicLifecycleSequencerErrorV1> {
        let recording = self
            .recordings
            .get_mut(&recording_id)
            .ok_or(PublicLifecycleSequencerErrorV1::UnknownRecording)?;
        if recording.operation_id != operation_id {
            return Err(PublicLifecycleSequencerErrorV1::UnknownRecording);
        }
        Ok(recording)
    }
}

impl fmt::Debug for PublicLifecycleSequencerV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublicLifecycleSequencerV1")
            .field("operations", &self.operations.len())
            .field("recordings", &self.recordings.len())
            .field("events", &self.events.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation(index: u128) -> OpenOperationIdentity {
        OpenOperationIdentity::new(index)
    }

    fn recording(index: u128) -> PublicRecordingIdentity {
        PublicRecordingIdentity::new(index)
    }

    #[test]
    fn recording_wrapper_and_behavior_effect_precede_ready() {
        let mut sequencer = PublicLifecycleSequencerV1::new_v1();
        sequencer.accept_operation_v1(operation(1)).unwrap();
        assert_eq!(sequencer.mark_request_ready_v1(operation(1)), Ok(()));
        assert_eq!(
            sequencer.activate_recording_v1(operation(1), recording(10)),
            Err(PublicLifecycleSequencerErrorV1::OperationAlreadyReady)
        );

        let mut sequencer = PublicLifecycleSequencerV1::new_v1();
        sequencer.accept_operation_v1(operation(1)).unwrap();
        sequencer
            .activate_recording_v1(operation(1), recording(10))
            .unwrap();
        assert_eq!(
            sequencer.mark_request_ready_v1(operation(1)),
            Err(PublicLifecycleSequencerErrorV1::MissingBehaviorEffect)
        );
        sequencer
            .apply_behavior_effect_v1(
                operation(1),
                recording(10),
                PublicRecordingBehaviorEffectV1::ForegroundSelected,
            )
            .unwrap();
        sequencer.mark_request_ready_v1(operation(1)).unwrap();
        sequencer
            .mark_presentation_ready_v1(operation(1), recording(10))
            .unwrap();

        assert_eq!(
            sequencer.events_v1(),
            &[
                PublicLifecycleEventV1::Accepted {
                    operation_id: operation(1),
                },
                PublicLifecycleEventV1::RecordingActivated {
                    operation_id: operation(1),
                    recording_id: recording(10),
                },
                PublicLifecycleEventV1::BehaviorEffect {
                    operation_id: operation(1),
                    recording_id: recording(10),
                    effect: PublicRecordingBehaviorEffectV1::ForegroundSelected,
                },
                PublicLifecycleEventV1::RequestReady {
                    operation_id: operation(1),
                },
                PublicLifecycleEventV1::RecordingPresentationReady {
                    operation_id: operation(1),
                    recording_id: recording(10),
                },
            ]
        );
    }

    #[test]
    fn pre_store_failure_has_no_recording_or_ready_events() {
        let mut sequencer = PublicLifecycleSequencerV1::new_v1();
        sequencer.accept_operation_v1(operation(1)).unwrap();
        sequencer
            .terminal_operation_v1(operation(1), OpenSourceTerminalCauseV1::OpeningFailure)
            .unwrap();
        assert_eq!(
            sequencer.activate_recording_v1(operation(1), recording(10)),
            Err(PublicLifecycleSequencerErrorV1::OperationAlreadyTerminal)
        );
        assert_eq!(
            sequencer.events_v1(),
            &[
                PublicLifecycleEventV1::Accepted {
                    operation_id: operation(1),
                },
                PublicLifecycleEventV1::Terminal {
                    operation_id: operation(1),
                    cause: OpenSourceTerminalCauseV1::OpeningFailure,
                },
            ]
        );
    }

    #[test]
    fn post_activation_failure_keeps_terminal_and_removed_reasons_typed() {
        let mut sequencer = PublicLifecycleSequencerV1::new_v1();
        sequencer.accept_operation_v1(operation(1)).unwrap();
        sequencer
            .activate_recording_v1(operation(1), recording(10))
            .unwrap();
        sequencer
            .apply_behavior_effect_v1(
                operation(1),
                recording(10),
                PublicRecordingBehaviorEffectV1::PanelOpened,
            )
            .unwrap();
        sequencer
            .terminal_operation_v1(operation(1), OpenSourceTerminalCauseV1::SessionFatal)
            .unwrap();
        sequencer
            .remove_recording_v1(recording(10), PublicRecordingRemovedReasonV1::SourceFailed)
            .unwrap();

        assert_eq!(
            sequencer.events_v1(),
            &[
                PublicLifecycleEventV1::Accepted {
                    operation_id: operation(1),
                },
                PublicLifecycleEventV1::RecordingActivated {
                    operation_id: operation(1),
                    recording_id: recording(10),
                },
                PublicLifecycleEventV1::BehaviorEffect {
                    operation_id: operation(1),
                    recording_id: recording(10),
                    effect: PublicRecordingBehaviorEffectV1::PanelOpened,
                },
                PublicLifecycleEventV1::Terminal {
                    operation_id: operation(1),
                    cause: OpenSourceTerminalCauseV1::SessionFatal,
                },
                PublicLifecycleEventV1::RecordingRemoved {
                    recording_id: recording(10),
                    reason: PublicRecordingRemovedReasonV1::SourceFailed,
                },
            ]
        );
    }

    #[test]
    fn public_effect_and_removal_reason_variants_are_explicitly_frozen() {
        assert_eq!(
            [
                PublicRecordingBehaviorEffectV1::ForegroundSelected,
                PublicRecordingBehaviorEffectV1::PanelOpened,
                PublicRecordingBehaviorEffectV1::CatalogOnly,
            ]
            .len(),
            3
        );
        assert_eq!(
            [
                PublicRecordingRemovedReasonV1::Closed,
                PublicRecordingRemovedReasonV1::SourceFailed,
                PublicRecordingRemovedReasonV1::MemoryPressure,
                PublicRecordingRemovedReasonV1::BackgroundPolicy,
                PublicRecordingRemovedReasonV1::ViewerStopped,
            ]
            .len(),
            5
        );
    }
}
