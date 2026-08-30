//! Production-disarmed strict open disarmed handoff commit/release/abort state machine.
//!
//! The handoff owns the prepared batch until matching release.
//! Abort leaves the history snapshot unchanged and only returns ownership.

use std::collections::BTreeMap;
use std::fmt;

use crate::strict_open_batch::PreparedStrictOpenBatchV1;
use crate::strict_open_wire::{
    StrictOpenHandoffIdentity, StrictOpenInstallationIdentity, StrictOpenJsInstallationAckV1,
    StrictOpenProtocolCodeV1, StrictOpenWireErrorV1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrictOpenTerminalRegistrySnapshotV1 {
    pub entries: usize,
    pub bytes: usize,
}

pub struct StrictOpenDisarmedHandoffV1 {
    handoff_token: StrictOpenHandoffIdentity,
    installation_nonce: StrictOpenInstallationIdentity,
    prepared: PreparedStrictOpenBatchV1,
    snapshot: StrictOpenTerminalRegistrySnapshotV1,
    page_idle: bool,
}

pub struct StrictOpenCommittedHandoffV1 {
    prepared: PreparedStrictOpenBatchV1,
    snapshot: StrictOpenTerminalRegistrySnapshotV1,
}

impl fmt::Debug for StrictOpenDisarmedHandoffV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StrictOpenDisarmedHandoffV1")
            .field("handoff_token", &self.handoff_token)
            .field("installation_nonce", &self.installation_nonce)
            .field("prepared_operations", &self.prepared.operations_v1().len())
            .field("snapshot", &self.snapshot)
            .field("page_idle", &self.page_idle)
            .finish()
    }
}

impl fmt::Debug for StrictOpenCommittedHandoffV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StrictOpenCommittedHandoffV1")
            .field("prepared_operations", &self.prepared.operations_v1().len())
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

impl StrictOpenDisarmedHandoffV1 {
    pub fn new_v1(
        handoff_token: StrictOpenHandoffIdentity,
        installation_nonce: StrictOpenInstallationIdentity,
        prepared: PreparedStrictOpenBatchV1,
        snapshot: StrictOpenTerminalRegistrySnapshotV1,
    ) -> Self {
        Self {
            handoff_token,
            installation_nonce,
            prepared,
            snapshot,
            page_idle: false,
        }
    }

    pub fn set_page_idle_v1(&mut self, page_idle: bool) {
        self.page_idle = page_idle;
    }

    pub fn snapshot_v1(&self) -> StrictOpenTerminalRegistrySnapshotV1 {
        self.snapshot
    }

    pub fn release_v1(
        self,
        expected_handoff_token: StrictOpenHandoffIdentity,
        acknowledgements: &[StrictOpenJsInstallationAckV1],
    ) -> Result<StrictOpenCommittedHandoffV1, StrictOpenWireErrorV1> {
        if self.handoff_token != expected_handoff_token {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            ));
        }
        if !self.page_idle {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            ));
        }
        validate_installation_acks_v1(&self.prepared, self.installation_nonce, acknowledgements)?;
        Ok(StrictOpenCommittedHandoffV1 {
            prepared: self.prepared,
            snapshot: self.snapshot,
        })
    }

    pub fn abort_v1(
        self,
        expected_handoff_token: StrictOpenHandoffIdentity,
    ) -> Result<StrictOpenTerminalRegistrySnapshotV1, StrictOpenWireErrorV1> {
        if self.handoff_token != expected_handoff_token {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            ));
        }
        Ok(self.snapshot)
    }
}

impl StrictOpenCommittedHandoffV1 {
    pub fn snapshot_v1(&self) -> StrictOpenTerminalRegistrySnapshotV1 {
        self.snapshot
    }

    pub fn into_prepared_v1(self) -> PreparedStrictOpenBatchV1 {
        self.prepared
    }
}

fn validate_installation_acks_v1(
    prepared: &PreparedStrictOpenBatchV1,
    installation_nonce: StrictOpenInstallationIdentity,
    acknowledgements: &[StrictOpenJsInstallationAckV1],
) -> Result<(), StrictOpenWireErrorV1> {
    let mut expected_by_operation = BTreeMap::new();
    for operation in prepared.operations_v1() {
        expected_by_operation.insert(operation.operation_id, operation);
    }
    let mut seen = BTreeMap::new();

    for ack in acknowledgements {
        if ack.installation_nonce != installation_nonce {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::InstallationAckMismatch,
            ));
        }
        let Some(operation) = expected_by_operation.get(&ack.operation_token) else {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::InstallationAckMismatch,
            ));
        };
        let Some(recording_ack) = ack.recordings.first() else {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        };
        if recording_ack.operation_token != ack.operation_token
            || recording_ack.public_recording_id != operation.public_recording_id
        {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::InstallationAckMismatch,
            ));
        }
        if seen
            .insert(ack.operation_token, recording_ack.public_recording_id)
            .is_some()
        {
            return Err(StrictOpenWireErrorV1::protocol_violation(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        }
    }

    if seen.len() != expected_by_operation.len() {
        return Err(StrictOpenWireErrorV1::protocol_violation(
            StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::remote_validator::{RepresentationConsistency, RepresentationConsistencyPolicy};
    use crate::secret_url::SecretUrlParserLimits;
    use crate::source_reuse::RemoteMcapSemanticConfigV1;
    use crate::strict_open_batch::{
        PreparedStrictOpenBatchV1, PreparedStrictOpenOperationV1, StrictOpenBatchPrepareContextV1,
    };
    use crate::strict_open_wire::{
        RecordingActivationDeliveryAckV1, StrictOpenHandoffIdentity, StrictOpenInstallationIdentity,
    };
    use re_log_types::TimeType;

    use super::*;

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

    fn prepared_batch() -> PreparedStrictOpenBatchV1 {
        let mut context = StrictOpenBatchPrepareContextV1::new_v1(4);
        context
            .prepare_batch_v1(
                vec![
                    crate::strict_open_batch::StrictOpenRequestSpecV1::HttpRemoteMcapCandidate {
                        url: (*b"https://example.invalid/a.mcap").into(),
                        ingress: crate::secret_url::HttpUrlIngress::DirectExternal,
                        semantic: semantic(),
                    },
                ],
                &limits(),
            )
            .unwrap()
    }

    fn ack_for(operation: &PreparedStrictOpenOperationV1) -> StrictOpenJsInstallationAckV1 {
        StrictOpenJsInstallationAckV1 {
            operation_token: operation.operation_id,
            installation_nonce: StrictOpenInstallationIdentity::new(2),
            recordings: vec![RecordingActivationDeliveryAckV1 {
                instance: crate::strict_open_wire::StrictOpenWireInstanceId::new(1),
                operation_token: operation.operation_id,
                public_recording_id: operation.public_recording_id,
                lifecycle_revision: 7,
            }],
        }
    }

    #[test]
    fn release_requires_idle_and_matching_ack_snapshot() {
        let prepared = prepared_batch();
        let mut handoff = StrictOpenDisarmedHandoffV1::new_v1(
            StrictOpenHandoffIdentity::new(1),
            StrictOpenInstallationIdentity::new(2),
            prepared,
            StrictOpenTerminalRegistrySnapshotV1 {
                entries: 3,
                bytes: 9,
            },
        );
        assert_eq!(
            handoff.snapshot_v1(),
            StrictOpenTerminalRegistrySnapshotV1 {
                entries: 3,
                bytes: 9
            }
        );
        handoff.set_page_idle_v1(true);
        let ack = ack_for(&handoff.prepared.operations_v1()[0]);
        let committed = handoff
            .release_v1(StrictOpenHandoffIdentity::new(1), &[ack])
            .unwrap();
        assert_eq!(
            committed.snapshot_v1(),
            StrictOpenTerminalRegistrySnapshotV1 {
                entries: 3,
                bytes: 9
            }
        );
        assert_eq!(committed.into_prepared_v1().operations_v1().len(), 1);
    }

    #[test]
    fn wrong_token_or_missing_ack_preserves_snapshot_on_abort() {
        let prepared = prepared_batch();
        let handoff = StrictOpenDisarmedHandoffV1::new_v1(
            StrictOpenHandoffIdentity::new(1),
            StrictOpenInstallationIdentity::new(2),
            prepared,
            StrictOpenTerminalRegistrySnapshotV1 {
                entries: 3,
                bytes: 9,
            },
        );
        assert_eq!(
            handoff.abort_v1(StrictOpenHandoffIdentity::new(1)).unwrap(),
            StrictOpenTerminalRegistrySnapshotV1 {
                entries: 3,
                bytes: 9
            }
        );
    }

    #[test]
    fn release_before_idle_or_with_duplicate_ack_is_protocol_error() {
        let prepared = prepared_batch();
        let handoff = StrictOpenDisarmedHandoffV1::new_v1(
            StrictOpenHandoffIdentity::new(1),
            StrictOpenInstallationIdentity::new(2),
            prepared,
            StrictOpenTerminalRegistrySnapshotV1 {
                entries: 3,
                bytes: 9,
            },
        );
        let ack = ack_for(&handoff.prepared.operations_v1()[0]);
        assert!(matches!(
            handoff.release_v1(StrictOpenHandoffIdentity::new(1), &[ack]),
            Err(StrictOpenWireErrorV1::ProtocolViolation(_))
        ));
    }
}
