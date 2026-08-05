//! Checked time canonicalization for Web remote-MCAP inputs.
//!
//! This module is not connected to native/local MCAP import or Viewer time handling.

use re_log_types::{TimeInt, TimeType};

/// An unconverted nanosecond value read from an MCAP record or index.
///
/// The raw value deliberately has no integer getter or integer conversion implementation.
/// Remote code must use [`canonicalize_raw_mcap_time`] before constructing temporal data.
///
/// ```compile_fail
/// use re_mcap::remote_time::RawMcapTime;
///
/// let raw: u64 = RawMcapTime::new(42).into();
/// # let _ = raw;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RawMcapTime(u64);

impl RawMcapTime {
    /// Wraps one raw MCAP `u64` without changing its value.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }
}

/// A raw MCAP value that is outside Rerun's non-static temporal domain.
///
/// The rejected value is intentionally not retained or exposed by this error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTemporalValue;

impl std::fmt::Display for InvalidTemporalValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("raw MCAP time is outside the valid temporal domain")
    }
}

impl std::error::Error for InvalidTemporalValue {}

/// Converts raw MCAP nanoseconds into the canonical Rerun temporal representation.
///
/// This is the only raw-time conversion entry point for the Web remote-MCAP path.
/// It applies a zero offset and performs no saturation, epoch inference, or unit adjustment.
pub fn canonicalize_raw_mcap_time(raw: RawMcapTime) -> Result<TimeInt, InvalidTemporalValue> {
    let signed = i64::try_from(raw.0).map_err(|_overflow| InvalidTemporalValue)?;
    TimeInt::try_from(signed).map_err(|_static_sentinel| InvalidTemporalValue)
}

/// The two timeline interpretations accepted by remote-MCAP open options.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoteMcapTimeType {
    /// Nanoseconds since the Unix epoch.
    #[default]
    TimestampNs,

    /// A nanosecond duration with no epoch interpretation.
    DurationNs,
}

impl From<RemoteMcapTimeType> for TimeType {
    fn from(value: RemoteMcapTimeType) -> Self {
        match value {
            RemoteMcapTimeType::TimestampNs => Self::TimestampNs,
            RemoteMcapTimeType::DurationNs => Self::DurationNs,
        }
    }
}

/// A generic timeline type that remote-MCAP open options do not support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedMcapTimeType;

impl std::fmt::Display for UnsupportedMcapTimeType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("remote MCAP time type must be timestamp_ns or duration_ns")
    }
}

impl std::error::Error for UnsupportedMcapTimeType {}

impl TryFrom<TimeType> for RemoteMcapTimeType {
    type Error = UnsupportedMcapTimeType;

    fn try_from(value: TimeType) -> Result<Self, Self::Error> {
        match value {
            TimeType::TimestampNs => Ok(Self::TimestampNs),
            TimeType::DurationNs => Ok(Self::DurationNs),
            TimeType::Sequence => Err(UnsupportedMcapTimeType),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalization_preserves_every_boundary_and_rejects_only_the_upper_half() {
        for raw in [0, 1, i64::MAX as u64 - 1, i64::MAX as u64] {
            let canonical = canonicalize_raw_mcap_time(RawMcapTime::new(raw)).unwrap();
            assert!(!canonical.is_static());
            assert_eq!(
                canonical.as_i64(),
                i64::try_from(raw).expect("boundary is within the canonical domain")
            );
        }

        for raw in [i64::MAX as u64 + 1, u64::MAX] {
            assert_eq!(
                canonicalize_raw_mcap_time(RawMcapTime::new(raw)),
                Err(InvalidTemporalValue)
            );
        }
    }

    #[test]
    fn i64_max_is_temporal_and_the_static_sentinel_cannot_enter_from_raw_u64() {
        let maximum = canonicalize_raw_mcap_time(RawMcapTime::new(i64::MAX as u64)).unwrap();
        assert_eq!(maximum, TimeInt::MAX);
        assert_ne!(maximum, TimeInt::STATIC);

        assert_eq!(
            TimeInt::try_from(i64::MIN),
            Err(re_log_types::TryFromIntError)
        );
        assert_eq!(
            canonicalize_raw_mcap_time(RawMcapTime::new(i64::MIN as u64)),
            Err(InvalidTemporalValue)
        );
    }

    #[test]
    fn boot_relative_and_epoch_like_values_never_trigger_heuristics_or_offsets() {
        for raw in [
            0,
            42,
            1_000_000_000,
            86_400_000_000_000,
            1_700_000_000_000_000_000,
        ] {
            assert_eq!(
                canonicalize_raw_mcap_time(RawMcapTime::new(raw))
                    .unwrap()
                    .as_i64(),
                i64::try_from(raw).expect("fixture value is within the canonical domain")
            );
        }
    }

    #[test]
    fn remote_time_type_defaults_to_timestamp_and_statically_excludes_sequence() {
        assert_eq!(
            RemoteMcapTimeType::default(),
            RemoteMcapTimeType::TimestampNs
        );
        assert_eq!(
            RemoteMcapTimeType::try_from(TimeType::TimestampNs),
            Ok(RemoteMcapTimeType::TimestampNs)
        );
        assert_eq!(
            RemoteMcapTimeType::try_from(TimeType::DurationNs),
            Ok(RemoteMcapTimeType::DurationNs)
        );
        assert_eq!(
            RemoteMcapTimeType::try_from(TimeType::Sequence),
            Err(UnsupportedMcapTimeType)
        );
        assert_eq!(
            TimeType::from(RemoteMcapTimeType::TimestampNs),
            TimeType::TimestampNs
        );
        assert_eq!(
            TimeType::from(RemoteMcapTimeType::DurationNs),
            TimeType::DurationNs
        );
    }

    #[test]
    fn timestamp_and_duration_interpretations_share_the_exact_canonical_number() {
        for raw in [0, 42, 9_876_543_210, i64::MAX as u64] {
            let canonical = canonicalize_raw_mcap_time(RawMcapTime::new(raw)).unwrap();
            let timestamp = (TimeType::from(RemoteMcapTimeType::TimestampNs), canonical);
            let duration = (TimeType::from(RemoteMcapTimeType::DurationNs), canonical);
            assert_eq!(timestamp.0, TimeType::TimestampNs);
            assert_eq!(duration.0, TimeType::DurationNs);
            assert_eq!(timestamp.1, duration.1);
            assert_eq!(
                timestamp.1.as_i64(),
                i64::try_from(raw).expect("fixture value is within the canonical domain")
            );
        }
    }

    #[test]
    fn deterministic_property_samples_are_exact_and_never_saturate() {
        let mut state = 0x6a09_e667_f3bc_c909_u64;
        for _ in 0..100_000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);

            let valid = state & (i64::MAX as u64);
            assert_eq!(
                canonicalize_raw_mcap_time(RawMcapTime::new(valid))
                    .unwrap()
                    .as_i64(),
                i64::try_from(valid).expect("the high-bit mask creates a valid value")
            );

            let invalid = state | (1_u64 << 63);
            assert_eq!(
                canonicalize_raw_mcap_time(RawMcapTime::new(invalid)),
                Err(InvalidTemporalValue)
            );
        }
    }
}
