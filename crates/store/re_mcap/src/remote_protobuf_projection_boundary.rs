//! Neutral ownership boundary between bounded ROS 2 and future protobuf initialization.

#![allow(dead_code)]

use crate::remote_ros2_reflection::RemoteRos2ProjectionEofAuthorityV1;
#[cfg(test)]
use crate::remote_ros2_reflection::{RemoteRos2InitializationError, RemoteRos2RecognitionIterV1};

/// Opaque move-only proof that protobuf projection reached EOF while retaining the exact ROS
/// result, source/policy identity, reservation, and unspent recognition authority.
///
/// MCAP-026 deliberately provides no result slot, generic binder, parts getter, or production
/// recognition accessor. MCAP-027 will privately own its concrete combined result; until then no
/// crate-root sibling can detach the authority or use it before successful projection EOF.
pub(crate) struct RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire> {
    authority: RemoteRos2ProjectionEofAuthorityV1<'definitions, 'input, 'source, 'wire>,
}

/// Sealed identity for the remote protobuf resource profile expected by one source.
///
/// No ordinary product constructor exists while the remote route is disarmed.
pub(crate) struct RemoteProtobufProfileScopeV1 {
    _sealed_identity: u8,
}

#[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
impl RemoteProtobufProfileScopeV1 {
    pub(crate) const fn new_disarmed_v1() -> Self {
        Self {
            _sealed_identity: 1,
        }
    }
}

/// Seals the unforgeable authority produced by the ROS 2 projection owner.
///
/// The input has no public constructor or fields, so another crate-root sibling cannot use this
/// function to mint a continuation from scalar claims or arbitrary definitions.
pub(crate) fn seal_remote_ros2_projection_eof_v1<'definitions, 'input, 'source, 'wire>(
    authority: RemoteRos2ProjectionEofAuthorityV1<'definitions, 'input, 'source, 'wire>,
) -> RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire> {
    RemoteProtobufProjectionEofContinuationV1 { authority }
}

impl<'definitions, 'input, 'source, 'wire>
    RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire>
{
    pub(super) fn ensure_current_for_assignment_v1(
        &self,
    ) -> Result<(), crate::remote_ros2_reflection::RemoteRos2InitializationError> {
        self.authority.ensure_current_for_assignment_v1()
    }

    pub(super) fn retained_bytes_for_assignment_v1(
        &self,
    ) -> Result<u64, crate::remote_ros2_reflection::RemoteRos2InitializationError> {
        self.authority.retained_bytes_for_assignment_v1()
    }

    pub(super) fn physical_source_binding_for_manifest_v1(
        &self,
    ) -> Result<
        &crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
        crate::remote_ros2_reflection::RemoteRos2InitializationError,
    > {
        self.authority.physical_source_binding_for_manifest_v1()
    }

    pub(super) fn policy_descriptor_for_assignment_v1(
        &self,
    ) -> Result<
        crate::remote_ros2_reflection::FrozenRemoteDecoderPolicyDescriptorViewV1<'_, 'wire>,
        crate::remote_ros2_reflection::RemoteRos2InitializationError,
    > {
        self.authority.policy_descriptor_for_assignment_v1()
    }

    pub(super) fn ensure_profile_current_v1(
        &self,
        viewer_scope: *const crate::remote_ros2_reflection::RemoteViewerScopeState,
        profile_scope: *const RemoteProtobufProfileScopeV1,
    ) -> Result<(), crate::remote_ros2_reflection::RemoteRos2InitializationError> {
        self.authority
            .ensure_protobuf_profile_current_v1(viewer_scope, profile_scope)
    }

    pub(super) fn take_bound_recognition_v1(
        &mut self,
    ) -> Result<
        crate::remote_ros2_reflection::RemoteRos2RecognitionIterV1<
            '_,
            'definitions,
            'input,
            'source,
            'wire,
        >,
        crate::remote_ros2_reflection::RemoteRos2InitializationError,
    > {
        self.authority.take_bound_recognition_v1()
    }

    pub(super) fn selected_count_for_assignment_v1(
        &self,
    ) -> Result<usize, crate::remote_ros2_reflection::RemoteRos2InitializationError> {
        self.authority.selected_count_for_assignment_v1()
    }

    pub(super) fn canonical_channel_count_for_assignment_v1(
        &self,
    ) -> Result<u64, crate::remote_ros2_reflection::RemoteRos2InitializationError> {
        self.authority.canonical_channel_count_for_assignment_v1()
    }

    pub(super) fn project_selected_membership_for_assignment_v1(
        &self,
    ) -> Result<
        crate::remote_ros2_reflection::RemoteMcapSelectedMembershipV1,
        crate::remote_ros2_reflection::RemoteRos2InitializationError,
    > {
        self.authority
            .project_selected_membership_for_assignment_v1()
    }
}

#[cfg(test)]
impl<'definitions, 'input, 'source, 'wire>
    RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire>
{
    pub(crate) fn take_bound_recognition_for_test_v1(
        &mut self,
    ) -> Result<
        RemoteRos2RecognitionIterV1<'_, 'definitions, 'input, 'source, 'wire>,
        RemoteRos2InitializationError,
    > {
        self.authority.take_bound_recognition_for_test_v1()
    }

    pub(crate) fn ensure_current_for_test_v1(&self) -> Result<(), RemoteRos2InitializationError> {
        self.authority.ensure_current_for_test_v1()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static_assertions::assert_not_impl_any!(
        RemoteProtobufProjectionEofContinuationV1<'static, 'static, 'static, 'static>: Clone, Copy
    );

    #[test]
    fn sealing_api_accepts_only_the_opaque_post_eof_authority() {
        let _sealer = seal_remote_ros2_projection_eof_v1;
    }
}
