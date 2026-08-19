//! Requested/committed remote navigation state for Web remote MCAP.
//!
//! This module is production-disarmed and owns no Viewer command routing, network transport, or
//! Store mutation. It is only the pure state layer that separates the requested navigation intent
//! from the committed presentation time and keeps the candidate clock held while a generation is
//! in flight or the session is frozen.

#![allow(dead_code)]

use re_log_types::{AbsoluteTimeRange, Duration, TimeInt, Timeline};

use crate::remote_loaded_coverage::CanonicalIndexedExtentV1;

/// Committed presentation time.
///
/// Recording queries may only use this value. The requested navigation intent is intentionally a
/// separate type and must never be mixed into a query-visible cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CommittedPresentationTimeV1 {
    pub(crate) timeline: Timeline,
    pub(crate) cursor: TimeInt,
}

impl CommittedPresentationTimeV1 {
    pub(crate) const fn new_v1(timeline: Timeline, cursor: TimeInt) -> Self {
        Self { timeline, cursor }
    }
}

/// Remote playback state. `Following` is deliberately not representable here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RemotePlayStateV1 {
    Paused,
    Playing,
}

impl RemotePlayStateV1 {
    pub(crate) const fn is_playing_v1(self) -> bool {
        matches!(self, Self::Playing)
    }
}

/// Generic viewer playback state accepted at a command boundary.
///
/// The `Following` variant is rejected structurally and cannot enter persistent requested state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GenericRemotePlayStateV1 {
    Paused,
    Playing,
    Following,
}

impl GenericRemotePlayStateV1 {
    pub(crate) const fn remote_play_state_v1(
        self,
    ) -> Result<RemotePlayStateV1, RemoteNavigationErrorV1> {
        match self {
            Self::Paused => Ok(RemotePlayStateV1::Paused),
            Self::Playing => Ok(RemotePlayStateV1::Playing),
            Self::Following => Err(RemoteNavigationErrorV1::UnsupportedRemotePlayState),
        }
    }
}

/// Why a pending navigation intent was created or replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RemoteNavigationTriggerV1 {
    Seek,
    Step,
    LoopJump,
    PlaybackAdvance,
    PlayStateChange,
}

/// Loop behavior used by the automatic candidate clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RemoteLoopModeV1 {
    Clamp,
    Loop,
}

/// Stable fields that determine whether a requested demand requires a new generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RemoteNavigationDemandKeyV1 {
    pub(crate) canonical_target: TimeInt,
    pub(crate) requires_minimum_playback_buffer: bool,
}

impl RemoteNavigationDemandKeyV1 {
    pub(crate) fn for_play_state_v1(
        canonical_target: TimeInt,
        play_state: RemotePlayStateV1,
    ) -> Result<Self, RemoteNavigationErrorV1> {
        validate_temporal_target_v1(canonical_target)?;
        Ok(Self {
            canonical_target,
            requires_minimum_playback_buffer: play_state.is_playing_v1(),
        })
    }
}

/// Complete latest-wins requested navigation intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PendingNavigationIntentV1 {
    pub(crate) generation: u64,
    pub(crate) canonical_target: TimeInt,
    pub(crate) play_state: RemotePlayStateV1,
    pub(crate) trigger: RemoteNavigationTriggerV1,
    pub(crate) demand_key: RemoteNavigationDemandKeyV1,
}

impl PendingNavigationIntentV1 {
    pub(crate) fn new_v1(
        generation: u64,
        canonical_target: TimeInt,
        play_state: RemotePlayStateV1,
        trigger: RemoteNavigationTriggerV1,
    ) -> Result<Self, RemoteNavigationErrorV1> {
        Ok(Self {
            generation,
            canonical_target,
            play_state,
            trigger,
            demand_key: RemoteNavigationDemandKeyV1::for_play_state_v1(
                canonical_target,
                play_state,
            )?,
        })
    }
}

/// Per-frame candidate-clock input supplied by the frame driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RemoteCandidateClockInputV1 {
    pub(crate) stable_dt: Duration,
    pub(crate) held: bool,
}

impl RemoteCandidateClockInputV1 {
    pub(crate) const fn new_v1(stable_dt: Duration, held: bool) -> Self {
        Self { stable_dt, held }
    }
}

/// Navigation ingress command.
///
/// Every variant carries the timeline that must match the manifest canonical timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RemoteNavigationCommandV1 {
    Seek {
        timeline: Timeline,
        target: TimeInt,
    },
    StepForward {
        timeline: Timeline,
        delta: TimeInt,
    },
    StepBack {
        timeline: Timeline,
        delta: TimeInt,
    },
    LoopJump {
        timeline: Timeline,
        target: TimeInt,
    },
    Play {
        timeline: Timeline,
    },
    Pause {
        timeline: Timeline,
    },
    SetPlayState {
        timeline: Timeline,
        play_state: GenericRemotePlayStateV1,
    },
    SetPlaybackSpeed {
        timeline: Timeline,
        speed: i64,
    },
    PlaybackAdvance {
        timeline: Timeline,
        target: TimeInt,
    },
}

impl RemoteNavigationCommandV1 {
    pub(crate) const fn timeline_v1(self) -> Timeline {
        match self {
            Self::Seek { timeline, .. }
            | Self::StepForward { timeline, .. }
            | Self::StepBack { timeline, .. }
            | Self::LoopJump { timeline, .. }
            | Self::Play { timeline }
            | Self::Pause { timeline }
            | Self::SetPlayState { timeline, .. }
            | Self::SetPlaybackSpeed { timeline, .. }
            | Self::PlaybackAdvance { timeline, .. } => timeline,
        }
    }

    pub(crate) const fn trigger_v1(self) -> Option<RemoteNavigationTriggerV1> {
        match self {
            Self::Seek { .. } => Some(RemoteNavigationTriggerV1::Seek),
            Self::StepForward { .. } | Self::StepBack { .. } => {
                Some(RemoteNavigationTriggerV1::Step)
            }
            Self::LoopJump { .. } => Some(RemoteNavigationTriggerV1::LoopJump),
            Self::Play { .. } | Self::Pause { .. } | Self::SetPlayState { .. } => {
                Some(RemoteNavigationTriggerV1::PlayStateChange)
            }
            Self::PlaybackAdvance { .. } => Some(RemoteNavigationTriggerV1::PlaybackAdvance),
            Self::SetPlaybackSpeed { .. } => None,
        }
    }
}

/// Typed remote navigation errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemoteNavigationErrorV1 {
    #[error("remote navigation command targets a non-canonical timeline")]
    UnsupportedRemoteNavigationTimeline,

    #[error("remote navigation does not support the generic Following play state")]
    UnsupportedRemotePlayState,

    #[error("remote playback speed must be positive")]
    InvalidRemotePlaybackSpeed,

    #[error("remote temporal navigation is unavailable when no messages are indexed")]
    NoIndexedMessages,

    #[error("remote navigation target is outside the canonical indexed extent")]
    TargetOutsideIndexedExtent,

    #[error("remote navigation received a non-temporal or invalid target")]
    InvalidTarget,

    #[error("remote navigation clock delta must be non-negative")]
    InvalidClockDelta,

    #[error("remote navigation arithmetic overflowed")]
    ArithmeticOverflow,

    #[error("playback advance is generated by preview_update and is not a mergeable command")]
    AutomaticPlaybackAdvanceNotMergeable,

    #[error("remote navigation received a presentation commit for a stale generation")]
    StalePresentationCommit,
}

/// Read-only navigation state published to the UI layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteNavigationUiStateV1 {
    pub(crate) requested: Option<PendingNavigationIntentV1>,
    pub(crate) committed: Option<CommittedPresentationTimeV1>,
    pub(crate) target_marker: Option<TimeInt>,
    pub(crate) is_loading: bool,
    pub(crate) is_buffering: bool,
    pub(crate) requested_generation_in_flight: bool,
    pub(crate) candidate_clock_held: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteNavigationBackgroundSnapshotV1 {
    committed: Option<CommittedPresentationTimeV1>,
    play_state: RemotePlayStateV1,
}

/// Pure requested/committed navigation state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteNavigationAdapterV1 {
    canonical_timeline: Timeline,
    indexed_extent: CanonicalIndexedExtentV1,
    play_state: RemotePlayStateV1,
    committed: Option<CommittedPresentationTimeV1>,
    requested: Option<PendingNavigationIntentV1>,
    target_marker: Option<TimeInt>,
    playback_speed: u64,
    requested_generation_in_flight: bool,
    buffering: bool,
    frozen: bool,
    is_background: bool,
    accepted_discrete_command_this_frame: bool,
    suppress_next_preview_dt: bool,
    preview_update_seen_this_frame: bool,
    background_snapshot: Option<RemoteNavigationBackgroundSnapshotV1>,
    wall_clock_accumulator_ns: u64,
    next_requested_generation: u64,
    cached_ui_state: RemoteNavigationUiStateV1,
}

impl RemoteNavigationAdapterV1 {
    pub(crate) fn new_v1(
        canonical_timeline: Timeline,
        initial_play_state: RemotePlayStateV1,
        committed: Option<CommittedPresentationTimeV1>,
    ) -> Result<Self, RemoteNavigationErrorV1> {
        Self::with_extent_and_play_state_v1(
            canonical_timeline,
            CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::EVERYTHING),
            initial_play_state,
            committed,
        )
    }

    pub(crate) fn from_extent_v1(
        canonical_timeline: Timeline,
        indexed_extent: CanonicalIndexedExtentV1,
        committed: Option<CommittedPresentationTimeV1>,
    ) -> Result<Self, RemoteNavigationErrorV1> {
        let initial_play_state = match indexed_extent {
            CanonicalIndexedExtentV1::Known(_) => RemotePlayStateV1::Playing,
            CanonicalIndexedExtentV1::NoIndexedMessages => RemotePlayStateV1::Paused,
        };
        Self::with_extent_and_play_state_v1(
            canonical_timeline,
            indexed_extent,
            initial_play_state,
            committed,
        )
    }

    fn with_extent_and_play_state_v1(
        canonical_timeline: Timeline,
        indexed_extent: CanonicalIndexedExtentV1,
        initial_play_state: RemotePlayStateV1,
        committed: Option<CommittedPresentationTimeV1>,
    ) -> Result<Self, RemoteNavigationErrorV1> {
        validate_indexed_extent_v1(indexed_extent, initial_play_state)?;
        validate_committed_time_v1(canonical_timeline, indexed_extent, committed)?;

        let mut adapter = Self {
            canonical_timeline,
            indexed_extent,
            play_state: initial_play_state,
            committed,
            requested: None,
            target_marker: None,
            playback_speed: 1,
            requested_generation_in_flight: false,
            buffering: false,
            frozen: false,
            is_background: false,
            accepted_discrete_command_this_frame: false,
            suppress_next_preview_dt: false,
            preview_update_seen_this_frame: false,
            background_snapshot: None,
            wall_clock_accumulator_ns: 0,
            next_requested_generation: 0,
            cached_ui_state: RemoteNavigationUiStateV1::default(),
        };
        adapter.refresh_ui_state_v1(false);
        Ok(adapter)
    }

    pub(crate) const fn canonical_timeline_v1(&self) -> Timeline {
        self.canonical_timeline
    }

    pub(crate) const fn indexed_extent_v1(&self) -> CanonicalIndexedExtentV1 {
        self.indexed_extent
    }

    pub(crate) const fn play_state_v1(&self) -> RemotePlayStateV1 {
        self.play_state
    }

    pub(crate) const fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        self.committed
    }

    pub(crate) const fn requested_v1(&self) -> Option<PendingNavigationIntentV1> {
        self.requested
    }

    pub(crate) const fn requested_generation_v1(&self) -> Option<u64> {
        match self.requested {
            Some(intent) => Some(intent.generation),
            None => None,
        }
    }

    pub(crate) const fn target_marker_v1(&self) -> Option<TimeInt> {
        self.target_marker
    }

    pub(crate) const fn playback_speed_v1(&self) -> u64 {
        self.playback_speed
    }

    pub(crate) const fn requested_generation_in_flight_v1(&self) -> bool {
        self.requested_generation_in_flight
    }

    pub(crate) const fn buffering_v1(&self) -> bool {
        self.buffering
    }

    pub(crate) const fn frozen_v1(&self) -> bool {
        self.frozen
    }

    pub(crate) const fn is_background_v1(&self) -> bool {
        self.is_background
    }

    pub(crate) const fn wall_clock_accumulator_ns_v1(&self) -> u64 {
        self.wall_clock_accumulator_ns
    }

    pub(crate) fn ui_state_v1(&self) -> &RemoteNavigationUiStateV1 {
        &self.cached_ui_state
    }

    pub(crate) fn merge_commands_v1(
        &mut self,
        commands: &[RemoteNavigationCommandV1],
    ) -> Result<&RemoteNavigationUiStateV1, RemoteNavigationErrorV1> {
        let mut prospective = self.clone();
        prospective.accepted_discrete_command_this_frame = false;
        for command in commands {
            prospective.apply_merge_command_v1(*command)?;
        }

        *self = prospective;
        self.refresh_ui_state_v1(false);
        Ok(self.ui_state_v1())
    }

    pub(crate) fn preview_update_v1(
        &mut self,
        indexed_extent: CanonicalIndexedExtentV1,
        loop_mode: RemoteLoopModeV1,
        clock: RemoteCandidateClockInputV1,
    ) -> Result<&RemoteNavigationUiStateV1, RemoteNavigationErrorV1> {
        let mut prospective = self.clone();
        validate_indexed_extent_v1(indexed_extent, prospective.play_state)?;
        prospective.indexed_extent = indexed_extent;
        validate_committed_time_v1(
            prospective.canonical_timeline,
            prospective.indexed_extent,
            prospective.committed,
        )?;

        // A successful `preview_update` marks a frame as having been driven. Operations that need
        // to know whether they still have a remaining preview in the same frame can use this latch.
        prospective.preview_update_seen_this_frame = false;

        let held = clock.held || prospective.candidate_clock_hold_flags_v1();
        if prospective.accepted_discrete_command_this_frame {
            prospective.accepted_discrete_command_this_frame = false;
            prospective.suppress_next_preview_dt = false;
        } else if held {
            // Preserve the requested target/demand key. The frame delta is not consumed,
            // accumulated, or retained until the next eligible frame.
        } else if prospective.suppress_next_preview_dt {
            prospective.suppress_next_preview_dt = false;
        } else if prospective.play_state.is_playing_v1() {
            if clock.stable_dt.as_nanos() < 0 {
                return Err(RemoteNavigationErrorV1::InvalidClockDelta);
            }
            let target = prospective.advance_candidate_target_v1(clock.stable_dt, loop_mode)?;
            prospective
                .set_requested_target_v1(target, RemoteNavigationTriggerV1::PlaybackAdvance)?;
            prospective.target_marker = Some(target);
            prospective.accumulate_wall_clock_v1(clock.stable_dt)?;
        }

        prospective.preview_update_seen_this_frame = true;
        *self = prospective;
        self.refresh_ui_state_v1(held);
        Ok(self.ui_state_v1())
    }

    pub(crate) fn commit_presentation_v1(
        &mut self,
        committed: Option<CommittedPresentationTimeV1>,
        generation: u64,
    ) -> Result<&RemoteNavigationUiStateV1, RemoteNavigationErrorV1> {
        let mut prospective = self.clone();
        validate_committed_time_v1(
            prospective.canonical_timeline,
            prospective.indexed_extent,
            committed,
        )?;

        if prospective.requested_generation_v1() != Some(generation) {
            return Err(RemoteNavigationErrorV1::StalePresentationCommit);
        }

        prospective.committed = committed;
        prospective.requested = None;
        prospective.requested_generation_in_flight = false;
        prospective.buffering = false;
        prospective.accepted_discrete_command_this_frame = false;
        prospective.wall_clock_accumulator_ns = 0;
        prospective.mark_commit_frame_hold_v1();

        *self = prospective;
        self.refresh_ui_state_v1(self.candidate_clock_hold_flags_v1());
        Ok(self.ui_state_v1())
    }

    pub(crate) fn set_requested_generation_in_flight_v1(
        &mut self,
        in_flight: bool,
    ) -> &RemoteNavigationUiStateV1 {
        if self.requested_generation_in_flight && !in_flight {
            self.mark_hold_release_frame_suppress_v1();
        }
        self.requested_generation_in_flight = in_flight;
        self.refresh_ui_state_v1(self.candidate_clock_hold_flags_v1());
        self.ui_state_v1()
    }

    pub(crate) fn set_buffering_v1(&mut self, buffering: bool) -> &RemoteNavigationUiStateV1 {
        if self.buffering && !buffering {
            self.mark_hold_release_frame_suppress_v1();
        }
        self.buffering = buffering;
        self.refresh_ui_state_v1(self.candidate_clock_hold_flags_v1());
        self.ui_state_v1()
    }

    pub(crate) fn set_frozen_v1(&mut self, frozen: bool) -> &RemoteNavigationUiStateV1 {
        if self.frozen && !frozen {
            self.mark_hold_release_frame_suppress_v1();
        }
        self.frozen = frozen;
        self.refresh_ui_state_v1(self.candidate_clock_hold_flags_v1());
        self.ui_state_v1()
    }

    pub(crate) fn enter_background_v1(&mut self) -> &RemoteNavigationUiStateV1 {
        self.background_snapshot = Some(RemoteNavigationBackgroundSnapshotV1 {
            committed: self.committed,
            play_state: self.play_state,
        });
        self.is_background = true;
        self.wall_clock_accumulator_ns = 0;
        self.accepted_discrete_command_this_frame = false;
        self.refresh_ui_state_v1(self.candidate_clock_hold_flags_v1());
        self.ui_state_v1()
    }

    pub(crate) fn enter_foreground_v1(&mut self) -> &RemoteNavigationUiStateV1 {
        if let Some(snapshot) = self.background_snapshot.take() {
            if self.committed.is_none() {
                self.committed = snapshot.committed;
            }
            self.play_state = snapshot.play_state;
        }
        self.is_background = false;
        self.wall_clock_accumulator_ns = 0;
        self.mark_hold_release_frame_suppress_v1();
        self.refresh_ui_state_v1(self.candidate_clock_hold_flags_v1());
        self.ui_state_v1()
    }

    fn apply_merge_command_v1(
        &mut self,
        command: RemoteNavigationCommandV1,
    ) -> Result<(), RemoteNavigationErrorV1> {
        if command.timeline_v1() != self.canonical_timeline {
            return Err(RemoteNavigationErrorV1::UnsupportedRemoteNavigationTimeline);
        }

        match command {
            RemoteNavigationCommandV1::Seek { target, .. }
            | RemoteNavigationCommandV1::LoopJump { target, .. } => {
                self.ensure_target_in_extent_v1(target)?;
                let trigger = command
                    .trigger_v1()
                    .expect("seek and loop commands always have a trigger");
                self.set_requested_target_v1(target, trigger)?;
                self.target_marker = Some(target);
                self.accepted_discrete_command_this_frame = true;
            }
            RemoteNavigationCommandV1::StepForward { delta, .. }
            | RemoteNavigationCommandV1::StepBack { delta, .. } => {
                self.ensure_temporal_navigation_allowed_v1()?;
                if delta.is_static() || delta.as_i64() <= 0 {
                    return Err(RemoteNavigationErrorV1::InvalidTarget);
                }

                let base = self.step_base_target_v1()?;
                let delta = i128::from(delta.as_i64());
                let base = i128::from(base.as_i64());
                let candidate = match command {
                    RemoteNavigationCommandV1::StepForward { .. } => base.checked_add(delta),
                    RemoteNavigationCommandV1::StepBack { .. } => base.checked_sub(delta),
                    _ => unreachable!(),
                }
                .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?;
                let target = temporal_time_from_i128_v1(candidate)?;
                self.ensure_target_in_extent_v1(target)?;
                self.set_requested_target_v1(target, RemoteNavigationTriggerV1::Step)?;
                self.target_marker = Some(target);
                self.accepted_discrete_command_this_frame = true;
            }
            RemoteNavigationCommandV1::SetPlayState { play_state, .. } => {
                self.set_play_state_v1(play_state.remote_play_state_v1()?)?;
                self.accepted_discrete_command_this_frame = true;
            }
            RemoteNavigationCommandV1::Play { .. } => {
                self.set_play_state_v1(RemotePlayStateV1::Playing)?;
                self.accepted_discrete_command_this_frame = true;
            }
            RemoteNavigationCommandV1::Pause { .. } => {
                self.set_play_state_v1(RemotePlayStateV1::Paused)?;
                self.accepted_discrete_command_this_frame = true;
            }
            RemoteNavigationCommandV1::SetPlaybackSpeed { speed, .. } => {
                if speed <= 0 {
                    return Err(RemoteNavigationErrorV1::InvalidRemotePlaybackSpeed);
                }
                self.playback_speed = u64::try_from(speed)
                    .map_err(|_error| RemoteNavigationErrorV1::ArithmeticOverflow)?;
            }
            RemoteNavigationCommandV1::PlaybackAdvance { .. } => {
                return Err(RemoteNavigationErrorV1::AutomaticPlaybackAdvanceNotMergeable);
            }
        }

        Ok(())
    }

    fn set_play_state_v1(
        &mut self,
        play_state: RemotePlayStateV1,
    ) -> Result<(), RemoteNavigationErrorV1> {
        if play_state.is_playing_v1()
            && matches!(
                self.indexed_extent,
                CanonicalIndexedExtentV1::NoIndexedMessages
            )
        {
            return Err(RemoteNavigationErrorV1::NoIndexedMessages);
        }
        self.play_state = play_state;
        let target = self.step_base_target_v1()?;
        self.set_requested_target_v1(target, RemoteNavigationTriggerV1::PlayStateChange)?;
        Ok(())
    }

    fn set_requested_target_v1(
        &mut self,
        target: TimeInt,
        trigger: RemoteNavigationTriggerV1,
    ) -> Result<(), RemoteNavigationErrorV1> {
        let demand_key = RemoteNavigationDemandKeyV1::for_play_state_v1(target, self.play_state)?;
        let reused_generation = self
            .requested
            .filter(|intent| intent.demand_key == demand_key)
            .map(|intent| intent.generation);
        let generation = reused_generation.unwrap_or(self.next_requested_generation);
        let intent =
            PendingNavigationIntentV1::new_v1(generation, target, self.play_state, trigger)?;
        if reused_generation.is_none() {
            self.next_requested_generation = generation
                .checked_add(1)
                .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?;
        }
        self.requested = Some(intent);
        Ok(())
    }

    fn step_base_target_v1(&self) -> Result<TimeInt, RemoteNavigationErrorV1> {
        if let Some(requested) = self.requested {
            return Ok(requested.canonical_target);
        }
        if let Some(target_marker) = self.target_marker {
            return Ok(target_marker);
        }
        if let Some(committed) = self.committed {
            return Ok(committed.cursor);
        }
        match self.indexed_extent {
            CanonicalIndexedExtentV1::Known(range) => Ok(range.min()),
            CanonicalIndexedExtentV1::NoIndexedMessages => {
                Err(RemoteNavigationErrorV1::NoIndexedMessages)
            }
        }
    }

    fn advance_candidate_target_v1(
        &self,
        stable_dt: Duration,
        loop_mode: RemoteLoopModeV1,
    ) -> Result<TimeInt, RemoteNavigationErrorV1> {
        let range = match self.indexed_extent {
            CanonicalIndexedExtentV1::Known(range) => range,
            CanonicalIndexedExtentV1::NoIndexedMessages => {
                return Err(RemoteNavigationErrorV1::NoIndexedMessages);
            }
        };
        let base = self.committed.map_or_else(
            || Ok(range.min()),
            |committed| {
                if range.contains(committed.cursor) {
                    Ok(committed.cursor)
                } else {
                    Err(RemoteNavigationErrorV1::TargetOutsideIndexedExtent)
                }
            },
        )?;
        let delta = i128::from(stable_dt.as_nanos())
            .checked_mul(i128::from(self.playback_speed))
            .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?;
        let candidate = i128::from(base.as_i64())
            .checked_add(delta)
            .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?;
        bound_candidate_v1(range, candidate, loop_mode)
    }

    fn accumulate_wall_clock_v1(
        &mut self,
        stable_dt: Duration,
    ) -> Result<(), RemoteNavigationErrorV1> {
        let delta = u64::try_from(stable_dt.as_nanos())
            .map_err(|_error| RemoteNavigationErrorV1::InvalidClockDelta)?;
        self.wall_clock_accumulator_ns = self
            .wall_clock_accumulator_ns
            .checked_add(delta)
            .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?;
        Ok(())
    }

    fn ensure_temporal_navigation_allowed_v1(&self) -> Result<(), RemoteNavigationErrorV1> {
        match self.indexed_extent {
            CanonicalIndexedExtentV1::Known(_) => Ok(()),
            CanonicalIndexedExtentV1::NoIndexedMessages => {
                Err(RemoteNavigationErrorV1::NoIndexedMessages)
            }
        }
    }

    fn ensure_target_in_extent_v1(&self, target: TimeInt) -> Result<(), RemoteNavigationErrorV1> {
        validate_temporal_target_v1(target)?;
        match self.indexed_extent {
            CanonicalIndexedExtentV1::Known(range) if !range.contains(target) => {
                Err(RemoteNavigationErrorV1::TargetOutsideIndexedExtent)
            }
            CanonicalIndexedExtentV1::Known(_) => Ok(()),
            CanonicalIndexedExtentV1::NoIndexedMessages => {
                Err(RemoteNavigationErrorV1::NoIndexedMessages)
            }
        }
    }

    const fn candidate_clock_hold_flags_v1(&self) -> bool {
        self.frozen || self.is_background || self.requested_generation_in_flight || self.buffering
    }

    fn mark_commit_frame_hold_v1(&mut self) {
        if self.preview_update_seen_this_frame {
            // A commit that lands after this frame's preview has no remaining part of this frame
            // left to hold. The next preview is already the next frame and may integrate once.
            self.suppress_next_preview_dt = false;
        } else {
            self.suppress_next_preview_dt = true;
        }
        self.accepted_discrete_command_this_frame = false;
    }

    fn mark_hold_release_frame_suppress_v1(&mut self) {
        self.suppress_next_preview_dt = true;
        self.accepted_discrete_command_this_frame = false;
    }

    fn refresh_ui_state_v1(&mut self, clock_held: bool) {
        let target_marker = self
            .requested
            .map(|intent| intent.canonical_target)
            .or(self.target_marker);
        self.cached_ui_state = RemoteNavigationUiStateV1 {
            requested: self.requested,
            committed: self.committed,
            target_marker,
            is_loading: self.requested.is_some(),
            is_buffering: self.buffering,
            requested_generation_in_flight: self.requested_generation_in_flight,
            candidate_clock_held: clock_held || self.candidate_clock_hold_flags_v1(),
        };
    }
}

fn validate_indexed_extent_v1(
    indexed_extent: CanonicalIndexedExtentV1,
    play_state: RemotePlayStateV1,
) -> Result<(), RemoteNavigationErrorV1> {
    match indexed_extent {
        CanonicalIndexedExtentV1::NoIndexedMessages if play_state.is_playing_v1() => {
            Err(RemoteNavigationErrorV1::NoIndexedMessages)
        }
        CanonicalIndexedExtentV1::Known(range)
            if range.min().is_static() || range.max().is_static() || range.min() > range.max() =>
        {
            Err(RemoteNavigationErrorV1::InvalidTarget)
        }
        CanonicalIndexedExtentV1::NoIndexedMessages | CanonicalIndexedExtentV1::Known(_) => Ok(()),
    }
}

fn validate_committed_time_v1(
    canonical_timeline: Timeline,
    indexed_extent: CanonicalIndexedExtentV1,
    committed: Option<CommittedPresentationTimeV1>,
) -> Result<(), RemoteNavigationErrorV1> {
    if let Some(committed) = committed {
        if committed.timeline != canonical_timeline {
            return Err(RemoteNavigationErrorV1::UnsupportedRemoteNavigationTimeline);
        }
        validate_temporal_target_v1(committed.cursor)?;
        match indexed_extent {
            CanonicalIndexedExtentV1::Known(range) if !range.contains(committed.cursor) => {
                return Err(RemoteNavigationErrorV1::TargetOutsideIndexedExtent);
            }
            CanonicalIndexedExtentV1::Known(_) => {}
            CanonicalIndexedExtentV1::NoIndexedMessages => {
                return Err(RemoteNavigationErrorV1::NoIndexedMessages);
            }
        }
    }
    Ok(())
}

fn validate_temporal_target_v1(target: TimeInt) -> Result<(), RemoteNavigationErrorV1> {
    if target.is_static() {
        Err(RemoteNavigationErrorV1::InvalidTarget)
    } else {
        Ok(())
    }
}

fn temporal_time_from_i128_v1(value: i128) -> Result<TimeInt, RemoteNavigationErrorV1> {
    let min = i128::from(TimeInt::MIN.as_i64());
    let max = i128::from(TimeInt::MAX.as_i64());
    if !(min..=max).contains(&value) {
        return Err(RemoteNavigationErrorV1::ArithmeticOverflow);
    }
    let value =
        i64::try_from(value).map_err(|_error| RemoteNavigationErrorV1::ArithmeticOverflow)?;
    Ok(TimeInt::new_temporal(value))
}

fn bound_candidate_v1(
    range: AbsoluteTimeRange,
    candidate: i128,
    loop_mode: RemoteLoopModeV1,
) -> Result<TimeInt, RemoteNavigationErrorV1> {
    let min = i128::from(range.min().as_i64());
    let max = i128::from(range.max().as_i64());
    let bound = match loop_mode {
        RemoteLoopModeV1::Clamp => candidate.max(min).min(max),
        RemoteLoopModeV1::Loop => {
            let length = max
                .checked_sub(min)
                .and_then(|length| length.checked_add(1))
                .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?;
            if length <= 0 {
                return Err(RemoteNavigationErrorV1::ArithmeticOverflow);
            }
            let offset = candidate
                .checked_sub(min)
                .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?;
            let wrapped_offset = offset.rem_euclid(length);
            min.checked_add(wrapped_offset)
                .ok_or(RemoteNavigationErrorV1::ArithmeticOverflow)?
        }
    };
    temporal_time_from_i128_v1(bound)
}

#[cfg(test)]
mod tests {
    use re_log_types::TimeType;

    use super::*;

    fn canonical_timeline() -> Timeline {
        Timeline::new("message_log_time", TimeType::DurationNs)
    }

    fn other_timeline() -> Timeline {
        Timeline::new("other_time", TimeType::DurationNs)
    }

    fn extent() -> CanonicalIndexedExtentV1 {
        CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(0, 100))
    }

    fn committed_at(cursor: i64) -> CommittedPresentationTimeV1 {
        CommittedPresentationTimeV1::new_v1(canonical_timeline(), TimeInt::new_temporal(cursor))
    }

    fn playing_adapter() -> RemoteNavigationAdapterV1 {
        RemoteNavigationAdapterV1::from_extent_v1(
            canonical_timeline(),
            extent(),
            Some(committed_at(10)),
        )
        .expect("known extent adapter should construct")
    }

    fn uncommitted_playing_adapter() -> RemoteNavigationAdapterV1 {
        RemoteNavigationAdapterV1::from_extent_v1(canonical_timeline(), extent(), None)
            .expect("known extent adapter without committed time should construct")
    }

    fn command(command: RemoteNavigationCommandV1) -> Vec<RemoteNavigationCommandV1> {
        vec![command]
    }

    fn clock(dt: i64, held: bool) -> RemoteCandidateClockInputV1 {
        RemoteCandidateClockInputV1::new_v1(Duration::from_nanos(dt), held)
    }

    #[test]
    fn rejects_non_canonical_timeline_without_mutation() {
        let mut adapter = playing_adapter();
        let before = adapter.clone();

        let error = adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: other_timeline(),
                target: TimeInt::new_temporal(20),
            }))
            .expect_err("non-canonical seek should fail");

        assert_eq!(
            error,
            RemoteNavigationErrorV1::UnsupportedRemoteNavigationTimeline
        );
        assert_eq!(adapter, before);
    }

    #[test]
    fn rejects_following_without_mutation() {
        let mut adapter = playing_adapter();
        let before = adapter.clone();

        let error = adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::SetPlayState {
                timeline: canonical_timeline(),
                play_state: GenericRemotePlayStateV1::Following,
            }))
            .expect_err("Following should fail");

        assert_eq!(error, RemoteNavigationErrorV1::UnsupportedRemotePlayState);
        assert_eq!(adapter, before);
    }

    #[test]
    fn rejects_non_positive_speed_without_mutation() {
        let mut adapter = playing_adapter();
        let before = adapter.clone();

        for speed in [0, -1] {
            let error = adapter
                .merge_commands_v1(&command(RemoteNavigationCommandV1::SetPlaybackSpeed {
                    timeline: canonical_timeline(),
                    speed,
                }))
                .expect_err("non-positive speed should fail");
            assert_eq!(error, RemoteNavigationErrorV1::InvalidRemotePlaybackSpeed);
            assert_eq!(adapter, before);
        }
    }

    #[test]
    fn held_clock_preserves_request_and_does_not_consume_dt() {
        let mut adapter = playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(30),
            }))
            .expect("seek should be accepted");
        let requested = adapter.requested_v1();

        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(5, true))
            .expect("held preview should succeed");

        assert_eq!(adapter.requested_v1(), requested);
        assert_eq!(adapter.wall_clock_accumulator_ns_v1(), 0);
        assert!(adapter.ui_state_v1().candidate_clock_held);
    }

    #[test]
    fn held_discrete_command_is_latest_wins_but_preview_does_not_auto_advance() {
        let mut adapter = playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(40),
            }))
            .expect("seek should be accepted");

        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::StepForward {
                timeline: canonical_timeline(),
                delta: TimeInt::new_temporal(2),
            }))
            .expect("step should be accepted");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(42))
        );

        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(100, true))
            .expect("held preview should succeed");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(42))
        );
        assert_eq!(adapter.wall_clock_accumulator_ns_v1(), 0);
    }

    #[test]
    fn play_pause_preserves_latest_requested_target_not_committed_cursor() {
        let mut adapter = playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(30),
            }))
            .expect("seek should be accepted");

        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Pause {
                timeline: canonical_timeline(),
            }))
            .expect("pause should be accepted");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(30))
        );
        assert_eq!(adapter.play_state_v1(), RemotePlayStateV1::Paused);

        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Play {
                timeline: canonical_timeline(),
            }))
            .expect("play should be accepted");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(30))
        );
        assert_eq!(adapter.play_state_v1(), RemotePlayStateV1::Playing);
    }

    #[test]
    fn commit_updates_only_committed_time_and_clears_latches() {
        let mut adapter = playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(30),
            }))
            .expect("seek should be accepted");
        let generation = adapter
            .requested_generation_v1()
            .expect("seek creates a generation");
        adapter.set_requested_generation_in_flight_v1(true);
        adapter.set_buffering_v1(true);

        adapter
            .commit_presentation_v1(Some(committed_at(30)), generation)
            .expect("commit should succeed");

        assert_eq!(adapter.committed_time_v1(), Some(committed_at(30)));
        assert_eq!(adapter.requested_v1(), None);
        assert!(!adapter.requested_generation_in_flight_v1());
        assert!(!adapter.buffering_v1());
        assert!(!adapter.ui_state_v1().is_loading);
    }

    #[test]
    fn foreground_resume_does_not_replay_background_dt() {
        let mut adapter = playing_adapter();
        adapter.enter_background_v1();
        adapter.enter_foreground_v1();

        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(100, false))
            .expect("first foreground preview should succeed");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            None
        );

        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(5, false))
            .expect("second foreground preview should succeed");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(15))
        );
    }

    #[test]
    fn extent_selects_initial_play_state_and_gates_temporal_navigation() {
        let known = RemoteNavigationAdapterV1::from_extent_v1(
            canonical_timeline(),
            extent(),
            Some(committed_at(10)),
        )
        .expect("known extent should construct");
        assert_eq!(known.play_state_v1(), RemotePlayStateV1::Playing);

        let no_messages = RemoteNavigationAdapterV1::from_extent_v1(
            canonical_timeline(),
            CanonicalIndexedExtentV1::NoIndexedMessages,
            None,
        )
        .expect("no-indexed extent should construct");
        assert_eq!(no_messages.play_state_v1(), RemotePlayStateV1::Paused);

        let mut no_messages = no_messages;
        let error = no_messages
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(1),
            }))
            .expect_err("temporal navigation should be gated");
        assert_eq!(error, RemoteNavigationErrorV1::NoIndexedMessages);

        let error = no_messages
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Play {
                timeline: canonical_timeline(),
            }))
            .expect_err("playing without indexed messages should be gated");
        assert_eq!(error, RemoteNavigationErrorV1::NoIndexedMessages);
    }

    #[test]
    fn positive_speed_is_accepted_and_stored() {
        let mut adapter = playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::SetPlaybackSpeed {
                timeline: canonical_timeline(),
                speed: 2,
            }))
            .expect("positive speed should be accepted");
        assert_eq!(adapter.playback_speed_v1(), 2);
    }

    #[test]
    fn invalid_preview_input_does_not_leave_partial_state() {
        let mut adapter = playing_adapter();
        let before = adapter.clone();

        let error = adapter
            .preview_update_v1(
                CanonicalIndexedExtentV1::NoIndexedMessages,
                RemoteLoopModeV1::Clamp,
                clock(1, false),
            )
            .expect_err("playing without indexed messages should fail");

        assert_eq!(error, RemoteNavigationErrorV1::NoIndexedMessages);
        assert_eq!(adapter, before);
    }

    #[test]
    fn loop_and_clamp_are_checked() {
        let mut adapter = playing_adapter();
        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Loop, clock(95, false))
            .expect("loop preview should succeed");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(4))
        );

        let mut adapter = playing_adapter();
        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(200, false))
            .expect("clamp preview should succeed");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(100))
        );
    }

    #[test]
    fn stale_presentation_commit_does_not_clear_latest_generation() {
        let mut adapter = playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(30),
            }))
            .expect("first seek should be accepted");
        let stale_generation = adapter
            .requested_generation_v1()
            .expect("first seek creates a generation");
        adapter.set_requested_generation_in_flight_v1(true);
        adapter.set_buffering_v1(true);

        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(40),
            }))
            .expect("latest-wins seek should be accepted");
        let current_generation = adapter
            .requested_generation_v1()
            .expect("latest-wins seek creates a generation");
        assert_ne!(stale_generation, current_generation);

        let before = adapter.clone();
        let error = adapter
            .commit_presentation_v1(Some(committed_at(30)), stale_generation)
            .expect_err("stale commit should fail");
        assert_eq!(error, RemoteNavigationErrorV1::StalePresentationCommit);
        assert_eq!(adapter, before);
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(40))
        );
        assert!(adapter.requested_generation_in_flight_v1());
        assert!(adapter.buffering_v1());
        assert_eq!(adapter.committed_time_v1(), Some(committed_at(10)));

        adapter
            .commit_presentation_v1(Some(committed_at(40)), current_generation)
            .expect("current generation commit should succeed");
        assert_eq!(adapter.committed_time_v1(), Some(committed_at(40)));
        assert_eq!(adapter.requested_v1(), None);
        assert!(!adapter.requested_generation_in_flight_v1());
        assert!(!adapter.buffering_v1());
    }

    #[test]
    fn accepted_discrete_no_indexed_messages_is_atomic() {
        let mut adapter = uncommitted_playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(20),
            }))
            .expect("seek should be accepted");
        let before = adapter.clone();

        let error = adapter
            .preview_update_v1(
                CanonicalIndexedExtentV1::NoIndexedMessages,
                RemoteLoopModeV1::Clamp,
                clock(5, false),
            )
            .expect_err("accepted-discrete NoIndexedMessages should fail atomically");

        assert_eq!(error, RemoteNavigationErrorV1::NoIndexedMessages);
        assert_eq!(adapter, before);
    }

    #[test]
    fn inverted_extent_is_rejected_atomically_before_clamp() {
        let inverted = CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(100, 0));

        let mut adapter = uncommitted_playing_adapter();
        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(20),
            }))
            .expect("seek should be accepted");
        let before = adapter.clone();
        let error = adapter
            .preview_update_v1(inverted, RemoteLoopModeV1::Clamp, clock(5, false))
            .expect_err("inverted extent should fail before auto-advance");
        assert_eq!(error, RemoteNavigationErrorV1::InvalidTarget);
        assert_eq!(adapter, before);

        let mut adapter = uncommitted_playing_adapter();
        let before = adapter.clone();
        let error = adapter
            .preview_update_v1(inverted, RemoteLoopModeV1::Clamp, clock(5, false))
            .expect_err("inverted extent should fail before auto-advance");
        assert_eq!(error, RemoteNavigationErrorV1::InvalidTarget);
        assert_eq!(adapter, before);
    }

    #[test]
    fn same_cursor_play_state_change_is_a_pending_demand() {
        let mut adapter = playing_adapter();
        assert!(!adapter.ui_state_v1().is_loading);

        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Pause {
                timeline: canonical_timeline(),
            }))
            .expect("pause should be accepted");
        let pause_generation = adapter
            .requested_generation_v1()
            .expect("pause at current cursor creates a requested demand");
        assert_eq!(adapter.play_state_v1(), RemotePlayStateV1::Paused);
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(committed_at(10).cursor)
        );
        assert!(adapter.ui_state_v1().is_loading);

        adapter
            .commit_presentation_v1(Some(committed_at(10)), pause_generation)
            .expect("pause demand should commit");
        assert!(!adapter.ui_state_v1().is_loading);

        adapter
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Play {
                timeline: canonical_timeline(),
            }))
            .expect("play should be accepted");
        assert_eq!(adapter.play_state_v1(), RemotePlayStateV1::Playing);
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(committed_at(10).cursor)
        );
        assert!(adapter.ui_state_v1().is_loading);
        assert!(
            adapter
                .requested_v1()
                .expect("play creates requested demand")
                .demand_key
                .requires_minimum_playback_buffer
        );
    }

    #[test]
    fn commit_hold_semantics_are_frame_order_aware() {
        let mut commit_before_preview = playing_adapter();
        commit_before_preview
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(30),
            }))
            .expect("seek should be accepted");
        let generation = commit_before_preview
            .requested_generation_v1()
            .expect("seek creates a generation");
        commit_before_preview
            .commit_presentation_v1(Some(committed_at(30)), generation)
            .expect("commit should succeed");
        commit_before_preview
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(100, false))
            .expect("same-frame preview should succeed");
        assert_eq!(commit_before_preview.requested_v1(), None);
        commit_before_preview
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(5, false))
            .expect("next-frame preview should succeed");
        assert_eq!(
            commit_before_preview
                .requested_v1()
                .map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(35))
        );

        let mut commit_after_preview = playing_adapter();
        commit_after_preview
            .merge_commands_v1(&command(RemoteNavigationCommandV1::Seek {
                timeline: canonical_timeline(),
                target: TimeInt::new_temporal(30),
            }))
            .expect("seek should be accepted");
        let generation = commit_after_preview
            .requested_generation_v1()
            .expect("seek creates a generation");
        commit_after_preview
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(100, false))
            .expect("command-frame preview should succeed");
        commit_after_preview
            .commit_presentation_v1(Some(committed_at(30)), generation)
            .expect("commit after preview should succeed");
        commit_after_preview
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(5, false))
            .expect("next-frame preview should integrate normally");
        assert_eq!(
            commit_after_preview
                .requested_v1()
                .map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(35))
        );
    }

    #[test]
    fn frozen_release_suppresses_one_preview_dt() {
        let mut adapter = playing_adapter();
        adapter.set_frozen_v1(true);
        adapter.set_frozen_v1(false);
        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(100, false))
            .expect("recovery preview should succeed");
        assert_eq!(adapter.requested_v1(), None);

        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(5, false))
            .expect("post-recovery preview should succeed");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(15))
        );
    }

    #[test]
    fn generation_in_flight_release_suppresses_one_preview_dt() {
        let mut adapter = playing_adapter();
        adapter.set_requested_generation_in_flight_v1(true);
        adapter.set_requested_generation_in_flight_v1(false);
        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(100, false))
            .expect("release preview should succeed");
        assert_eq!(adapter.requested_v1(), None);

        adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(5, false))
            .expect("post-release preview should succeed");
        assert_eq!(
            adapter.requested_v1().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(15))
        );
    }

    #[test]
    fn checked_error_paths_leave_state_unchanged() {
        let mut adapter = playing_adapter();
        let before = adapter.clone();
        let error = adapter
            .preview_update_v1(extent(), RemoteLoopModeV1::Clamp, clock(-1, false))
            .expect_err("negative stable_dt should fail");
        assert_eq!(error, RemoteNavigationErrorV1::InvalidClockDelta);
        assert_eq!(adapter, before);

        assert_eq!(
            temporal_time_from_i128_v1(i128::from(TimeInt::MAX.as_i64()) + 1),
            Err(RemoteNavigationErrorV1::ArithmeticOverflow)
        );
        assert_eq!(
            temporal_time_from_i128_v1(i128::from(TimeInt::MIN.as_i64()) - 1),
            Err(RemoteNavigationErrorV1::ArithmeticOverflow)
        );
        assert_eq!(
            validate_temporal_target_v1(TimeInt::STATIC),
            Err(RemoteNavigationErrorV1::InvalidTarget)
        );

        let mut adapter = playing_adapter();
        let before = adapter.clone();
        let error = adapter
            .commit_presentation_v1(
                Some(CommittedPresentationTimeV1::new_v1(
                    other_timeline(),
                    TimeInt::new_temporal(10),
                )),
                0,
            )
            .expect_err("wrong committed timeline should fail");
        assert_eq!(
            error,
            RemoteNavigationErrorV1::UnsupportedRemoteNavigationTimeline
        );
        assert_eq!(adapter, before);
    }
}
