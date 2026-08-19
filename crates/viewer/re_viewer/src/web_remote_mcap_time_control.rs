//! Production-disarmed remote `TimeControl`, `TimePanel`, and navigation-range boundary.
//!
//! This module keeps requested navigation intent separate from the committed presentation cursor.
//! It deliberately consumes only the narrow storage-free recording-query capability from
//! MCAP-096; it does not import or construct `ViewerContext`, `AppContext`, `StoreHub`,
//! `StoreBundle`, `EntityDb`, or a storage engine.

#![allow(dead_code)]

use re_chunk::{TimeInt, TimelineName};
use re_log_types::{AbsoluteTimeRange, Duration};
use re_sdk_types::blueprint::components::{LoopMode, PlayState};
use re_viewer_context::{MoveDirection, MoveSpeed, TimeControlCommand};

use crate::web_remote_mcap_activation::RemotePlayStateV1;
use crate::web_remote_mcap_consumer::RecordingConsumerQueryV1;
use crate::web_remote_mcap_query::{
    CommittedPresentationTimeV1, CompleteIndexedCoverageV1, ConsumerStorageFreeV1,
    RemoteCanonicalIndexedExtentV1, RemoteLoadedCoverageV1,
};

const REMOTE_STEP_NANOS_V1: i64 = 1_000_000_000;

mod private {
    pub(crate) struct TimeControlAdapterSealV1;
    pub(crate) struct RemoteTimeControlConsumerSealV1;
    pub(crate) struct LocalTimeControlPassthroughSealV1;

    pub(crate) trait SealedRemoteTimeControlAdapterV1 {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteNavigationTriggerV1 {
    Seek,
    Step,
    LoopJump,
    PlaybackAdvance,
    RemotePlayStateChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteNavigationDemandKeyV1 {
    pub(crate) canonical_target: TimeInt,
    pub(crate) requires_minimum_playback_buffer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingNavigationIntentV1 {
    pub(crate) canonical_target: TimeInt,
    pub(crate) play_state: RemotePlayStateV1,
    pub(crate) trigger: RemoteNavigationTriggerV1,
    pub(crate) demand_key: RemoteNavigationDemandKeyV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
pub(crate) struct RemoteCandidateClockInputV1 {
    pub(crate) stable_dt: Duration,
    pub(crate) held: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteTimeControlErrorV1 {
    UnsupportedRemoteNavigationTimeline { requested: TimelineName },
    UnsupportedRemotePlayState,
    UnsupportedRemoteLoopMode { requested: LoopMode },
    InvalidRemotePlaybackSpeed,
}

/// Projection of manifest/coverage and navigation UI state for the remote `TimePanel`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTimePanelProjectionV1 {
    pub(crate) indexed_extent: Option<RemoteCanonicalIndexedExtentV1>,
    pub(crate) loaded_ranges: Vec<AbsoluteTimeRange>,
    pub(crate) loading_window: Option<AbsoluteTimeRange>,
    pub(crate) is_buffering: bool,
    pub(crate) candidate_clock_held: bool,
    pub(crate) no_temporal_data: bool,
    pub(crate) complete_indexed_coverage: CompleteIndexedCoverageV1,
}

/// Canonical extent helper.
///
/// `None` means there is no known temporal boundary for `TimeControl`. This is deliberately
/// distinct from `Some(RemoteCanonicalIndexedExtentV1::NoIndexedMessages)`.
pub(crate) fn canonical_time_control_extent_v1(
    timeline: &TimelineName,
    canonical_timeline: &TimelineName,
    extent: Option<&RemoteCanonicalIndexedExtentV1>,
) -> Result<Option<RemoteCanonicalIndexedExtentV1>, RemoteTimeControlErrorV1> {
    if timeline != canonical_timeline {
        return Err(
            RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                requested: *timeline,
            },
        );
    }

    Ok(extent.cloned())
}

/// Converts a generic blueprint play state to the canonical remote play-state domain.
pub(crate) fn remote_play_state_from_generic_v1(
    play_state: PlayState,
) -> Result<RemotePlayStateV1, RemoteTimeControlErrorV1> {
    match play_state {
        PlayState::Paused => Ok(RemotePlayStateV1::Paused),
        PlayState::Playing => Ok(RemotePlayStateV1::Playing),
        PlayState::Following => Err(RemoteTimeControlErrorV1::UnsupportedRemotePlayState),
    }
}

/// Storage-free `TimeControl` adapter state machine.
pub(crate) struct RemoteTimeControlAdapterStateV1 {
    canonical_timeline: TimelineName,
    requested: Option<PendingNavigationIntentV1>,
    committed: Option<CommittedPresentationTimeV1>,
    committed_demand_key: Option<RemoteNavigationDemandKeyV1>,
    play_state: RemotePlayStateV1,
    speed: f32,
    loop_mode: LoopMode,
    requested_generation_in_flight: bool,
    is_buffering: bool,
    candidate_clock_held: bool,
    ui_state: RemoteNavigationUiStateV1,
}

impl RemoteTimeControlAdapterStateV1 {
    pub(crate) fn new_v1(
        canonical_timeline: TimelineName,
        initial_play_state: RemotePlayStateV1,
    ) -> Self {
        let mut state = Self {
            canonical_timeline,
            requested: None,
            committed: None,
            committed_demand_key: None,
            play_state: initial_play_state,
            speed: 1.0,
            loop_mode: LoopMode::Off,
            requested_generation_in_flight: false,
            is_buffering: false,
            candidate_clock_held: false,
            ui_state: RemoteNavigationUiStateV1 {
                requested: None,
                committed: None,
                target_marker: None,
                is_loading: false,
                is_buffering: false,
                requested_generation_in_flight: false,
                candidate_clock_held: false,
            },
        };
        state.refresh_ui_state_v1();
        state
    }

    pub(crate) fn canonical_timeline_v1(&self) -> TimelineName {
        self.canonical_timeline
    }

    pub(crate) fn play_state_v1(&self) -> RemotePlayStateV1 {
        self.play_state
    }

    pub(crate) fn speed_v1(&self) -> f32 {
        self.speed
    }

    pub(crate) fn set_buffering_v1(&mut self, buffering: bool) {
        self.is_buffering = buffering;
        self.refresh_ui_state_v1();
    }

    fn ensure_requested_v1(&mut self, trigger: RemoteNavigationTriggerV1) {
        if self.requested.is_some() {
            return;
        }

        let target = self
            .requested
            .as_ref()
            .map(|intent| intent.canonical_target)
            .or_else(|| self.committed.as_ref().map(|time| time.cursor))
            .unwrap_or(TimeInt::ZERO);

        self.requested = Some(Self::make_intent_v1(target, self.play_state, trigger));
    }

    fn set_target_v1(&mut self, target: TimeInt, trigger: RemoteNavigationTriggerV1) {
        let Some(demand_key) = self
            .requested
            .as_ref()
            .map(|intent| Self::make_demand_key_v1(target, intent.play_state))
        else {
            self.requested = Some(Self::make_intent_v1(target, self.play_state, trigger));
            return;
        };

        if let Some(intent) = &mut self.requested {
            intent.canonical_target = target;
            intent.trigger = trigger;
            intent.demand_key = demand_key;
        }
    }

    fn set_play_state_v1(&mut self, play_state: RemotePlayStateV1) {
        self.play_state = play_state;

        let Some(demand_key) = self
            .requested
            .as_ref()
            .map(|intent| Self::make_demand_key_v1(intent.canonical_target, play_state))
        else {
            self.ensure_requested_v1(RemoteNavigationTriggerV1::RemotePlayStateChange);
            return;
        };

        if let Some(intent) = &mut self.requested {
            intent.play_state = play_state;
            intent.trigger = RemoteNavigationTriggerV1::RemotePlayStateChange;
            intent.demand_key = demand_key;
        }
    }

    fn move_by_v1(&mut self, nanos: i64) {
        let base = self
            .requested
            .as_ref()
            .map(|intent| intent.canonical_target)
            .or_else(|| self.committed.as_ref().map(|time| time.cursor))
            .unwrap_or(TimeInt::ZERO);
        let target = add_nanos_v1(base, nanos);
        self.set_target_v1(target, RemoteNavigationTriggerV1::Step);
    }

    fn make_demand_key_v1(
        target: TimeInt,
        play_state: RemotePlayStateV1,
    ) -> RemoteNavigationDemandKeyV1 {
        RemoteNavigationDemandKeyV1 {
            canonical_target: target,
            requires_minimum_playback_buffer: play_state == RemotePlayStateV1::Playing,
        }
    }

    fn make_intent_v1(
        target: TimeInt,
        play_state: RemotePlayStateV1,
        trigger: RemoteNavigationTriggerV1,
    ) -> PendingNavigationIntentV1 {
        PendingNavigationIntentV1 {
            canonical_target: target,
            play_state,
            trigger,
            demand_key: Self::make_demand_key_v1(target, play_state),
        }
    }

    fn apply_command_v1(
        &mut self,
        command: &TimeControlCommand,
    ) -> Result<bool, RemoteTimeControlErrorV1> {
        match command {
            TimeControlCommand::HighlightRange(_)
            | TimeControlCommand::ResetActiveTimeline
            | TimeControlCommand::SetTimeSelection(_)
            | TimeControlCommand::SetTimeSelectionClamped(_)
            | TimeControlCommand::RemoveTimeSelection
            | TimeControlCommand::SetTimeView(_)
            | TimeControlCommand::ResetTimeView
            | TimeControlCommand::SetFps(_)
            | TimeControlCommand::Buffer => Ok(false),

            TimeControlCommand::SetActiveTimeline(timeline) => {
                if timeline != &self.canonical_timeline {
                    Err(
                        RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                            requested: *timeline,
                        },
                    )
                } else {
                    Ok(false)
                }
            }

            TimeControlCommand::SetLoopMode(loop_mode) => match *loop_mode {
                LoopMode::Off | LoopMode::All => {
                    self.loop_mode = *loop_mode;
                    Ok(false)
                }
                LoopMode::Selection => Err(RemoteTimeControlErrorV1::UnsupportedRemoteLoopMode {
                    requested: *loop_mode,
                }),
            },

            TimeControlCommand::SetPlayState(play_state) => match *play_state {
                PlayState::Paused => {
                    let changed = self.play_state != RemotePlayStateV1::Paused;
                    self.set_play_state_v1(RemotePlayStateV1::Paused);
                    Ok(changed)
                }
                PlayState::Playing => {
                    let changed = self.play_state != RemotePlayStateV1::Playing;
                    self.set_play_state_v1(RemotePlayStateV1::Playing);
                    Ok(changed)
                }
                PlayState::Following => Err(RemoteTimeControlErrorV1::UnsupportedRemotePlayState),
            },

            TimeControlCommand::Pause => {
                let changed = self.play_state != RemotePlayStateV1::Paused;
                self.set_play_state_v1(RemotePlayStateV1::Paused);
                Ok(changed)
            }

            TimeControlCommand::TogglePlayPause => {
                let next = match self.play_state {
                    RemotePlayStateV1::Paused => RemotePlayStateV1::Playing,
                    RemotePlayStateV1::Playing => RemotePlayStateV1::Paused,
                };
                self.set_play_state_v1(next);
                Ok(true)
            }

            TimeControlCommand::StepTimeBack => {
                self.move_by_v1(-REMOTE_STEP_NANOS_V1);
                Ok(true)
            }
            TimeControlCommand::StepTimeForward => {
                self.move_by_v1(REMOTE_STEP_NANOS_V1);
                Ok(true)
            }

            TimeControlCommand::Move { direction, speed } => {
                let magnitude = REMOTE_STEP_NANOS_V1
                    * match speed {
                        MoveSpeed::Normal => 1,
                        MoveSpeed::Fast => 8,
                    };
                let signed = match direction {
                    MoveDirection::Back => -magnitude,
                    MoveDirection::Forward => magnitude,
                };
                self.move_by_v1(signed);
                Ok(true)
            }

            TimeControlCommand::MoveBeginning => {
                self.set_target_v1(TimeInt::MIN, RemoteNavigationTriggerV1::Seek);
                Ok(true)
            }
            TimeControlCommand::MoveEndAndFollow => {
                Err(RemoteTimeControlErrorV1::UnsupportedRemotePlayState)
            }

            TimeControlCommand::SetSpeed(speed) => {
                if !speed.is_finite() || *speed <= 0.0 {
                    Err(RemoteTimeControlErrorV1::InvalidRemotePlaybackSpeed)
                } else {
                    self.speed = *speed;
                    Ok(false)
                }
            }

            TimeControlCommand::SetTime(time) | TimeControlCommand::SetTimeClamped(time) => {
                self.set_target_v1(time.floor(), RemoteNavigationTriggerV1::Seek);
                Ok(true)
            }
        }
    }

    fn validate_command_v1(
        &self,
        command: &TimeControlCommand,
    ) -> Result<(), RemoteTimeControlErrorV1> {
        match command {
            TimeControlCommand::SetActiveTimeline(timeline)
                if timeline != &self.canonical_timeline =>
            {
                Err(
                    RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                        requested: *timeline,
                    },
                )
            }
            TimeControlCommand::SetLoopMode(loop_mode) => match *loop_mode {
                LoopMode::Off | LoopMode::All => Ok(()),
                LoopMode::Selection => Err(RemoteTimeControlErrorV1::UnsupportedRemoteLoopMode {
                    requested: *loop_mode,
                }),
            },
            TimeControlCommand::SetPlayState(PlayState::Following)
            | TimeControlCommand::MoveEndAndFollow => {
                Err(RemoteTimeControlErrorV1::UnsupportedRemotePlayState)
            }
            TimeControlCommand::SetSpeed(speed) if !speed.is_finite() || *speed <= 0.0 => {
                Err(RemoteTimeControlErrorV1::InvalidRemotePlaybackSpeed)
            }
            _ => Ok(()),
        }
    }

    fn validate_committed_presentation_v1(
        &self,
        committed: Option<&CommittedPresentationTimeV1>,
    ) -> Result<(), RemoteTimeControlErrorV1> {
        if let Some(committed) = committed
            && committed.timeline != self.canonical_timeline
        {
            return Err(
                RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                    requested: committed.timeline,
                },
            );
        }
        Ok(())
    }

    fn refresh_ui_state_v1(&mut self) {
        let target_marker = self
            .requested
            .as_ref()
            .map(|intent| intent.canonical_target);
        let requested_demand_differs = match (&self.requested, &self.committed_demand_key) {
            (Some(intent), Some(committed_demand_key)) => {
                intent.demand_key != *committed_demand_key
            }
            (Some(_), None) => true,
            (None, _) => false,
        };

        self.ui_state = RemoteNavigationUiStateV1 {
            requested: self.requested.clone(),
            committed: self.committed.clone(),
            target_marker,
            is_loading: self.requested_generation_in_flight || requested_demand_differs,
            is_buffering: self.is_buffering,
            requested_generation_in_flight: self.requested_generation_in_flight,
            candidate_clock_held: self.candidate_clock_held,
        };
    }
}

pub(crate) trait RemoteTimeControlAdapterV1:
    private::SealedRemoteTimeControlAdapterV1 + ConsumerStorageFreeV1
{
    fn merge_commands_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        commands: &[TimeControlCommand],
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1>;

    fn preview_update_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        clock: RemoteCandidateClockInputV1,
        extent: &RemoteCanonicalIndexedExtentV1,
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1>;

    fn commit_presentation_v1(&mut self, committed: Option<CommittedPresentationTimeV1>);
}

impl private::SealedRemoteTimeControlAdapterV1 for RemoteTimeControlAdapterStateV1 {}

impl RemoteTimeControlAdapterV1 for RemoteTimeControlAdapterStateV1 {
    fn merge_commands_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        commands: &[TimeControlCommand],
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1> {
        self.validate_committed_presentation_v1(committed)?;
        for command in commands {
            self.validate_command_v1(command)?;
        }

        self.committed = committed.cloned();

        let mut command_created_demand = false;
        for command in commands {
            command_created_demand |= self.apply_command_v1(command)?;
        }
        if command_created_demand {
            self.requested_generation_in_flight = true;
        }

        self.refresh_ui_state_v1();
        Ok(&self.ui_state)
    }

    fn preview_update_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        clock: RemoteCandidateClockInputV1,
        extent: &RemoteCanonicalIndexedExtentV1,
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1> {
        self.validate_committed_presentation_v1(committed)?;
        self.committed = committed.cloned();
        self.candidate_clock_held = clock.held;

        match extent {
            RemoteCanonicalIndexedExtentV1::Known(range) => {
                if !clock.held {
                    let base = self
                        .requested
                        .as_ref()
                        .map(|intent| intent.canonical_target)
                        .or_else(|| self.committed.as_ref().map(|time| time.cursor))
                        .unwrap_or_else(|| range.min());
                    let base = clamp_to_range_v1(base, *range);

                    let (next, trigger) = if self.play_state == RemotePlayStateV1::Playing {
                        playback_advance_v1(
                            base,
                            clock.stable_dt,
                            self.speed,
                            *range,
                            self.loop_mode,
                        )
                    } else {
                        (base, RemoteNavigationTriggerV1::Seek)
                    };

                    let Some(demand_key) = self
                        .requested
                        .as_ref()
                        .filter(|intent| intent.canonical_target != next)
                        .map(|intent| Self::make_demand_key_v1(next, intent.play_state))
                    else {
                        if self.requested.is_none() {
                            self.requested =
                                Some(Self::make_intent_v1(next, self.play_state, trigger));
                            self.requested_generation_in_flight = true;
                        }
                        self.refresh_ui_state_v1();
                        return Ok(&self.ui_state);
                    };

                    if let Some(intent) = &mut self.requested {
                        intent.canonical_target = next;
                        intent.trigger = trigger;
                        intent.demand_key = demand_key;
                        self.requested_generation_in_flight = true;
                    }
                }
            }

            RemoteCanonicalIndexedExtentV1::NoIndexedMessages => {
                // No temporal navigation cursor exists; static presentation remains available.
                self.requested = None;
                self.play_state = RemotePlayStateV1::Paused;
                self.requested_generation_in_flight = false;
            }
        }

        self.refresh_ui_state_v1();
        Ok(&self.ui_state)
    }

    fn commit_presentation_v1(&mut self, committed: Option<CommittedPresentationTimeV1>) {
        self.committed = committed.clone();
        self.committed_demand_key = committed
            .as_ref()
            .map(|time| Self::make_demand_key_v1(time.cursor, self.play_state));
        self.requested_generation_in_flight = false;
        self.is_buffering = false;
        self.candidate_clock_held = false;
        self.refresh_ui_state_v1();
    }
}

/// Narrow remote `TimeControl` consumer.
///
/// It receives committed presentation time only through the MCAP-096 recording-query capability.
pub(crate) struct RemoteTimeControlConsumerV1<'a> {
    query: &'a dyn RecordingConsumerQueryV1,
    state: RemoteTimeControlAdapterStateV1,
    _sealed: private::RemoteTimeControlConsumerSealV1,
}

impl<'a> RemoteTimeControlConsumerV1<'a> {
    pub(crate) fn new_v1(
        query: &'a dyn RecordingConsumerQueryV1,
        canonical_timeline: TimelineName,
        initial_play_state: RemotePlayStateV1,
    ) -> Self {
        Self {
            query,
            state: RemoteTimeControlAdapterStateV1::new_v1(canonical_timeline, initial_play_state),
            _sealed: private::RemoteTimeControlConsumerSealV1,
        }
    }

    pub(crate) fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        let lease = self.query.try_lease_v1().ok()?;
        lease.committed_time_v1()
    }
}

impl private::SealedRemoteTimeControlAdapterV1 for RemoteTimeControlConsumerV1<'_> {}

impl RemoteTimeControlAdapterV1 for RemoteTimeControlConsumerV1<'_> {
    fn merge_commands_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        commands: &[TimeControlCommand],
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1> {
        self.state.merge_commands_v1(committed, commands)
    }

    fn preview_update_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        clock: RemoteCandidateClockInputV1,
        extent: &RemoteCanonicalIndexedExtentV1,
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1> {
        self.state.preview_update_v1(committed, clock, extent)
    }

    fn commit_presentation_v1(&mut self, committed: Option<CommittedPresentationTimeV1>) {
        self.state.commit_presentation_v1(committed);
    }
}

/// Production-disarmed native/local passthrough using the same trait surface.
pub(crate) struct LocalTimeControlPassthroughV1<'a> {
    query: &'a dyn RecordingConsumerQueryV1,
    state: RemoteTimeControlAdapterStateV1,
    _sealed: private::LocalTimeControlPassthroughSealV1,
}

impl<'a> LocalTimeControlPassthroughV1<'a> {
    pub(crate) fn new_v1(
        query: &'a dyn RecordingConsumerQueryV1,
        canonical_timeline: TimelineName,
        initial_play_state: RemotePlayStateV1,
    ) -> Self {
        Self {
            query,
            state: RemoteTimeControlAdapterStateV1::new_v1(canonical_timeline, initial_play_state),
            _sealed: private::LocalTimeControlPassthroughSealV1,
        }
    }

    pub(crate) fn committed_time_v1(&self) -> Option<CommittedPresentationTimeV1> {
        let lease = self.query.try_lease_v1().ok()?;
        lease.committed_time_v1()
    }
}

impl private::SealedRemoteTimeControlAdapterV1 for LocalTimeControlPassthroughV1<'_> {}

impl RemoteTimeControlAdapterV1 for LocalTimeControlPassthroughV1<'_> {
    fn merge_commands_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        commands: &[TimeControlCommand],
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1> {
        self.state.merge_commands_v1(committed, commands)
    }

    fn preview_update_v1(
        &mut self,
        committed: Option<&CommittedPresentationTimeV1>,
        clock: RemoteCandidateClockInputV1,
        extent: &RemoteCanonicalIndexedExtentV1,
    ) -> Result<&RemoteNavigationUiStateV1, RemoteTimeControlErrorV1> {
        self.state.preview_update_v1(committed, clock, extent)
    }

    fn commit_presentation_v1(&mut self, committed: Option<CommittedPresentationTimeV1>) {
        self.state.commit_presentation_v1(committed);
    }
}

impl RemoteTimePanelProjectionV1 {
    pub(crate) fn from_coverage_v1(
        timeline: &TimelineName,
        canonical_timeline: &TimelineName,
        coverage: &RemoteLoadedCoverageV1,
        ui: &RemoteNavigationUiStateV1,
        loading_window: Option<AbsoluteTimeRange>,
    ) -> Result<Self, RemoteTimeControlErrorV1> {
        let indexed_extent = canonical_time_control_extent_v1(
            timeline,
            canonical_timeline,
            Some(coverage.canonical_extent_v1()),
        )?;

        Ok(Self {
            indexed_extent,
            loaded_ranges: coverage.loaded_ranges_v1().to_vec(),
            loading_window,
            is_buffering: ui.is_buffering,
            candidate_clock_held: ui.candidate_clock_held,
            no_temporal_data: false,
            complete_indexed_coverage: coverage.complete_indexed_coverage_v1(),
        })
    }

    pub(crate) fn no_temporal_data_v1(
        ui: &RemoteNavigationUiStateV1,
        loading_window: Option<AbsoluteTimeRange>,
    ) -> Self {
        Self {
            indexed_extent: None,
            loaded_ranges: Vec::new(),
            loading_window,
            is_buffering: ui.is_buffering,
            candidate_clock_held: ui.candidate_clock_held,
            no_temporal_data: true,
            complete_indexed_coverage: CompleteIndexedCoverageV1::Incomplete,
        }
    }
}

fn clamp_to_range_v1(time: TimeInt, range: AbsoluteTimeRange) -> TimeInt {
    time.max(range.min()).min(range.max())
}

fn add_nanos_v1(time: TimeInt, nanos: i64) -> TimeInt {
    TimeInt::saturated_temporal_i64(time.as_i64().saturating_add(nanos))
}

fn playback_advance_v1(
    time: TimeInt,
    stable_dt: Duration,
    speed: f32,
    range: AbsoluteTimeRange,
    loop_mode: LoopMode,
) -> (TimeInt, RemoteNavigationTriggerV1) {
    let nanos = stable_dt.as_nanos();
    let scaled = (nanos as f64 * speed as f64).round();
    let scaled = scaled.clamp(i64::MIN as f64, i64::MAX as f64) as i64;
    let candidate = i128::from(time.as_i64()) + i128::from(scaled);

    let min = i128::from(range.min().as_i64());
    let max = i128::from(range.max().as_i64());
    let (target, trigger) = match loop_mode {
        LoopMode::All => {
            let length = max
                .checked_sub(min)
                .and_then(|length| length.checked_add(1))
                .expect("canonical indexed extent must fit in i128");
            let offset = candidate
                .checked_sub(min)
                .expect("candidate must fit in i128");
            let wrapped_offset = offset.rem_euclid(length);
            let target = min
                .checked_add(wrapped_offset)
                .expect("wrapped candidate must fit in i128");
            let trigger = if target == candidate {
                RemoteNavigationTriggerV1::PlaybackAdvance
            } else {
                RemoteNavigationTriggerV1::LoopJump
            };
            (target, trigger)
        }
        LoopMode::Off | LoopMode::Selection => (
            candidate.clamp(min, max),
            RemoteNavigationTriggerV1::PlaybackAdvance,
        ),
    };

    let target = target.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    (TimeInt::new_temporal(target), trigger)
}

impl ConsumerStorageFreeV1 for f32 {}
impl ConsumerStorageFreeV1 for Duration {}
impl ConsumerStorageFreeV1 for LoopMode {}
impl ConsumerStorageFreeV1 for RemotePlayStateV1 {}
impl ConsumerStorageFreeV1 for dyn RecordingConsumerQueryV1 + '_ {}

impl ConsumerStorageFreeV1 for RemoteNavigationTriggerV1 {}
impl ConsumerStorageFreeV1 for RemoteNavigationDemandKeyV1 where TimeInt: ConsumerStorageFreeV1 {}

impl ConsumerStorageFreeV1 for PendingNavigationIntentV1
where
    TimeInt: ConsumerStorageFreeV1,
    RemotePlayStateV1: ConsumerStorageFreeV1,
    RemoteNavigationTriggerV1: ConsumerStorageFreeV1,
    RemoteNavigationDemandKeyV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteCandidateClockInputV1 where Duration: ConsumerStorageFreeV1 {}

impl ConsumerStorageFreeV1 for RemoteTimeControlErrorV1
where
    TimelineName: ConsumerStorageFreeV1,
    LoopMode: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteNavigationUiStateV1
where
    Option<PendingNavigationIntentV1>: ConsumerStorageFreeV1,
    Option<CommittedPresentationTimeV1>: ConsumerStorageFreeV1,
    Option<TimeInt>: ConsumerStorageFreeV1,
    bool: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteTimeControlAdapterStateV1
where
    TimelineName: ConsumerStorageFreeV1,
    Option<PendingNavigationIntentV1>: ConsumerStorageFreeV1,
    Option<CommittedPresentationTimeV1>: ConsumerStorageFreeV1,
    Option<RemoteNavigationDemandKeyV1>: ConsumerStorageFreeV1,
    RemotePlayStateV1: ConsumerStorageFreeV1,
    f32: ConsumerStorageFreeV1,
    LoopMode: ConsumerStorageFreeV1,
    bool: ConsumerStorageFreeV1,
    RemoteNavigationUiStateV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for RemoteTimePanelProjectionV1
where
    Option<RemoteCanonicalIndexedExtentV1>: ConsumerStorageFreeV1,
    Vec<AbsoluteTimeRange>: ConsumerStorageFreeV1,
    Option<AbsoluteTimeRange>: ConsumerStorageFreeV1,
    bool: ConsumerStorageFreeV1,
    CompleteIndexedCoverageV1: ConsumerStorageFreeV1,
{
}

impl ConsumerStorageFreeV1 for private::TimeControlAdapterSealV1 {}
impl ConsumerStorageFreeV1 for private::RemoteTimeControlConsumerSealV1 {}
impl ConsumerStorageFreeV1 for private::LocalTimeControlPassthroughSealV1 {}

impl<'a> ConsumerStorageFreeV1 for RemoteTimeControlConsumerV1<'a>
where
    &'a dyn RecordingConsumerQueryV1: ConsumerStorageFreeV1,
    RemoteTimeControlAdapterStateV1: ConsumerStorageFreeV1,
    private::RemoteTimeControlConsumerSealV1: ConsumerStorageFreeV1,
{
}

impl<'a> ConsumerStorageFreeV1 for LocalTimeControlPassthroughV1<'a>
where
    &'a dyn RecordingConsumerQueryV1: ConsumerStorageFreeV1,
    RemoteTimeControlAdapterStateV1: ConsumerStorageFreeV1,
    private::LocalTimeControlPassthroughSealV1: ConsumerStorageFreeV1,
{
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use re_entity_db::{EntityDb, StoreBundle};
    use re_query::StorageEngine;
    use re_viewer_context::StoreHub;

    use super::*;
    use crate::web_remote_mcap_activation::RemoteRecordingUseStateV1;
    use crate::web_remote_mcap_consumer::{
        LocalRecordingPassthroughV1, RecordingConsumerContextV1,
    };
    use crate::web_remote_mcap_query::{
        PrivilegedViewerFrameContextV1, RemoteCanonicalIndexedExtentV1, RemotePresentationFacadeV1,
    };

    fn canonical_timeline() -> TimelineName {
        TimelineName::log_time()
    }

    fn non_canonical_timeline() -> TimelineName {
        TimelineName::from_static_str("message_publish_time")
    }

    fn extent() -> AbsoluteTimeRange {
        AbsoluteTimeRange::new(0i64, 100i64)
    }

    fn known_extent() -> RemoteCanonicalIndexedExtentV1 {
        RemoteCanonicalIndexedExtentV1::Known(extent())
    }

    fn committed_time(cursor: i64) -> CommittedPresentationTimeV1 {
        CommittedPresentationTimeV1 {
            timeline: canonical_timeline(),
            cursor: TimeInt::new_temporal(cursor),
        }
    }

    fn store_id() -> re_log_types::StoreId {
        re_log_types::StoreId::recording("time-control-test-app", "time-control-test-recording")
    }

    fn open_facade() -> RemotePresentationFacadeV1 {
        let facade = RemotePresentationFacadeV1::new_for_test_v1(
            43,
            store_id(),
            RemoteRecordingUseStateV1::Foreground,
            RemoteLoadedCoverageV1::new_v1(known_extent(), false),
            2,
            Rc::new(|| {}),
        );

        let frame = PrivilegedViewerFrameContextV1::new_for_test_v1();
        let privileged = frame.remote_storage_capability_v1(&facade);
        privileged.set_opening_static_satisfied_v1(true);
        privileged
            .commit_initial_presentation_v1(Some(committed_time(3)))
            .expect("open facade");

        facade
    }

    fn assert_adapter_trait_surface<T: RemoteTimeControlAdapterV1 + 'static>() {
        let type_name = std::any::type_name::<T>();
        assert!(!type_name.contains("EntityDb"));
        assert!(!type_name.contains("StorageEngine"));
        assert!(!type_name.contains("StoreHub"));
        assert!(!type_name.contains("StoreBundle"));
    }

    fn assert_storage_free_v1<T: ConsumerStorageFreeV1>() {}

    macro_rules! assert_not_storage_free_v1 {
        ($type:ty) => {
            const _: fn() = || {
                trait AmbiguousIfImpl<A> {
                    fn some_item() {}
                }

                impl<T: ?Sized> AmbiguousIfImpl<()> for T {}

                struct Invalid;

                impl<T: ?Sized + ConsumerStorageFreeV1> AmbiguousIfImpl<Invalid> for T {}

                let _ = <$type as AmbiguousIfImpl<_>>::some_item;
            };
        };
    }

    #[test]
    fn non_canonical_following_and_invalid_speed_never_reach_planner() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );

        let result = adapter.merge_commands_v1(
            Some(&committed_time(3)),
            &[TimeControlCommand::SetActiveTimeline(
                non_canonical_timeline(),
            )],
        );
        assert_eq!(
            result,
            Err(
                RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                    requested: non_canonical_timeline(),
                }
            )
        );
        assert!(adapter.ui_state.requested.is_none());

        let result = adapter.merge_commands_v1(
            Some(&committed_time(3)),
            &[TimeControlCommand::SetPlayState(PlayState::Following)],
        );
        assert_eq!(
            result,
            Err(RemoteTimeControlErrorV1::UnsupportedRemotePlayState)
        );
        assert_eq!(adapter.play_state_v1(), RemotePlayStateV1::Paused);
        assert_eq!(adapter.ui_state.target_marker, None);

        let result = adapter.merge_commands_v1(
            Some(&committed_time(3)),
            &[TimeControlCommand::SetSpeed(0.0)],
        );
        assert_eq!(
            result,
            Err(RemoteTimeControlErrorV1::InvalidRemotePlaybackSpeed)
        );
        assert_eq!(adapter.speed_v1(), 1.0);
        assert!(adapter.ui_state.requested.is_none());
    }

    #[test]
    fn merge_command_batch_is_atomic_on_late_invalid_command() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );
        adapter
            .merge_commands_v1(
                Some(&committed_time(3)),
                &[TimeControlCommand::SetSpeed(2.0)],
            )
            .expect("valid speed should be accepted");

        let result = adapter.merge_commands_v1(
            Some(&committed_time(3)),
            &[
                TimeControlCommand::SetTime(8i64.into()),
                TimeControlCommand::SetPlayState(PlayState::Following),
            ],
        );
        assert_eq!(
            result,
            Err(RemoteTimeControlErrorV1::UnsupportedRemotePlayState)
        );
        assert_eq!(adapter.speed_v1(), 2.0);
        assert_eq!(adapter.play_state_v1(), RemotePlayStateV1::Paused);
        assert_eq!(adapter.ui_state.committed, Some(committed_time(3)));
        assert!(adapter.ui_state.requested.is_none());
        assert_eq!(adapter.ui_state.target_marker, None);

        let result = adapter.merge_commands_v1(
            Some(&committed_time(3)),
            &[
                TimeControlCommand::SetTime(9i64.into()),
                TimeControlCommand::SetActiveTimeline(non_canonical_timeline()),
            ],
        );
        assert_eq!(
            result,
            Err(
                RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                    requested: non_canonical_timeline(),
                }
            )
        );
        assert_eq!(adapter.speed_v1(), 2.0);
        assert_eq!(adapter.ui_state.committed, Some(committed_time(3)));
        assert!(adapter.ui_state.requested.is_none());
        assert_eq!(adapter.ui_state.target_marker, None);
    }

    #[test]
    fn preview_created_and_replaced_demands_latch_in_flight() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Playing,
        );

        let ui = adapter
            .preview_update_v1(
                Some(&committed_time(5)),
                RemoteCandidateClockInputV1 {
                    stable_dt: Duration::from_nanos(1),
                    held: false,
                },
                &known_extent(),
            )
            .expect("preview should create a requested demand");
        assert!(ui.requested_generation_in_flight);
        assert_eq!(ui.target_marker, Some(TimeInt::new_temporal(6)));

        adapter.commit_presentation_v1(Some(committed_time(6)));
        assert!(!adapter.ui_state.requested_generation_in_flight);

        let ui = adapter
            .preview_update_v1(
                Some(&committed_time(6)),
                RemoteCandidateClockInputV1 {
                    stable_dt: Duration::from_nanos(4),
                    held: false,
                },
                &known_extent(),
            )
            .expect("preview should replace the requested demand");
        assert!(ui.requested_generation_in_flight);
        assert_eq!(ui.target_marker, Some(TimeInt::new_temporal(10)));
    }

    #[test]
    fn all_loop_mode_wraps_with_loop_jump_and_selection_is_rejected() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Playing,
        );
        adapter
            .merge_commands_v1(
                Some(&committed_time(95)),
                &[TimeControlCommand::SetLoopMode(LoopMode::All)],
            )
            .expect("whole recording loop should be supported");

        let ui = adapter
            .preview_update_v1(
                Some(&committed_time(95)),
                RemoteCandidateClockInputV1 {
                    stable_dt: Duration::from_nanos(10),
                    held: false,
                },
                &known_extent(),
            )
            .expect("loop preview should succeed");
        assert_eq!(ui.target_marker, Some(TimeInt::new_temporal(4)));
        assert_eq!(
            ui.requested.as_ref().map(|intent| intent.trigger),
            Some(RemoteNavigationTriggerV1::LoopJump)
        );
        assert!(ui.requested_generation_in_flight);

        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );
        let result = adapter.merge_commands_v1(
            Some(&committed_time(3)),
            &[TimeControlCommand::SetLoopMode(LoopMode::Selection)],
        );
        assert_eq!(
            result,
            Err(RemoteTimeControlErrorV1::UnsupportedRemoteLoopMode {
                requested: LoopMode::Selection,
            })
        );
        assert_eq!(adapter.loop_mode, LoopMode::Off);
        assert_eq!(adapter.ui_state.committed, None);
        assert!(adapter.ui_state.requested.is_none());
    }

    #[test]
    fn buffer_command_does_not_set_remote_buffering() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );
        adapter
            .merge_commands_v1(Some(&committed_time(3)), &[TimeControlCommand::Buffer])
            .expect("generic buffer command should be harmless");

        assert!(!adapter.is_buffering);
        assert!(!adapter.ui_state.is_buffering);

        adapter.set_buffering_v1(true);
        assert!(adapter.is_buffering);
        assert!(adapter.ui_state.is_buffering);
    }

    #[test]
    fn non_canonical_committed_time_is_rejected_without_state_change() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );
        let non_canonical_committed = CommittedPresentationTimeV1 {
            timeline: non_canonical_timeline(),
            cursor: TimeInt::new_temporal(3),
        };

        let result = adapter.merge_commands_v1(Some(&non_canonical_committed), &[]);
        assert_eq!(
            result,
            Err(
                RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                    requested: non_canonical_timeline(),
                }
            )
        );
        assert_eq!(adapter.ui_state.committed, None);
        assert!(adapter.ui_state.requested.is_none());
        assert_eq!(adapter.ui_state.target_marker, None);

        let result = adapter.preview_update_v1(
            Some(&non_canonical_committed),
            RemoteCandidateClockInputV1 {
                stable_dt: Duration::from_nanos(1),
                held: false,
            },
            &known_extent(),
        );
        assert_eq!(
            result,
            Err(
                RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                    requested: non_canonical_timeline(),
                }
            )
        );
        assert_eq!(adapter.ui_state.committed, None);
        assert!(adapter.ui_state.requested.is_none());
        assert_eq!(adapter.ui_state.target_marker, None);
    }

    #[test]
    fn canonical_target_commands_update_requested_marker_but_not_committed() {
        let committed = committed_time(5);
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );

        let ui = adapter
            .merge_commands_v1(
                Some(&committed),
                &[TimeControlCommand::SetTime(8i64.into())],
            )
            .expect("merge command");

        assert_eq!(ui.committed, Some(committed));
        assert_eq!(ui.target_marker, Some(TimeInt::new_temporal(8)));
        assert_eq!(
            ui.requested.as_ref().map(|intent| intent.canonical_target),
            Some(TimeInt::new_temporal(8))
        );

        let ui = adapter
            .merge_commands_v1(
                Some(&committed_time(5)),
                &[TimeControlCommand::SetTimeClamped(11i64.into())],
            )
            .expect("merge clamped command");
        assert_eq!(ui.committed, Some(committed_time(5)));
        assert_eq!(ui.target_marker, Some(TimeInt::new_temporal(11)));
    }

    #[test]
    fn candidate_clock_hold_preserves_target_and_ignores_stable_dt() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Playing,
        );
        adapter
            .merge_commands_v1(
                Some(&committed_time(5)),
                &[TimeControlCommand::SetTime(5i64.into())],
            )
            .expect("seek to five");

        let ui = adapter
            .preview_update_v1(
                Some(&committed_time(5)),
                RemoteCandidateClockInputV1 {
                    stable_dt: Duration::from_nanos(100),
                    held: true,
                },
                &known_extent(),
            )
            .expect("held preview");

        assert!(ui.candidate_clock_held);
        assert_eq!(ui.target_marker, Some(TimeInt::new_temporal(5)));

        let ui = adapter
            .preview_update_v1(
                Some(&committed_time(5)),
                RemoteCandidateClockInputV1 {
                    stable_dt: Duration::from_nanos(10),
                    held: false,
                },
                &known_extent(),
            )
            .expect("playing preview");
        assert!(!ui.candidate_clock_held);
        assert_eq!(ui.target_marker, Some(TimeInt::new_temporal(15)));
    }

    #[test]
    fn commit_presentation_updates_committed_and_keeps_requested_semantics() {
        let mut adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );
        adapter
            .merge_commands_v1(
                Some(&committed_time(5)),
                &[TimeControlCommand::SetTime(8i64.into())],
            )
            .expect("merge seek");

        adapter.commit_presentation_v1(Some(committed_time(8)));
        assert_eq!(adapter.ui_state.committed, Some(committed_time(8)));
        assert!(adapter.ui_state.requested.is_some());
        assert_eq!(
            adapter.ui_state.target_marker,
            Some(TimeInt::new_temporal(8))
        );
        assert!(!adapter.ui_state.is_loading);
    }

    #[test]
    fn no_indexed_messages_and_missing_extent_are_distinct() {
        assert_eq!(
            canonical_time_control_extent_v1(&canonical_timeline(), &canonical_timeline(), None,),
            Ok(None)
        );
        assert_eq!(
            canonical_time_control_extent_v1(
                &canonical_timeline(),
                &canonical_timeline(),
                Some(&RemoteCanonicalIndexedExtentV1::NoIndexedMessages),
            ),
            Ok(Some(RemoteCanonicalIndexedExtentV1::NoIndexedMessages))
        );
        assert_eq!(
            canonical_time_control_extent_v1(
                &non_canonical_timeline(),
                &canonical_timeline(),
                Some(&known_extent()),
            ),
            Err(
                RemoteTimeControlErrorV1::UnsupportedRemoteNavigationTimeline {
                    requested: non_canonical_timeline(),
                }
            )
        );
    }

    #[test]
    fn partial_loaded_range_is_not_reported_as_complete() {
        let adapter = RemoteTimeControlAdapterStateV1::new_v1(
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );
        let mut coverage = RemoteLoadedCoverageV1::new_v1(known_extent(), true);
        coverage.replace_loaded_ranges_v1([AbsoluteTimeRange::new(0i64, 50i64)]);

        let projection = RemoteTimePanelProjectionV1::from_coverage_v1(
            &canonical_timeline(),
            &canonical_timeline(),
            &coverage,
            &adapter.ui_state,
            Some(AbsoluteTimeRange::new(40i64, 60i64)),
        )
        .expect("canonical projection");

        assert_eq!(
            projection.complete_indexed_coverage,
            CompleteIndexedCoverageV1::Incomplete
        );
        assert_eq!(
            projection.loaded_ranges,
            vec![AbsoluteTimeRange::new(0i64, 50i64)]
        );
    }

    #[test]
    fn remote_and_local_passthrough_produce_identical_navigation_state() {
        let facade = open_facade();
        let remote_query = RecordingConsumerContextV1::from_remote_facade_v1(&facade);
        let local_query = LocalRecordingPassthroughV1::from_facade_v1(&facade);

        let mut remote = RemoteTimeControlConsumerV1::new_v1(
            &remote_query,
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );
        let mut local = LocalTimeControlPassthroughV1::new_v1(
            &local_query,
            canonical_timeline(),
            RemotePlayStateV1::Paused,
        );

        let commands = [
            TimeControlCommand::SetTime(7i64.into()),
            TimeControlCommand::SetPlayState(PlayState::Playing),
        ];
        let committed = Some(committed_time(3));
        let remote_ui = remote
            .merge_commands_v1(committed.as_ref(), &commands)
            .expect("remote merge")
            .clone();
        let local_ui = local
            .merge_commands_v1(committed.as_ref(), &commands)
            .expect("local merge")
            .clone();
        assert_eq!(remote_ui, local_ui);

        let clock = RemoteCandidateClockInputV1 {
            stable_dt: Duration::from_nanos(10),
            held: false,
        };
        let remote_ui = remote
            .preview_update_v1(committed.as_ref(), clock, &known_extent())
            .expect("remote preview")
            .clone();
        let local_ui = local
            .preview_update_v1(committed.as_ref(), clock, &known_extent())
            .expect("local preview")
            .clone();
        assert_eq!(remote_ui, local_ui);

        assert_eq!(remote.committed_time_v1(), Some(committed_time(3)));
        assert_eq!(local.committed_time_v1(), Some(committed_time(3)));
    }

    #[test]
    fn remote_time_control_surface_is_storage_free() {
        assert_adapter_trait_surface::<RemoteTimeControlAdapterStateV1>();
        assert_adapter_trait_surface::<RemoteTimeControlConsumerV1<'static>>();
        assert_adapter_trait_surface::<LocalTimeControlPassthroughV1<'static>>();

        assert_storage_free_v1::<RemoteTimeControlAdapterStateV1>();
        assert_storage_free_v1::<RemoteNavigationUiStateV1>();
        assert_storage_free_v1::<RemoteTimePanelProjectionV1>();

        assert_not_storage_free_v1!(EntityDb);
        assert_not_storage_free_v1!(StorageEngine);
        assert_not_storage_free_v1!(StoreHub);
        assert_not_storage_free_v1!(StoreBundle);
    }
}
