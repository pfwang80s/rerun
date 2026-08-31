//! Stable opaque physical transport contracts shared by Web remote-MCAP adapters.
//!
//! This module intentionally contains no parser, manifest, decoder, URL, validator, or Store
//! implementation. The fields remain private so profile-specific owners cannot be reconstructed
//! from caller-provided scalars.

/// A checked half-open byte range for one remote physical read.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemotePhysicalRangeV1 {
    start: u64,
    end_exclusive: u64,
}

impl RemotePhysicalRangeV1 {
    /// Creates a non-empty checked range.
    pub(crate) const fn new(start: u64, end_exclusive: u64) -> Option<Self> {
        if start < end_exclusive {
            Some(Self {
                start,
                end_exclusive,
            })
        } else {
            None
        }
    }

    /// Returns the checked byte length.
    pub(crate) const fn len(self) -> u64 {
        self.end_exclusive - self.start
    }
}

/// Opaque source/read identity projection for a remote physical transport owner.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemotePhysicalReadIdentityV1 {
    source_generation: u64,
    read_generation: u64,
    canonical_ordinal: u32,
    range: RemotePhysicalRangeV1,
}

impl RemotePhysicalReadIdentityV1 {
    /// Creates a checked source/read identity projection.
    pub(crate) const fn new(
        source_generation: u64,
        read_generation: u64,
        canonical_ordinal: u32,
        range: RemotePhysicalRangeV1,
    ) -> Option<Self> {
        if source_generation == 0 || read_generation == 0 {
            return None;
        }
        Some(Self {
            source_generation,
            read_generation,
            canonical_ordinal,
            range,
        })
    }

    /// Returns the source generation.
    pub(crate) const fn source_generation(self) -> u64 {
        self.source_generation
    }

    /// Returns the read generation.
    pub(crate) const fn read_generation(self) -> u64 {
        self.read_generation
    }

    /// Returns the canonical physical ordinal.
    pub(crate) const fn canonical_ordinal(self) -> u32 {
        self.canonical_ordinal
    }

    /// Returns the exact checked byte range.
    pub(crate) const fn range(self) -> RemotePhysicalRangeV1 {
        self.range
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_identity_requires_nonzero_generations_and_preserves_range() {
        let range = RemotePhysicalRangeV1::new(4, 12).expect("non-empty range");
        assert!(RemotePhysicalRangeV1::new(12, 12).is_none());
        assert!(RemotePhysicalRangeV1::new(13, 12).is_none());
        assert!(RemotePhysicalReadIdentityV1::new(0, 1, 0, range).is_none());
        assert!(RemotePhysicalReadIdentityV1::new(1, 0, 0, range).is_none());
        let identity = RemotePhysicalReadIdentityV1::new(1, 2, 3, range).expect("identity");
        assert_eq!(identity.source_generation(), 1);
        assert_eq!(identity.read_generation(), 2);
        assert_eq!(identity.canonical_ordinal(), 3);
        assert_eq!(identity.range(), range);
        assert_eq!(identity.range().len(), 8);
    }
}
