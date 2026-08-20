//! Production-disarmed strict startup one-shot handoff state machine.
//!
//! The startup handoff owns a prepared strict batch until one matching release.
//! It can generate the strict-open success wire envelope, consume one matching
//! installation acknowledgement, and execute a matching tokenized release.
//! Abort keeps the terminal registry snapshot and all preexisting source,
//! operation, recording, and nonremote-route identity unchanged.
//!
//! This module only models local type and ownership transitions. It never
//! starts `Fetch`, calls `window.fetch`, creates a receiver/gRPC/Redap
//! transport, or mutates the Store.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::strict_open_batch::{PreparedStrictOpenBatchV1, PreparedStrictOpenOperationV1};
use crate::strict_open_handoff::StrictOpenTerminalRegistrySnapshotV1;
use crate::strict_open_wire::{
    BoundedRedactedRecordingLabelV1, CommittedOpenDescriptorV1, CommittedRecordingAttachmentV1,
    PublicRecordingDescriptorV1, RecordingActivationDeliveryAckV1, StrictOpenFatalAbiCodeV1,
    StrictOpenHandoffIdentity, StrictOpenInstallationIdentity, StrictOpenJsInstallationAckV1,
    StrictOpenProtocolCodeV1, StrictOpenSuccessV1, StrictOpenWireCodecV1, StrictOpenWireErrorV1,
    StrictOpenWireInstanceId, StrictOpenWireV1,
};

/// The lifecycle revision assigned to a fresh strict-startup recording.
const STRICT_STARTUP_FRESH_RECORDING_REVISION_V1: u64 = 0;

/// Disarmed, one-shot strict startup handoff.
///
/// The handoff owns the prepared batch until a matching release. Abort leaves
/// the supplied terminal registry snapshot unchanged and only drops the new
/// prepared ownership.
pub struct DisarmedStrictStartupHandoffV1 {
    handoff_token: StrictOpenHandoffIdentity,
    installation_nonces: Vec<StrictOpenInstallationIdentity>,
    prepared: Option<PreparedStrictOpenBatchV1>,
    snapshot: StrictOpenTerminalRegistrySnapshotV1,
    instance: StrictOpenWireInstanceId,
    codec: StrictOpenWireCodecV1,
    page_idle: bool,
    ack_consumed: bool,
    released: bool,
    aborted: bool,
}

/// A successfully released startup handoff.
///
/// Dropping this value releases the prepared ownership; the terminal registry
/// snapshot carried by the handoff remains unchanged until a future remote
/// capability publishes it.
pub struct CommittedStrictStartupHandoffV1 {
    handoff_token: StrictOpenHandoffIdentity,
    prepared: PreparedStrictOpenBatchV1,
    snapshot: StrictOpenTerminalRegistrySnapshotV1,
}

/// The result of a disarmed-to-committed startup handoff.
pub struct StrictStartupPreparedActivationV1 {
    pub success_wire: StrictOpenWireV1,
    pub committed: CommittedStrictStartupHandoffV1,
}

impl fmt::Debug for DisarmedStrictStartupHandoffV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DisarmedStrictStartupHandoffV1")
            .field("handoff_token", &self.handoff_token)
            .field("prepared_operations", &self.prepared_operations_len())
            .field("snapshot", &self.snapshot)
            .field("instance", &"<opaque>")
            .field("page_idle", &self.page_idle)
            .field("ack_consumed", &self.ack_consumed)
            .field("released", &self.released)
            .field("aborted", &self.aborted)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for CommittedStrictStartupHandoffV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommittedStrictStartupHandoffV1")
            .field("prepared_operations", &self.prepared.operations_v1().len())
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}

impl DisarmedStrictStartupHandoffV1 {
    /// Creates a disarmed startup handoff.
    ///
    /// `installation_nonce_base` is the base nonce used to derive one unique
    /// installation nonce per prepared operation, which the success wire codec
    /// requires for its descriptor identity checks.
    pub fn new_v1(
        handoff_token: StrictOpenHandoffIdentity,
        installation_nonce_base: u128,
        prepared: PreparedStrictOpenBatchV1,
        snapshot: StrictOpenTerminalRegistrySnapshotV1,
        instance: StrictOpenWireInstanceId,
        codec: StrictOpenWireCodecV1,
    ) -> Result<Self, StrictOpenWireErrorV1> {
        let mut installation_nonces = Vec::with_capacity(prepared.operations_v1().len());
        for (index, _operation) in prepared.operations_v1().iter().enumerate() {
            let offset = u128::try_from(index).map_err(|_overflow| codec_invariant_error_v1())?;
            let nonce = installation_nonce_base
                .checked_add(offset)
                .ok_or_else(codec_invariant_error_v1)?;
            installation_nonces.push(StrictOpenInstallationIdentity::new(nonce));
        }
        Ok(Self {
            handoff_token,
            installation_nonces,
            prepared: Some(prepared),
            snapshot,
            instance,
            codec,
            page_idle: false,
            ack_consumed: false,
            released: false,
            aborted: false,
        })
    }

    /// Records whether the startup visibility bootstrap observed a visible page.
    ///
    /// A hidden race keeps the handoff releasable but rejects the matching
    /// release until the caller observes an idle page.
    pub fn set_page_idle_v1(&mut self, page_idle: bool) {
        self.page_idle = page_idle;
    }

    pub fn snapshot_v1(&self) -> StrictOpenTerminalRegistrySnapshotV1 {
        self.snapshot
    }

    pub fn handoff_token_v1(&self) -> StrictOpenHandoffIdentity {
        self.handoff_token
    }

    /// Generates the strict-open success wire envelope for this handoff.
    ///
    /// Each prepared operation is emitted with its fresh recording attachment,
    /// so a caller can materialize public handles before release.
    pub fn success_wire_v1(&self) -> Result<StrictOpenWireV1, StrictOpenWireErrorV1> {
        self.ensure_active_v1()?;
        let descriptors = self
            .prepared_operations()
            .iter()
            .zip(&self.installation_nonces)
            .map(
                |(operation, installation_nonce)| CommittedOpenDescriptorV1 {
                    operation_token: operation.operation_id,
                    public_request_id: operation.public_request_id,
                    installation_nonce: *installation_nonce,
                    preexisting_recordings: vec![CommittedRecordingAttachmentV1 {
                        public_recording_id: operation.public_recording_id,
                        lifecycle_revision: STRICT_STARTUP_FRESH_RECORDING_REVISION_V1,
                        descriptor: PublicRecordingDescriptorV1 {
                            request_id: operation.public_request_id,
                            recording_handle_id: operation.public_recording_id,
                            display_name: BoundedRedactedRecordingLabelV1::generic(),
                        },
                    }],
                },
            )
            .collect();
        self.codec.encode_success(StrictOpenSuccessV1 {
            handoff_token: self.handoff_token,
            descriptors,
        })
    }

    /// Builds the matching installation acknowledgements for this handoff.
    ///
    /// The startup bootstrap is self-contained: the same Rust ownership that
    /// generated the success envelope also produces and validates the matching
    /// ack, so no raw-JS ack codec is needed before the measured capability
    /// bridge lands.
    pub fn matching_installation_acks_v1(&self) -> Vec<StrictOpenJsInstallationAckV1> {
        self.prepared_operations()
            .iter()
            .zip(&self.installation_nonces)
            .map(
                |(operation, installation_nonce)| StrictOpenJsInstallationAckV1 {
                    operation_token: operation.operation_id,
                    installation_nonce: *installation_nonce,
                    recordings: vec![RecordingActivationDeliveryAckV1 {
                        instance: self.instance,
                        operation_token: operation.operation_id,
                        public_recording_id: operation.public_recording_id,
                        lifecycle_revision: STRICT_STARTUP_FRESH_RECORDING_REVISION_V1,
                    }],
                },
            )
            .collect()
    }

    /// Consumes one complete set of matching installation acknowledgements.
    ///
    /// A duplicate, missing, mismatched, or wrong-instance ack fails without
    /// releasing the prepared ownership.
    pub fn consume_installation_acks_v1(
        &mut self,
        acknowledgements: &[StrictOpenJsInstallationAckV1],
    ) -> Result<(), StrictOpenWireErrorV1> {
        self.ensure_active_v1()?;
        if self.ack_consumed {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        }
        validate_startup_installation_acks_v1(
            self.prepared_operations(),
            &self.installation_nonces,
            self.instance,
            acknowledgements,
        )?;
        self.ack_consumed = true;
        Ok(())
    }

    /// Executes the matching tokenized release.
    ///
    /// The release requires the exact handoff token, an idle page observation,
    /// and a previously consumed matching ack. On success the prepared batch is
    /// moved into the committed value and the handoff becomes terminal.
    pub fn release_v1(
        &mut self,
        expected_handoff_token: StrictOpenHandoffIdentity,
    ) -> Result<CommittedStrictStartupHandoffV1, StrictOpenWireErrorV1> {
        self.ensure_active_v1()?;
        if self.handoff_token != expected_handoff_token {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            ));
        }
        if !self.page_idle {
            return Err(StrictOpenWireErrorV1::handoff_state_changed());
        }
        if !self.ack_consumed {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        }
        self.released = true;
        let prepared = self
            .prepared
            .take()
            .expect("an active startup handoff owns its prepared batch");
        Ok(CommittedStrictStartupHandoffV1 {
            handoff_token: self.handoff_token,
            prepared,
            snapshot: self.snapshot,
        })
    }

    /// Tokenized abort.
    ///
    /// Abort works before release and after a failed ack or failed release. It
    /// drops the prepared ownership and returns the unchanged terminal registry
    /// snapshot. Wrong-token aborts are rejected without a state change.
    pub fn abort_v1(
        &mut self,
        expected_handoff_token: StrictOpenHandoffIdentity,
    ) -> Result<StrictOpenTerminalRegistrySnapshotV1, StrictOpenWireErrorV1> {
        if self.handoff_token != expected_handoff_token {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            ));
        }
        if self.released || self.aborted {
            return Err(StrictOpenWireErrorV1::handoff_state_changed());
        }
        self.aborted = true;
        self.prepared = None;
        Ok(self.snapshot)
    }

    fn ensure_active_v1(&self) -> Result<(), StrictOpenWireErrorV1> {
        if self.aborted || self.released {
            return Err(StrictOpenWireErrorV1::handoff_state_changed());
        }
        Ok(())
    }

    fn prepared_operations(&self) -> &[PreparedStrictOpenOperationV1] {
        self.prepared
            .as_ref()
            .expect("an active startup handoff owns its prepared batch")
            .operations_v1()
    }

    fn prepared_operations_len(&self) -> usize {
        self.prepared
            .as_ref()
            .map_or(0, |prepared| prepared.operations_v1().len())
    }
}

impl CommittedStrictStartupHandoffV1 {
    pub fn snapshot_v1(&self) -> StrictOpenTerminalRegistrySnapshotV1 {
        self.snapshot
    }

    /// Hands the prepared ownership to the successful activation path.
    pub fn into_prepared_v1(
        self,
        expected_handoff_token: StrictOpenHandoffIdentity,
    ) -> Result<PreparedStrictOpenBatchV1, StrictOpenWireErrorV1> {
        if self.handoff_token != expected_handoff_token {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            ));
        }
        Ok(self.prepared)
    }

    /// Tokenized abort for a runner activation failure.
    ///
    /// Activation failure keeps the terminal registry snapshot unchanged and
    /// drops the prepared ownership without publishing anything.
    pub fn activation_failed_v1(
        self,
        expected_handoff_token: StrictOpenHandoffIdentity,
    ) -> Result<StrictOpenTerminalRegistrySnapshotV1, StrictOpenWireErrorV1> {
        if self.handoff_token != expected_handoff_token {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            ));
        }
        Ok(self.snapshot)
    }
}

/// Prepares a strict startup handoff for activation.
///
/// The returned success wire and committed batch are only produced after the
/// page-idle observation, success envelope generation, matching ack, and
/// tokenized release all succeed. The caller then runs the fallible runner
/// activation; a later activation failure is an activation-level abort that
/// drops the committed prepared ownership without mutating the terminal
/// registry.
pub fn prepare_strict_startup_activation_v1(
    mut handoff: DisarmedStrictStartupHandoffV1,
    page_idle: bool,
) -> Result<StrictStartupPreparedActivationV1, StrictOpenWireErrorV1> {
    handoff.set_page_idle_v1(page_idle);
    let token = handoff.handoff_token_v1();

    let success_wire = match handoff.success_wire_v1() {
        Ok(success_wire) => success_wire,
        Err(error) => {
            drop(handoff.abort_v1(token));
            return Err(error);
        }
    };
    let acknowledgements = handoff.matching_installation_acks_v1();
    if let Err(error) = handoff.consume_installation_acks_v1(&acknowledgements) {
        drop(handoff.abort_v1(token));
        return Err(error);
    }
    let committed = match handoff.release_v1(token) {
        Ok(committed) => committed,
        Err(error) => {
            drop(handoff.abort_v1(token));
            return Err(error);
        }
    };
    Ok(StrictStartupPreparedActivationV1 {
        success_wire,
        committed,
    })
}

fn validate_startup_installation_acks_v1(
    prepared_operations: &[PreparedStrictOpenOperationV1],
    installation_nonces: &[StrictOpenInstallationIdentity],
    instance: StrictOpenWireInstanceId,
    acknowledgements: &[StrictOpenJsInstallationAckV1],
) -> Result<(), StrictOpenWireErrorV1> {
    let mut expected_by_operation = BTreeMap::new();
    for (operation, installation_nonce) in prepared_operations.iter().zip(installation_nonces) {
        expected_by_operation.insert(operation.operation_id, (operation, *installation_nonce));
    }
    let mut seen = BTreeSet::new();

    for ack in acknowledgements {
        let Some((operation, expected_nonce)) = expected_by_operation.get(&ack.operation_token)
        else {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::InstallationAckMismatch,
            ));
        };
        if ack.installation_nonce != *expected_nonce {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::InstallationAckMismatch,
            ));
        }
        let Some(recording_ack) = ack.recordings.first() else {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        };
        if recording_ack.operation_token != ack.operation_token
            || recording_ack.public_recording_id != operation.public_recording_id
            || recording_ack.instance != instance
            || recording_ack.lifecycle_revision != STRICT_STARTUP_FRESH_RECORDING_REVISION_V1
        {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::InstallationAckMismatch,
            ));
        }
        if !seen.insert(ack.operation_token) {
            return Err(protocol_error_v1(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        }
    }

    if seen.len() != expected_by_operation.len() {
        return Err(protocol_error_v1(
            StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
        ));
    }

    Ok(())
}

fn protocol_error_v1(code: StrictOpenProtocolCodeV1) -> StrictOpenWireErrorV1 {
    StrictOpenWireErrorV1::protocol_violation(code)
}

fn codec_invariant_error_v1() -> StrictOpenWireErrorV1 {
    StrictOpenWireErrorV1::fatal_abi(StrictOpenFatalAbiCodeV1::CodecInvariantBroken)
}

#[cfg(test)]
mod tests {
    use re_log_types::TimeType;

    use crate::remote_limits::{WebRemoteLimitKey, tests::test_profile_with};
    use crate::remote_validator::{RepresentationConsistency, RepresentationConsistencyPolicy};
    use crate::secret_url::{HttpUrlIngress, SecretUrlParserLimits};
    use crate::source_reuse::RemoteMcapSemanticConfigV1;
    use crate::strict_open_batch::{StrictOpenBatchPrepareContextV1, StrictOpenRequestSpecV1};
    use crate::strict_open_wire::{
        OpenOperationIdentity, PublicRecordingIdentity, StrictOpenHandoffIdentity,
        StrictOpenInstallationIdentity, StrictOpenWireCodecV1, StrictOpenWireInstanceId,
    };

    use super::*;

    const INSTANCE_NONCE: u128 = 0x1234;
    const INSTANCE: StrictOpenWireInstanceId = StrictOpenWireInstanceId::new(INSTANCE_NONCE);

    fn limits() -> SecretUrlParserLimits {
        SecretUrlParserLimits::explicit_for_internal_v1(
            4_096, 65_536, 1_024, 16, 256, 262_144, 1_048_576,
        )
    }

    fn semantic() -> RemoteMcapSemanticConfigV1 {
        RemoteMcapSemanticConfigV1::new_v1(
            b"topic:*".to_vec().into_boxed_slice(),
            1,
            1,
            TimeType::Sequence,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistency::StrongValidator,
        )
    }

    fn codec(operation_count: usize) -> StrictOpenWireCodecV1 {
        let root = test_profile_with(&[
            (
                WebRemoteLimitKey::OpenBatchUrls,
                u64::try_from(operation_count.max(1)).unwrap(),
            ),
            (
                WebRemoteLimitKey::PreexistingRecordingAttachments,
                u64::try_from(operation_count.max(1)).unwrap(),
            ),
            (
                WebRemoteLimitKey::InstallationAcks,
                u64::try_from(operation_count.max(1)).unwrap(),
            ),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let batch = viewer.create_atomic_batch_scope().unwrap();
        StrictOpenWireCodecV1::for_batch(INSTANCE, &batch).unwrap()
    }

    fn prepared_batch(operation_count: usize) -> PreparedStrictOpenBatchV1 {
        let mut context = StrictOpenBatchPrepareContextV1::new_v1(operation_count);
        let specs = (0..operation_count)
            .map(|index| StrictOpenRequestSpecV1::HttpRemoteMcapCandidate {
                url: format!("https://example.invalid/{index}.mcap")
                    .as_bytes()
                    .to_vec()
                    .into_boxed_slice(),
                ingress: HttpUrlIngress::DirectExternal,
                semantic: semantic(),
            })
            .collect();
        context.prepare_batch_v1(specs, &limits()).unwrap()
    }

    fn handoff(operation_count: usize) -> DisarmedStrictStartupHandoffV1 {
        DisarmedStrictStartupHandoffV1::new_v1(
            StrictOpenHandoffIdentity::new(1),
            2,
            prepared_batch(operation_count),
            StrictOpenTerminalRegistrySnapshotV1 {
                entries: 3,
                bytes: 9,
            },
            INSTANCE,
            codec(operation_count),
        )
        .unwrap()
    }

    fn snapshot() -> StrictOpenTerminalRegistrySnapshotV1 {
        StrictOpenTerminalRegistrySnapshotV1 {
            entries: 3,
            bytes: 9,
        }
    }

    #[test]
    fn success_wire_uses_unique_installation_nonces_per_operation() {
        let handoff = handoff(2);
        let wire = handoff.success_wire_v1().unwrap();
        assert_eq!(wire.descriptors.len(), 2);
        assert_ne!(
            wire.descriptors[0].installation_nonce.wire_str(),
            wire.descriptors[1].installation_nonce.wire_str()
        );
        assert_eq!(wire.descriptors[0].preexisting_recordings.len(), 1);
        assert_eq!(
            wire.descriptors[0].preexisting_recordings[0]
                .lifecycle_revision_decimal
                .wire_str(),
            "0"
        );
    }

    #[test]
    fn success_wire_and_ack_are_secret_safe() {
        let handoff = handoff(1);
        let wire = handoff.success_wire_v1().unwrap();
        let wire_debug = format!("{wire:?}");
        assert!(!wire_debug.contains("example.invalid"));
        assert!(!wire_debug.contains("secret"));
        for ack in handoff.matching_installation_acks_v1() {
            assert!(!format!("{ack:?}").contains("secret"));
        }
    }

    #[test]
    fn release_requires_idle_matching_ack_and_token() {
        let mut handoff = handoff(1);
        assert_eq!(handoff.snapshot_v1(), snapshot());
        handoff.set_page_idle_v1(true);
        let acks = handoff.matching_installation_acks_v1();
        assert_eq!(acks.len(), 1);
        assert_eq!(
            acks[0].recordings[0].public_recording_id,
            PublicRecordingIdentity::new(3)
        );
        handoff.consume_installation_acks_v1(&acks).unwrap();
        let committed = handoff
            .release_v1(StrictOpenHandoffIdentity::new(1))
            .unwrap();
        assert_eq!(committed.snapshot_v1(), snapshot());
        let prepared = committed
            .into_prepared_v1(StrictOpenHandoffIdentity::new(1))
            .unwrap();
        assert_eq!(prepared.operations_v1().len(), 1);
    }

    #[test]
    fn wrong_token_aborts_and_preserves_snapshot() {
        let mut handoff = handoff(1);
        assert!(matches!(
            handoff.abort_v1(StrictOpenHandoffIdentity::new(2)),
            Err(StrictOpenWireErrorV1::ProtocolViolation(_))
        ));
        assert_eq!(
            handoff.abort_v1(StrictOpenHandoffIdentity::new(1)).unwrap(),
            snapshot()
        );
        assert!(matches!(
            handoff.abort_v1(StrictOpenHandoffIdentity::new(1)),
            Err(StrictOpenWireErrorV1::HandoffStateChanged(_))
        ));
    }

    #[test]
    fn release_before_idle_is_retryable_hidden_race() {
        let mut handoff = handoff(1);
        let acks = handoff.matching_installation_acks_v1();
        handoff.consume_installation_acks_v1(&acks).unwrap();
        let error = handoff
            .release_v1(StrictOpenHandoffIdentity::new(1))
            .unwrap_err();
        assert!(matches!(
            error,
            StrictOpenWireErrorV1::HandoffStateChanged(_)
        ));
        assert_eq!(
            handoff.abort_v1(StrictOpenHandoffIdentity::new(1)).unwrap(),
            snapshot()
        );
    }

    #[test]
    fn duplicate_and_missing_ack_are_protocol_errors() {
        let mut first = handoff(1);
        first.set_page_idle_v1(true);
        let acks = first.matching_installation_acks_v1();
        first.consume_installation_acks_v1(&acks).unwrap();
        assert!(matches!(
            first.consume_installation_acks_v1(&acks),
            Err(StrictOpenWireErrorV1::ProtocolViolation(_))
        ));

        let mut missing = handoff(2);
        missing.set_page_idle_v1(true);
        let acks = missing.matching_installation_acks_v1();
        assert!(matches!(
            missing.consume_installation_acks_v1(&acks[..1]),
            Err(StrictOpenWireErrorV1::ProtocolViolation(_))
        ));
    }

    #[test]
    fn wrong_ack_identity_is_rejected_before_release() {
        let mut handoff = handoff(1);
        handoff.set_page_idle_v1(true);
        let mut acks = handoff.matching_installation_acks_v1();
        acks[0].installation_nonce = StrictOpenInstallationIdentity::new(99);
        assert!(matches!(
            handoff.consume_installation_acks_v1(&acks),
            Err(StrictOpenWireErrorV1::ProtocolViolation(_))
        ));
        assert_eq!(
            handoff.abort_v1(StrictOpenHandoffIdentity::new(1)).unwrap(),
            snapshot()
        );
    }

    #[test]
    fn activation_failure_aborts_with_snapshot_and_no_publish() {
        let mut handoff = handoff(1);
        handoff.set_page_idle_v1(true);
        let acks = handoff.matching_installation_acks_v1();
        handoff.consume_installation_acks_v1(&acks).unwrap();
        let committed = handoff
            .release_v1(StrictOpenHandoffIdentity::new(1))
            .unwrap();
        assert_eq!(
            committed
                .activation_failed_v1(StrictOpenHandoffIdentity::new(1))
                .unwrap(),
            snapshot()
        );
    }

    #[test]
    fn committed_ownership_requires_matching_token() {
        let mut handoff = handoff(1);
        handoff.set_page_idle_v1(true);
        let acks = handoff.matching_installation_acks_v1();
        handoff.consume_installation_acks_v1(&acks).unwrap();
        let committed = handoff
            .release_v1(StrictOpenHandoffIdentity::new(1))
            .unwrap();
        assert!(matches!(
            committed.into_prepared_v1(StrictOpenHandoffIdentity::new(2)),
            Err(StrictOpenWireErrorV1::ProtocolViolation(_))
        ));
    }

    #[test]
    fn prepare_activation_orders_ack_release_before_success() {
        let prepared = handoff(1);
        let outcome = prepare_strict_startup_activation_v1(prepared, true).unwrap();
        assert_eq!(outcome.success_wire.descriptors.len(), 1);
        assert_eq!(outcome.committed.snapshot_v1(), snapshot());
    }

    #[test]
    fn prepare_activation_hidden_race_aborts_without_release() {
        let prepared = handoff(1);
        assert!(matches!(
            prepare_strict_startup_activation_v1(prepared, false),
            Err(StrictOpenWireErrorV1::HandoffStateChanged(_))
        ));
    }

    #[test]
    fn aborted_handoff_cannot_generate_success_or_release() {
        let mut handoff = handoff(1);
        handoff.abort_v1(StrictOpenHandoffIdentity::new(1)).unwrap();
        assert!(matches!(
            handoff.success_wire_v1(),
            Err(StrictOpenWireErrorV1::HandoffStateChanged(_))
        ));
        assert!(matches!(
            handoff.release_v1(StrictOpenHandoffIdentity::new(1)),
            Err(StrictOpenWireErrorV1::HandoffStateChanged(_))
        ));
    }

    #[test]
    fn operation_identities_are_stable_across_handoff_roundtrip() {
        let handoff = handoff(2);
        let wire = handoff.success_wire_v1().unwrap();
        assert_eq!(
            wire.descriptors[0].operation_token.wire_str(),
            format!("rso1:operation:{INSTANCE_NONCE:032x}:{:032x}", 1)
        );
        assert_eq!(
            wire.descriptors[1].operation_token.wire_str(),
            format!("rso1:operation:{INSTANCE_NONCE:032x}:{:032x}", 4)
        );
        let prepared_ids: Vec<OpenOperationIdentity> = handoff
            .prepared_operations()
            .iter()
            .map(|operation| operation.operation_id)
            .collect();
        assert_eq!(prepared_ids.len(), 2);
    }
}
