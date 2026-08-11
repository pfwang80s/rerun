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

/// Seals the unforgeable authority produced by the ROS 2 projection owner.
///
/// The input has no public constructor or fields, so another crate-root sibling cannot use this
/// function to mint a continuation from scalar claims or arbitrary definitions.
pub(crate) fn seal_remote_ros2_projection_eof_v1<'definitions, 'input, 'source, 'wire>(
    authority: RemoteRos2ProjectionEofAuthorityV1<'definitions, 'input, 'source, 'wire>,
) -> RemoteProtobufProjectionEofContinuationV1<'definitions, 'input, 'source, 'wire> {
    RemoteProtobufProjectionEofContinuationV1 { authority }
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
