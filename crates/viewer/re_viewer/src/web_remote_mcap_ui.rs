//! Production-disarmed remote-MCAP recording panel, catalog card, and open-options UI boundary.
//!
//! This module consumes the selection effects produced by MCAP-105 and turns them into a
//! storage-free surface projection. It deliberately does not construct or import a `ViewerContext`,
//! `AppContext`, `StoreHub`, `StoreBundle`, `EntityDb`, `egui`, blueprint store, command sender,
//! network transport, or real Store/query implementation.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt;

use re_chunk::TimeInt;
use re_log_types::{Duration, TimeType};

use crate::web_remote_mcap_activation::{
    RemoteCanonicalNavigationV1, RemotePlayStateV1, RemoteRecordingUseStateV1,
};
use crate::web_remote_mcap_selection::{
    RemoteMcapRecordingOpenBehaviorV1, RemoteProgrammaticSelectionIntentV1,
    RemoteSelectionEffectV1, RemoteSelectionErrorV1, RemoteSelectionOperationIdV1,
    UserNavigationRevisionV1,
};

/// Frozen UI-visible time type.
///
/// The remote open path is intentionally restricted to canonical nanosecond time types. Sequence
/// and any future time type remain outside this UI projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteOpenTimeTypeV1 {
    TimestampNs,
    DurationNs,
}

impl RemoteOpenTimeTypeV1 {
    pub(crate) const fn from_time_type_v1(
        time_type: TimeType,
    ) -> Result<Self, RemoteOpenOptionsErrorV1> {
        match time_type {
            TimeType::TimestampNs => Ok(Self::TimestampNs),
            TimeType::DurationNs => Ok(Self::DurationNs),
            TimeType::Sequence => Err(RemoteOpenOptionsErrorV1::UnsupportedTimeType),
        }
    }

    pub(crate) const fn label_v1(self) -> &'static str {
        match self {
            Self::TimestampNs => "timestamp_ns",
            Self::DurationNs => "duration_ns",
        }
    }
}

/// Frozen UI-visible representation-consistency policy.
///
/// `RequireStrongValidator` is the default and cannot display a deployment-assumed warning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RemoteOpenConsistencyPolicyV1 {
    #[default]
    RequireStrongValidator,
    AllowDeploymentAssumed,
}

/// Observed entity-tag shape, without retaining the opaque or secret tag bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteOpenObservedEntityTagV1 {
    Strong,
    Weak,
    Absent,
}

/// Actual consistency retained alongside the requested policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteOpenActualConsistencyV1 {
    StrongValidator,
    DeploymentAssumed,
}

/// Redacted topic-filter summary.
///
/// The raw Topic names are intentionally not retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteOpenTopicFilterSummaryV1 {
    AllTopics,
    Redacted { filter_count: usize },
}

impl RemoteOpenTopicFilterSummaryV1 {
    pub(crate) const fn label_v1(self) -> &'static str {
        match self {
            Self::AllTopics => "all topics",
            Self::Redacted { .. } => "topic filter configured",
        }
    }
}

/// Redacted decoder-selector summary.
///
/// The raw selector values are intentionally not retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteOpenDecoderSelectorSummaryV1 {
    DefaultDecoders,
    Redacted { selector_count: usize },
}

impl RemoteOpenDecoderSelectorSummaryV1 {
    pub(crate) const fn label_v1(self) -> &'static str {
        match self {
            Self::DefaultDecoders => "default decoders",
            Self::Redacted { .. } => "decoder selector configured",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteOpenOptionsErrorV1 {
    UnsupportedTimeType,
    StrongValidatorRequired,
}

/// Resolves the requested policy and observed entity-tag shape into actual consistency.
///
/// This is the UI-side projection of the MCAP-014 admission truth table. A weak or absent tag can
/// only become `DeploymentAssumed` when the caller explicitly allowed it.
pub(crate) const fn resolve_open_consistency_v1(
    policy: RemoteOpenConsistencyPolicyV1,
    observed: RemoteOpenObservedEntityTagV1,
) -> Result<RemoteOpenActualConsistencyV1, RemoteOpenOptionsErrorV1> {
    match (policy, observed) {
        (
            RemoteOpenConsistencyPolicyV1::RequireStrongValidator
            | RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed,
            RemoteOpenObservedEntityTagV1::Strong,
        ) => Ok(RemoteOpenActualConsistencyV1::StrongValidator),
        (RemoteOpenConsistencyPolicyV1::RequireStrongValidator, _) => {
            Err(RemoteOpenOptionsErrorV1::StrongValidatorRequired)
        }
        (RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed, _) => {
            Ok(RemoteOpenActualConsistencyV1::DeploymentAssumed)
        }
    }
}

/// Redacted frozen open-options UI projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteOpenOptionsUiProjectionV1 {
    pub(crate) time_type: RemoteOpenTimeTypeV1,
    pub(crate) topic_filter: RemoteOpenTopicFilterSummaryV1,
    pub(crate) decoder_selector: RemoteOpenDecoderSelectorSummaryV1,
    pub(crate) requested_consistency: RemoteOpenConsistencyPolicyV1,
    pub(crate) actual_consistency: Option<RemoteOpenActualConsistencyV1>,
}

impl RemoteOpenOptionsUiProjectionV1 {
    pub(crate) fn new_v1(
        time_type: TimeType,
        topic_filter: RemoteOpenTopicFilterSummaryV1,
        decoder_selector: RemoteOpenDecoderSelectorSummaryV1,
        requested_consistency: RemoteOpenConsistencyPolicyV1,
        observed_entity_tag: RemoteOpenObservedEntityTagV1,
    ) -> Result<Self, RemoteOpenOptionsErrorV1> {
        let time_type = RemoteOpenTimeTypeV1::from_time_type_v1(time_type)?;
        let actual_consistency =
            resolve_open_consistency_v1(requested_consistency, observed_entity_tag)?;
        Ok(Self {
            time_type,
            topic_filter,
            decoder_selector,
            requested_consistency,
            actual_consistency: Some(actual_consistency),
        })
    }

    pub(crate) const fn time_type_v1(self) -> RemoteOpenTimeTypeV1 {
        self.time_type
    }

    pub(crate) const fn topic_filter_v1(self) -> RemoteOpenTopicFilterSummaryV1 {
        self.topic_filter
    }

    pub(crate) const fn decoder_selector_v1(self) -> RemoteOpenDecoderSelectorSummaryV1 {
        self.decoder_selector
    }

    pub(crate) const fn requested_consistency_v1(self) -> RemoteOpenConsistencyPolicyV1 {
        self.requested_consistency
    }

    pub(crate) const fn actual_consistency_v1(self) -> Option<RemoteOpenActualConsistencyV1> {
        self.actual_consistency
    }

    pub(crate) fn has_deployment_assumed_warning_v1(self) -> bool {
        self.requested_consistency == RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed
            && self.actual_consistency == Some(RemoteOpenActualConsistencyV1::DeploymentAssumed)
    }

    pub(crate) const fn label_v1(self) -> &'static str {
        let _ = self;
        "remote MCAP open options"
    }

    pub(crate) fn status_label_v1(self) -> &'static str {
        if self.has_deployment_assumed_warning_v1() {
            "deployment-assumed consistency"
        } else {
            "strong validator consistency"
        }
    }
}

/// Confirmed remote-MCAP route classes that enter this UI surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapUiRouteClassV1 {
    CompatibilityExplicitMcap,
    StrictExplicitMcap,
    StrictExtensionlessSniff,
}

impl RemoteMcapUiRouteClassV1 {
    pub(crate) const fn is_strict_v1(self) -> bool {
        matches!(
            self,
            Self::StrictExplicitMcap | Self::StrictExtensionlessSniff
        )
    }

    pub(crate) const fn operation_local_ready_v1(self) -> bool {
        self.is_strict_v1()
    }
}

/// All remote-MCAP route inputs recognized by this boundary.
///
/// Compatibility extensionless and strict non-MCAP sniff inputs are represented explicitly so
/// they can be structurally excluded from the confirmed remote surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapUiRouteInputV1 {
    CompatibilityExplicitMcap,
    CompatibilityExtensionless,
    StrictExplicitMcap,
    StrictExtensionlessSniff,
    StrictNonMcapSniff,
}

/// Route projection after the compatibility/strict split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapUiRouteDispositionV1 {
    ConfirmedRemoteMcap(RemoteMcapUiRouteClassV1),
    CompatibilityExtensionlessDispatcher,
    StrictNonMcapUnsupported,
}

pub(crate) const fn project_remote_mcap_ui_route_v1(
    input: RemoteMcapUiRouteInputV1,
) -> RemoteMcapUiRouteDispositionV1 {
    match input {
        RemoteMcapUiRouteInputV1::CompatibilityExplicitMcap => {
            RemoteMcapUiRouteDispositionV1::ConfirmedRemoteMcap(
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
            )
        }
        RemoteMcapUiRouteInputV1::StrictExplicitMcap => {
            RemoteMcapUiRouteDispositionV1::ConfirmedRemoteMcap(
                RemoteMcapUiRouteClassV1::StrictExplicitMcap,
            )
        }
        RemoteMcapUiRouteInputV1::StrictExtensionlessSniff => {
            RemoteMcapUiRouteDispositionV1::ConfirmedRemoteMcap(
                RemoteMcapUiRouteClassV1::StrictExtensionlessSniff,
            )
        }
        RemoteMcapUiRouteInputV1::CompatibilityExtensionless => {
            RemoteMcapUiRouteDispositionV1::CompatibilityExtensionlessDispatcher
        }
        RemoteMcapUiRouteInputV1::StrictNonMcapSniff => {
            RemoteMcapUiRouteDispositionV1::StrictNonMcapUnsupported
        }
    }
}

/// Surface kind produced by a confirmed route and behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRecordingUiSurfaceKindV1 {
    SelectedRecordingPanel,
    RecordingPanel,
    MetadataCatalogCard,
}

impl RemoteRecordingUiSurfaceKindV1 {
    pub(crate) const fn is_selected_v1(self) -> bool {
        matches!(self, Self::SelectedRecordingPanel)
    }

    pub(crate) const fn is_panel_v1(self) -> bool {
        matches!(self, Self::SelectedRecordingPanel | Self::RecordingPanel)
    }

    pub(crate) const fn is_catalog_card_v1(self) -> bool {
        matches!(self, Self::MetadataCatalogCard)
    }

    pub(crate) const fn is_catalog_only_v1(self) -> bool {
        self.is_catalog_card_v1()
    }
}

/// Storage-free route/behavior projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRecordingUiProjectionV1 {
    pub(crate) route: RemoteMcapUiRouteClassV1,
    pub(crate) operation_id: RemoteSelectionOperationIdV1,
    pub(crate) surface_kind: RemoteRecordingUiSurfaceKindV1,
    pub(crate) operation_local_ready: bool,
}

pub(crate) const fn project_remote_recording_ui_v1(
    route: RemoteMcapUiRouteClassV1,
    behavior: RemoteMcapRecordingOpenBehaviorV1,
    operation_id: RemoteSelectionOperationIdV1,
) -> RemoteRecordingUiProjectionV1 {
    let surface_kind = match behavior {
        RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect => {
            RemoteRecordingUiSurfaceKindV1::SelectedRecordingPanel
        }
        RemoteMcapRecordingOpenBehaviorV1::Open => RemoteRecordingUiSurfaceKindV1::RecordingPanel,
        RemoteMcapRecordingOpenBehaviorV1::Background => {
            RemoteRecordingUiSurfaceKindV1::MetadataCatalogCard
        }
    };

    RemoteRecordingUiProjectionV1 {
        route,
        operation_id,
        surface_kind,
        operation_local_ready: route.operation_local_ready_v1(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteMcapUiErrorV1 {
    StaleProgrammaticSelectionIntent,
    BehaviorEffectWouldDowngradeForeground,
    BehaviorEffectWouldUndoSelection,
    NonForegroundTemporalWorkRejected,
    NonForegroundQueryRejected,
    NonForegroundStoreMutationRejected,
}

/// Resolves the existing selection authority effect without constructing a selection-state owner.
pub(crate) fn remote_selection_effect_v1(
    operation_id: RemoteSelectionOperationIdV1,
    behavior: RemoteMcapRecordingOpenBehaviorV1,
    frozen_user_navigation_revision: UserNavigationRevisionV1,
    current_user_navigation_revision: UserNavigationRevisionV1,
) -> Result<RemoteSelectionEffectV1, RemoteMcapUiErrorV1> {
    let intent = RemoteProgrammaticSelectionIntentV1::new_v1(
        operation_id,
        behavior,
        frozen_user_navigation_revision,
        None,
    );
    intent
        .effect_v1(current_user_navigation_revision)
        .map_err(|error| match error {
            RemoteSelectionErrorV1::StaleProgrammaticSelectionIntent => {
                RemoteMcapUiErrorV1::StaleProgrammaticSelectionIntent
            }
            _ => unreachable!("non-selection effects cannot fail for frozen programmatic intents"),
        })
}

fn entry_kind_from_effect_v1(effect: RemoteSelectionEffectV1) -> RemoteRecordingUiSurfaceKindV1 {
    match effect {
        RemoteSelectionEffectV1::SelectRecording { .. } => {
            RemoteRecordingUiSurfaceKindV1::SelectedRecordingPanel
        }
        RemoteSelectionEffectV1::InstallPanelEntry { .. } => {
            RemoteRecordingUiSurfaceKindV1::RecordingPanel
        }
        RemoteSelectionEffectV1::InstallCatalogEntry { .. } => {
            RemoteRecordingUiSurfaceKindV1::MetadataCatalogCard
        }
        RemoteSelectionEffectV1::NoRemoteSelectionEffect => {
            unreachable!("programmatic intents always produce a concrete remote selection effect")
        }
    }
}

/// Observable surface entry with operation-owned close identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRecordingUiEntryV1 {
    pub(crate) operation_id: RemoteSelectionOperationIdV1,
    pub(crate) route: RemoteMcapUiRouteClassV1,
    pub(crate) surface_kind: RemoteRecordingUiSurfaceKindV1,
    pub(crate) operation_local_ready: bool,
}

impl RemoteRecordingUiEntryV1 {
    pub(crate) const fn operation_id_v1(self) -> RemoteSelectionOperationIdV1 {
        self.operation_id
    }

    pub(crate) const fn surface_kind_v1(self) -> RemoteRecordingUiSurfaceKindV1 {
        self.surface_kind
    }

    pub(crate) const fn is_selected_v1(self) -> bool {
        self.surface_kind.is_selected_v1()
    }

    pub(crate) const fn is_catalog_only_v1(self) -> bool {
        self.surface_kind.is_catalog_only_v1()
    }

    pub(crate) const fn status_label_v1(self) -> &'static str {
        match self.surface_kind {
            RemoteRecordingUiSurfaceKindV1::SelectedRecordingPanel => "selected remote recording",
            RemoteRecordingUiSurfaceKindV1::RecordingPanel => "remote recording panel",
            RemoteRecordingUiSurfaceKindV1::MetadataCatalogCard => "remote metadata catalog",
        }
    }
}

/// Storage-free panel/catalog surface state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteRecordingUiSurfaceV1 {
    entries: BTreeMap<RemoteSelectionOperationIdV1, RemoteRecordingUiEntryV1>,
    selected_operation: Option<RemoteSelectionOperationIdV1>,
    temporal_work_units: u64,
    query_work_units: u64,
    store_mutation_units: u64,
}

impl Default for RemoteRecordingUiSurfaceV1 {
    fn default() -> Self {
        Self::new_v1()
    }
}

impl RemoteRecordingUiSurfaceV1 {
    pub(crate) const fn new_v1() -> Self {
        Self {
            entries: BTreeMap::new(),
            selected_operation: None,
            temporal_work_units: 0,
            query_work_units: 0,
            store_mutation_units: 0,
        }
    }

    pub(crate) fn install_v1(
        &mut self,
        operation_id: RemoteSelectionOperationIdV1,
        behavior: RemoteMcapRecordingOpenBehaviorV1,
        route: RemoteMcapUiRouteClassV1,
        frozen_user_navigation_revision: UserNavigationRevisionV1,
        current_user_navigation_revision: UserNavigationRevisionV1,
    ) -> Result<RemoteRecordingUiEntryV1, RemoteMcapUiErrorV1> {
        let effect = remote_selection_effect_v1(
            operation_id,
            behavior,
            frozen_user_navigation_revision,
            current_user_navigation_revision,
        )?;
        let surface_kind = entry_kind_from_effect_v1(effect);
        let new_entry = RemoteRecordingUiEntryV1 {
            operation_id,
            route,
            surface_kind,
            operation_local_ready: route.operation_local_ready_v1(),
        };

        let was_selected = self.selected_operation == Some(operation_id);
        if was_selected && !surface_kind.is_selected_v1() {
            return Err(if surface_kind.is_catalog_card_v1() {
                RemoteMcapUiErrorV1::BehaviorEffectWouldDowngradeForeground
            } else {
                RemoteMcapUiErrorV1::BehaviorEffectWouldUndoSelection
            });
        }

        if surface_kind.is_selected_v1() {
            if let Some(previous) = self.selected_operation
                && previous != operation_id
                && let Some(previous_entry) = self.entries.get_mut(&previous)
                && previous_entry.surface_kind.is_selected_v1()
            {
                previous_entry.surface_kind = RemoteRecordingUiSurfaceKindV1::RecordingPanel;
            }
            self.selected_operation = Some(operation_id);
        }
        self.entries.insert(operation_id, new_entry);
        Ok(new_entry)
    }

    pub(crate) fn close_v1(
        &mut self,
        operation_id: RemoteSelectionOperationIdV1,
    ) -> Option<RemoteRecordingUiEntryV1> {
        let entry = self.entries.remove(&operation_id)?;
        if self.selected_operation == Some(operation_id) {
            self.selected_operation = None;
        }
        Some(entry)
    }

    pub(crate) fn entry_v1(
        &self,
        operation_id: RemoteSelectionOperationIdV1,
    ) -> Option<RemoteRecordingUiEntryV1> {
        self.entries.get(&operation_id).copied()
    }

    pub(crate) fn has_ownership_v1(&self, operation_id: RemoteSelectionOperationIdV1) -> bool {
        self.entries.contains_key(&operation_id)
    }

    pub(crate) const fn selected_operation_v1(&self) -> Option<RemoteSelectionOperationIdV1> {
        self.selected_operation
    }

    pub(crate) fn is_foreground_v1(&self, operation_id: RemoteSelectionOperationIdV1) -> bool {
        self.selected_operation == Some(operation_id)
    }

    pub(crate) fn use_state_v1(&self) -> RemoteRecordingUseStateV1 {
        if self.selected_operation.is_some() {
            RemoteRecordingUseStateV1::Foreground
        } else if self.entries.is_empty() {
            RemoteRecordingUseStateV1::Inactive
        } else {
            RemoteRecordingUseStateV1::CatalogOnly
        }
    }

    pub(crate) fn record_temporal_work_v1(&mut self) -> Result<(), RemoteMcapUiErrorV1> {
        if self.use_state_v1() != RemoteRecordingUseStateV1::Foreground {
            return Err(RemoteMcapUiErrorV1::NonForegroundTemporalWorkRejected);
        }
        self.temporal_work_units += 1;
        Ok(())
    }

    pub(crate) fn record_query_work_v1(&mut self) -> Result<(), RemoteMcapUiErrorV1> {
        if self.use_state_v1() != RemoteRecordingUseStateV1::Foreground {
            return Err(RemoteMcapUiErrorV1::NonForegroundQueryRejected);
        }
        self.query_work_units += 1;
        Ok(())
    }

    pub(crate) fn record_store_mutation_v1(&mut self) -> Result<(), RemoteMcapUiErrorV1> {
        if self.use_state_v1() != RemoteRecordingUseStateV1::Foreground {
            return Err(RemoteMcapUiErrorV1::NonForegroundStoreMutationRejected);
        }
        self.store_mutation_units += 1;
        Ok(())
    }

    pub(crate) const fn temporal_work_units_v1(&self) -> u64 {
        self.temporal_work_units
    }

    pub(crate) const fn query_work_units_v1(&self) -> u64 {
        self.query_work_units
    }

    pub(crate) const fn store_mutation_units_v1(&self) -> u64 {
        self.store_mutation_units
    }
}

/// Projection returned when a source leaves or re-enters foreground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCommittedCursorResumeProjectionV1 {
    pub(crate) committed_cursor: Option<TimeInt>,
    pub(crate) play_state: RemotePlayStateV1,
    pub(crate) consumed_background_wall_clock: bool,
}

/// Storage-free committed-cursor resume state.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RemoteCommittedCursorResumeStateV1 {
    use_state: RemoteRecordingUseStateV1,
    committed_cursor: Option<TimeInt>,
    desired_play_state: RemotePlayStateV1,
}

impl RemoteCommittedCursorResumeStateV1 {
    pub(crate) fn new_v1(
        use_state: RemoteRecordingUseStateV1,
        navigation: &RemoteCanonicalNavigationV1,
    ) -> Self {
        let committed_cursor = if use_state == RemoteRecordingUseStateV1::Foreground {
            navigation.committed_cursor
        } else {
            None
        };
        let desired_play_state = if use_state == RemoteRecordingUseStateV1::Foreground {
            navigation.play_state
        } else {
            RemotePlayStateV1::Paused
        };
        Self {
            use_state,
            committed_cursor,
            desired_play_state,
        }
    }

    pub(crate) const fn no_temporal_data_v1(use_state: RemoteRecordingUseStateV1) -> Self {
        Self {
            use_state,
            committed_cursor: None,
            desired_play_state: RemotePlayStateV1::Paused,
        }
    }

    pub(crate) fn record_navigation_v1(&mut self, navigation: &RemoteCanonicalNavigationV1) {
        if self.use_state == RemoteRecordingUseStateV1::Foreground {
            self.committed_cursor = navigation.committed_cursor;
            self.desired_play_state = navigation.play_state;
        }
    }

    pub(crate) fn transition_to_v1(
        &mut self,
        use_state: RemoteRecordingUseStateV1,
        _background_wall_clock: Duration,
    ) -> RemoteCommittedCursorResumeProjectionV1 {
        let was_foreground = self.use_state == RemoteRecordingUseStateV1::Foreground;
        self.use_state = use_state;
        RemoteCommittedCursorResumeProjectionV1 {
            committed_cursor: self.committed_cursor,
            play_state: self.desired_play_state,
            consumed_background_wall_clock: use_state == RemoteRecordingUseStateV1::Foreground
                && was_foreground,
        }
    }

    pub(crate) const fn use_state_v1(&self) -> RemoteRecordingUseStateV1 {
        self.use_state
    }

    pub(crate) const fn committed_cursor_v1(&self) -> Option<TimeInt> {
        self.committed_cursor
    }

    pub(crate) const fn desired_play_state_v1(&self) -> RemotePlayStateV1 {
        self.desired_play_state
    }
}

/// Custom debug output for the cursor-resume state keeps the surface secret-free.
impl fmt::Debug for RemoteCommittedCursorResumeStateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteCommittedCursorResumeStateV1")
            .field("use_state", &self.use_state)
            .field("has_committed_cursor", &self.committed_cursor.is_some())
            .field("desired_play_state", &self.desired_play_state)
            .finish()
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

    const fn revision(value: u64) -> UserNavigationRevisionV1 {
        UserNavigationRevisionV1::new_v1(value)
    }

    fn navigation(cursor: Option<i64>) -> RemoteCanonicalNavigationV1 {
        RemoteCanonicalNavigationV1 {
            timeline: re_chunk::TimelineName::log_time(),
            committed_cursor: cursor.map(TimeInt::new_temporal),
            play_state: RemotePlayStateV1::Playing,
        }
    }

    #[test]
    fn web_remote_mcap_ui_open_options_time_type_is_frozen() {
        assert_eq!(
            RemoteOpenTimeTypeV1::from_time_type_v1(TimeType::Sequence),
            Err(RemoteOpenOptionsErrorV1::UnsupportedTimeType)
        );
        assert_eq!(
            RemoteOpenTimeTypeV1::from_time_type_v1(TimeType::TimestampNs),
            Ok(RemoteOpenTimeTypeV1::TimestampNs)
        );
        assert_eq!(
            RemoteOpenTimeTypeV1::from_time_type_v1(TimeType::DurationNs),
            Ok(RemoteOpenTimeTypeV1::DurationNs)
        );
    }

    #[test]
    fn web_remote_mcap_ui_consistency_truth_table_does_not_fake_strong_validator() {
        assert_eq!(
            resolve_open_consistency_v1(
                RemoteOpenConsistencyPolicyV1::RequireStrongValidator,
                RemoteOpenObservedEntityTagV1::Strong,
            ),
            Ok(RemoteOpenActualConsistencyV1::StrongValidator)
        );
        assert_eq!(
            resolve_open_consistency_v1(
                RemoteOpenConsistencyPolicyV1::RequireStrongValidator,
                RemoteOpenObservedEntityTagV1::Weak,
            ),
            Err(RemoteOpenOptionsErrorV1::StrongValidatorRequired)
        );
        assert_eq!(
            resolve_open_consistency_v1(
                RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed,
                RemoteOpenObservedEntityTagV1::Absent,
            ),
            Ok(RemoteOpenActualConsistencyV1::DeploymentAssumed)
        );
        assert_eq!(
            resolve_open_consistency_v1(
                RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed,
                RemoteOpenObservedEntityTagV1::Strong,
            ),
            Ok(RemoteOpenActualConsistencyV1::StrongValidator)
        );
    }

    #[test]
    fn web_remote_mcap_ui_deployment_assumed_warning_requires_explicit_allow() {
        let strong_required = RemoteOpenOptionsUiProjectionV1::new_v1(
            TimeType::TimestampNs,
            RemoteOpenTopicFilterSummaryV1::AllTopics,
            RemoteOpenDecoderSelectorSummaryV1::DefaultDecoders,
            RemoteOpenConsistencyPolicyV1::RequireStrongValidator,
            RemoteOpenObservedEntityTagV1::Strong,
        )
        .unwrap();
        assert!(!strong_required.has_deployment_assumed_warning_v1());

        let deployment_assumed = RemoteOpenOptionsUiProjectionV1::new_v1(
            TimeType::DurationNs,
            RemoteOpenTopicFilterSummaryV1::Redacted { filter_count: 3 },
            RemoteOpenDecoderSelectorSummaryV1::Redacted { selector_count: 2 },
            RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed,
            RemoteOpenObservedEntityTagV1::Weak,
        )
        .unwrap();
        assert!(deployment_assumed.has_deployment_assumed_warning_v1());
        assert_eq!(
            deployment_assumed.actual_consistency_v1(),
            Some(RemoteOpenActualConsistencyV1::DeploymentAssumed)
        );

        let explicit_but_strong = RemoteOpenOptionsUiProjectionV1::new_v1(
            TimeType::TimestampNs,
            RemoteOpenTopicFilterSummaryV1::AllTopics,
            RemoteOpenDecoderSelectorSummaryV1::DefaultDecoders,
            RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed,
            RemoteOpenObservedEntityTagV1::Strong,
        )
        .unwrap();
        assert!(!explicit_but_strong.has_deployment_assumed_warning_v1());
    }

    #[test]
    fn web_remote_mcap_ui_open_options_debug_and_labels_are_redacted() {
        let projection = RemoteOpenOptionsUiProjectionV1::new_v1(
            TimeType::TimestampNs,
            RemoteOpenTopicFilterSummaryV1::Redacted { filter_count: 4 },
            RemoteOpenDecoderSelectorSummaryV1::Redacted { selector_count: 7 },
            RemoteOpenConsistencyPolicyV1::AllowDeploymentAssumed,
            RemoteOpenObservedEntityTagV1::Absent,
        )
        .unwrap();
        let debug = format!("{projection:?}");
        let status = projection.status_label_v1();
        let label = projection.label_v1();

        for forbidden in [
            "http",
            "token",
            "topic://",
            "store_id",
            "generation",
            "example",
            "entity_path",
        ] {
            assert!(!debug.to_ascii_lowercase().contains(forbidden));
            assert!(!status.to_ascii_lowercase().contains(forbidden));
            assert!(!label.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn web_remote_mcap_ui_route_matrix_projects_all_three_behaviors() {
        for route in [
            RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
            RemoteMcapUiRouteClassV1::StrictExplicitMcap,
            RemoteMcapUiRouteClassV1::StrictExtensionlessSniff,
        ] {
            assert_eq!(
                project_remote_recording_ui_v1(
                    route,
                    RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
                    operation(1),
                )
                .surface_kind,
                RemoteRecordingUiSurfaceKindV1::SelectedRecordingPanel
            );
            assert_eq!(
                project_remote_recording_ui_v1(
                    route,
                    RemoteMcapRecordingOpenBehaviorV1::Open,
                    operation(2),
                )
                .surface_kind,
                RemoteRecordingUiSurfaceKindV1::RecordingPanel
            );
            assert_eq!(
                project_remote_recording_ui_v1(
                    route,
                    RemoteMcapRecordingOpenBehaviorV1::Background,
                    operation(3),
                )
                .surface_kind,
                RemoteRecordingUiSurfaceKindV1::MetadataCatalogCard
            );
        }
    }

    #[test]
    fn web_remote_mcap_ui_compatibility_extensionless_and_strict_non_mcap_stay_outside() {
        assert_eq!(
            project_remote_mcap_ui_route_v1(RemoteMcapUiRouteInputV1::CompatibilityExtensionless),
            RemoteMcapUiRouteDispositionV1::CompatibilityExtensionlessDispatcher
        );
        assert_eq!(
            project_remote_mcap_ui_route_v1(RemoteMcapUiRouteInputV1::StrictNonMcapSniff),
            RemoteMcapUiRouteDispositionV1::StrictNonMcapUnsupported
        );
    }

    #[test]
    fn web_remote_mcap_ui_surface_installs_selected_panel_panel_and_catalog() {
        let mut surface = RemoteRecordingUiSurfaceV1::new_v1();

        let selected = surface
            .install_v1(
                operation(1),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
                RemoteMcapUiRouteClassV1::StrictExplicitMcap,
                revision(4),
                revision(4),
            )
            .unwrap();
        assert!(selected.is_selected_v1());
        assert_eq!(
            surface.use_state_v1(),
            RemoteRecordingUseStateV1::Foreground
        );
        assert_eq!(surface.selected_operation_v1(), Some(operation(1)));

        let panel = surface
            .install_v1(
                operation(2),
                RemoteMcapRecordingOpenBehaviorV1::Open,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(4),
                revision(4),
            )
            .unwrap();
        assert!(!panel.is_selected_v1());
        assert!(!panel.is_catalog_only_v1());
        assert_eq!(surface.selected_operation_v1(), Some(operation(1)));

        let catalog = surface
            .install_v1(
                operation(3),
                RemoteMcapRecordingOpenBehaviorV1::Background,
                RemoteMcapUiRouteClassV1::StrictExtensionlessSniff,
                revision(4),
                revision(4),
            )
            .unwrap();
        assert!(catalog.is_catalog_only_v1());
        assert_eq!(surface.selected_operation_v1(), Some(operation(1)));
        assert_eq!(
            surface.use_state_v1(),
            RemoteRecordingUseStateV1::Foreground
        );
    }

    #[test]
    fn web_remote_mcap_ui_open_and_background_do_not_foreground_when_alone() {
        let mut surface = RemoteRecordingUiSurfaceV1::new_v1();
        surface
            .install_v1(
                operation(2),
                RemoteMcapRecordingOpenBehaviorV1::Open,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(5),
                revision(5),
            )
            .unwrap();
        assert_eq!(surface.selected_operation_v1(), None);
        assert_eq!(
            surface.use_state_v1(),
            RemoteRecordingUseStateV1::CatalogOnly
        );
        assert!(!surface.is_foreground_v1(operation(2)));

        surface
            .install_v1(
                operation(3),
                RemoteMcapRecordingOpenBehaviorV1::Background,
                RemoteMcapUiRouteClassV1::StrictExplicitMcap,
                revision(5),
                revision(5),
            )
            .unwrap();
        assert_eq!(surface.selected_operation_v1(), None);
        assert_eq!(
            surface.use_state_v1(),
            RemoteRecordingUseStateV1::CatalogOnly
        );
        assert!(!surface.is_foreground_v1(operation(3)));
    }

    #[test]
    fn web_remote_mcap_ui_close_removes_operation_ownership() {
        let mut surface = RemoteRecordingUiSurfaceV1::new_v1();
        surface
            .install_v1(
                operation(4),
                RemoteMcapRecordingOpenBehaviorV1::Open,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(6),
                revision(6),
            )
            .unwrap();
        assert!(surface.has_ownership_v1(operation(4)));

        let closed = surface.close_v1(operation(4)).unwrap();
        assert_eq!(closed.operation_id_v1(), operation(4));
        assert!(!surface.has_ownership_v1(operation(4)));
        assert_eq!(surface.use_state_v1(), RemoteRecordingUseStateV1::Inactive);
    }

    #[test]
    fn web_remote_mcap_ui_monotonic_behavior_effects_are_enforced() {
        let mut surface = RemoteRecordingUiSurfaceV1::new_v1();
        surface
            .install_v1(
                operation(5),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(7),
                revision(7),
            )
            .unwrap();

        assert_eq!(
            surface.install_v1(
                operation(5),
                RemoteMcapRecordingOpenBehaviorV1::Background,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(7),
                revision(7),
            ),
            Err(RemoteMcapUiErrorV1::BehaviorEffectWouldDowngradeForeground)
        );
        assert_eq!(
            surface.install_v1(
                operation(5),
                RemoteMcapRecordingOpenBehaviorV1::Open,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(7),
                revision(7),
            ),
            Err(RemoteMcapUiErrorV1::BehaviorEffectWouldUndoSelection)
        );
        assert_eq!(surface.selected_operation_v1(), Some(operation(5)));
    }

    #[test]
    fn web_remote_mcap_ui_stale_open_and_select_does_not_replace_selection() {
        let mut surface = RemoteRecordingUiSurfaceV1::new_v1();
        surface
            .install_v1(
                operation(6),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(8),
                revision(8),
            )
            .unwrap();

        assert_eq!(
            surface.install_v1(
                operation(7),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(8),
                revision(9),
            ),
            Err(RemoteMcapUiErrorV1::StaleProgrammaticSelectionIntent)
        );
        assert_eq!(surface.selected_operation_v1(), Some(operation(6)));
        assert!(!surface.has_ownership_v1(operation(7)));
    }

    #[test]
    fn web_remote_mcap_ui_new_open_and_select_downgrades_previous_entry() {
        let mut surface = RemoteRecordingUiSurfaceV1::new_v1();
        surface
            .install_v1(
                operation(11),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
                RemoteMcapUiRouteClassV1::CompatibilityExplicitMcap,
                revision(12),
                revision(12),
            )
            .unwrap();
        surface
            .install_v1(
                operation(12),
                RemoteMcapRecordingOpenBehaviorV1::OpenAndSelect,
                RemoteMcapUiRouteClassV1::StrictExplicitMcap,
                revision(12),
                revision(12),
            )
            .unwrap();

        assert_eq!(surface.selected_operation_v1(), Some(operation(12)));
        assert!(surface.is_foreground_v1(operation(12)));
        assert!(!surface.entry_v1(operation(11)).unwrap().is_selected_v1());
        assert_eq!(
            surface.entry_v1(operation(11)).unwrap().surface_kind_v1(),
            RemoteRecordingUiSurfaceKindV1::RecordingPanel
        );
    }

    #[test]
    fn web_remote_mcap_ui_catalog_only_and_inactive_have_no_temporal_work() {
        let mut catalog = RemoteRecordingUiSurfaceV1::new_v1();
        catalog
            .install_v1(
                operation(8),
                RemoteMcapRecordingOpenBehaviorV1::Background,
                RemoteMcapUiRouteClassV1::StrictExplicitMcap,
                revision(10),
                revision(10),
            )
            .unwrap();
        assert_eq!(
            catalog.use_state_v1(),
            RemoteRecordingUseStateV1::CatalogOnly
        );
        assert_eq!(
            catalog.record_temporal_work_v1(),
            Err(RemoteMcapUiErrorV1::NonForegroundTemporalWorkRejected)
        );
        assert_eq!(
            catalog.record_query_work_v1(),
            Err(RemoteMcapUiErrorV1::NonForegroundQueryRejected)
        );
        assert_eq!(
            catalog.record_store_mutation_v1(),
            Err(RemoteMcapUiErrorV1::NonForegroundStoreMutationRejected)
        );
        assert_eq!(catalog.temporal_work_units_v1(), 0);
        assert_eq!(catalog.query_work_units_v1(), 0);
        assert_eq!(catalog.store_mutation_units_v1(), 0);

        let mut inactive = RemoteRecordingUiSurfaceV1::new_v1();
        assert_eq!(inactive.use_state_v1(), RemoteRecordingUseStateV1::Inactive);
        assert_eq!(
            inactive.record_temporal_work_v1(),
            Err(RemoteMcapUiErrorV1::NonForegroundTemporalWorkRejected)
        );
        assert_eq!(inactive.temporal_work_units_v1(), 0);
    }

    #[test]
    fn web_remote_mcap_ui_cursor_resume_ignores_background_delta() {
        let foreground_navigation = navigation(Some(42));
        let mut cursor = RemoteCommittedCursorResumeStateV1::new_v1(
            RemoteRecordingUseStateV1::Foreground,
            &foreground_navigation,
        );
        cursor.record_navigation_v1(&navigation(Some(99)));

        let background = cursor.transition_to_v1(
            RemoteRecordingUseStateV1::CatalogOnly,
            Duration::from_nanos(123),
        );
        assert_eq!(background.committed_cursor, Some(TimeInt::new_temporal(99)));
        assert!(!background.consumed_background_wall_clock);

        let resumed = cursor.transition_to_v1(
            RemoteRecordingUseStateV1::Foreground,
            Duration::from_nanos(456),
        );
        assert_eq!(resumed.committed_cursor, Some(TimeInt::new_temporal(99)));
        assert_eq!(resumed.play_state, RemotePlayStateV1::Playing);
        assert!(!resumed.consumed_background_wall_clock);
    }

    #[test]
    fn web_remote_mcap_ui_no_temporal_data_does_not_fabricate_committed_cursor() {
        let mut cursor = RemoteCommittedCursorResumeStateV1::no_temporal_data_v1(
            RemoteRecordingUseStateV1::CatalogOnly,
        );
        cursor.transition_to_v1(
            RemoteRecordingUseStateV1::Foreground,
            Duration::from_nanos(7),
        );
        assert_eq!(cursor.committed_cursor_v1(), None);
        assert_eq!(cursor.desired_play_state_v1(), RemotePlayStateV1::Paused);
    }
}
