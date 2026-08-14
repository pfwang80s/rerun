//! Immutable remote-MCAP manifest and partition/root identity authority.
//!
//! This module is production-disarmed. It deliberately performs no payload decoding or Store
//! publication; it only freezes identities and bounded registration metadata for later decode.

#![allow(dead_code)]

use re_chunk::ChunkId;

use crate::remote_channel_group::{ImmutableRemoteChannelGroupsV1, StableDecoderGroupIdV1};
use crate::remote_physical_resolution::ResolvedRemotePhysicalSourceRefV1;

const MANIFEST_VERSION_V1: u16 = 1;
const MAX_MANIFEST_RETAINED_BYTES_V1: u64 = 64 * 1024 * 1024;
const OPENING_STATIC_MAX_EXTERNAL_ORIGIN_BYTES_V1: u64 = 128;

fn partition_count_v1(units: usize, groups: usize) -> Result<usize, RemoteManifestErrorV1> {
    units
        .checked_mul(groups)
        .and_then(|count| count.checked_add(1))
        .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RemoteMcapSessionIdV1(re_tuid::Tuid);

impl RemoteMcapSessionIdV1 {
    fn fresh() -> Self {
        Self(re_tuid::Tuid::new())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SourceUnitIdV1 {
    session: RemoteMcapSessionIdV1,
    source_generation: u64,
    ordinal: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DerivationPartitionKindV1 {
    TemporalChannelGroup(StableDecoderGroupIdV1),
    OpeningStatic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DerivationPartitionKeyV1 {
    source_unit: SourceUnitIdV1,
    kind: DerivationPartitionKindV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ManifestRootDescriptorV1 {
    key: DerivationPartitionKeyV1,
    output_ordinal: u32,
    root_chunk_id: ChunkId,
}

impl ManifestRootDescriptorV1 {
    pub(crate) const fn root_chunk_id_v1(self) -> ChunkId {
        self.root_chunk_id
    }

    pub(crate) const fn output_ordinal_v1(self) -> u32 {
        self.output_ordinal
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ManifestPartitionDescriptorV1 {
    key: DerivationPartitionKeyV1,
    registration_bound: u32,
    root_namespace: u16,
}

impl ManifestPartitionDescriptorV1 {
    pub(crate) const fn source_generation_v1(self) -> u64 {
        self.key.source_unit.source_generation
    }
}

impl ManifestPartitionDescriptorV1 {
    pub(crate) const fn key_v1(self) -> DerivationPartitionKeyV1 {
        self.key
    }

    pub(crate) const fn registration_bound_v1(self) -> u32 {
        self.registration_bound
    }

    pub(crate) fn identity_bytes_v1(self) -> [u8; 24] {
        let mut bytes = [0_u8; 24];
        bytes[..8].copy_from_slice(&self.key.source_unit.source_generation.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.key.source_unit.ordinal.to_le_bytes());
        bytes[12..20].copy_from_slice(&self.root_namespace.to_le_bytes().repeat(4));
        let kind = match self.key.kind {
            DerivationPartitionKindV1::TemporalChannelGroup(group) => group.as_u32(),
            DerivationPartitionKindV1::OpeningStatic => u32::MAX,
        };
        bytes[20..24].copy_from_slice(&kind.to_le_bytes());
        bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteManifestErrorV1 {
    ArithmeticOverflow,
    ResourceLimitExceeded,
    StaleSource,
    InvalidPartition,
}

pub(crate) struct ManifestRootDescriptorIssuerV1 {
    key: DerivationPartitionKeyV1,
    registration_bound: u32,
    namespace: u16,
}

impl ManifestRootDescriptorIssuerV1 {
    pub(crate) fn descriptor_v1(
        &self,
        output_ordinal: u32,
    ) -> Result<ManifestRootDescriptorV1, RemoteManifestErrorV1> {
        if output_ordinal >= self.registration_bound {
            return Err(RemoteManifestErrorV1::InvalidPartition);
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"rerun.remote-mcap.root.v1");
        hasher.update(&MANIFEST_VERSION_V1.to_le_bytes());
        hasher.update(&self.namespace.to_le_bytes());
        hasher.update(&self.key.source_unit.session.0.as_bytes());
        hasher.update(&self.key.source_unit.source_generation.to_le_bytes());
        hasher.update(&self.key.source_unit.ordinal.to_le_bytes());
        match self.key.kind {
            DerivationPartitionKindV1::TemporalChannelGroup(group) => {
                hasher.update(&[0]);
                hasher.update(&group.as_u32().to_le_bytes());
            }
            DerivationPartitionKindV1::OpeningStatic => {
                hasher.update(&[1]);
            }
        }
        hasher.update(&output_ordinal.to_le_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest.as_bytes()[..16]);
        Ok(ManifestRootDescriptorV1 {
            key: self.key,
            output_ordinal,
            root_chunk_id: ChunkId::from_tuid(re_tuid::Tuid::from_u128(u128::from_be_bytes(bytes))),
        })
    }
}

pub(crate) struct ImmutableRemoteMcapManifestV1<'a, 'd, 'i, 's, 'w> {
    source: ResolvedRemotePhysicalSourceRefV1<'a, 'i>,
    groups: ImmutableRemoteChannelGroupsV1<'d, 'i, 's, 'w>,
    session_id: RemoteMcapSessionIdV1,
    partitions: Box<[ManifestPartitionDescriptorV1]>,
    issuers: Box<[ManifestRootDescriptorIssuerV1]>,
}

/// Opaque authority for one temporal source-unit/group partition.
///
/// This ties the selected group and its executable factories to the matching partition and root
/// issuer; callers cannot construct or rebind those pieces independently.
pub(crate) struct ManifestTemporalPartitionAuthorityV1<'manifest, 'a, 'd, 'i, 's, 'w> {
    manifest: &'manifest ImmutableRemoteMcapManifestV1<'a, 'd, 'i, 's, 'w>,
    source_unit_ordinal: u32,
    group_id: StableDecoderGroupIdV1,
    partition: ManifestPartitionDescriptorV1,
    issuer: &'manifest ManifestRootDescriptorIssuerV1,
}

impl<'a, 'd, 'i, 's, 'w> ManifestTemporalPartitionAuthorityV1<'_, 'a, 'd, 'i, 's, 'w> {
    pub(crate) const fn source_unit_ordinal_v1(&self) -> u32 {
        self.source_unit_ordinal
    }

    pub(crate) const fn group_id_v1(&self) -> StableDecoderGroupIdV1 {
        self.group_id
    }

    pub(crate) fn session_identity_v1(&self) -> u128 {
        self.manifest.session_identity_v1()
    }

    pub(crate) const fn partition_v1(&self) -> ManifestPartitionDescriptorV1 {
        self.partition
    }

    pub(crate) fn issue_root_v1(
        &self,
        output_ordinal: u32,
    ) -> Result<ManifestRootDescriptorV1, RemoteManifestErrorV1> {
        self.issuer.descriptor_v1(output_ordinal)
    }

    pub(crate) fn groups_v1(&self) -> &ImmutableRemoteChannelGroupsV1<'d, 'i, 's, 'w> {
        &self.manifest.groups
    }

    pub(crate) fn channels_v1(&self) -> &[u16] {
        self.manifest
            .groups
            .channels_for_group_v1(self.group_id)
            .expect("manifest authority retains its resolved group")
    }

    pub(crate) fn bind_executable_factory_v1(
        &self,
        channel_id: u16,
    ) -> Result<
        crate::remote_protobuf_descriptor::RemoteExecutableFactoryV1<'_, '_, '_, '_, '_>,
        crate::remote_protobuf_descriptor::RemoteExecutableAdapterErrorV1,
    > {
        if !self.channels_v1().contains(&channel_id) {
            return Err(
                crate::remote_protobuf_descriptor::RemoteExecutableAdapterErrorV1::ConfigMismatch,
            );
        }
        self.manifest.groups.bind_executable_factory_v1(channel_id)
    }

    pub(crate) fn ensure_current_v1(&self) -> Result<(), RemoteManifestErrorV1> {
        self.manifest
            .groups
            .ensure_current_for_validation_v1()
            .map_err(|_| RemoteManifestErrorV1::StaleSource)
    }

    pub(crate) fn matches_group_v1(&self, group_id: StableDecoderGroupIdV1) -> bool {
        self.group_id == group_id
    }
}

impl<'a, 'd, 'i, 's, 'w> ImmutableRemoteMcapManifestV1<'a, 'd, 'i, 's, 'w> {
    pub(crate) fn build_v1(
        source: ResolvedRemotePhysicalSourceRefV1<'a, 'i>,
        groups: ImmutableRemoteChannelGroupsV1<'d, 'i, 's, 'w>,
        max_partitions: u32,
    ) -> Result<Self, RemoteManifestErrorV1> {
        let session_id = RemoteMcapSessionIdV1::fresh();
        Self::build_with_session_v1(source, groups, session_id, max_partitions)
    }

    #[cfg(test)]
    pub(crate) fn build_with_session_v1(
        source: ResolvedRemotePhysicalSourceRefV1<'a, 'i>,
        groups: ImmutableRemoteChannelGroupsV1<'d, 'i, 's, 'w>,
        session_id: RemoteMcapSessionIdV1,
        max_partitions: u32,
    ) -> Result<Self, RemoteManifestErrorV1> {
        groups
            .ensure_current_for_validation_v1()
            .map_err(|_| RemoteManifestErrorV1::StaleSource)?;
        let units = source.layout_v1().canonical_chunk_count_v1();
        let source_generation = source.source_generation_v1();
        let group_count = groups.groups_v1().len();
        let count = partition_count_v1(units, group_count)?;
        if count
            > usize::try_from(max_partitions)
                .map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?
        {
            return Err(RemoteManifestErrorV1::ResourceLimitExceeded);
        }
        // Perform the complete metadata census before allocating either manifest vector.  The
        // estimate intentionally includes empty-entry descriptors, root issuers, external-origin
        // headroom, the transferred group owner and vector backing storage.
        let empty_entries = u64::try_from(units)
            .map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?
            .checked_mul(
                u64::try_from(group_count)
                    .map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?,
            )
            .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
        let mut external_origin_bytes = 0_u64;
        for group in groups.groups_v1() {
            let per_partition = group
                .registration_bounds_v1()
                .max_external_origin_bytes_per_partition_v1();
            external_origin_bytes = external_origin_bytes
                .checked_add(
                    per_partition
                        .checked_mul(
                            u64::try_from(units)
                                .map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?,
                        )
                        .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?,
                )
                .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
        }
        external_origin_bytes = external_origin_bytes
            .checked_add(OPENING_STATIC_MAX_EXTERNAL_ORIGIN_BYTES_V1)
            .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
        let descriptor_bytes = u64::try_from(std::mem::size_of::<ManifestPartitionDescriptorV1>())
            .map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?
            .checked_mul(
                u64::try_from(count).map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?,
            )
            .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
        let issuer_bytes = u64::try_from(std::mem::size_of::<ManifestRootDescriptorIssuerV1>())
            .map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?
            .checked_mul(
                u64::try_from(count).map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?,
            )
            .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
        let retained_bytes = groups
            .retained_bytes_for_validation_v1()
            .checked_add(descriptor_bytes)
            .and_then(|value| value.checked_add(issuer_bytes))
            .and_then(|value| value.checked_add(external_origin_bytes))
            .and_then(|value| value.checked_add(empty_entries))
            .ok_or(RemoteManifestErrorV1::ArithmeticOverflow)?;
        if retained_bytes > MAX_MANIFEST_RETAINED_BYTES_V1 {
            return Err(RemoteManifestErrorV1::ResourceLimitExceeded);
        }

        let mut partitions = Vec::with_capacity(count);
        let mut issuers = Vec::with_capacity(count);
        let opening_key = DerivationPartitionKeyV1 {
            source_unit: SourceUnitIdV1 {
                session: session_id,
                source_generation,
                ordinal: u32::MAX,
            },
            kind: DerivationPartitionKindV1::OpeningStatic,
        };
        partitions.push(ManifestPartitionDescriptorV1 {
            key: opening_key,
            registration_bound: 1,
            root_namespace: 1,
        });
        issuers.push(ManifestRootDescriptorIssuerV1 {
            key: opening_key,
            registration_bound: 1,
            namespace: 1,
        });
        for ordinal in 0..units {
            let source_unit = SourceUnitIdV1 {
                session: session_id,
                source_generation,
                ordinal: u32::try_from(ordinal)
                    .map_err(|_| RemoteManifestErrorV1::ArithmeticOverflow)?,
            };
            for group in groups.groups_v1() {
                let kind = DerivationPartitionKindV1::TemporalChannelGroup(group.group_id());
                let bound = groups
                    .resolve_group(group.group_id())
                    .map(|g| {
                        g.descriptor()
                            .registration_bounds_v1()
                            .max_roots_per_partition_v1()
                    })
                    .ok_or(RemoteManifestErrorV1::InvalidPartition)?;
                let key = DerivationPartitionKeyV1 { source_unit, kind };
                partitions.push(ManifestPartitionDescriptorV1 {
                    key,
                    registration_bound: bound,
                    root_namespace: 1,
                });
                issuers.push(ManifestRootDescriptorIssuerV1 {
                    key,
                    registration_bound: bound,
                    namespace: 1,
                });
            }
        }
        Ok(Self {
            source,
            groups,
            session_id,
            partitions: partitions.into_boxed_slice(),
            issuers: issuers.into_boxed_slice(),
        })
    }

    pub(crate) fn partition_v1(&self, index: usize) -> Option<ManifestPartitionDescriptorV1> {
        self.partitions.get(index).copied()
    }
    pub(crate) fn issuer_v1(&self, index: usize) -> Option<&ManifestRootDescriptorIssuerV1> {
        self.issuers.get(index)
    }
    pub(crate) fn session_id_v1(&self) -> RemoteMcapSessionIdV1 {
        self.session_id
    }

    pub(crate) fn session_identity_v1(&self) -> u128 {
        self.session_id.0.as_u128()
    }
    pub(crate) fn source_generation_v1(&self) -> u64 {
        self.source.source_generation_v1()
    }

    pub(crate) fn temporal_partition_v1(
        &self,
        source_unit_ordinal: u32,
        group_id: StableDecoderGroupIdV1,
    ) -> Result<ManifestTemporalPartitionAuthorityV1<'_, 'a, 'd, 'i, 's, 'w>, RemoteManifestErrorV1>
    {
        let key = DerivationPartitionKeyV1 {
            source_unit: SourceUnitIdV1 {
                session: self.session_id,
                source_generation: self.source.source_generation_v1(),
                ordinal: source_unit_ordinal,
            },
            kind: DerivationPartitionKindV1::TemporalChannelGroup(group_id),
        };
        let partition = self
            .partitions
            .iter()
            .find(|descriptor| descriptor.key == key)
            .ok_or(RemoteManifestErrorV1::InvalidPartition)?;
        let issuer = self
            .issuers
            .iter()
            .find(|issuer| issuer.key == key)
            .ok_or(RemoteManifestErrorV1::InvalidPartition)?;
        if self.groups.resolve_group(group_id).is_none() {
            return Err(RemoteManifestErrorV1::InvalidPartition);
        }
        Ok(ManifestTemporalPartitionAuthorityV1 {
            manifest: self,
            source_unit_ordinal,
            group_id,
            partition: *partition,
            issuer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_identity_is_stable_and_domain_separated() {
        let key = DerivationPartitionKeyV1 {
            source_unit: SourceUnitIdV1 {
                session: RemoteMcapSessionIdV1(re_tuid::Tuid::from_u128(7)),
                source_generation: 11,
                ordinal: 3,
            },
            kind: DerivationPartitionKindV1::OpeningStatic,
        };
        let issuer = ManifestRootDescriptorIssuerV1 {
            key,
            registration_bound: 2,
            namespace: 1,
        };
        let a = issuer.descriptor_v1(0).unwrap().root_chunk_id_v1();
        let b = issuer.descriptor_v1(0).unwrap().root_chunk_id_v1();
        assert_eq!(a, b);
        assert_ne!(a, issuer.descriptor_v1(1).unwrap().root_chunk_id_v1());

        let other_generation = ManifestRootDescriptorIssuerV1 {
            key: DerivationPartitionKeyV1 {
                source_unit: SourceUnitIdV1 {
                    session: RemoteMcapSessionIdV1(re_tuid::Tuid::from_u128(7)),
                    source_generation: 12,
                    ordinal: 3,
                },
                kind: DerivationPartitionKindV1::OpeningStatic,
            },
            registration_bound: 2,
            namespace: 1,
        };
        assert_ne!(
            a,
            other_generation
                .descriptor_v1(0)
                .unwrap()
                .root_chunk_id_v1()
        );

        let other_session = ManifestRootDescriptorIssuerV1 {
            key: DerivationPartitionKeyV1 {
                source_unit: SourceUnitIdV1 {
                    session: RemoteMcapSessionIdV1(re_tuid::Tuid::from_u128(8)),
                    source_generation: 11,
                    ordinal: 3,
                },
                kind: DerivationPartitionKindV1::OpeningStatic,
            },
            registration_bound: 2,
            namespace: 1,
        };
        assert_ne!(
            a,
            other_session.descriptor_v1(0).unwrap().root_chunk_id_v1()
        );
        let temporal = ManifestRootDescriptorIssuerV1 {
            key: DerivationPartitionKeyV1 {
                source_unit: key.source_unit,
                kind: DerivationPartitionKindV1::TemporalChannelGroup(
                    StableDecoderGroupIdV1::first_for_assignment_test_v1(),
                ),
            },
            registration_bound: 2,
            namespace: 1,
        };
        assert_ne!(a, temporal.descriptor_v1(0).unwrap().root_chunk_id_v1());
        assert!(matches!(
            issuer.descriptor_v1(2),
            Err(RemoteManifestErrorV1::InvalidPartition)
        ));
    }

    #[test]
    fn checked_partition_count_covers_empty_and_overflow() {
        assert_eq!(partition_count_v1(0, 0).unwrap(), 1);
        assert_eq!(partition_count_v1(2, 3).unwrap(), 7);
        assert_eq!(
            partition_count_v1(usize::MAX, 2),
            Err(RemoteManifestErrorV1::ArithmeticOverflow)
        );
    }
}
