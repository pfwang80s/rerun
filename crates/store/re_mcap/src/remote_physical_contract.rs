//! Pure opaque contracts for the Web remote-MCAP physical handoff.
//!
//! This module defines scheduler-facing types only.
//! Authentic issuance and consumption are implemented by later work items.

use std::fmt;

/// Scheduler-visible state for an opaque physical operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePhysicalOperationStateV1 {
    /// The operation is waiting for its single consumption.
    Prepared,

    /// The operation has completed.
    Completed,

    /// The operation ended with a redacted failure.
    Failed,
}

impl fmt::Display for RemotePhysicalOperationStateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Prepared => "prepared",
            Self::Completed => "completed",
            Self::Failed => "failed",
        })
    }
}

/// Redacted failure categories crossing the physical-operation boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePhysicalOperationErrorV1 {
    /// The operation is no longer current.
    Stale,

    /// The operation belongs to another execution configuration.
    WrongProfile,

    /// The supplied physical data did not validate.
    InvalidData,

    /// A bounded resource reservation failed.
    ResourceLimit,

    /// The active profile does not support the operation.
    Unsupported,
}

impl fmt::Display for RemotePhysicalOperationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stale => "stale",
            Self::WrongProfile => "wrong profile",
            Self::InvalidData => "invalid data",
            Self::ResourceLimit => "resource limit",
            Self::Unsupported => "unsupported",
        })
    }
}

mod private {
    /// Private issuance seal reserved for the trusted producer.
    pub struct OperationSealV1(pub u8);

    /// Private completion seal reserved for the trusted producer.
    pub struct ResultSealV1(pub u8);
}

/// A move-only operation issued by the trusted physical producer.
///
/// There is intentionally no constructor or physical-identity projection in this work item.
pub struct RemotePhysicalOperationV1 {
    seal: private::OperationSealV1,
    state: RemotePhysicalOperationStateV1,
}

impl RemotePhysicalOperationV1 {
    /// Returns scheduler state without exposing physical ownership or identity.
    pub fn state_v1(&self) -> RemotePhysicalOperationStateV1 {
        self.state
    }
}

impl fmt::Debug for RemotePhysicalOperationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = self.seal.0;
        formatter
            .debug_struct("RemotePhysicalOperationV1")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// A move-only completion issued by the trusted physical producer.
///
/// Its private non-zero-sized seal prevents downstream construction of completion evidence.
pub struct RemotePhysicalOperationResultV1 {
    seal: private::ResultSealV1,
}

impl fmt::Debug for RemotePhysicalOperationResultV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = self.seal.0;
        formatter
            .debug_struct("RemotePhysicalOperationResultV1")
            .finish_non_exhaustive()
    }
}

/// Terminal scheduler outcome of an opaque physical operation.
pub enum RemotePhysicalOperationOutcomeV1 {
    /// The trusted producer returned completion evidence.
    Completed(RemotePhysicalOperationResultV1),

    /// The trusted producer returned a redacted failure.
    Rejected(RemotePhysicalOperationErrorV1),
}

impl fmt::Debug for RemotePhysicalOperationOutcomeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed(result) => formatter.debug_tuple("Completed").field(result).finish(),
            Self::Rejected(err) => formatter.debug_tuple("Rejected").field(err).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(RemotePhysicalOperationV1: Clone, Copy);
    assert_not_impl_any!(RemotePhysicalOperationResultV1: Clone, Copy);
    assert_not_impl_any!(RemotePhysicalOperationOutcomeV1: Clone, Copy);

    #[test]
    fn contract_types_are_nonzero_and_cover_scheduler_outcomes() {
        assert!(std::mem::size_of::<RemotePhysicalOperationV1>() > 0);
        assert!(std::mem::size_of::<RemotePhysicalOperationResultV1>() > 0);

        let operation = RemotePhysicalOperationV1 {
            seal: private::OperationSealV1(1),
            state: RemotePhysicalOperationStateV1::Prepared,
        };
        assert_eq!(
            operation.state_v1(),
            RemotePhysicalOperationStateV1::Prepared
        );

        for state in [
            RemotePhysicalOperationStateV1::Prepared,
            RemotePhysicalOperationStateV1::Completed,
            RemotePhysicalOperationStateV1::Failed,
        ] {
            assert!(!format!("{state}").is_empty());
        }

        for err in [
            RemotePhysicalOperationErrorV1::Stale,
            RemotePhysicalOperationErrorV1::WrongProfile,
            RemotePhysicalOperationErrorV1::InvalidData,
            RemotePhysicalOperationErrorV1::ResourceLimit,
            RemotePhysicalOperationErrorV1::Unsupported,
        ] {
            assert!(!format!("{err}").is_empty());
        }

        let completed =
            RemotePhysicalOperationOutcomeV1::Completed(RemotePhysicalOperationResultV1 {
                seal: private::ResultSealV1(1),
            });
        let rejected =
            RemotePhysicalOperationOutcomeV1::Rejected(RemotePhysicalOperationErrorV1::Unsupported);
        assert!(matches!(
            completed,
            RemotePhysicalOperationOutcomeV1::Completed(_)
        ));
        assert!(matches!(
            rejected,
            RemotePhysicalOperationOutcomeV1::Rejected(RemotePhysicalOperationErrorV1::Unsupported)
        ));
    }

    #[test]
    fn production_contract_has_no_prohibited_dependencies_or_projections() {
        let source = include_str!("remote_physical_contract.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production contract precedes its tests");
        let lowercase = source.to_ascii_lowercase();

        for forbidden in [
            "http",
            "url",
            "query",
            "etag",
            "authorization",
            "decoder",
            "manifest",
            "dispatch",
            "runtime_intern",
            "store",
            "secret",
        ] {
            assert!(
                !lowercase.contains(forbidden),
                "contract contains prohibited dependency token: {forbidden}"
            );
        }

        for forbidden in [
            "pub fn new",
            "pub fn source_generation",
            "pub fn read_generation",
            "pub fn ordinal",
            "pub fn profile",
            "pub fn attempt",
            "pub fn range",
            "pub fn body",
            "pub fn lease",
            "pub fn cache",
            "pub fn token",
            "pub fn identity",
        ] {
            assert!(
                !lowercase.contains(forbidden),
                "contract exposes prohibited projection: {forbidden}"
            );
        }
    }
}
