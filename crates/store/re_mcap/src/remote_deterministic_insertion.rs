//! Deterministic, bounded pre-splitting for Web remote-MCAP Store insertion.

use std::ops::Range;

use re_byte_size::SizeBytes as _;
use re_chunk::Chunk;

pub(crate) const REMOTE_DERIVED_INSERTION_PROFILE_VERSION_V1: u16 = 1;
pub(crate) const MAX_REMOTE_DERIVED_ROOTS_PER_CHANNEL_V1: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteDerivedChunkProfileV1 {
    version: u16,
    max_rows_per_root: u64,
    max_physical_bytes_per_root: u64,
    max_output_physical_bytes_per_partition: u64,
    max_components_per_root: u32,
    max_timelines_per_root: u32,
    max_roots_per_channel: u32,
}

impl RemoteDerivedChunkProfileV1 {
    pub(crate) const fn phase_a_candidate_v1() -> Self {
        Self {
            version: REMOTE_DERIVED_INSERTION_PROFILE_VERSION_V1,
            max_rows_per_root: 1_000_000,
            max_physical_bytes_per_root: u64::MAX,
            max_output_physical_bytes_per_partition: u64::MAX,
            max_components_per_root: u32::MAX,
            max_timelines_per_root: u32::MAX,
            max_roots_per_channel: MAX_REMOTE_DERIVED_ROOTS_PER_CHANNEL_V1,
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test_v1(
        max_rows_per_root: u64,
        max_physical_bytes_per_root: u64,
        max_output_physical_bytes_per_partition: u64,
        max_components_per_root: u32,
        max_timelines_per_root: u32,
        max_roots_per_channel: u32,
    ) -> Self {
        Self {
            version: REMOTE_DERIVED_INSERTION_PROFILE_VERSION_V1,
            max_rows_per_root,
            max_physical_bytes_per_root,
            max_output_physical_bytes_per_partition,
            max_components_per_root,
            max_timelines_per_root,
            max_roots_per_channel,
        }
    }

    pub(crate) const fn registration_contract_v1(self) -> (u32, u64) {
        (
            self.max_roots_per_channel,
            self.max_roots_per_channel as u64 * 128,
        )
    }

    pub(crate) fn partition_limits_v1(
        self,
        channel_count: u32,
    ) -> Result<RemoteDerivedChunkLimitsV1, RemoteDerivedChunkResourceLimitV1> {
        let max_roots_per_partition = self
            .max_roots_per_channel
            .checked_mul(channel_count)
            .ok_or(RemoteDerivedChunkResourceLimitV1::Arithmetic)?;
        Ok(RemoteDerivedChunkLimitsV1::new_v1(
            self.max_rows_per_root,
            self.max_physical_bytes_per_root,
            self.max_output_physical_bytes_per_partition,
            self.max_components_per_root,
            self.max_timelines_per_root,
            max_roots_per_partition,
        ))
    }

    pub(crate) fn identity_digest_v1(self) -> [u8; 16] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"rerun.remote-mcap.derived-insertion-profile.v1");
        hasher.update(&self.version.to_le_bytes());
        hasher.update(&self.max_rows_per_root.to_le_bytes());
        hasher.update(&self.max_physical_bytes_per_root.to_le_bytes());
        hasher.update(&self.max_output_physical_bytes_per_partition.to_le_bytes());
        hasher.update(&self.max_components_per_root.to_le_bytes());
        hasher.update(&self.max_timelines_per_root.to_le_bytes());
        hasher.update(&self.max_roots_per_channel.to_le_bytes());
        let hash = hasher.finalize();
        let mut digest = [0; 16];
        digest.copy_from_slice(&hash.as_bytes()[..16]);
        digest
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteDerivedChunkLimitsV1 {
    profile_version: u16,
    max_rows_per_root: u64,
    max_physical_bytes_per_root: u64,
    max_output_physical_bytes_per_partition: u64,
    max_components_per_root: u32,
    max_timelines_per_root: u32,
    max_roots_per_partition: u32,
}

impl RemoteDerivedChunkLimitsV1 {
    pub(crate) const fn new_v1(
        max_rows_per_root: u64,
        max_physical_bytes_per_root: u64,
        max_output_physical_bytes_per_partition: u64,
        max_components_per_root: u32,
        max_timelines_per_root: u32,
        max_roots_per_partition: u32,
    ) -> Self {
        Self {
            profile_version: REMOTE_DERIVED_INSERTION_PROFILE_VERSION_V1,
            max_rows_per_root,
            max_physical_bytes_per_root,
            max_output_physical_bytes_per_partition,
            max_components_per_root,
            max_timelines_per_root,
            max_roots_per_partition,
        }
    }

    pub(crate) const fn max_roots_per_partition_v1(self) -> u32 {
        self.max_roots_per_partition
    }

    pub(crate) const fn max_rows_per_root_v1(self) -> u64 {
        self.max_rows_per_root
    }

    pub(crate) const fn max_output_physical_bytes_per_partition_v1(self) -> u64 {
        self.max_output_physical_bytes_per_partition
    }

    pub(crate) const fn with_partition_remainder_v1(
        mut self,
        max_roots_per_partition: u32,
        max_output_physical_bytes_per_partition: u64,
    ) -> Self {
        self.max_roots_per_partition = max_roots_per_partition;
        self.max_output_physical_bytes_per_partition = max_output_physical_bytes_per_partition;
        self
    }

    pub(crate) fn validate_prebuilt_root_v1(
        self,
        chunk: &Chunk,
    ) -> Result<(), RemoteDerivedChunkResourceLimitV1> {
        if chunk.is_empty() || chunk.is_static() {
            return Err(RemoteDerivedChunkResourceLimitV1::BuilderContract);
        }
        let rows = u64::try_from(chunk.num_rows())
            .map_err(|_error| RemoteDerivedChunkResourceLimitV1::Arithmetic)?;
        if rows > self.max_rows_per_root
            || chunk.total_size_bytes() > self.max_physical_bytes_per_root
        {
            return Err(RemoteDerivedChunkResourceLimitV1::DeepSplitUnavailable);
        }
        let components = u32::try_from(chunk.components().len())
            .map_err(|_error| RemoteDerivedChunkResourceLimitV1::Arithmetic)?;
        if components > self.max_components_per_root {
            return Err(RemoteDerivedChunkResourceLimitV1::ComponentsPerRoot);
        }
        let timelines = u32::try_from(chunk.timelines().len())
            .map_err(|_error| RemoteDerivedChunkResourceLimitV1::Arithmetic)?;
        if timelines > self.max_timelines_per_root {
            return Err(RemoteDerivedChunkResourceLimitV1::TimelinesPerRoot);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteDerivedChunkResourceLimitV1 {
    InvalidProfile,
    Arithmetic,
    PhysicalBytesPerRoot,
    OutputPhysicalBytes,
    ComponentsPerRoot,
    TimelinesPerRoot,
    RootsPerPartition,
    BuilderContract,
    DeepSplitUnavailable,
    FallibleAllocation,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoteDerivedChunkPreparationErrorV1<E> {
    ResourceLimitExceeded(RemoteDerivedChunkResourceLimitV1),
    Build(E),
}

struct DerivedChunkPreparationV1<'limits, F, E>
where
    F: FnMut(Range<usize>) -> Result<Chunk, E>,
{
    limits: &'limits RemoteDerivedChunkLimitsV1,
    build: F,
    chunks: Vec<Chunk>,
    output_physical_bytes: u64,
}

impl<F, E> DerivedChunkPreparationV1<'_, F, E>
where
    F: FnMut(Range<usize>) -> Result<Chunk, E>,
{
    fn prepare_range_v1(
        &mut self,
        range: Range<usize>,
    ) -> Result<(), RemoteDerivedChunkPreparationErrorV1<E>> {
        let expected_rows = range.end.checked_sub(range.start).ok_or(
            RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::Arithmetic,
            ),
        )?;
        if expected_rows == 0 {
            return Ok(());
        }
        if self.chunks.len()
            >= usize::try_from(self.limits.max_roots_per_partition).map_err(|_error| {
                RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                    RemoteDerivedChunkResourceLimitV1::Arithmetic,
                )
            })?
        {
            return Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::RootsPerPartition,
            ));
        }
        let chunk =
            (self.build)(range.clone()).map_err(RemoteDerivedChunkPreparationErrorV1::Build)?;
        if chunk.num_rows() != expected_rows || chunk.is_empty() || chunk.is_static() {
            return Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::BuilderContract,
            ));
        }
        if u32::try_from(chunk.components().len()).ok() > Some(self.limits.max_components_per_root)
        {
            return Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::ComponentsPerRoot,
            ));
        }
        if u32::try_from(chunk.timelines().len()).ok() > Some(self.limits.max_timelines_per_root) {
            return Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::TimelinesPerRoot,
            ));
        }
        let physical_bytes = chunk.total_size_bytes();
        if physical_bytes > self.limits.max_physical_bytes_per_root {
            if expected_rows == 1 {
                return Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                    RemoteDerivedChunkResourceLimitV1::PhysicalBytesPerRoot,
                ));
            }
            drop(chunk);
            let midpoint = range.start.checked_add(expected_rows / 2).ok_or(
                RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                    RemoteDerivedChunkResourceLimitV1::Arithmetic,
                ),
            )?;
            self.prepare_range_v1(range.start..midpoint)?;
            self.prepare_range_v1(midpoint..range.end)?;
            return Ok(());
        }
        self.output_physical_bytes = self
            .output_physical_bytes
            .checked_add(physical_bytes)
            .filter(|bytes| *bytes <= self.limits.max_output_physical_bytes_per_partition)
            .ok_or(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::OutputPhysicalBytes,
            ))?;
        self.chunks.push(chunk);
        Ok(())
    }
}

pub(crate) fn prepare_deterministic_derived_chunks_v1<E>(
    rows: u64,
    limits: &RemoteDerivedChunkLimitsV1,
    build: impl FnMut(Range<usize>) -> Result<Chunk, E>,
) -> Result<Vec<Chunk>, RemoteDerivedChunkPreparationErrorV1<E>> {
    if limits.profile_version != REMOTE_DERIVED_INSERTION_PROFILE_VERSION_V1
        || limits.max_rows_per_root == 0
        || limits.max_physical_bytes_per_root == 0
        || limits.max_output_physical_bytes_per_partition == 0
        || limits.max_components_per_root == 0
        || limits.max_timelines_per_root == 0
        || limits.max_roots_per_partition == 0
    {
        return Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
            RemoteDerivedChunkResourceLimitV1::InvalidProfile,
        ));
    }
    let rows = usize::try_from(rows).map_err(|_error| {
        RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
            RemoteDerivedChunkResourceLimitV1::Arithmetic,
        )
    })?;
    if rows == 0 {
        return Ok(Vec::new());
    }
    let max_rows = usize::try_from(limits.max_rows_per_root).map_err(|_error| {
        RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
            RemoteDerivedChunkResourceLimitV1::Arithmetic,
        )
    })?;
    let initial_roots = rows
        .checked_add(max_rows - 1)
        .and_then(|rows| rows.checked_div(max_rows))
        .ok_or(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
            RemoteDerivedChunkResourceLimitV1::Arithmetic,
        ))?;
    if initial_roots
        > usize::try_from(limits.max_roots_per_partition).map_err(|_error| {
            RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::Arithmetic,
            )
        })?
    {
        return Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
            RemoteDerivedChunkResourceLimitV1::RootsPerPartition,
        ));
    }
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(
            usize::try_from(limits.max_roots_per_partition).map_err(|_error| {
                RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                    RemoteDerivedChunkResourceLimitV1::Arithmetic,
                )
            })?,
        )
        .map_err(|_error| {
            RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::FallibleAllocation,
            )
        })?;
    let mut preparation = DerivedChunkPreparationV1 {
        limits,
        build,
        chunks,
        output_physical_bytes: 0,
    };
    let mut start = 0;
    while start < rows {
        let end = start.saturating_add(max_rows).min(rows);
        preparation.prepare_range_v1(start..end)?;
        start = end;
    }
    Ok(preparation.chunks)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use re_chunk::{Chunk, ChunkId, RowId};
    use re_log_types::Timeline;
    use re_sdk_types::archetypes;

    use super::*;

    fn source_chunk(rows: usize) -> Chunk {
        let mut builder = Chunk::builder_with_id(ChunkId::new(), "world/points");
        for row in 0..rows {
            builder = builder.with_archetype(
                RowId::new(),
                [(Timeline::log_tick(), i64::try_from(row).unwrap())],
                &archetypes::Points3D::new([[row as f32, 0.0, 0.0]]),
            );
        }
        builder.build().unwrap()
    }

    fn multi_cardinality_chunk() -> Chunk {
        Chunk::builder_with_id(ChunkId::new(), "world/points")
            .with_archetype(
                RowId::new(),
                [(Timeline::log_tick(), 1), (Timeline::log_time(), 2)],
                &archetypes::Points3D::new([[1.0, 2.0, 3.0]]).with_colors([0xFF00FFFF]),
            )
            .build()
            .unwrap()
    }

    #[test]
    fn row_and_physical_caps_split_deterministically() {
        let source = source_chunk(8);
        let two_rows = source.row_sliced_deep(0, 2).total_size_bytes();
        let limits = RemoteDerivedChunkLimitsV1::new_v1(4, two_rows, u64::MAX, 1, 1, 8);
        let build =
            |range: Range<usize>| Ok::<_, ()>(source.row_sliced_deep(range.start, range.len()));
        let first = prepare_deterministic_derived_chunks_v1(8, &limits, build).unwrap();
        let second = prepare_deterministic_derived_chunks_v1(8, &limits, |range| {
            Ok::<_, ()>(source.row_sliced_deep(range.start, range.len()))
        })
        .unwrap();
        assert_eq!(
            first.iter().map(Chunk::num_rows).collect::<Vec<_>>(),
            second.iter().map(Chunk::num_rows).collect::<Vec<_>>()
        );
        assert!(first.iter().all(|chunk| chunk.num_rows() <= 4));
        assert!(
            first
                .iter()
                .all(|chunk| chunk.total_size_bytes() <= two_rows)
        );
        assert_eq!(first.iter().map(Chunk::num_rows).sum::<usize>(), 8);
    }

    #[test]
    fn all_caps_fail_before_any_handoff_is_returned() {
        let source = source_chunk(2);
        let one_row = source.row_sliced_deep(0, 1).total_size_bytes();
        let limits = RemoteDerivedChunkLimitsV1::new_v1(1, one_row, one_row, 1, 1, 2);
        assert!(matches!(
            prepare_deterministic_derived_chunks_v1(2, &limits, |range| {
                Ok::<_, ()>(source.row_sliced_deep(range.start, range.len()))
            }),
            Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::OutputPhysicalBytes
            ))
        ));

        let build_calls = Cell::new(0_u32);
        let root_limited = RemoteDerivedChunkLimitsV1::new_v1(1, one_row, u64::MAX, 1, 1, 1);
        assert!(matches!(
            prepare_deterministic_derived_chunks_v1(2, &root_limited, |range| {
                build_calls.set(build_calls.get() + 1);
                Ok::<_, ()>(source.row_sliced_deep(range.start, range.len()))
            }),
            Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::RootsPerPartition
            ))
        ));
        assert_eq!(
            build_calls.get(),
            0,
            "the initial root census rejects before copying"
        );

        let too_small = RemoteDerivedChunkLimitsV1::new_v1(1, one_row - 1, u64::MAX, 1, 1, 2);
        assert!(matches!(
            prepare_deterministic_derived_chunks_v1(2, &too_small, |range| {
                Ok::<_, ()>(source.row_sliced_deep(range.start, range.len()))
            }),
            Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::PhysicalBytesPerRoot
            ))
        ));

        let multi = multi_cardinality_chunk();
        let physical_bytes = multi.total_size_bytes();
        let component_limited =
            RemoteDerivedChunkLimitsV1::new_v1(1, physical_bytes, physical_bytes, 1, 2, 1);
        assert!(matches!(
            prepare_deterministic_derived_chunks_v1(1, &component_limited, |range| {
                Ok::<_, ()>(multi.row_sliced_deep(range.start, range.len()))
            }),
            Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::ComponentsPerRoot
            ))
        ));
        let timeline_limited =
            RemoteDerivedChunkLimitsV1::new_v1(1, physical_bytes, physical_bytes, 2, 1, 1);
        assert!(matches!(
            prepare_deterministic_derived_chunks_v1(1, &timeline_limited, |range| {
                Ok::<_, ()>(multi.row_sliced_deep(range.start, range.len()))
            }),
            Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::TimelinesPerRoot
            ))
        ));
    }

    #[test]
    fn recursive_physical_split_checks_root_capacity_before_the_last_copy() {
        let source = source_chunk(4);
        let one_row = source.row_sliced_deep(0, 1).total_size_bytes();
        let limits = RemoteDerivedChunkLimitsV1::new_v1(4, one_row, u64::MAX, 1, 1, 3);
        let copied_ranges = RefCell::new(Vec::new());
        assert!(matches!(
            prepare_deterministic_derived_chunks_v1(4, &limits, |range| {
                copied_ranges.borrow_mut().push(range.clone());
                Ok::<_, ()>(source.row_sliced_deep(range.start, range.len()))
            }),
            Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::RootsPerPartition
            ))
        ));
        assert!(
            !copied_ranges.borrow().contains(&(3..4)),
            "the root-cap-plus-one leaf must be rejected before deep copy"
        );
    }

    #[test]
    fn one_oversized_row_fails_without_a_partial_result() {
        let source = source_chunk(1);
        let physical_bytes = source.total_size_bytes();
        assert!(matches!(
            prepare_deterministic_derived_chunks_v1(
                1,
                &RemoteDerivedChunkLimitsV1::new_v1(1, physical_bytes - 1, u64::MAX, 1, 1, 1,),
                |range| Ok::<_, ()>(source.row_sliced_deep(range.start, range.len())),
            ),
            Err(RemoteDerivedChunkPreparationErrorV1::ResourceLimitExceeded(
                RemoteDerivedChunkResourceLimitV1::PhysicalBytesPerRoot
            ))
        ));
    }

    #[test]
    fn sealed_profile_digest_covers_every_split_semantic() {
        let baseline = RemoteDerivedChunkProfileV1::for_test_v1(2, 3, 4, 5, 6, 7);
        assert_eq!(baseline.identity_digest_v1(), baseline.identity_digest_v1());
        for changed in [
            RemoteDerivedChunkProfileV1::for_test_v1(1, 3, 4, 5, 6, 7),
            RemoteDerivedChunkProfileV1::for_test_v1(2, 2, 4, 5, 6, 7),
            RemoteDerivedChunkProfileV1::for_test_v1(2, 3, 3, 5, 6, 7),
            RemoteDerivedChunkProfileV1::for_test_v1(2, 3, 4, 4, 6, 7),
            RemoteDerivedChunkProfileV1::for_test_v1(2, 3, 4, 5, 5, 7),
            RemoteDerivedChunkProfileV1::for_test_v1(2, 3, 4, 5, 6, 6),
        ] {
            assert_ne!(baseline.identity_digest_v1(), changed.identity_digest_v1());
        }
        assert_eq!(
            baseline.partition_limits_v1(2).unwrap(),
            baseline.partition_limits_v1(2).unwrap()
        );
        assert_ne!(
            baseline.partition_limits_v1(1).unwrap(),
            baseline.partition_limits_v1(2).unwrap()
        );
    }

    #[test]
    fn production_prebuilt_validation_rejects_required_split_without_copying() {
        let source = source_chunk(2);
        let one_row = source.row_sliced_deep(0, 1).total_size_bytes();
        let row_limited = RemoteDerivedChunkLimitsV1::new_v1(1, u64::MAX, u64::MAX, 1, 1, 2);
        assert_eq!(
            row_limited.validate_prebuilt_root_v1(&source),
            Err(RemoteDerivedChunkResourceLimitV1::DeepSplitUnavailable)
        );
        let byte_limited = RemoteDerivedChunkLimitsV1::new_v1(2, one_row, u64::MAX, 1, 1, 2);
        assert_eq!(
            byte_limited.validate_prebuilt_root_v1(&source),
            Err(RemoteDerivedChunkResourceLimitV1::DeepSplitUnavailable)
        );
    }
}
