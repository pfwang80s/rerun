//! Incremental canonical indexed extent and loaded-range coverage for Web remote MCAP.

use std::alloc::Layout;

use re_log_types::{AbsoluteTimeRange, TimeInt};

use crate::remote_manifest::RemoteRegistrationReservationV1;
use crate::remote_physical_resolution::{
    CanonicalPhysicalExtentV1, ResolvedRemotePhysicalSourceRefV1,
};

const MAX_REMOTE_LOADED_COVERAGE_BYTES_V1: u64 = 64 * 1024 * 1024;

fn locked_array_footprint_v1<T>(count: usize) -> Result<u64, RemoteLoadedCoverageErrorV1> {
    let layout = Layout::array::<T>(count)
        .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
    crate::remote_chunk_validation_count::locked_footprint(layout)
        .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTemporalCoverageCensusV1 {
    pub(crate) retained_bytes: u64,
    pub(crate) construction_temporary_bytes: u64,
}

fn try_boxed_filled_v1<T: Clone>(
    len: usize,
    value: T,
) -> Result<Box<[T]>, RemoteLoadedCoverageErrorV1> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(len)
        .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
    values.resize(len, value);
    Ok(values.into_boxed_slice())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CanonicalIndexedExtentV1 {
    NoIndexedMessages,
    Known(AbsoluteTimeRange),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompleteIndexedCoverageV1 {
    NoIndexedMessages,
    Incomplete,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RemoteLoadedCoverageErrorV1 {
    #[error("remote MCAP loaded coverage exceeds its admitted resource limits")]
    ResourceLimitExceeded,

    #[error("remote MCAP loaded coverage received an invalid source-unit transition")]
    InvalidSourceUnit,

    #[error("remote MCAP loaded coverage counter invariant was violated")]
    CounterInvariant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PartitionSatisfactionTransitionV1 {
    BecameSatisfied,
    BecameUnsatisfied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoverageCellDomainV1 {
    Point(TimeInt),
    ExclusiveGap { lower: TimeInt, upper: TimeInt },
}

impl CoverageCellDomainV1 {
    fn inclusive_range_v1(self) -> Option<AbsoluteTimeRange> {
        match self {
            Self::Point(time) => Some(AbsoluteTimeRange::point(time)),
            Self::ExclusiveGap { lower, upper } => {
                let min = lower.as_i64().checked_add(1)?;
                let max = upper.as_i64().checked_sub(1)?;
                (min <= max).then(|| AbsoluteTimeRange::new(min, max))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CoverageCellPlanV1 {
    domain: CoverageCellDomainV1,
    expected_source_units: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceUnitCoveragePlanV1 {
    expected_selected_group_count: u32,
    coverage_cells: Option<(usize, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EndpointEventV1 {
    time: TimeInt,
    is_start: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EndpointV1 {
    time: TimeInt,
    starts: u32,
    ends: u32,
}

pub(crate) struct RemoteTemporalCoveragePlanV1 {
    indexed_extent: CanonicalIndexedExtentV1,
    source_units: Box<[SourceUnitCoveragePlanV1]>,
    cells: Box<[CoverageCellPlanV1]>,
}

impl RemoteTemporalCoveragePlanV1 {
    pub(crate) fn census_upper_bound_for_units_v1(
        unit_count: usize,
    ) -> Result<RemoteTemporalCoverageCensusV1, RemoteLoadedCoverageErrorV1> {
        let max_endpoints = unit_count
            .checked_mul(2)
            .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        let max_cells = max_endpoints
            .checked_mul(2)
            .and_then(|count| count.checked_sub(usize::from(max_endpoints != 0)))
            .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        let retained_bytes = locked_array_footprint_v1::<SourceUnitCoveragePlanV1>(unit_count)?
            .checked_add(locked_array_footprint_v1::<CoverageCellPlanV1>(max_cells)?)
            .and_then(|bytes| bytes.checked_add(locked_array_footprint_v1::<u32>(unit_count).ok()?))
            .and_then(|bytes| bytes.checked_add(locked_array_footprint_v1::<u32>(max_cells).ok()?))
            .and_then(|bytes| {
                bytes.checked_add(locked_array_footprint_v1::<AbsoluteTimeRange>(max_cells).ok()?)
            })
            .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        let construction_temporary_bytes =
            locked_array_footprint_v1::<EndpointEventV1>(max_endpoints)?
                .checked_add(locked_array_footprint_v1::<EndpointV1>(max_endpoints)?)
                .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        if retained_bytes
            .checked_add(construction_temporary_bytes)
            .is_none_or(|peak| peak > MAX_REMOTE_LOADED_COVERAGE_BYTES_V1)
        {
            return Err(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded);
        }
        Ok(RemoteTemporalCoverageCensusV1 {
            retained_bytes,
            construction_temporary_bytes,
        })
    }

    pub(crate) fn retained_index_bytes_v1(&self) -> Result<u64, RemoteLoadedCoverageErrorV1> {
        locked_array_footprint_v1::<SourceUnitCoveragePlanV1>(self.source_units.len())?
            .checked_add(locked_array_footprint_v1::<CoverageCellPlanV1>(
                self.cells.len(),
            )?)
            .and_then(|bytes| {
                bytes.checked_add(locked_array_footprint_v1::<u32>(self.source_units.len()).ok()?)
            })
            .and_then(|bytes| {
                bytes.checked_add(locked_array_footprint_v1::<u32>(self.cells.len()).ok()?)
            })
            .and_then(|bytes| {
                bytes.checked_add(
                    locked_array_footprint_v1::<AbsoluteTimeRange>(self.cells.len()).ok()?,
                )
            })
            .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)
    }

    pub(crate) fn build_v1(
        source: &ResolvedRemotePhysicalSourceRefV1<'_, '_>,
        expected_selected_group_count: u32,
        temporary_reservation: &RemoteRegistrationReservationV1,
    ) -> Result<Self, RemoteLoadedCoverageErrorV1> {
        let unit_count = source.layout_v1().canonical_chunk_count_v1();
        let census = Self::census_upper_bound_for_units_v1(unit_count)?;
        if temporary_reservation.bytes_v1() < census.construction_temporary_bytes {
            return Err(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded);
        }
        let max_endpoints = unit_count
            .checked_mul(2)
            .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        let indexed_extent = match source.layout_v1().canonical_extent_v1() {
            CanonicalPhysicalExtentV1::KnownEmpty => CanonicalIndexedExtentV1::NoIndexedMessages,
            CanonicalPhysicalExtentV1::Known { start, end } => {
                CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(start, end))
            }
        };

        let max_events = max_endpoints;
        let mut events = Vec::new();
        events
            .try_reserve_exact(max_events)
            .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        for ordinal in 0..unit_count {
            if let Some((start, end)) = source
                .source_unit_v1(ordinal)
                .and_then(|unit| unit.canonical_interval_v1())
            {
                events.push(EndpointEventV1 {
                    time: start,
                    is_start: true,
                });
                events.push(EndpointEventV1 {
                    time: end,
                    is_start: false,
                });
            }
        }
        events.sort_unstable_by_key(|event| event.time);

        let mut endpoints = Vec::<EndpointV1>::new();
        endpoints
            .try_reserve_exact(events.len())
            .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        for event in events {
            if endpoints
                .last()
                .is_none_or(|endpoint| endpoint.time != event.time)
            {
                endpoints.push(EndpointV1 {
                    time: event.time,
                    starts: 0,
                    ends: 0,
                });
            }
            let endpoint = endpoints
                .last_mut()
                .expect("an endpoint was inserted for the current event");
            let counter = if event.is_start {
                &mut endpoint.starts
            } else {
                &mut endpoint.ends
            };
            *counter = counter
                .checked_add(1)
                .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        }

        let max_cells = endpoints
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_sub(usize::from(!endpoints.is_empty())))
            .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(max_cells)
            .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        let mut active = 0_u32;
        for (index, endpoint) in endpoints.iter().copied().enumerate() {
            active = active
                .checked_add(endpoint.starts)
                .ok_or(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
            cells.push(CoverageCellPlanV1 {
                domain: CoverageCellDomainV1::Point(endpoint.time),
                expected_source_units: active,
            });
            active = active
                .checked_sub(endpoint.ends)
                .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?;
            if let Some(next) = endpoints.get(index + 1) {
                cells.push(CoverageCellPlanV1 {
                    domain: CoverageCellDomainV1::ExclusiveGap {
                        lower: endpoint.time,
                        upper: next.time,
                    },
                    expected_source_units: active,
                });
            }
        }
        if active != 0 {
            return Err(RemoteLoadedCoverageErrorV1::CounterInvariant);
        }

        let mut source_units = Vec::new();
        source_units
            .try_reserve_exact(unit_count)
            .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?;
        for ordinal in 0..unit_count {
            let coverage_cells = source
                .source_unit_v1(ordinal)
                .and_then(|unit| unit.canonical_interval_v1())
                .map(|(start, end)| {
                    let start_endpoint = endpoints
                        .binary_search_by_key(&start, |endpoint| endpoint.time)
                        .expect("every interval start was retained as an endpoint");
                    let end_endpoint = endpoints
                        .binary_search_by_key(&end, |endpoint| endpoint.time)
                        .expect("every interval end was retained as an endpoint");
                    (start_endpoint * 2, end_endpoint * 2)
                });
            source_units.push(SourceUnitCoveragePlanV1 {
                expected_selected_group_count,
                coverage_cells,
            });
        }

        Ok(Self {
            indexed_extent,
            source_units: source_units.into_boxed_slice(),
            cells: cells.into_boxed_slice(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_v1(
        intervals: &[Option<(i64, i64)>],
        expected_selected_group_count: u32,
    ) -> Self {
        let source_units = intervals
            .iter()
            .map(|interval| SourceUnitCoveragePlanV1 {
                expected_selected_group_count,
                coverage_cells: interval.map(|_| (0, 0)),
            })
            .collect::<Vec<_>>();
        let nonempty = intervals
            .iter()
            .filter_map(|interval| *interval)
            .collect::<Vec<_>>();
        if nonempty.is_empty() {
            return Self {
                indexed_extent: CanonicalIndexedExtentV1::NoIndexedMessages,
                source_units: source_units.into_boxed_slice(),
                cells: Box::new([]),
            };
        }

        // Tests requiring temporal coverage use the same production builder semantics through this
        // small deterministic equivalent.
        let mut endpoints = nonempty
            .iter()
            .flat_map(|(start, end)| [*start, *end])
            .map(TimeInt::new_temporal)
            .collect::<Vec<_>>();
        endpoints.sort_unstable();
        endpoints.dedup();
        let mut cells = Vec::with_capacity(endpoints.len() * 2 - 1);
        for (index, time) in endpoints.iter().copied().enumerate() {
            let expected_source_units = u32::try_from(
                nonempty
                    .iter()
                    .filter(|(start, end)| {
                        TimeInt::new_temporal(*start) <= time && time <= TimeInt::new_temporal(*end)
                    })
                    .count(),
            )
            .unwrap();
            cells.push(CoverageCellPlanV1 {
                domain: CoverageCellDomainV1::Point(time),
                expected_source_units,
            });
            if let Some(next) = endpoints.get(index + 1).copied() {
                let probe = time
                    .as_i64()
                    .checked_add(1)
                    .filter(|probe| *probe < next.as_i64());
                let expected_source_units = probe.map_or(0, |probe| {
                    u32::try_from(
                        nonempty
                            .iter()
                            .filter(|(start, end)| *start <= probe && probe <= *end)
                            .count(),
                    )
                    .unwrap()
                });
                cells.push(CoverageCellPlanV1 {
                    domain: CoverageCellDomainV1::ExclusiveGap {
                        lower: time,
                        upper: next,
                    },
                    expected_source_units,
                });
            }
        }
        let source_units = intervals
            .iter()
            .map(|interval| SourceUnitCoveragePlanV1 {
                expected_selected_group_count,
                coverage_cells: interval.map(|(start, end)| {
                    let start = endpoints
                        .binary_search(&TimeInt::new_temporal(start))
                        .unwrap();
                    let end = endpoints
                        .binary_search(&TimeInt::new_temporal(end))
                        .unwrap();
                    (start * 2, end * 2)
                }),
            })
            .collect::<Vec<_>>();
        let min = nonempty.iter().map(|(start, _)| *start).min().unwrap();
        let max = nonempty.iter().map(|(_, end)| *end).max().unwrap();
        Self {
            indexed_extent: CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(min, max)),
            source_units: source_units.into_boxed_slice(),
            cells: cells.into_boxed_slice(),
        }
    }

    pub(crate) const fn indexed_extent_v1(&self) -> CanonicalIndexedExtentV1 {
        self.indexed_extent
    }
}

pub(crate) struct RemoteLoadedCoverageIndexV1 {
    indexed_extent: CanonicalIndexedExtentV1,
    source_plans: Box<[SourceUnitCoveragePlanV1]>,
    satisfied_selected_group_counts: Box<[u32]>,
    satisfied_source_units_per_cell: Box<[u32]>,
    cell_plans: Box<[CoverageCellPlanV1]>,
    loaded_ranges: Box<[AbsoluteTimeRange]>,
    loaded_range_count: usize,
    indexed_source_unit_count: u32,
    satisfied_indexed_source_unit_count: u32,
    #[cfg(test)]
    rebuild_count: u64,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteLoadedCoverageSnapshotV1 {
    satisfied_selected_group_counts: Box<[u32]>,
    satisfied_source_units_per_cell: Box<[u32]>,
    loaded_ranges: Box<[AbsoluteTimeRange]>,
    indexed_source_unit_count: u32,
    satisfied_indexed_source_unit_count: u32,
    rebuild_count: u64,
}

impl RemoteLoadedCoverageIndexV1 {
    #[cfg(test)]
    pub(crate) fn snapshot_v1(&self) -> RemoteLoadedCoverageSnapshotV1 {
        RemoteLoadedCoverageSnapshotV1 {
            satisfied_selected_group_counts: self.satisfied_selected_group_counts.clone(),
            satisfied_source_units_per_cell: self.satisfied_source_units_per_cell.clone(),
            loaded_ranges: self.loaded_ranges_v1().into(),
            indexed_source_unit_count: self.indexed_source_unit_count,
            satisfied_indexed_source_unit_count: self.satisfied_indexed_source_unit_count,
            rebuild_count: self.rebuild_count,
        }
    }

    pub(crate) fn from_plan_v1(
        plan: RemoteTemporalCoveragePlanV1,
        retained_reservation: &RemoteRegistrationReservationV1,
    ) -> Result<Self, RemoteLoadedCoverageErrorV1> {
        if plan.retained_index_bytes_v1()? > retained_reservation.bytes_v1() {
            return Err(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded);
        }
        let mut index = Self {
            indexed_extent: plan.indexed_extent,
            satisfied_selected_group_counts: try_boxed_filled_v1(plan.source_units.len(), 0)?,
            satisfied_source_units_per_cell: try_boxed_filled_v1(plan.cells.len(), 0)?,
            loaded_ranges: try_boxed_filled_v1(plan.cells.len(), AbsoluteTimeRange::EMPTY)?,
            loaded_range_count: 0,
            indexed_source_unit_count: u32::try_from(
                plan.source_units
                    .iter()
                    .filter(|source| source.coverage_cells.is_some())
                    .count(),
            )
            .map_err(|_error| RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)?,
            satisfied_indexed_source_unit_count: 0,
            source_plans: plan.source_units,
            cell_plans: plan.cells,
            #[cfg(test)]
            rebuild_count: 0,
        };
        for ordinal in 0..index.source_plans.len() {
            if index.source_plans[ordinal].expected_selected_group_count == 0 {
                index.set_source_unit_satisfied_v1(ordinal, true)?;
            }
        }
        index.rebuild_loaded_ranges_v1()?;
        Ok(index)
    }

    pub(crate) fn validate_temporal_source_unit_v1(
        &self,
        ordinal: u32,
    ) -> Result<(), RemoteLoadedCoverageErrorV1> {
        self.source_plans
            .get(
                usize::try_from(ordinal)
                    .map_err(|_error| RemoteLoadedCoverageErrorV1::InvalidSourceUnit)?,
            )
            .map(|_| ())
            .ok_or(RemoteLoadedCoverageErrorV1::InvalidSourceUnit)
    }

    pub(crate) fn apply_partition_transition_in_batch_v1(
        &mut self,
        source_unit_ordinal: u32,
        transition: PartitionSatisfactionTransitionV1,
    ) -> Result<bool, RemoteLoadedCoverageErrorV1> {
        let ordinal = usize::try_from(source_unit_ordinal)
            .map_err(|_error| RemoteLoadedCoverageErrorV1::InvalidSourceUnit)?;
        let plan = *self
            .source_plans
            .get(ordinal)
            .ok_or(RemoteLoadedCoverageErrorV1::InvalidSourceUnit)?;
        let count = self
            .satisfied_selected_group_counts
            .get_mut(ordinal)
            .ok_or(RemoteLoadedCoverageErrorV1::InvalidSourceUnit)?;
        let source_was_satisfied = *count == plan.expected_selected_group_count;
        *count = match transition {
            PartitionSatisfactionTransitionV1::BecameSatisfied => count
                .checked_add(1)
                .filter(|count| *count <= plan.expected_selected_group_count)
                .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?,
            PartitionSatisfactionTransitionV1::BecameUnsatisfied => count
                .checked_sub(1)
                .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?,
        };
        let source_is_satisfied = *count == plan.expected_selected_group_count;
        if source_was_satisfied != source_is_satisfied {
            self.set_source_unit_satisfied_v1(ordinal, source_is_satisfied)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn finish_transition_batch_v1(
        &mut self,
        ranges_dirty: bool,
    ) -> Result<(), RemoteLoadedCoverageErrorV1> {
        if ranges_dirty {
            self.rebuild_loaded_ranges_v1()?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn apply_partition_transition_v1(
        &mut self,
        source_unit_ordinal: u32,
        transition: PartitionSatisfactionTransitionV1,
    ) -> Result<(), RemoteLoadedCoverageErrorV1> {
        let ranges_dirty =
            self.apply_partition_transition_in_batch_v1(source_unit_ordinal, transition)?;
        self.finish_transition_batch_v1(ranges_dirty)
    }

    fn set_source_unit_satisfied_v1(
        &mut self,
        ordinal: usize,
        satisfied: bool,
    ) -> Result<(), RemoteLoadedCoverageErrorV1> {
        let Some((start, end)) = self.source_plans[ordinal].coverage_cells else {
            return Ok(());
        };
        self.satisfied_indexed_source_unit_count = if satisfied {
            self.satisfied_indexed_source_unit_count
                .checked_add(1)
                .filter(|count| *count <= self.indexed_source_unit_count)
                .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?
        } else {
            self.satisfied_indexed_source_unit_count
                .checked_sub(1)
                .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?
        };
        for count in self
            .satisfied_source_units_per_cell
            .get_mut(start..=end)
            .ok_or(RemoteLoadedCoverageErrorV1::InvalidSourceUnit)?
        {
            *count = if satisfied {
                count
                    .checked_add(1)
                    .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?
            } else {
                count
                    .checked_sub(1)
                    .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?
            };
        }
        Ok(())
    }

    fn rebuild_loaded_ranges_v1(&mut self) -> Result<(), RemoteLoadedCoverageErrorV1> {
        #[cfg(test)]
        {
            self.rebuild_count = self
                .rebuild_count
                .checked_add(1)
                .expect("test coverage rebuild count cannot overflow");
        }
        self.loaded_range_count = 0;
        for (plan, satisfied) in self
            .cell_plans
            .iter()
            .zip(self.satisfied_source_units_per_cell.iter().copied())
        {
            if plan.expected_source_units == 0 || satisfied != plan.expected_source_units {
                continue;
            }
            let Some(range) = plan.domain.inclusive_range_v1() else {
                continue;
            };
            if let Some(previous) = self
                .loaded_range_count
                .checked_sub(1)
                .and_then(|index| self.loaded_ranges.get_mut(index))
            {
                let adjacent_or_overlapping =
                    range.min.as_i64() <= previous.max.as_i64().saturating_add(1);
                if adjacent_or_overlapping {
                    previous.max = previous.max.max(range.max);
                    continue;
                }
            }
            let slot = self
                .loaded_ranges
                .get_mut(self.loaded_range_count)
                .ok_or(RemoteLoadedCoverageErrorV1::CounterInvariant)?;
            *slot = range;
            self.loaded_range_count += 1;
        }
        Ok(())
    }

    pub(crate) fn loaded_ranges_v1(&self) -> &[AbsoluteTimeRange] {
        &self.loaded_ranges[..self.loaded_range_count]
    }

    pub(crate) const fn indexed_extent_v1(&self) -> CanonicalIndexedExtentV1 {
        self.indexed_extent
    }

    pub(crate) fn complete_indexed_coverage_v1(&self) -> CompleteIndexedCoverageV1 {
        match self.indexed_extent {
            CanonicalIndexedExtentV1::NoIndexedMessages => {
                CompleteIndexedCoverageV1::NoIndexedMessages
            }
            CanonicalIndexedExtentV1::Known(_) => {
                if self.satisfied_indexed_source_unit_count == self.indexed_source_unit_count {
                    CompleteIndexedCoverageV1::Complete
                } else {
                    CompleteIndexedCoverageV1::Incomplete
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn satisfied_selected_group_count_v1(&self, ordinal: usize) -> u32 {
        self.satisfied_selected_group_counts[ordinal]
    }

    #[cfg(test)]
    pub(crate) const fn rebuild_count_v1(&self) -> u64 {
        self.rebuild_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_from_plan_v1(
        plan: RemoteTemporalCoveragePlanV1,
    ) -> (RemoteLoadedCoverageIndexV1, RemoteRegistrationReservationV1) {
        let retained_bytes = plan.retained_index_bytes_v1().unwrap();
        let budget =
            crate::remote_manifest::RemoteRegistrationBudgetV1::new_disarmed_v1(retained_bytes);
        let reservation = budget.reserve_v1(retained_bytes).unwrap();
        let index = RemoteLoadedCoverageIndexV1::from_plan_v1(plan, &reservation).unwrap();
        (index, reservation)
    }

    #[test]
    fn overlapping_source_units_update_incrementally() {
        let plan = RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((0, 10)), Some((5, 15))], 2);
        let (mut coverage, _reservation) = index_from_plan_v1(plan);
        assert_eq!(
            coverage.indexed_extent_v1(),
            CanonicalIndexedExtentV1::Known(AbsoluteTimeRange::new(0, 15))
        );
        assert_eq!(
            coverage.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Incomplete
        );
        assert!(coverage.loaded_ranges_v1().is_empty());

        coverage
            .apply_partition_transition_v1(0, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        assert!(coverage.loaded_ranges_v1().is_empty());
        coverage
            .apply_partition_transition_v1(0, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        assert_eq!(coverage.loaded_ranges_v1(), &[AbsoluteTimeRange::new(0, 4)]);
        assert_eq!(coverage.satisfied_selected_group_count_v1(0), 2);

        coverage
            .apply_partition_transition_v1(1, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        coverage
            .apply_partition_transition_v1(1, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        assert_eq!(
            coverage.loaded_ranges_v1(),
            &[AbsoluteTimeRange::new(0, 15)]
        );
        assert_eq!(
            coverage.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Complete
        );

        coverage
            .apply_partition_transition_v1(0, PartitionSatisfactionTransitionV1::BecameUnsatisfied)
            .unwrap();
        assert_eq!(
            coverage.loaded_ranges_v1(),
            &[AbsoluteTimeRange::new(11, 15)]
        );
        assert_eq!(
            coverage.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Incomplete
        );
    }

    #[test]
    fn no_indexed_messages_is_not_an_empty_or_point_range() {
        let plan = RemoteTemporalCoveragePlanV1::for_test_v1(&[None, None], 1);
        let (mut coverage, _reservation) = index_from_plan_v1(plan);
        assert_eq!(
            coverage.indexed_extent_v1(),
            CanonicalIndexedExtentV1::NoIndexedMessages
        );
        assert_eq!(
            coverage.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::NoIndexedMessages
        );
        coverage
            .apply_partition_transition_v1(0, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        coverage
            .apply_partition_transition_v1(1, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        assert!(coverage.loaded_ranges_v1().is_empty());
        assert_eq!(
            coverage.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::NoIndexedMessages
        );
    }

    #[test]
    fn endpoint_max_does_not_require_a_successor() {
        let min = TimeInt::MAX.as_i64() - 2;
        let max = TimeInt::MAX.as_i64();
        let plan = RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((min, max))], 1);
        let (mut coverage, _reservation) = index_from_plan_v1(plan);
        coverage
            .apply_partition_transition_v1(0, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        assert_eq!(
            coverage.loaded_ranges_v1(),
            &[AbsoluteTimeRange::new(min, max)]
        );
    }

    #[test]
    fn complete_coverage_can_contain_indexed_gaps() {
        let plan = RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((0, 2)), Some((10, 12))], 1);
        let (mut coverage, _reservation) = index_from_plan_v1(plan);
        coverage
            .apply_partition_transition_v1(0, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        coverage
            .apply_partition_transition_v1(1, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();

        assert_eq!(
            coverage.loaded_ranges_v1(),
            &[AbsoluteTimeRange::new(0, 2), AbsoluteTimeRange::new(10, 12),]
        );
        assert_eq!(
            coverage.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Complete
        );
    }

    #[test]
    fn zero_selected_groups_are_vacuously_complete_without_query_rebuilds() {
        let plan =
            RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((0, 5)), Some((3, 8)), None], 0);
        let (coverage, _reservation) = index_from_plan_v1(plan);
        assert_eq!(coverage.loaded_ranges_v1(), &[AbsoluteTimeRange::new(0, 8)]);
        assert_eq!(
            coverage.complete_indexed_coverage_v1(),
            CompleteIndexedCoverageV1::Complete
        );

        let before = coverage.snapshot_v1();
        for _ in 0..16 {
            assert_eq!(coverage.loaded_ranges_v1(), &[AbsoluteTimeRange::new(0, 8)]);
            assert_eq!(
                coverage.complete_indexed_coverage_v1(),
                CompleteIndexedCoverageV1::Complete
            );
        }
        assert_eq!(coverage.snapshot_v1(), before);
    }

    #[test]
    fn repeated_getters_do_not_change_cached_coverage() {
        let plan = RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((0, 10))], 1);
        let (mut coverage, _reservation) = index_from_plan_v1(plan);
        coverage
            .apply_partition_transition_v1(0, PartitionSatisfactionTransitionV1::BecameSatisfied)
            .unwrap();
        let loaded = coverage.snapshot_v1();
        for _ in 0..16 {
            assert_eq!(
                coverage.loaded_ranges_v1(),
                &[AbsoluteTimeRange::new(0, 10)]
            );
            assert_eq!(
                coverage.complete_indexed_coverage_v1(),
                CompleteIndexedCoverageV1::Complete
            );
        }
        assert_eq!(coverage.snapshot_v1(), loaded);
    }

    #[test]
    fn locked_wasm_census_is_exact_and_combined_budget_is_atomic() {
        let units = 2;
        let endpoints = 4;
        let cells = 7;
        let census = RemoteTemporalCoveragePlanV1::census_upper_bound_for_units_v1(units).unwrap();
        let expected_retained = locked_array_footprint_v1::<SourceUnitCoveragePlanV1>(units)
            .unwrap()
            .checked_add(locked_array_footprint_v1::<CoverageCellPlanV1>(cells).unwrap())
            .and_then(|bytes| bytes.checked_add(locked_array_footprint_v1::<u32>(units).unwrap()))
            .and_then(|bytes| bytes.checked_add(locked_array_footprint_v1::<u32>(cells).unwrap()))
            .and_then(|bytes| {
                bytes.checked_add(locked_array_footprint_v1::<AbsoluteTimeRange>(cells).unwrap())
            })
            .unwrap();
        let expected_temporary = locked_array_footprint_v1::<EndpointEventV1>(endpoints)
            .unwrap()
            .checked_add(locked_array_footprint_v1::<EndpointV1>(endpoints).unwrap())
            .unwrap();
        assert_eq!(
            census,
            RemoteTemporalCoverageCensusV1 {
                retained_bytes: expected_retained,
                construction_temporary_bytes: expected_temporary,
            }
        );

        let combined = census
            .retained_bytes
            .checked_add(census.construction_temporary_bytes)
            .unwrap();
        let budget =
            crate::remote_manifest::RemoteRegistrationBudgetV1::new_disarmed_v1(combined - 1);
        let retained = budget.reserve_v1(census.retained_bytes).unwrap();
        assert_eq!(
            budget.reserve_v1(census.construction_temporary_bytes).err(),
            Some(crate::remote_manifest::RemoteManifestErrorV1::ResourceLimitExceeded)
        );
        assert_eq!(budget.used_bytes_v1(), census.retained_bytes);
        drop(retained);
        assert_eq!(budget.used_bytes_v1(), 0);

        let budget = crate::remote_manifest::RemoteRegistrationBudgetV1::new_disarmed_v1(combined);
        let retained = budget.reserve_v1(census.retained_bytes).unwrap();
        let temporary = budget
            .reserve_v1(census.construction_temporary_bytes)
            .unwrap();
        assert_eq!(budget.used_bytes_v1(), combined);
        drop(temporary);
        assert_eq!(budget.used_bytes_v1(), census.retained_bytes);
        drop(retained);
        assert_eq!(budget.used_bytes_v1(), 0);

        assert_eq!(
            RemoteTemporalCoveragePlanV1::census_upper_bound_for_units_v1(1_000_000),
            Err(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)
        );
    }

    #[test]
    fn retained_reservation_is_checked_before_mutable_index_allocation() {
        let plan = RemoteTemporalCoveragePlanV1::for_test_v1(&[Some((0, 10))], 1);
        let retained_bytes = plan.retained_index_bytes_v1().unwrap();
        let budget =
            crate::remote_manifest::RemoteRegistrationBudgetV1::new_disarmed_v1(retained_bytes - 1);
        let reservation = budget.reserve_v1(retained_bytes - 1).unwrap();
        assert_eq!(
            RemoteLoadedCoverageIndexV1::from_plan_v1(plan, &reservation).err(),
            Some(RemoteLoadedCoverageErrorV1::ResourceLimitExceeded)
        );
        drop(reservation);
        assert_eq!(budget.used_bytes_v1(), 0);
    }
}
