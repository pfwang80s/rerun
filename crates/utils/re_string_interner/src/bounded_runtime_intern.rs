//! Additive, bounded runtime interning for remote-MCAP identifiers.
//!
//! This module does not replace or disable any existing constructor.
//! It adds a separately budgeted side-map which shares lookup identity and a coordination revision
//! with the legacy global interner.
//!
//! Prepared transactions are opaque and cannot be assembled from unchecked fields:
//!
//! ```compile_fail
//! use re_string_interner::bounded_runtime_intern::PreparedRemoteInternBatch;
//! let _ = PreparedRemoteInternBatch {};
//! ```
//!
//! Census results and construction tokens are likewise unforgeable:
//!
//! ```compile_fail
//! use re_string_interner::bounded_runtime_intern::{
//!     BoundedRemoteIdentifierCensus, RemoteMcapDomainConstructionToken,
//! };
//! let _ = BoundedRemoteIdentifierCensus {};
//! let _ = RemoteMcapDomainConstructionToken {};
//! ```

use std::fmt;
use std::num::{NonZeroU64, NonZeroUsize};

use nohash_hasher::IntMap;

use super::{InternedString, StringInterner, hash};

/// Version of the conservative side-map capacity accounting formula.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteInternCapacityAccountingVersion {
    /// Charges one complete map entry plus an equally-sized control/allocation allowance per
    /// reserved bucket, plus a fixed allocation allowance.
    V1,
}

/// Frozen limit values supplied by the Web remote resource profile.
///
/// The values are opaque after construction and every public transaction revalidates against the
/// module-owned copy.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RemoteMcapRuntimeInternLimits {
    max_string_bytes: NonZeroU64,
    max_entry_and_capacity_bytes: NonZeroU64,
    max_census_identifiers: NonZeroU64,
    max_census_retained_bytes: NonZeroU64,
    max_candidate_peak_bytes: NonZeroU64,
}

impl RemoteMcapRuntimeInternLimits {
    /// Creates the interner limits from the corresponding frozen Web remote profile values.
    pub const fn from_profile_values(
        max_string_bytes: NonZeroU64,
        max_entry_and_capacity_bytes: NonZeroU64,
        max_census_identifiers: NonZeroU64,
        max_census_retained_bytes: NonZeroU64,
        max_candidate_peak_bytes: NonZeroU64,
    ) -> Self {
        Self {
            max_string_bytes,
            max_entry_and_capacity_bytes,
            max_census_identifiers,
            max_census_retained_bytes,
            max_candidate_peak_bytes,
        }
    }
}

impl fmt::Debug for RemoteMcapRuntimeInternLimits {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteMcapRuntimeInternLimits")
            .field("values", &"<sealed>")
            .finish()
    }
}

/// A nonzero revision shared by legacy-map and remote-side-map insertions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteInternCoordinationRevision(NonZeroU64);

impl RemoteInternCoordinationRevision {
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// A nonzero revision of the permanently burned remote budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteInternBudgetRevision(NonZeroU64);

impl RemoteInternBudgetRevision {
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// Redacted failure from bounded remote runtime interning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteMcapRuntimeInternError {
    ModuleBudgetNotInitialized,
    ModuleBudgetAlreadyInitializedWithDifferentLimits,
    InvalidLimitProfile,
    CensusIdentifierLimitExceeded,
    CensusRawBytesExceeded,
    CensusRetainedBytesExceeded,
    CensusCanonicalizationFailed,
    CandidatePeakExceeded,
    IdentifierHashCollision,
    AllocationFailed,
    StringBudgetExceeded,
    EntryAndCapacityBudgetExceeded,
    SideMapEntryLimitExceeded,
    CoordinationRevisionExhausted,
    BudgetRevisionExhausted,
    ArithmeticOverflow,
    ProtocolViolation,
}

impl fmt::Display for RemoteMcapRuntimeInternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ModuleBudgetNotInitialized => "remote runtime interning is not initialized",
            Self::ModuleBudgetAlreadyInitializedWithDifferentLimits => {
                "remote runtime interning is initialized with a different profile"
            }
            Self::InvalidLimitProfile => "remote runtime interning profile is invalid",
            Self::CensusIdentifierLimitExceeded => "remote identifier census count limit exceeded",
            Self::CensusRawBytesExceeded => "remote identifier census byte limit exceeded",
            Self::CensusRetainedBytesExceeded => {
                "remote identifier census retained-byte limit exceeded"
            }
            Self::CensusCanonicalizationFailed => {
                "remote identifier census canonicalization failed"
            }
            Self::CandidatePeakExceeded => "remote identifier candidate peak limit exceeded",
            Self::IdentifierHashCollision => "remote identifier hash collision",
            Self::AllocationFailed => "remote identifier allocation failed",
            Self::StringBudgetExceeded => "remote identifier string budget exhausted",
            Self::EntryAndCapacityBudgetExceeded => "remote identifier entry budget exhausted",
            Self::SideMapEntryLimitExceeded => "remote identifier entry limit exceeded",
            Self::CoordinationRevisionExhausted => {
                "remote identifier coordination revision exhausted"
            }
            Self::BudgetRevisionExhausted => "remote identifier budget revision exhausted",
            Self::ArithmeticOverflow => "remote identifier accounting overflow",
            Self::ProtocolViolation => "remote identifier transaction protocol violation",
        })
    }
}

impl std::error::Error for RemoteMcapRuntimeInternError {}

/// Bounded, identifier-free module telemetry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteMcapRuntimeInternSnapshot {
    pub capacity_accounting_version: RemoteInternCapacityAccountingVersion,
    pub coordination_revision: RemoteInternCoordinationRevision,
    pub coordination_revision_exhausted: bool,
    pub budget_revision: RemoteInternBudgetRevision,
    pub legacy_entries: u64,
    pub legacy_capacity: u64,
    pub burned_string_bytes: u64,
    pub burned_entry_bytes: u64,
    pub burned_side_map_capacity_bytes: u64,
    pub remote_entries: u64,
    pub remote_capacity: u64,
}

/// Identifier-free telemetry for one prepared/committed census.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteInternBatchTelemetry {
    /// Duplicate-inclusive number of submitted raw identifiers.
    pub candidate_identifiers: u64,
    /// Duplicate-inclusive UTF-8 byte count of submitted raw identifiers.
    pub candidate_raw_bytes: u64,
    /// Unique domain identifiers after canonicalization and deduplication.
    pub unique_identifiers: u64,
    /// Unique canonical string identities retained by those domain identifiers.
    ///
    /// A full entity path can reference multiple canonical part identities.
    pub canonical_unique_identifiers: u64,
    /// Unique canonical UTF-8 byte count retained by the census.
    pub canonical_bytes: u64,
    /// Census capacity actually retained by strings and deduplication/result structures.
    pub retained_bytes: u64,
    /// Conservative byte bound for all duplicate-inclusive census candidates.
    pub candidate_peak_upper_bound: u64,
    /// Canonical string identities already present in the legacy map.
    pub existing_legacy: u64,
    /// Canonical string identities already present in the remote side-map.
    pub existing_remote: u64,
    /// Canonical string identities absent at preparation/revalidation time.
    pub missing: u64,
    pub missing_string_bytes: u64,
    pub missing_entry_bytes: u64,
    pub prepared_coordination_revision: RemoteInternCoordinationRevision,
    pub recomputed_after_revision_change: bool,
}

/// The remote-MCAP identifier families covered by the V1 raw census.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RemoteMcapRawIdentifierKind {
    Timeline,
    EntityPath,
    EntityPathPart,
    Component,
}

/// One borrowed, not-yet-interned identifier.
///
/// Construction validates the raw shape for the requested family. Entity paths and path parts use
/// the same slash tokenization and forgiving unescaping as the local parser, but without
/// constructing domain values or interning any part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RemoteMcapRawIdentifier<'a> {
    kind: RemoteMcapRawIdentifierKind,
    raw: &'a str,
}

impl<'a> RemoteMcapRawIdentifier<'a> {
    pub fn timeline(raw: &'a str) -> Result<Self, RemoteMcapRuntimeInternError> {
        Self::exact(RemoteMcapRawIdentifierKind::Timeline, raw)
    }

    pub fn entity_path(raw: &'a str) -> Self {
        Self {
            kind: RemoteMcapRawIdentifierKind::EntityPath,
            raw,
        }
    }

    pub fn entity_path_part(raw: &'a str) -> Result<Self, RemoteMcapRuntimeInternError> {
        Self::exact(RemoteMcapRawIdentifierKind::EntityPathPart, raw)
    }

    pub fn component(raw: &'a str) -> Result<Self, RemoteMcapRuntimeInternError> {
        Self::exact(RemoteMcapRawIdentifierKind::Component, raw)
    }

    pub const fn kind(&self) -> RemoteMcapRawIdentifierKind {
        self.kind
    }

    pub const fn as_str(&self) -> &'a str {
        self.raw
    }

    fn exact(
        kind: RemoteMcapRawIdentifierKind,
        raw: &'a str,
    ) -> Result<Self, RemoteMcapRuntimeInternError> {
        if raw.is_empty() {
            return Err(RemoteMcapRuntimeInternError::CensusCanonicalizationFailed);
        }
        Ok(Self { kind, raw })
    }

    fn unchecked(kind: RemoteMcapRawIdentifierKind, raw: &'a str) -> Self {
        Self { kind, raw }
    }
}

/// One unique canonical string owner, referenced by one or more domain records.
#[derive(Debug)]
struct OwnedCanonicalIdentifier {
    hash: u64,
    raw: Option<String>,
}

/// One unique domain record after canonicalization.
#[derive(Debug)]
enum OwnedDomainIdentifier {
    Timeline { canonical: usize },
    EntityPathPart { canonical: usize },
    Component { canonical: usize },
    EntityPath { parts: (usize, usize) },
}

/// A complete, canonical, owned raw identifier census.
///
/// The fields are private and the value is move-only. It cannot be assembled from caller-selected
/// internals, and it owns every temporary string until the interner transaction consumes it.
pub struct BoundedRemoteIdentifierCensus {
    identifiers: Vec<OwnedDomainIdentifier>,
    part_references: Vec<usize>,
    canonical_identifiers: Vec<OwnedCanonicalIdentifier>,
    candidate_identifiers: u64,
    candidate_raw_bytes: u64,
    canonical_bytes: u64,
    retained_bytes: u64,
    candidate_peak_upper_bound: u64,
}

static_assertions::assert_not_impl_any!(BoundedRemoteIdentifierCensus: Clone, Copy);

impl fmt::Debug for BoundedRemoteIdentifierCensus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedRemoteIdentifierCensus")
            .field("identifier_count", &self.identifiers.len())
            .field("candidate_identifiers", &self.candidate_identifiers)
            .field("candidate_raw_bytes", &self.candidate_raw_bytes)
            .field(
                "canonical_identifier_count",
                &self.canonical_identifiers.len(),
            )
            .field("canonical_bytes", &self.canonical_bytes)
            .field("retained_bytes", &self.retained_bytes)
            .field(
                "candidate_peak_upper_bound",
                &self.candidate_peak_upper_bound,
            )
            .finish_non_exhaustive()
    }
}

/// Move-only authority to redeem one complete census through the MCAP-012 transaction.
pub struct RemoteMcapDomainConstructionToken {
    census: BoundedRemoteIdentifierCensus,
}

static_assertions::assert_not_impl_any!(RemoteMcapDomainConstructionToken: Clone, Copy);

impl fmt::Debug for RemoteMcapDomainConstructionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteMcapDomainConstructionToken")
            .field("census", &self.census)
            .finish_non_exhaustive()
    }
}

/// An opaque transaction which owns all temporary strings and result capacity.
pub struct PreparedRemoteInternBatch {
    census: BoundedRemoteIdentifierCensus,
    resolved: Vec<Option<InternedString>>,
    part_handles: Vec<InternedString>,
    telemetry: RemoteInternBatchTelemetry,
}

impl fmt::Debug for PreparedRemoteInternBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedRemoteInternBatch")
            .field(
                "candidate_identifiers",
                &self.telemetry.candidate_identifiers,
            )
            .field("candidate_raw_bytes", &self.telemetry.candidate_raw_bytes)
            .field("unique_identifiers", &self.telemetry.unique_identifiers)
            .field(
                "canonical_unique_identifiers",
                &self.telemetry.canonical_unique_identifiers,
            )
            .field("canonical_bytes", &self.telemetry.canonical_bytes)
            .field("retained_bytes", &self.telemetry.retained_bytes)
            .finish_non_exhaustive()
    }
}

impl PreparedRemoteInternBatch {
    pub const fn telemetry(&self) -> RemoteInternBatchTelemetry {
        self.telemetry
    }

    /// Revalidates and atomically commits against the module singleton.
    pub fn commit(self) -> Result<CommittedRemoteInternBatch, RemoteMcapRuntimeInternError> {
        super::GLOBAL_INTERNER.lock().commit_remote(self)
    }
}

/// Domain-neutral handles returned by one successful atomic batch.
pub enum RemoteMcapDomainIdentifierHandle<'a> {
    Timeline(InternedString),
    EntityPathPart(InternedString),
    Component(InternedString),
    EntityPath(&'a [InternedString]),
}

impl fmt::Debug for RemoteMcapDomainIdentifierHandle<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeline(_) => formatter.write_str("Timeline(<identifier-free>)"),
            Self::EntityPathPart(_) => formatter.write_str("EntityPathPart(<identifier-free>)"),
            Self::Component(_) => formatter.write_str("Component(<identifier-free>)"),
            Self::EntityPath(parts) => write!(formatter, "EntityPath({} parts)", parts.len()),
        }
    }
}

pub struct CommittedRemoteInternBatch {
    identifiers: Vec<OwnedDomainIdentifier>,
    part_handles: Vec<InternedString>,
    handles: Vec<Option<InternedString>>,
    telemetry: RemoteInternBatchTelemetry,
    module_snapshot: RemoteMcapRuntimeInternSnapshot,
}

impl fmt::Debug for CommittedRemoteInternBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommittedRemoteInternBatch")
            .field("handles", &self.handles.len())
            .field("telemetry", &self.telemetry)
            .field("module_snapshot", &self.module_snapshot)
            .finish_non_exhaustive()
    }
}

impl CommittedRemoteInternBatch {
    pub fn len(&self) -> usize {
        self.identifiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.identifiers.is_empty()
    }

    /// Returns a scalar identifier handle, or `None` for a full entity path.
    pub fn handle(&self, index: usize) -> Option<InternedString> {
        match self.identifiers.get(index)? {
            OwnedDomainIdentifier::Timeline { canonical }
            | OwnedDomainIdentifier::EntityPathPart { canonical }
            | OwnedDomainIdentifier::Component { canonical } => {
                self.handles.get(*canonical).copied().flatten()
            }
            OwnedDomainIdentifier::EntityPath { .. } => None,
        }
    }

    /// Returns the canonical identity needed to construct the corresponding typed domain value.
    pub fn domain_handle(&self, index: usize) -> Option<RemoteMcapDomainIdentifierHandle<'_>> {
        match self.identifiers.get(index)? {
            OwnedDomainIdentifier::Timeline { canonical } => self
                .handles
                .get(*canonical)
                .and_then(|handle| *handle)
                .map(RemoteMcapDomainIdentifierHandle::Timeline),
            OwnedDomainIdentifier::EntityPathPart { canonical } => self
                .handles
                .get(*canonical)
                .and_then(|handle| *handle)
                .map(RemoteMcapDomainIdentifierHandle::EntityPathPart),
            OwnedDomainIdentifier::Component { canonical } => self
                .handles
                .get(*canonical)
                .and_then(|handle| *handle)
                .map(RemoteMcapDomainIdentifierHandle::Component),
            OwnedDomainIdentifier::EntityPath { parts } => self
                .part_handles
                .get(parts.0..parts.0.checked_add(parts.1)?)
                .map(RemoteMcapDomainIdentifierHandle::EntityPath),
        }
    }

    pub const fn telemetry(&self) -> RemoteInternBatchTelemetry {
        self.telemetry
    }

    pub const fn module_snapshot(&self) -> RemoteMcapRuntimeInternSnapshot {
        self.module_snapshot
    }
}

/// Initializes the Wasm-module-lifetime remote budget and fixed-capacity side-map.
///
/// Repeating this with the same frozen limits is idempotent.
pub fn initialize_remote_mcap_runtime_intern(
    limits: RemoteMcapRuntimeInternLimits,
) -> Result<RemoteMcapRuntimeInternSnapshot, RemoteMcapRuntimeInternError> {
    initialize_remote_at(&super::GLOBAL_INTERNER, limits, TestFault::None)
}

/// Validates that the frozen profile can construct at least one fixed-capacity side-map entry.
///
/// This performs only checked arithmetic and does not allocate or mutate the module singleton.
pub fn validate_remote_mcap_runtime_intern_profile(
    limits: RemoteMcapRuntimeInternLimits,
) -> Result<(), RemoteMcapRuntimeInternError> {
    PreparedRemoteState::plan(limits).map(|_plan| ())
}

fn initialize_remote_at(
    coordinator: &parking_lot::Mutex<CoordinatedStringInterner>,
    limits: RemoteMcapRuntimeInternLimits,
    fault: TestFault,
) -> Result<RemoteMcapRuntimeInternSnapshot, RemoteMcapRuntimeInternError> {
    {
        let coordinator = coordinator.lock();
        if let Some(remote) = &coordinator.remote {
            if remote.limits == limits {
                return Ok(coordinator.snapshot_remote(remote));
            }
            return Err(
                RemoteMcapRuntimeInternError::ModuleBudgetAlreadyInitializedWithDifferentLimits,
            );
        }
    }

    let candidate = PreparedRemoteState::prepare(limits, fault)?;
    // Another initializer may have won while the fallible candidate was prepared.
    // `install_remote` performs the same limits check under the exclusive lock.
    coordinator.lock().install_remote(candidate)
}

/// Returns bounded module telemetry without exposing any interned identifier.
pub fn remote_mcap_runtime_intern_snapshot()
-> Result<RemoteMcapRuntimeInternSnapshot, RemoteMcapRuntimeInternError> {
    let coordinator = super::GLOBAL_INTERNER.lock();
    let remote = coordinator
        .remote
        .as_ref()
        .ok_or(RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized)?;
    Ok(coordinator.snapshot_remote(remote))
}

/// Builds a bounded raw census and prepares an atomic module transaction.
///
/// Canonical duplicates are returned once, in first-seen order.
pub fn prepare_remote_mcap_runtime_intern(
    raw_identifiers: &[&str],
) -> Result<PreparedRemoteInternBatch, RemoteMcapRuntimeInternError> {
    let limits = super::GLOBAL_INTERNER
        .lock()
        .remote
        .as_ref()
        .ok_or(RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized)?
        .limits;
    let census = BoundedRemoteIdentifierCensus::try_new(raw_identifiers, limits, TestFault::None)?;
    super::GLOBAL_INTERNER.lock().prepare_remote(census)
}

/// Prepares the MCAP-012 transaction by redeeming one complete construction token.
pub fn prepare_remote_mcap_runtime_intern_from_domain_construction_token(
    token: RemoteMcapDomainConstructionToken,
) -> Result<PreparedRemoteInternBatch, RemoteMcapRuntimeInternError> {
    super::GLOBAL_INTERNER.lock().prepare_remote(token.census)
}

const fn entry_bytes() -> u64 {
    std::mem::size_of::<(u64, &'static str)>() as u64
}

fn checked_capacity_bytes(capacity: usize) -> Result<u64, RemoteMcapRuntimeInternError> {
    if capacity == 0 {
        return Ok(0);
    }
    let capacity = u64::try_from(capacity)
        .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    let per_bucket = entry_bytes()
        .checked_mul(2)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    capacity
        .checked_mul(per_bucket)
        .and_then(|bytes| bytes.checked_add(64))
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)
}

fn checked_vec_bytes<T>(capacity: usize) -> Result<u64, RemoteMcapRuntimeInternError> {
    u64::try_from(capacity)
        .ok()
        .and_then(|capacity| capacity.checked_mul(std::mem::size_of::<T>() as u64))
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)
}

fn checked_add_checked(
    left: u64,
    right: Result<u64, RemoteMcapRuntimeInternError>,
) -> Result<u64, RemoteMcapRuntimeInternError> {
    left.checked_add(right?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)
}

fn conservative_hash_capacity_for_entries(
    entries: usize,
) -> Result<usize, RemoteMcapRuntimeInternError> {
    if entries == 0 {
        return Ok(0);
    }
    entries
        .checked_mul(2)
        .and_then(usize::checked_next_power_of_two)
        .map(|capacity| capacity.max(4))
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)
}

fn checked_census_capacity_upper_bound(
    canonical_bytes: u64,
    canonical_identifiers: usize,
    domain_identifiers: usize,
    part_references: usize,
) -> Result<u64, RemoteMcapRuntimeInternError> {
    let retained = canonical_bytes
        .checked_add(checked_vec_bytes::<OwnedCanonicalIdentifier>(
            canonical_identifiers,
        )?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    let retained = checked_add_checked(
        retained,
        checked_vec_bytes::<OwnedDomainIdentifier>(domain_identifiers),
    )?;
    let retained = checked_add_checked(retained, checked_vec_bytes::<usize>(part_references))?;
    let canonical_dedup = conservative_hash_capacity_for_entries(canonical_identifiers)?;
    let domain_dedup = conservative_hash_capacity_for_entries(domain_identifiers)?;
    let retained = retained
        .checked_add(checked_capacity_bytes(canonical_dedup)?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    checked_add_checked(retained, checked_capacity_bytes(domain_dedup))
}

fn checked_candidate_peak_upper_bound(
    canonical_bytes: u64,
    canonical_identifiers: usize,
    domain_identifiers: usize,
    part_references: usize,
) -> Result<u64, RemoteMcapRuntimeInternError> {
    let peak = checked_census_capacity_upper_bound(
        canonical_bytes,
        canonical_identifiers,
        domain_identifiers,
        part_references,
    )?;
    let peak = peak
        .checked_add(checked_vec_bytes::<Option<InternedString>>(
            canonical_identifiers,
        )?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    checked_add_checked(peak, checked_vec_bytes::<InternedString>(part_references))
}

fn checked_census_retained_bytes(
    string_capacity_bytes: u64,
    canonical_capacity: usize,
    domain_capacity: usize,
    part_reference_capacity: usize,
    canonical_dedup_capacity: usize,
    domain_dedup_capacity: usize,
) -> Result<u64, RemoteMcapRuntimeInternError> {
    let retained = string_capacity_bytes
        .checked_add(checked_vec_bytes::<OwnedCanonicalIdentifier>(
            canonical_capacity,
        )?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    let retained = checked_add_checked(
        retained,
        checked_vec_bytes::<OwnedDomainIdentifier>(domain_capacity),
    )?;
    let retained = checked_add_checked(
        retained,
        checked_vec_bytes::<usize>(part_reference_capacity),
    )?;
    let retained = retained
        .checked_add(checked_capacity_bytes(canonical_dedup_capacity)?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    checked_add_checked(retained, checked_capacity_bytes(domain_dedup_capacity))
}

fn parse_unicode_escape<'a>(input: &mut &'a str) -> Result<char, &'a str> {
    let consumed_start = *input;
    let mut consumed_bytes = 0_usize;
    while let Some(character) = input.chars().next() {
        *input = &input[character.len_utf8()..];
        consumed_bytes += character.len_utf8();
        // EntityPathPart stops at exactly six consumed UTF-8 bytes, not six characters.
        if character == '}' || consumed_bytes == 6 {
            break;
        }
    }
    let consumed = &consumed_start[..consumed_bytes];
    let Some(body) = consumed.strip_prefix('{') else {
        return Err(consumed);
    };
    let Some(digits) = body.strip_suffix('}') else {
        return Err(consumed);
    };
    if digits.len() != 4 {
        return Err(consumed);
    }
    u32::from_str_radix(digits, 16)
        .ok()
        .and_then(char::from_u32)
        .ok_or(consumed)
}

fn push_unescaped_character(input: &mut &str, first: char, output: &mut String) {
    if first != '\\' {
        output.push(first);
        return;
    }

    let Some(next) = input.chars().next() else {
        output.push('\\');
        return;
    };
    *input = &input[next.len_utf8()..];
    match next {
        'n' => output.push('\n'),
        'r' => output.push('\r'),
        't' => output.push('\t'),
        'u' => match parse_unicode_escape(input) {
            Ok(character) => output.push(character),
            Err(invalid) => {
                output.push('\\');
                output.push('u');
                output.push_str(invalid);
            }
        },
        character => output.push(character),
    }
}

fn canonicalize_entity_path_part_into(raw: &str, output: &mut String) {
    let mut input = raw;
    while let Some(first) = input.chars().next() {
        input = &input[first.len_utf8()..];
        push_unescaped_character(&mut input, first, output);
    }
}

struct EntityPathTokenIter<'a> {
    bytes: &'a [u8],
}

impl<'a> EntityPathTokenIter<'a> {
    fn new(raw: &'a str) -> Self {
        Self {
            bytes: raw.as_bytes(),
        }
    }
}

impl<'a> Iterator for EntityPathTokenIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.is_empty() {
            return None;
        }
        let mut index = 0;
        let mut is_in_escape = false;
        while index < self.bytes.len() {
            if !is_in_escape && self.bytes[index] == b'/' {
                break;
            }
            is_in_escape = self.bytes[index] == b'\\';
            index += 1;
        }
        if index == 0 {
            index = 1;
        }
        // UTF-8 is only split on either side of the one-byte ASCII `/`.
        let token = std::str::from_utf8(&self.bytes[..index]).ok()?;
        self.bytes = &self.bytes[index..];
        Some(token)
    }
}

fn entity_path_part_count(raw: &str) -> usize {
    EntityPathTokenIter::new(raw)
        .filter(|token| *token != "/")
        .count()
}

fn canonical_domain_identifier_hash(
    kind: RemoteMcapRawIdentifierKind,
    canonical_indices: &[usize],
) -> u64 {
    hash((kind, canonical_indices))
}

fn domain_identifiers_equal(
    left: &OwnedDomainIdentifier,
    right: &OwnedDomainIdentifier,
    canonical_identifiers: &[OwnedCanonicalIdentifier],
    part_references: &[usize],
) -> bool {
    let canonical = |index: usize| {
        let identifier = canonical_identifiers.get(index)?;
        identifier.raw.as_deref()
    };
    match (left, right) {
        (
            OwnedDomainIdentifier::Timeline { canonical: left },
            OwnedDomainIdentifier::Timeline { canonical: right },
        )
        | (
            OwnedDomainIdentifier::EntityPathPart { canonical: left },
            OwnedDomainIdentifier::EntityPathPart { canonical: right },
        )
        | (
            OwnedDomainIdentifier::Component { canonical: left },
            OwnedDomainIdentifier::Component { canonical: right },
        ) => canonical(*left) == canonical(*right),
        (
            OwnedDomainIdentifier::EntityPath {
                parts: (left_start, left_len),
            },
            OwnedDomainIdentifier::EntityPath {
                parts: (right_start, right_len),
            },
        ) => {
            let Some(left_parts) = part_references.get(*left_start..left_start + *left_len) else {
                return false;
            };
            let Some(right_parts) = part_references.get(*right_start..right_start + *right_len)
            else {
                return false;
            };
            left_parts.len() == right_parts.len()
                && left_parts
                    .iter()
                    .zip(right_parts)
                    .all(|(left, right)| canonical(*left) == canonical(*right))
        }
        _ => false,
    }
}

fn canonical_byte_upper_bound(identifier: RemoteMcapRawIdentifier<'_>) -> Option<u64> {
    u64::try_from(identifier.raw.len()).ok()
}

struct CensusBuilder {
    limits: RemoteMcapRuntimeInternLimits,
    fault: TestFault,
    canonical_identifiers: Vec<OwnedCanonicalIdentifier>,
    part_references: Vec<usize>,
    domain_identifiers: Vec<OwnedDomainIdentifier>,
    canonical_dedup: IntMap<u64, usize>,
    domain_dedup: IntMap<u64, usize>,
    string_capacity_bytes: u64,
    canonical_bytes: u64,
}

impl CensusBuilder {
    fn retained_bytes(&self) -> Result<u64, RemoteMcapRuntimeInternError> {
        checked_census_retained_bytes(
            self.string_capacity_bytes,
            self.canonical_identifiers.capacity(),
            self.domain_identifiers.capacity(),
            self.part_references.capacity(),
            self.canonical_dedup.capacity(),
            self.domain_dedup.capacity(),
        )
    }

    fn add_canonical(
        &mut self,
        kind: RemoteMcapRawIdentifierKind,
        raw: &str,
    ) -> Result<usize, RemoteMcapRuntimeInternError> {
        let canonical_capacity_bound =
            canonical_byte_upper_bound(RemoteMcapRawIdentifier::unchecked(kind, raw))
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        let temporary_peak = self
            .retained_bytes()?
            .checked_add(canonical_capacity_bound)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if temporary_peak > self.limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        if temporary_peak > self.limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }

        if self.fault == TestFault::CensusString {
            return Err(RemoteMcapRuntimeInternError::AllocationFailed);
        }
        let canonical_capacity = usize::try_from(canonical_capacity_bound)
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        let mut canonical = String::new();
        canonical
            .try_reserve_exact(canonical_capacity)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        let actual_canonical_capacity = u64::try_from(canonical.capacity())
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if actual_canonical_capacity > canonical_capacity_bound {
            return Err(RemoteMcapRuntimeInternError::ProtocolViolation);
        }
        match kind {
            RemoteMcapRawIdentifierKind::Timeline | RemoteMcapRawIdentifierKind::Component => {
                canonical.push_str(raw);
            }
            RemoteMcapRawIdentifierKind::EntityPathPart => {
                canonicalize_entity_path_part_into(raw, &mut canonical);
            }
            RemoteMcapRawIdentifierKind::EntityPath => {
                return Err(RemoteMcapRuntimeInternError::ProtocolViolation);
            }
        }
        if u64::try_from(canonical.len())
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?
            > canonical_capacity_bound
        {
            return Err(RemoteMcapRuntimeInternError::ProtocolViolation);
        }

        let canonical_hash = hash(&canonical);
        if let Some(index) = self.canonical_dedup.get(&canonical_hash).copied() {
            let existing = self
                .canonical_identifiers
                .get(index)
                .and_then(|identifier| identifier.raw.as_deref())
                .ok_or(RemoteMcapRuntimeInternError::ProtocolViolation)?;
            if existing != canonical {
                return Err(RemoteMcapRuntimeInternError::IdentifierHashCollision);
            }
            return Ok(index);
        }

        let canonical_len = u64::try_from(canonical.len())
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        self.canonical_bytes = self
            .canonical_bytes
            .checked_add(canonical_len)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        self.string_capacity_bytes = self
            .string_capacity_bytes
            .checked_add(actual_canonical_capacity)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        let retained_bytes = self.retained_bytes()?;
        if retained_bytes > self.limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        if retained_bytes > self.limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }

        let index = self.canonical_identifiers.len();
        self.canonical_identifiers.push(OwnedCanonicalIdentifier {
            hash: canonical_hash,
            raw: Some(canonical),
        });
        self.canonical_dedup.insert(canonical_hash, index);
        Ok(index)
    }

    fn add_domain(
        &mut self,
        domain_hash: u64,
        record: OwnedDomainIdentifier,
    ) -> Result<(), RemoteMcapRuntimeInternError> {
        if let Some(index) = self.domain_dedup.get(&domain_hash).copied() {
            let Some(existing) = self.domain_identifiers.get(index) else {
                return Err(RemoteMcapRuntimeInternError::ProtocolViolation);
            };
            if !domain_identifiers_equal(
                existing,
                &record,
                &self.canonical_identifiers,
                &self.part_references,
            ) {
                return Err(RemoteMcapRuntimeInternError::IdentifierHashCollision);
            }
            return Ok(());
        }

        let index = self.domain_identifiers.len();
        self.domain_identifiers.push(record);
        self.domain_dedup.insert(domain_hash, index);
        Ok(())
    }
}

impl BoundedRemoteIdentifierCensus {
    fn try_new(
        raw_identifiers: &[&str],
        limits: RemoteMcapRuntimeInternLimits,
        fault: TestFault,
    ) -> Result<Self, RemoteMcapRuntimeInternError> {
        Self::try_new_identifiers(
            raw_identifiers.iter().map(|&raw| {
                RemoteMcapRawIdentifier::unchecked(RemoteMcapRawIdentifierKind::Component, raw)
            }),
            limits,
            fault,
        )
    }

    fn try_new_identifiers<'a, I>(
        identifiers: I,
        limits: RemoteMcapRuntimeInternLimits,
        fault: TestFault,
    ) -> Result<Self, RemoteMcapRuntimeInternError>
    where
        I: IntoIterator<Item = RemoteMcapRawIdentifier<'a>>,
        I::IntoIter: Clone,
    {
        let input_identifiers = identifiers.into_iter();
        let max_identifiers = usize::try_from(limits.max_census_identifiers.get())
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::InvalidLimitProfile)?;

        // Validate the complete borrowed input before hashing or allocating anything. Duplicate
        // candidate positions and full-path part expansions are both deliberately counted here.
        let mut candidate_count = 0_usize;
        let mut candidate_raw_bytes = 0_u64;
        let mut canonical_bytes_upper_bound = 0_u64;
        let mut canonical_count_upper_bound = 0_usize;
        let mut part_reference_count_upper_bound = 0_usize;
        for identifier in input_identifiers.clone() {
            candidate_count = candidate_count
                .checked_add(1)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            if candidate_count > max_identifiers {
                return Err(RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded);
            }
            let raw_bytes = canonical_byte_upper_bound(identifier)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            candidate_raw_bytes =
                candidate_raw_bytes
                    .checked_add(u64::try_from(identifier.raw.len()).map_err(
                        |_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow,
                    )?)
                    .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            canonical_bytes_upper_bound = canonical_bytes_upper_bound
                .checked_add(raw_bytes)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            let canonical_positions = match identifier.kind {
                RemoteMcapRawIdentifierKind::EntityPath => entity_path_part_count(identifier.raw),
                RemoteMcapRawIdentifierKind::Timeline
                | RemoteMcapRawIdentifierKind::EntityPathPart
                | RemoteMcapRawIdentifierKind::Component => 1,
            };
            canonical_count_upper_bound = canonical_count_upper_bound
                .checked_add(canonical_positions)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            if identifier.kind == RemoteMcapRawIdentifierKind::EntityPath {
                part_reference_count_upper_bound = part_reference_count_upper_bound
                    .checked_add(canonical_positions)
                    .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            }
        }
        if candidate_raw_bytes > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRawBytesExceeded);
        }
        if canonical_bytes_upper_bound > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }

        let census_capacity_upper_bound = checked_census_capacity_upper_bound(
            canonical_bytes_upper_bound,
            canonical_count_upper_bound,
            candidate_count,
            part_reference_count_upper_bound,
        )?;
        if census_capacity_upper_bound > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        let candidate_peak_upper_bound = checked_candidate_peak_upper_bound(
            canonical_bytes_upper_bound,
            canonical_count_upper_bound,
            candidate_count,
            part_reference_count_upper_bound,
        )?;
        if candidate_peak_upper_bound > limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }

        let mut canonical_identifiers = Vec::new();
        let mut part_references = Vec::new();
        let mut domain_identifiers = Vec::new();
        let mut canonical_dedup = IntMap::<u64, usize>::default();
        let mut domain_dedup = IntMap::<u64, usize>::default();
        if fault == TestFault::CensusStructures {
            return Err(RemoteMcapRuntimeInternError::AllocationFailed);
        }
        canonical_identifiers
            .try_reserve_exact(canonical_count_upper_bound)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        part_references
            .try_reserve_exact(part_reference_count_upper_bound)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        domain_identifiers
            .try_reserve_exact(candidate_count)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        canonical_dedup
            .try_reserve(canonical_count_upper_bound)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        domain_dedup
            .try_reserve(candidate_count)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        if canonical_dedup.capacity()
            > conservative_hash_capacity_for_entries(canonical_count_upper_bound)?
            || domain_dedup.capacity() > conservative_hash_capacity_for_entries(candidate_count)?
        {
            return Err(RemoteMcapRuntimeInternError::InvalidLimitProfile);
        }

        let mut builder = CensusBuilder {
            limits,
            fault,
            canonical_identifiers,
            part_references,
            domain_identifiers,
            canonical_dedup,
            domain_dedup,
            string_capacity_bytes: 0,
            canonical_bytes: 0,
        };

        for identifier in input_identifiers {
            match identifier.kind {
                RemoteMcapRawIdentifierKind::Timeline => {
                    let canonical = builder.add_canonical(identifier.kind, identifier.raw)?;
                    let domain_hash =
                        canonical_domain_identifier_hash(identifier.kind, &[canonical]);
                    builder
                        .add_domain(domain_hash, OwnedDomainIdentifier::Timeline { canonical })?;
                }
                RemoteMcapRawIdentifierKind::EntityPathPart => {
                    if identifier.raw.is_empty() {
                        return Err(RemoteMcapRuntimeInternError::CensusCanonicalizationFailed);
                    }
                    let canonical = builder.add_canonical(identifier.kind, identifier.raw)?;
                    let domain_hash =
                        canonical_domain_identifier_hash(identifier.kind, &[canonical]);
                    builder.add_domain(
                        domain_hash,
                        OwnedDomainIdentifier::EntityPathPart { canonical },
                    )?;
                }
                RemoteMcapRawIdentifierKind::Component => {
                    let canonical = builder.add_canonical(identifier.kind, identifier.raw)?;
                    let domain_hash =
                        canonical_domain_identifier_hash(identifier.kind, &[canonical]);
                    builder
                        .add_domain(domain_hash, OwnedDomainIdentifier::Component { canonical })?;
                }
                RemoteMcapRawIdentifierKind::EntityPath => {
                    let start = builder.part_references.len();
                    let mut length = 0_usize;
                    for token in
                        EntityPathTokenIter::new(identifier.raw).filter(|token| *token != "/")
                    {
                        let canonical = builder
                            .add_canonical(RemoteMcapRawIdentifierKind::EntityPathPart, token)?;
                        builder.part_references.push(canonical);
                        length = length
                            .checked_add(1)
                            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
                    }
                    let end = start
                        .checked_add(length)
                        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
                    let domain_hash = builder
                        .part_references
                        .get(start..end)
                        .map(|canonical_indices| {
                            canonical_domain_identifier_hash(identifier.kind, canonical_indices)
                        })
                        .ok_or(RemoteMcapRuntimeInternError::ProtocolViolation)?;
                    builder.add_domain(
                        domain_hash,
                        OwnedDomainIdentifier::EntityPath {
                            parts: (start, length),
                        },
                    )?;
                }
            }
        }

        let retained_bytes = builder.retained_bytes()?;
        Ok(Self {
            identifiers: builder.domain_identifiers,
            part_references: builder.part_references,
            canonical_identifiers: builder.canonical_identifiers,
            candidate_identifiers: u64::try_from(candidate_count)
                .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?,
            candidate_raw_bytes,
            canonical_bytes: builder.canonical_bytes,
            retained_bytes,
            candidate_peak_upper_bound,
        })
    }

    /// Builds a field-private census from all four remote identifier families.
    pub fn try_new_v1<'a, I>(identifiers: I) -> Result<Self, RemoteMcapRuntimeInternError>
    where
        I: IntoIterator<Item = RemoteMcapRawIdentifier<'a>>,
        I::IntoIter: Clone,
    {
        let limits = super::GLOBAL_INTERNER
            .lock()
            .remote
            .as_ref()
            .ok_or(RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized)?
            .limits;
        Self::try_new_identifiers(identifiers, limits, TestFault::None)
    }

    /// Converts the complete census into a one-shot MCAP-012 construction authority.
    pub fn into_domain_construction_token(self) -> RemoteMcapDomainConstructionToken {
        RemoteMcapDomainConstructionToken { census: self }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Plan {
    existing_legacy: u64,
    existing_remote: u64,
    missing: u64,
    missing_string_bytes: u64,
    missing_entry_bytes: u64,
}

struct RemoteMcapRuntimeInternBudget {
    burned_string_bytes: u64,
    burned_entry_bytes: u64,
    burned_side_map_capacity_bytes: u64,
    revision: RemoteInternBudgetRevision,
}

struct RemoteRuntimeState {
    limits: RemoteMcapRuntimeInternLimits,
    side_map: IntMap<u64, &'static str>,
    max_side_map_entries: NonZeroUsize,
    budget: RemoteMcapRuntimeInternBudget,
}

struct PreparedRemoteState {
    state: RemoteRuntimeState,
}

impl PreparedRemoteState {
    fn plan(
        limits: RemoteMcapRuntimeInternLimits,
    ) -> Result<(NonZeroUsize, usize), RemoteMcapRuntimeInternError> {
        let combined_limit = limits.max_entry_and_capacity_bytes.get();
        let capacity_bytes_per_entry = entry_bytes()
            .checked_mul(2)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        let capacity_entry_upper = limits
            .max_candidate_peak_bytes
            .get()
            .checked_sub(64)
            .map(|bytes| bytes / capacity_bytes_per_entry)
            .ok_or(RemoteMcapRuntimeInternError::InvalidLimitProfile)?;
        let upper = (combined_limit / entry_bytes()).min(capacity_entry_upper);
        let upper = usize::try_from(upper)
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::InvalidLimitProfile)?;
        let mut requested = upper;
        loop {
            let requested_nonzero = NonZeroUsize::new(requested)
                .ok_or(RemoteMcapRuntimeInternError::InvalidLimitProfile)?;
            let conservative_capacity =
                conservative_hash_capacity_for_entries(requested_nonzero.get())?;
            let conservative_capacity_bytes = checked_capacity_bytes(conservative_capacity)?;
            let maximum_entry_bytes = u64::try_from(requested_nonzero.get())
                .ok()
                .and_then(|entries| entries.checked_mul(entry_bytes()))
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            let maximum_combined = conservative_capacity_bytes
                .checked_add(maximum_entry_bytes)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            if maximum_combined <= combined_limit
                && conservative_capacity_bytes <= limits.max_candidate_peak_bytes.get()
            {
                return Ok((requested_nonzero, conservative_capacity));
            }
            requested = requested_nonzero.get() / 2;
        }
    }

    fn prepare(
        limits: RemoteMcapRuntimeInternLimits,
        fault: TestFault,
    ) -> Result<Self, RemoteMcapRuntimeInternError> {
        let (requested, conservative_capacity) = Self::plan(limits)?;

        if fault == TestFault::SideMapCandidate {
            return Err(RemoteMcapRuntimeInternError::AllocationFailed);
        }
        let mut side_map = IntMap::default();
        side_map
            .try_reserve(requested.get())
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        if side_map.capacity() > conservative_capacity {
            return Err(RemoteMcapRuntimeInternError::InvalidLimitProfile);
        }
        let capacity_bytes = checked_capacity_bytes(side_map.capacity())?;
        let maximum_entry_bytes = u64::try_from(requested.get())
            .ok()
            .and_then(|entries| entries.checked_mul(entry_bytes()))
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if capacity_bytes
            .checked_add(maximum_entry_bytes)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?
            > limits.max_entry_and_capacity_bytes.get()
            || capacity_bytes > limits.max_candidate_peak_bytes.get()
        {
            return Err(RemoteMcapRuntimeInternError::InvalidLimitProfile);
        }
        Ok(Self {
            state: RemoteRuntimeState {
                limits,
                side_map,
                max_side_map_entries: requested,
                budget: RemoteMcapRuntimeInternBudget {
                    burned_string_bytes: 0,
                    burned_entry_bytes: 0,
                    burned_side_map_capacity_bytes: capacity_bytes,
                    revision: RemoteInternBudgetRevision(NonZeroU64::MIN),
                },
            },
        })
    }
}

pub(crate) struct CoordinatedStringInterner {
    legacy: StringInterner,
    remote: Option<RemoteRuntimeState>,
    coordination_revision: RemoteInternCoordinationRevision,
    coordination_revision_exhausted: bool,
}

impl Default for CoordinatedStringInterner {
    fn default() -> Self {
        Self {
            legacy: StringInterner::default(),
            remote: None,
            coordination_revision: RemoteInternCoordinationRevision(NonZeroU64::MIN),
            coordination_revision_exhausted: false,
        }
    }
}

impl CoordinatedStringInterner {
    pub(crate) fn bytes_used(&self) -> usize {
        let remote_bytes = self.remote.as_ref().map_or(0, |remote| {
            // V1 capacity accounting already contains the complete entry storage and its
            // control/allocation allowance, so adding used entries here would double count them.
            // Burned string bytes use the pre-leak `String::capacity`, not the visible `str::len`.
            usize::try_from(
                remote
                    .budget
                    .burned_side_map_capacity_bytes
                    .saturating_add(remote.budget.burned_string_bytes),
            )
            .unwrap_or(usize::MAX)
        });
        self.legacy.bytes_used().saturating_add(remote_bytes)
    }

    pub(crate) fn intern_legacy(&mut self, string: &str) -> InternedString {
        let string_hash = hash(string);
        if let Some(existing) = self.legacy.map.get(&string_hash) {
            return InternedString {
                hash: string_hash,
                string: existing,
            };
        }
        if let Some(existing) = self
            .remote
            .as_ref()
            .and_then(|remote| remote.side_map.get(&string_hash))
        {
            return InternedString {
                hash: string_hash,
                string: existing,
            };
        }

        let interned = self.legacy.intern(string);
        self.bump_coordination_after_legacy_insert();
        interned
    }

    fn bump_coordination_after_legacy_insert(&mut self) {
        if self.coordination_revision_exhausted {
            return;
        }
        match self.coordination_revision.checked_next() {
            Some(next) => self.coordination_revision = next,
            None => self.coordination_revision_exhausted = true,
        }
    }

    fn install_remote(
        &mut self,
        candidate: PreparedRemoteState,
    ) -> Result<RemoteMcapRuntimeInternSnapshot, RemoteMcapRuntimeInternError> {
        if let Some(remote) = &self.remote {
            if remote.limits != candidate.state.limits {
                return Err(
                    RemoteMcapRuntimeInternError::ModuleBudgetAlreadyInitializedWithDifferentLimits,
                );
            }
            return Ok(self.snapshot_remote(remote));
        }
        self.remote = Some(candidate.state);
        let remote = self
            .remote
            .as_ref()
            .ok_or(RemoteMcapRuntimeInternError::ProtocolViolation)?;
        Ok(self.snapshot_remote(remote))
    }

    fn snapshot_remote(&self, remote: &RemoteRuntimeState) -> RemoteMcapRuntimeInternSnapshot {
        RemoteMcapRuntimeInternSnapshot {
            capacity_accounting_version: RemoteInternCapacityAccountingVersion::V1,
            coordination_revision: self.coordination_revision,
            coordination_revision_exhausted: self.coordination_revision_exhausted,
            budget_revision: remote.budget.revision,
            legacy_entries: self.legacy.map.len() as u64,
            legacy_capacity: self.legacy.map.capacity() as u64,
            burned_string_bytes: remote.budget.burned_string_bytes,
            burned_entry_bytes: remote.budget.burned_entry_bytes,
            burned_side_map_capacity_bytes: remote.budget.burned_side_map_capacity_bytes,
            remote_entries: remote.side_map.len() as u64,
            remote_capacity: remote.side_map.capacity() as u64,
        }
    }

    fn checked_lookup(
        &self,
        raw: &str,
        string_hash: u64,
    ) -> Result<Lookup, RemoteMcapRuntimeInternError> {
        if let Some(existing) = self.legacy.map.get(&string_hash) {
            if **existing != *raw {
                return Err(RemoteMcapRuntimeInternError::IdentifierHashCollision);
            }
            return Ok(Lookup::Legacy(InternedString {
                hash: string_hash,
                string: existing,
            }));
        }
        if let Some(existing) = self
            .remote
            .as_ref()
            .and_then(|remote| remote.side_map.get(&string_hash))
        {
            if **existing != *raw {
                return Err(RemoteMcapRuntimeInternError::IdentifierHashCollision);
            }
            return Ok(Lookup::Remote(InternedString {
                hash: string_hash,
                string: existing,
            }));
        }
        Ok(Lookup::Missing)
    }

    fn plan_census(
        &self,
        census: &BoundedRemoteIdentifierCensus,
        mut resolved: Option<&mut [Option<InternedString>]>,
    ) -> Result<Plan, RemoteMcapRuntimeInternError> {
        let mut plan = Plan::default();
        for (index, identifier) in census.canonical_identifiers.iter().enumerate() {
            let raw = identifier
                .raw
                .as_deref()
                .ok_or(RemoteMcapRuntimeInternError::ProtocolViolation)?;
            let lookup = self.checked_lookup(raw, identifier.hash)?;
            if let Some(resolved) = &mut resolved {
                resolved[index] = lookup.handle();
            }
            match lookup {
                Lookup::Legacy(_) => {
                    plan.existing_legacy = plan
                        .existing_legacy
                        .checked_add(1)
                        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
                }
                Lookup::Remote(_) => {
                    plan.existing_remote = plan
                        .existing_remote
                        .checked_add(1)
                        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
                }
                Lookup::Missing => {
                    plan.missing = plan
                        .missing
                        .checked_add(1)
                        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
                    plan.missing_string_bytes = plan
                        .missing_string_bytes
                        .checked_add(
                            u64::try_from(
                                identifier
                                    .raw
                                    .as_ref()
                                    .ok_or(RemoteMcapRuntimeInternError::ProtocolViolation)?
                                    .capacity(),
                            )
                            .map_err(|_conversion_error| {
                                RemoteMcapRuntimeInternError::ArithmeticOverflow
                            })?,
                        )
                        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
                }
            }
        }
        plan.missing_entry_bytes = plan
            .missing
            .checked_mul(entry_bytes())
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        Ok(plan)
    }

    fn prepare_remote(
        &self,
        census: BoundedRemoteIdentifierCensus,
    ) -> Result<PreparedRemoteInternBatch, RemoteMcapRuntimeInternError> {
        let remote = self
            .remote
            .as_ref()
            .ok_or(RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized)?;
        Self::validate_census_again(&census, remote.limits)?;
        let plan = self.plan_census(&census, None)?;
        self.validate_plan(remote, &plan)?;
        let resolved_capacity_upper_bound =
            checked_vec_bytes::<Option<InternedString>>(census.canonical_identifiers.len())?;
        let part_handle_capacity_upper_bound =
            checked_vec_bytes::<InternedString>(census.part_references.len())?;
        let candidate_peak_before_result_allocation = census
            .retained_bytes
            .checked_add(resolved_capacity_upper_bound)
            .and_then(|bytes| bytes.checked_add(part_handle_capacity_upper_bound))
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if candidate_peak_before_result_allocation > remote.limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }
        let mut resolved = Vec::new();
        let mut part_handles = Vec::new();
        resolved
            .try_reserve_exact(census.canonical_identifiers.len())
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        part_handles
            .try_reserve_exact(census.part_references.len())
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        let actual_candidate_peak = census
            .retained_bytes
            .checked_add(checked_vec_bytes::<Option<InternedString>>(
                resolved.capacity(),
            )?)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        let actual_candidate_peak = checked_add_checked(
            actual_candidate_peak,
            checked_vec_bytes::<InternedString>(part_handles.capacity()),
        )?;
        if actual_candidate_peak > remote.limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }
        resolved.resize(census.canonical_identifiers.len(), None);
        let telemetry = self.telemetry(&census, plan, false);
        Ok(PreparedRemoteInternBatch {
            census,
            resolved,
            part_handles,
            telemetry,
        })
    }

    fn commit_remote(
        &mut self,
        mut prepared: PreparedRemoteInternBatch,
    ) -> Result<CommittedRemoteInternBatch, RemoteMcapRuntimeInternError> {
        let remote_limits = self
            .remote
            .as_ref()
            .ok_or(RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized)?
            .limits;
        Self::validate_census_again(&prepared.census, remote_limits)?;
        prepared.resolved.fill(None);
        let revision_changed = self.coordination_revision
            != prepared.telemetry.prepared_coordination_revision
            || self.coordination_revision_exhausted;
        let plan = self.plan_census(&prepared.census, Some(&mut prepared.resolved))?;
        {
            let remote = self
                .remote
                .as_ref()
                .ok_or(RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized)?;
            self.validate_plan(remote, &plan)?;
        }
        let next_coordination_revision = if plan.missing == 0 {
            None
        } else {
            if self.coordination_revision_exhausted {
                return Err(RemoteMcapRuntimeInternError::CoordinationRevisionExhausted);
            }
            Some(
                self.coordination_revision
                    .checked_next()
                    .ok_or(RemoteMcapRuntimeInternError::CoordinationRevisionExhausted)?,
            )
        };
        let next_budget_revision = if plan.missing == 0 {
            None
        } else {
            Some(
                self.remote
                    .as_ref()
                    .ok_or(RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized)?
                    .budget
                    .revision
                    .checked_next()
                    .ok_or(RemoteMcapRuntimeInternError::BudgetRevisionExhausted)?,
            )
        };
        let telemetry = self.telemetry_after_commit(&prepared, plan, revision_changed);

        // No operation below this point returns a recoverable error.
        let remote = self
            .remote
            .as_mut()
            .expect("remote state was validated under the same exclusive lock");
        let canonical_identifiers = std::mem::take(&mut prepared.census.canonical_identifiers);
        for (index, mut identifier) in canonical_identifiers.into_iter().enumerate() {
            if prepared.resolved[index].is_some() {
                continue;
            }
            let raw = identifier
                .raw
                .take()
                .expect("the complete preflight validated every raw owner");
            let static_string: &'static str = raw.leak();
            let previous = remote.side_map.insert(identifier.hash, static_string);
            assert!(
                previous.is_none(),
                "the complete remote interner preflight rejected duplicate hashes"
            );
            prepared.resolved[index] = Some(InternedString {
                hash: identifier.hash,
                string: static_string,
            });
        }
        for canonical in &prepared.census.part_references {
            let handle = prepared.resolved[*canonical]
                .expect("the complete preflight resolved every canonical identity");
            prepared.part_handles.push(handle);
        }
        if let Some(next) = next_coordination_revision {
            self.coordination_revision = next;
        }
        if let Some(next) = next_budget_revision {
            remote.budget.burned_string_bytes += plan.missing_string_bytes;
            remote.budget.burned_entry_bytes += plan.missing_entry_bytes;
            remote.budget.revision = next;
        }
        let module_snapshot = RemoteMcapRuntimeInternSnapshot {
            capacity_accounting_version: RemoteInternCapacityAccountingVersion::V1,
            coordination_revision: self.coordination_revision,
            coordination_revision_exhausted: self.coordination_revision_exhausted,
            budget_revision: remote.budget.revision,
            legacy_entries: self.legacy.map.len() as u64,
            legacy_capacity: self.legacy.map.capacity() as u64,
            burned_string_bytes: remote.budget.burned_string_bytes,
            burned_entry_bytes: remote.budget.burned_entry_bytes,
            burned_side_map_capacity_bytes: remote.budget.burned_side_map_capacity_bytes,
            remote_entries: remote.side_map.len() as u64,
            remote_capacity: remote.side_map.capacity() as u64,
        };
        Ok(CommittedRemoteInternBatch {
            identifiers: prepared.census.identifiers,
            part_handles: prepared.part_handles,
            // The domain records still index this canonical-handle vector.
            handles: prepared.resolved,
            telemetry,
            module_snapshot,
        })
    }

    fn validate_census_again(
        census: &BoundedRemoteIdentifierCensus,
        limits: RemoteMcapRuntimeInternLimits,
    ) -> Result<(), RemoteMcapRuntimeInternError> {
        let count = census.candidate_identifiers;
        if count > limits.max_census_identifiers.get() {
            return Err(RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded);
        }
        if census.candidate_raw_bytes > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRawBytesExceeded);
        }
        if census.canonical_bytes > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        if u64::try_from(census.canonical_identifiers.len())
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?
            > limits.max_census_retained_bytes.get()
        {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        if census.retained_bytes > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        if census.retained_bytes > limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }
        if census.candidate_peak_upper_bound > limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }
        Ok(())
    }

    fn validate_plan(
        &self,
        remote: &RemoteRuntimeState,
        plan: &Plan,
    ) -> Result<(), RemoteMcapRuntimeInternError> {
        let projected_entries = u64::try_from(remote.side_map.len())
            .ok()
            .and_then(|entries| entries.checked_add(plan.missing))
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if projected_entries > remote.max_side_map_entries.get() as u64 {
            return Err(RemoteMcapRuntimeInternError::SideMapEntryLimitExceeded);
        }
        let projected_string_bytes = remote
            .budget
            .burned_string_bytes
            .checked_add(plan.missing_string_bytes)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if projected_string_bytes > remote.limits.max_string_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::StringBudgetExceeded);
        }
        let projected_entry_bytes = remote
            .budget
            .burned_entry_bytes
            .checked_add(plan.missing_entry_bytes)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        let projected_combined = projected_entry_bytes
            .checked_add(remote.budget.burned_side_map_capacity_bytes)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if projected_combined > remote.limits.max_entry_and_capacity_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::EntryAndCapacityBudgetExceeded);
        }
        if plan.missing != 0 {
            if self.coordination_revision_exhausted
                || self.coordination_revision.checked_next().is_none()
            {
                return Err(RemoteMcapRuntimeInternError::CoordinationRevisionExhausted);
            }
            if remote.budget.revision.checked_next().is_none() {
                return Err(RemoteMcapRuntimeInternError::BudgetRevisionExhausted);
            }
        }
        Ok(())
    }

    fn telemetry(
        &self,
        census: &BoundedRemoteIdentifierCensus,
        plan: Plan,
        recomputed_after_revision_change: bool,
    ) -> RemoteInternBatchTelemetry {
        RemoteInternBatchTelemetry {
            candidate_identifiers: census.candidate_identifiers,
            candidate_raw_bytes: census.candidate_raw_bytes,
            unique_identifiers: census.identifiers.len() as u64,
            canonical_unique_identifiers: census.canonical_identifiers.len() as u64,
            canonical_bytes: census.canonical_bytes,
            retained_bytes: census.retained_bytes,
            candidate_peak_upper_bound: census.candidate_peak_upper_bound,
            existing_legacy: plan.existing_legacy,
            existing_remote: plan.existing_remote,
            missing: plan.missing,
            missing_string_bytes: plan.missing_string_bytes,
            missing_entry_bytes: plan.missing_entry_bytes,
            prepared_coordination_revision: self.coordination_revision,
            recomputed_after_revision_change,
        }
    }

    fn telemetry_after_commit(
        &self,
        prepared: &PreparedRemoteInternBatch,
        plan: Plan,
        recomputed_after_revision_change: bool,
    ) -> RemoteInternBatchTelemetry {
        RemoteInternBatchTelemetry {
            prepared_coordination_revision: prepared.telemetry.prepared_coordination_revision,
            ..self.telemetry(&prepared.census, plan, recomputed_after_revision_change)
        }
    }
}

#[derive(Clone, Copy)]
enum Lookup {
    Legacy(InternedString),
    Remote(InternedString),
    Missing,
}

impl Lookup {
    fn handle(self) -> Option<InternedString> {
        match self {
            Self::Legacy(handle) | Self::Remote(handle) => Some(handle),
            Self::Missing => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestFault {
    None,
    CensusStructures,
    CensusString,
    SideMapCandidate,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Barrier};

    use super::*;

    fn canonicalize_entity_path(raw: &str) -> Vec<String> {
        EntityPathTokenIter::new(raw)
            .filter(|token| *token != "/")
            .map(canonicalize_entity_path_part)
            .collect()
    }

    fn canonicalize_entity_path_part(raw: &str) -> String {
        let mut canonical = String::new();
        canonicalize_entity_path_part_into(raw, &mut canonical);
        canonical
    }

    #[test]
    fn entity_path_canonicalization_matches_forgiving_display_shape() {
        for (raw, expected) in [
            ("", Vec::new()),
            ("/", Vec::new()),
            ("///", Vec::new()),
            ("foo///bar/", vec!["foo".to_owned(), "bar".to_owned()]),
            (r"foo\/bar", vec!["foo/bar".to_owned()]),
            (r"foo\ bar\!", vec!["foo bar!".to_owned()]),
            (r"foo\bar", vec!["foobar".to_owned()]),
            (r"foo\", vec!["foo\\".to_owned()]),
            (r"\u{00E5}", vec!["å".to_owned()]),
            (r"\u{apa}", vec![r"\u{apa}".to_owned()]),
        ] {
            assert_eq!(canonicalize_entity_path(raw), expected, "raw: {raw:?}");
        }

        assert_eq!(canonicalize_entity_path_part(r"\u{apa}"), r"\u{apa}");
        assert_eq!(canonicalize_entity_path_part(r"foo\!"), "foo!");
    }

    #[test]
    fn entity_path_canonicalization_matches_malformed_tokenizer_semantics() {
        assert_eq!(
            canonicalize_entity_path(r"foo\\\\/bar"),
            vec![r"foo\\/bar".to_owned()],
        );
        assert_eq!(canonicalize_entity_path_part(r"\u{å}"), r"\u{å}");
        assert_eq!(canonicalize_entity_path_part(r"\u{åå"), r"\u{åå");
        assert_eq!(canonicalize_entity_path_part(r"\u{😀😀"), r"\u{😀😀",);
    }

    #[test]
    fn root_entity_path_is_a_domain_record_not_a_flattened_string() {
        let mut coordinator = initialized(generous_limits());
        let prepared = prepare_identifier_local(
            &coordinator,
            &[RemoteMcapRawIdentifier::entity_path("/")],
            TestFault::None,
        )
        .unwrap();
        assert_eq!(prepared.telemetry().unique_identifiers, 1);
        assert_eq!(prepared.telemetry().canonical_unique_identifiers, 0);
        let committed = coordinator.commit_remote(prepared).unwrap();

        assert_eq!(committed.len(), 1);
        assert!(!committed.is_empty());
        assert_eq!(committed.handle(0), None);
        match committed.domain_handle(0).unwrap() {
            RemoteMcapDomainIdentifierHandle::EntityPath(parts) => assert_eq!(parts.len(), 0),
            handle => panic!("unexpected domain handle: {handle:?}"),
        }
    }

    fn nz(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).unwrap()
    }

    fn limits(
        strings: u64,
        entries_and_capacity: u64,
        census_count: u64,
        census_bytes: u64,
        candidate_peak: u64,
    ) -> RemoteMcapRuntimeInternLimits {
        RemoteMcapRuntimeInternLimits::from_profile_values(
            nz(strings),
            nz(entries_and_capacity),
            nz(census_count),
            nz(census_bytes),
            nz(candidate_peak),
        )
    }

    fn generous_limits() -> RemoteMcapRuntimeInternLimits {
        limits(16 * 1024, 128 * 1024, 64, 32 * 1024, 128 * 1024)
    }

    fn initialized(limits: RemoteMcapRuntimeInternLimits) -> CoordinatedStringInterner {
        let candidate = PreparedRemoteState::prepare(limits, TestFault::None).unwrap();
        let mut coordinator = CoordinatedStringInterner::default();
        coordinator.install_remote(candidate).unwrap();
        coordinator
    }

    fn prepare_local(
        coordinator: &CoordinatedStringInterner,
        raw: &[&str],
        fault: TestFault,
    ) -> Result<PreparedRemoteInternBatch, RemoteMcapRuntimeInternError> {
        let limits = coordinator.remote.as_ref().unwrap().limits;
        let census = BoundedRemoteIdentifierCensus::try_new(raw, limits, fault)?;
        coordinator.prepare_remote(census)
    }

    fn prepare_identifier_local(
        coordinator: &CoordinatedStringInterner,
        identifiers: &[RemoteMcapRawIdentifier<'_>],
        fault: TestFault,
    ) -> Result<PreparedRemoteInternBatch, RemoteMcapRuntimeInternError> {
        let limits = coordinator.remote.as_ref().unwrap().limits;
        let census = BoundedRemoteIdentifierCensus::try_new_identifiers(
            identifiers.iter().copied(),
            limits,
            fault,
        )?;
        coordinator.prepare_remote(census)
    }

    #[test]
    fn raw_identifier_census_canonicalizes_and_deduplicates_all_families() {
        let mut coordinator = initialized(generous_limits());
        let identifiers = [
            RemoteMcapRawIdentifier::timeline("message_log_time").unwrap(),
            RemoteMcapRawIdentifier::component("McapMessage").unwrap(),
            RemoteMcapRawIdentifier::entity_path("/foo///bar/"),
            RemoteMcapRawIdentifier::entity_path("foo/bar"),
            RemoteMcapRawIdentifier::entity_path(r"foo/remote path!"),
            RemoteMcapRawIdentifier::entity_path_part(r"remote path!").unwrap(),
            RemoteMcapRawIdentifier::entity_path_part(r"remote\ path\!").unwrap(),
        ];

        assert_eq!(
            canonicalize_entity_path("/foo///bar/"),
            canonicalize_entity_path("foo/bar")
        );
        assert_eq!(
            canonicalize_entity_path_part(r"remote path!"),
            canonicalize_entity_path_part(r"remote\ path\!")
        );

        let prepared =
            prepare_identifier_local(&coordinator, &identifiers, TestFault::None).unwrap();
        assert_eq!(prepared.telemetry().unique_identifiers, 5);
        assert_eq!(prepared.telemetry().canonical_unique_identifiers, 5);
        let committed = coordinator.commit_remote(prepared).unwrap();

        assert_eq!(committed.len(), 5);
        assert_eq!(committed.handle(0).unwrap().as_str(), "message_log_time");
        assert_eq!(committed.handle(1).unwrap().as_str(), "McapMessage");
        assert_eq!(committed.handle(2), None);
        assert_eq!(committed.handle(3), None);
        assert_eq!(committed.handle(4).unwrap().as_str(), "remote path!");
        match committed.domain_handle(2).unwrap() {
            RemoteMcapDomainIdentifierHandle::EntityPath(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0].as_str(), "foo");
                assert_eq!(parts[1].as_str(), "bar");
            }
            handle => panic!("unexpected domain handle: {handle:?}"),
        }
        match committed.domain_handle(3).unwrap() {
            RemoteMcapDomainIdentifierHandle::EntityPath(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0].as_str(), "foo");
                assert_eq!(parts[1].as_str(), "remote path!");
            }
            handle => panic!("unexpected domain handle: {handle:?}"),
        }
        assert_eq!(
            committed.handle(4).unwrap(),
            match committed.domain_handle(4).unwrap() {
                RemoteMcapDomainIdentifierHandle::EntityPathPart(handle) => handle,
                handle => panic!("unexpected domain handle: {handle:?}"),
            }
        );
    }

    #[test]
    fn raw_census_failures_have_zero_intern_effect() {
        let coordinator = initialized(generous_limits());
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        let fail_locally = |identifiers: &[RemoteMcapRawIdentifier<'_>]| {
            BoundedRemoteIdentifierCensus::try_new_identifiers(
                identifiers.iter().copied(),
                coordinator.remote.as_ref().unwrap().limits,
                TestFault::None,
            )
        };

        assert_eq!(
            RemoteMcapRawIdentifier::timeline("").unwrap_err(),
            RemoteMcapRuntimeInternError::CensusCanonicalizationFailed
        );
        assert_eq!(
            RemoteMcapRawIdentifier::component("").unwrap_err(),
            RemoteMcapRuntimeInternError::CensusCanonicalizationFailed
        );
        assert_eq!(
            fail_locally(&[RemoteMcapRawIdentifier::unchecked(
                RemoteMcapRawIdentifierKind::EntityPathPart,
                "",
            )])
            .unwrap_err(),
            RemoteMcapRuntimeInternError::CensusCanonicalizationFailed
        );

        let oversized_raw = "x".repeat(32 * 1024 + 1);
        assert_eq!(
            fail_locally(&[RemoteMcapRawIdentifier::timeline(&oversized_raw).unwrap()])
                .unwrap_err(),
            RemoteMcapRuntimeInternError::CensusRawBytesExceeded
        );

        let too_many = (0..65).map(|index| {
            RemoteMcapRawIdentifier::timeline(Box::leak(
                format!("raw-census-count-{index}").into_boxed_str(),
            ))
            .unwrap()
        });
        assert_eq!(
            BoundedRemoteIdentifierCensus::try_new_identifiers(
                too_many,
                coordinator.remote.as_ref().unwrap().limits,
                TestFault::None,
            )
            .unwrap_err(),
            RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded
        );
        assert_eq!(
            coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap()),
            before
        );
    }

    #[test]
    fn telemetry_separates_raw_candidates_from_canonical_retention() {
        let coordinator = initialized(generous_limits());
        let identifiers = [
            RemoteMcapRawIdentifier::timeline("same").unwrap(),
            RemoteMcapRawIdentifier::timeline("same").unwrap(),
            RemoteMcapRawIdentifier::entity_path("/foo///bar/"),
            RemoteMcapRawIdentifier::entity_path("foo/bar"),
            RemoteMcapRawIdentifier::entity_path_part(r"remote\ path").unwrap(),
            RemoteMcapRawIdentifier::entity_path_part("remote path").unwrap(),
        ];
        let prepared =
            prepare_identifier_local(&coordinator, &identifiers, TestFault::None).unwrap();
        let telemetry = prepared.telemetry();

        assert_eq!(telemetry.candidate_identifiers, 6);
        assert_eq!(
            telemetry.candidate_raw_bytes,
            identifiers
                .iter()
                .map(|identifier| identifier.as_str().len() as u64)
                .sum::<u64>()
        );
        assert_eq!(telemetry.unique_identifiers, 3);
        assert_eq!(telemetry.canonical_unique_identifiers, 4);
        assert_eq!(
            telemetry.canonical_bytes,
            ["same", "foo", "bar", "remote path"]
                .iter()
                .map(|identifier| identifier.len() as u64)
                .sum::<u64>()
        );
        assert!(telemetry.retained_bytes > telemetry.canonical_bytes);
        assert!(telemetry.candidate_peak_upper_bound >= telemetry.retained_bytes);
    }

    #[test]
    fn legacy_unescaped_handles_reuse_without_second_intern_pass() {
        let mut coordinator = initialized(generous_limits());
        let first = coordinator.intern_legacy("first");
        let second = coordinator.intern_legacy("legacy part!");
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());

        let prepared = prepare_identifier_local(
            &coordinator,
            &[
                RemoteMcapRawIdentifier::entity_path(r"first/legacy\ part\!"),
                RemoteMcapRawIdentifier::entity_path_part(r"legacy\ part\!").unwrap(),
            ],
            TestFault::None,
        )
        .unwrap();
        assert_eq!(prepared.telemetry().existing_legacy, 2);
        assert_eq!(prepared.telemetry().missing, 0);
        let committed = coordinator.commit_remote(prepared).unwrap();

        assert_eq!(committed.handle(0), None);
        assert_eq!(committed.handle(1).unwrap(), second);
        match committed.domain_handle(0).unwrap() {
            RemoteMcapDomainIdentifierHandle::EntityPath(parts) => {
                assert_eq!(parts[0], first);
                assert_eq!(parts[1], second);
                assert!(std::ptr::eq(parts[0].as_str(), first.as_str()));
                assert!(std::ptr::eq(parts[1].as_str(), second.as_str()));
            }
            handle => panic!("unexpected domain handle: {handle:?}"),
        }
        assert_eq!(
            coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap()),
            before
        );
    }

    #[test]
    fn canonical_identity_is_owned_and_counted_before_domain_construction() {
        let coordinator = initialized(generous_limits());
        let raw = r"\u{262E}";
        let canonical = "☮";
        let census = BoundedRemoteIdentifierCensus::try_new_identifiers(
            [
                RemoteMcapRawIdentifier::entity_path(raw),
                RemoteMcapRawIdentifier::component("shared").unwrap(),
                RemoteMcapRawIdentifier::timeline("shared").unwrap(),
            ],
            coordinator.remote.as_ref().unwrap().limits,
            TestFault::None,
        )
        .unwrap();

        assert!(census.canonical_bytes < census.candidate_raw_bytes);
        assert_eq!(census.identifiers.len(), 3);
        assert_eq!(census.canonical_identifiers.len(), 2);
        assert_eq!(
            census.canonical_identifiers[0].raw.as_deref(),
            Some(canonical)
        );
        assert_eq!(
            census.canonical_identifiers[1].raw.as_deref(),
            Some("shared")
        );

        let token = census.into_domain_construction_token();
        let debug = format!("{token:?}");
        assert!(!debug.contains(raw));
        assert!(!debug.contains("shared"));
    }

    #[test]
    fn path_part_result_capacity_counts_toward_candidate_preflight() {
        let coordinator = initialized(generous_limits());
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        let identifiers = [
            RemoteMcapRawIdentifier::entity_path(r"\u{262E}"),
            RemoteMcapRawIdentifier::entity_path(r"\u{262E}"),
        ];
        let raw_bytes = identifiers
            .iter()
            .map(|identifier| identifier.as_str().len() as u64)
            .sum::<u64>();
        let canonical_bytes_upper_bound = identifiers
            .iter()
            .map(|identifier| canonical_byte_upper_bound(*identifier).unwrap())
            .sum::<u64>();
        assert!(raw_bytes > 0);
        let complete_peak =
            checked_candidate_peak_upper_bound(canonical_bytes_upper_bound, 2, 2, 2).unwrap();

        let limits = limits(16 * 1024, 128 * 1024, 2, 32 * 1024, complete_peak - 1);
        assert_eq!(
            BoundedRemoteIdentifierCensus::try_new_identifiers(
                identifiers,
                limits,
                TestFault::CensusStructures,
            )
            .unwrap_err(),
            RemoteMcapRuntimeInternError::CandidatePeakExceeded
        );
        assert_eq!(
            coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap()),
            before
        );
    }

    #[test]
    fn domain_construction_token_is_redeemed_by_the_complete_transaction() {
        let mut coordinator = initialized(generous_limits());
        let census = BoundedRemoteIdentifierCensus::try_new_identifiers(
            [
                RemoteMcapRawIdentifier::timeline("token-timeline").unwrap(),
                RemoteMcapRawIdentifier::entity_path("//token/path"),
                RemoteMcapRawIdentifier::entity_path_part("token part").unwrap(),
                RemoteMcapRawIdentifier::component("TokenComponent").unwrap(),
            ],
            coordinator.remote.as_ref().unwrap().limits,
            TestFault::None,
        )
        .unwrap();
        let token = census.into_domain_construction_token();
        let prepared = coordinator.prepare_remote(token.census).unwrap();
        let committed = coordinator.commit_remote(prepared).unwrap();

        assert_eq!(committed.telemetry().unique_identifiers, 4);
        assert_eq!(committed.telemetry().canonical_unique_identifiers, 5);
        assert_eq!(committed.len(), 4);
        assert_eq!(committed.handle(1), None);
        match committed.domain_handle(1).unwrap() {
            RemoteMcapDomainIdentifierHandle::EntityPath(parts) => {
                assert_eq!(parts[0].as_str(), "token");
                assert_eq!(parts[1].as_str(), "path");
            }
            handle => panic!("unexpected domain handle: {handle:?}"),
        }
        assert!(
            [0, 2, 3]
                .iter()
                .all(|index| committed.handle(*index).is_some())
        );
    }

    #[test]
    fn mixed_duplicate_legacy_remote_and_missing_identifiers_are_exact() {
        let mut coordinator = initialized(generous_limits());
        let legacy = coordinator.intern_legacy("legacy-existing");
        let remote_first =
            prepare_local(&coordinator, &["remote-existing"], TestFault::None).unwrap();
        let remote_first = coordinator.commit_remote(remote_first).unwrap();
        let remote = remote_first.handle(0).unwrap();
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());

        let prepared = prepare_local(
            &coordinator,
            &[
                "legacy-existing",
                "remote-existing",
                "new-one",
                "new-one",
                "new-two",
            ],
            TestFault::None,
        )
        .unwrap();
        assert_eq!(prepared.telemetry().unique_identifiers, 4);
        assert_eq!(prepared.telemetry().existing_legacy, 1);
        assert_eq!(prepared.telemetry().existing_remote, 1);
        assert_eq!(prepared.telemetry().missing, 2);
        let committed = coordinator.commit_remote(prepared).unwrap();

        assert_eq!(committed.len(), 4);
        assert_eq!(committed.handle(0), Some(legacy));
        assert_eq!(committed.handle(1), Some(remote));
        assert_eq!(committed.handle(2).unwrap().as_str(), "new-one");
        assert_eq!(committed.handle(3).unwrap().as_str(), "new-two");
        assert_eq!(
            committed.module_snapshot().remote_entries,
            before.remote_entries + 2
        );
    }

    #[test]
    fn remote_then_legacy_reuses_the_exact_handle_without_second_identity() {
        let mut coordinator = initialized(generous_limits());
        let prepared = prepare_local(&coordinator, &["shared"], TestFault::None).unwrap();
        let remote = coordinator
            .commit_remote(prepared)
            .unwrap()
            .handle(0)
            .unwrap();
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        let legacy = coordinator.intern_legacy("shared");
        let after = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());

        assert_eq!(remote, legacy);
        assert!(std::ptr::eq(remote.as_str(), legacy.as_str()));
        assert_eq!(before, after);
        assert_eq!(coordinator.legacy.len(), 0);
    }

    #[test]
    fn legacy_race_is_recomputed_without_duplicate_burn() {
        let mut coordinator = initialized(generous_limits());
        let prepared = prepare_local(&coordinator, &["raced", "new"], TestFault::None).unwrap();
        let before_race = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        let raced = coordinator.intern_legacy("raced");
        let committed = coordinator.commit_remote(prepared).unwrap();

        assert_eq!(committed.handle(0), Some(raced));
        assert_eq!(committed.telemetry().existing_legacy, 1);
        assert_eq!(committed.telemetry().missing, 1);
        assert!(committed.telemetry().recomputed_after_revision_change);
        assert_eq!(
            committed.module_snapshot().burned_entry_bytes,
            before_race.burned_entry_bytes + entry_bytes()
        );
    }

    #[test]
    fn census_and_candidate_failures_have_zero_global_effect() {
        let coordinator = initialized(generous_limits());
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        for fault in [TestFault::CensusStructures, TestFault::CensusString] {
            assert_eq!(
                prepare_local(&coordinator, &["never-retained"], fault).unwrap_err(),
                RemoteMcapRuntimeInternError::AllocationFailed
            );
            assert_eq!(
                coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap()),
                before
            );
        }

        assert_eq!(
            PreparedRemoteState::prepare(generous_limits(), TestFault::SideMapCandidate)
                .err()
                .unwrap(),
            RemoteMcapRuntimeInternError::AllocationFailed
        );
    }

    #[test]
    fn census_count_raw_and_retained_caps_fail_before_global_mutation() {
        let count_limited = initialized(limits(4096, 32768, 1, 4096, 32768));
        let before = count_limited.snapshot_remote(count_limited.remote.as_ref().unwrap());
        assert_eq!(
            prepare_local(&count_limited, &["a", "b"], TestFault::None).unwrap_err(),
            RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded
        );
        assert_eq!(
            count_limited.snapshot_remote(count_limited.remote.as_ref().unwrap()),
            before
        );

        let raw_limited = initialized(limits(4096, 32768, 8, 4, 32768));
        assert_eq!(
            prepare_local(&raw_limited, &["12345"], TestFault::None).unwrap_err(),
            RemoteMcapRuntimeInternError::CensusRawBytesExceeded
        );

        let retained_limited = initialized(limits(4096, 32768, 8, 64, 32768));
        assert_eq!(
            prepare_local(&retained_limited, &["a"], TestFault::None).unwrap_err(),
            RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded
        );
    }

    #[test]
    fn census_candidate_preflight_covers_raw_structures_and_result_capacity() {
        let huge = "x".repeat(4096);
        assert_eq!(
            BoundedRemoteIdentifierCensus::try_new(
                &[huge.as_str()],
                limits(8192, 32768, 8, 4095, 32768),
                TestFault::CensusString,
            )
            .unwrap_err(),
            RemoteMcapRuntimeInternError::CensusRawBytesExceeded
        );

        let duplicate_positions = ["same"; 8];
        let raw_bytes = duplicate_positions
            .iter()
            .map(|raw| raw.len() as u64)
            .sum::<u64>();
        let complete_peak = checked_candidate_peak_upper_bound(
            raw_bytes,
            duplicate_positions.len(),
            duplicate_positions.len(),
            0,
        )
        .unwrap();
        let one_byte_short = limits(
            8192,
            32768,
            duplicate_positions.len() as u64,
            32768,
            complete_peak - 1,
        );
        assert_eq!(
            BoundedRemoteIdentifierCensus::try_new(
                &duplicate_positions,
                one_byte_short,
                TestFault::CensusStructures,
            )
            .unwrap_err(),
            RemoteMcapRuntimeInternError::CandidatePeakExceeded
        );

        let without_result = checked_census_capacity_upper_bound(1, 1, 1, 0).unwrap();
        let with_result = checked_candidate_peak_upper_bound(1, 1, 1, 0).unwrap();
        assert!(with_result > without_result);
        assert_eq!(
            BoundedRemoteIdentifierCensus::try_new(
                &["x"],
                limits(8192, 32768, 1, 32768, with_result - 1),
                TestFault::CensusStructures,
            )
            .unwrap_err(),
            RemoteMcapRuntimeInternError::CandidatePeakExceeded
        );
    }

    #[test]
    fn side_map_candidate_cap_is_checked_before_the_only_allocation_attempt() {
        let minimum_capacity = checked_capacity_bytes(
            conservative_hash_capacity_for_entries(NonZeroUsize::MIN.get()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            PreparedRemoteState::prepare(
                limits(8192, 1024 * 1024, 8, 32768, minimum_capacity - 1),
                TestFault::SideMapCandidate,
            )
            .err()
            .unwrap(),
            RemoteMcapRuntimeInternError::InvalidLimitProfile
        );

        let prepared = PreparedRemoteState::prepare(
            limits(8192, 1024 * 1024, 8, 32768, minimum_capacity),
            TestFault::None,
        )
        .unwrap();
        assert!(prepared.state.max_side_map_entries.get() >= 1);
        assert!(prepared.state.side_map.capacity() <= 4);
        assert!(prepared.state.budget.burned_side_map_capacity_bytes <= minimum_capacity);
    }

    #[test]
    fn complete_web_test_profile_values_create_a_local_prepared_state() {
        let portable_minimum = limits(64, 280, 64, 64, 256);
        validate_remote_mcap_runtime_intern_profile(portable_minimum).unwrap();
        PreparedRemoteState::prepare(portable_minimum, TestFault::None).unwrap();

        let web_profile = limits(64, 512, 64, 64, 512);
        validate_remote_mcap_runtime_intern_profile(web_profile).unwrap();
        let prepared = PreparedRemoteState::prepare(web_profile, TestFault::None).unwrap();
        assert!(prepared.state.max_side_map_entries.get() >= 1);
        assert!(prepared.state.budget.burned_side_map_capacity_bytes <= 512);
    }

    #[test]
    fn initialization_fast_path_is_deterministic_and_race_safe() {
        let coordinator = parking_lot::Mutex::new(CoordinatedStringInterner::default());
        let original = initialize_remote_at(&coordinator, generous_limits(), TestFault::None)
            .expect("first initialization succeeds");
        assert_eq!(
            initialize_remote_at(&coordinator, generous_limits(), TestFault::SideMapCandidate,)
                .unwrap(),
            original
        );
        assert_eq!(
            initialize_remote_at(
                &coordinator,
                limits(16 * 1024 + 1, 128 * 1024, 64, 32 * 1024, 128 * 1024),
                TestFault::SideMapCandidate,
            )
            .unwrap_err(),
            RemoteMcapRuntimeInternError::ModuleBudgetAlreadyInitializedWithDifferentLimits
        );
        let after_conflict = {
            let coordinator = coordinator.lock();
            coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap())
        };
        assert_eq!(after_conflict, original);

        let raced = Arc::new(parking_lot::Mutex::new(CoordinatedStringInterner::default()));
        let start = Arc::new(Barrier::new(3));
        let limits_a = generous_limits();
        let limits_b = limits(16 * 1024 + 1, 128 * 1024, 64, 32 * 1024, 128 * 1024);
        let spawn = |limits| {
            let raced = Arc::clone(&raced);
            let start = Arc::clone(&start);
            std::thread::Builder::new()
                .name("remote-interner-initialize".to_owned())
                .spawn(move || {
                    start.wait();
                    initialize_remote_at(&raced, limits, TestFault::None)
                })
                .unwrap()
        };
        let first = spawn(limits_a);
        let second = spawn(limits_b);
        start.wait();
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(RemoteMcapRuntimeInternError::ModuleBudgetAlreadyInitializedWithDifferentLimits)
                ))
                .count(),
            1
        );
    }

    #[test]
    fn each_budget_axis_fails_one_byte_short_without_partial_prefix() {
        let baseline = initialized(generous_limits());
        let census = BoundedRemoteIdentifierCensus::try_new(
            &["budget-axis"],
            generous_limits(),
            TestFault::None,
        )
        .unwrap();
        let plan = baseline.plan_census(&census, None).unwrap();

        let string_short = initialized(limits(
            plan.missing_string_bytes - 1,
            128 * 1024,
            64,
            32 * 1024,
            128 * 1024,
        ));
        let prepared = prepare_local(&string_short, &["budget-axis"], TestFault::None).unwrap_err();
        assert_eq!(prepared, RemoteMcapRuntimeInternError::StringBudgetExceeded);
        assert_eq!(string_short.remote.as_ref().unwrap().side_map.len(), 0);

        let mut entry_short = initialized(generous_limits());
        let remote = entry_short.remote.as_mut().unwrap();
        remote.budget.burned_entry_bytes = remote.limits.max_entry_and_capacity_bytes.get()
            - remote.budget.burned_side_map_capacity_bytes
            - plan.missing_entry_bytes
            + 1;
        let before = entry_short.snapshot_remote(entry_short.remote.as_ref().unwrap());
        assert_eq!(
            prepare_local(&entry_short, &["budget-axis"], TestFault::None).unwrap_err(),
            RemoteMcapRuntimeInternError::EntryAndCapacityBudgetExceeded
        );
        assert_eq!(
            entry_short.snapshot_remote(entry_short.remote.as_ref().unwrap()),
            before
        );

        let mut minimum_map = IntMap::<u64, &'static str>::default();
        minimum_map.try_reserve(1).unwrap();
        let minimum_capacity_bytes = checked_capacity_bytes(minimum_map.capacity()).unwrap();
        assert_eq!(
            PreparedRemoteState::prepare(
                limits(4096, 128 * 1024, 64, 32 * 1024, minimum_capacity_bytes - 1,),
                TestFault::None,
            )
            .err()
            .unwrap(),
            RemoteMcapRuntimeInternError::InvalidLimitProfile
        );
    }

    #[test]
    fn exhausted_string_budget_still_accepts_existing_identifiers() {
        let mut coordinator = initialized(generous_limits());
        let prepared = prepare_local(&coordinator, &["only"], TestFault::None).unwrap();
        let existing = coordinator
            .commit_remote(prepared)
            .unwrap()
            .handle(0)
            .unwrap();
        coordinator
            .remote
            .as_mut()
            .unwrap()
            .budget
            .burned_string_bytes = coordinator
            .remote
            .as_ref()
            .unwrap()
            .limits
            .max_string_bytes
            .get();

        let duplicate = prepare_local(&coordinator, &["only"], TestFault::None).unwrap();
        let committed = coordinator.commit_remote(duplicate).unwrap();
        assert_eq!(committed.handle(0), Some(existing));
        assert_eq!(committed.telemetry().missing, 0);
    }

    #[test]
    fn fixed_side_map_entry_limit_rejects_the_next_batch_atomically() {
        let mut coordinator = initialized(limits(1024, 512, 8, 4096, 4096));
        let max_entries = coordinator
            .remote
            .as_ref()
            .unwrap()
            .max_side_map_entries
            .get();
        let owned = (0..max_entries)
            .map(|index| format!("fixed-{index}"))
            .collect::<Vec<_>>();
        for chunk in owned.chunks(8) {
            let raw = chunk.iter().map(String::as_str).collect::<Vec<_>>();
            let prepared = prepare_local(&coordinator, &raw, TestFault::None).unwrap();
            coordinator.commit_remote(prepared).unwrap();
        }
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        assert_eq!(
            prepare_local(&coordinator, &["one-too-many"], TestFault::None).unwrap_err(),
            RemoteMcapRuntimeInternError::SideMapEntryLimitExceeded
        );
        assert_eq!(
            coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap()),
            before
        );
    }

    #[test]
    fn revisions_near_max_never_wrap_or_partially_commit() {
        let mut coordinator = initialized(generous_limits());
        coordinator.coordination_revision = RemoteInternCoordinationRevision(NonZeroU64::MAX);
        let prepared =
            prepare_local(&coordinator, &["cannot-commit"], TestFault::None).unwrap_err();
        assert_eq!(
            prepared,
            RemoteMcapRuntimeInternError::CoordinationRevisionExhausted
        );
        assert_eq!(coordinator.remote.as_ref().unwrap().side_map.len(), 0);

        let mut coordinator = initialized(generous_limits());
        coordinator.remote.as_mut().unwrap().budget.revision =
            RemoteInternBudgetRevision(NonZeroU64::MAX);
        let prepared = prepare_local(&coordinator, &["cannot-burn"], TestFault::None).unwrap_err();
        assert_eq!(
            prepared,
            RemoteMcapRuntimeInternError::BudgetRevisionExhausted
        );
        assert_eq!(coordinator.remote.as_ref().unwrap().side_map.len(), 0);
    }

    #[test]
    fn stale_budget_revalidation_fails_without_partial_commit() {
        let mut coordinator = initialized(generous_limits());
        let prepared = prepare_local(&coordinator, &["stale-budget"], TestFault::None).unwrap();
        let remote = coordinator.remote.as_mut().unwrap();
        remote.budget.burned_string_bytes = remote.limits.max_string_bytes.get();
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());

        assert_eq!(
            coordinator.commit_remote(prepared).unwrap_err(),
            RemoteMcapRuntimeInternError::StringBudgetExceeded
        );
        assert_eq!(
            coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap()),
            before
        );
        assert_eq!(coordinator.remote.as_ref().unwrap().side_map.len(), 0);
    }

    #[test]
    fn legacy_constructor_survives_coordination_exhaustion() {
        let mut coordinator = initialized(generous_limits());
        coordinator.coordination_revision = RemoteInternCoordinationRevision(NonZeroU64::MAX);
        let legacy = coordinator.intern_legacy("legacy-after-revision-max");
        assert_eq!(legacy.as_str(), "legacy-after-revision-max");
        assert!(coordinator.coordination_revision_exhausted);

        let existing = prepare_local(
            &coordinator,
            &["legacy-after-revision-max"],
            TestFault::None,
        )
        .unwrap();
        assert_eq!(
            coordinator.commit_remote(existing).unwrap().handle(0),
            Some(legacy)
        );
        assert_eq!(
            prepare_local(&coordinator, &["new-after-revision-max"], TestFault::None).unwrap_err(),
            RemoteMcapRuntimeInternError::CoordinationRevisionExhausted
        );
    }

    #[test]
    fn checked_capacity_arithmetic_and_real_reserve_failure_are_typed() {
        if usize::BITS == 64 {
            assert_eq!(
                checked_capacity_bytes(usize::MAX).unwrap_err(),
                RemoteMcapRuntimeInternError::ArithmeticOverflow
            );
        } else {
            assert!(checked_capacity_bytes(usize::MAX).is_ok());
        }
        let result = PreparedRemoteState::prepare(
            limits(u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX),
            TestFault::None,
        );
        assert!(matches!(
            result,
            Err(RemoteMcapRuntimeInternError::AllocationFailed
                | RemoteMcapRuntimeInternError::ArithmeticOverflow
                | RemoteMcapRuntimeInternError::InvalidLimitProfile)
        ));
    }

    #[test]
    fn burned_usage_survives_simulated_viewer_restart_and_apply_failure() {
        let mut coordinator = initialized(generous_limits());
        let before = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        let viewer_a = prepare_local(&coordinator, &["viewer-a"], TestFault::None).unwrap();
        let committed = coordinator.commit_remote(viewer_a).unwrap();
        let after_viewer_a = committed.module_snapshot();
        drop(committed); // Simulated downstream Store-apply failure and Viewer teardown.

        let viewer_b = prepare_local(&coordinator, &["viewer-b"], TestFault::None).unwrap();
        let after_viewer_b = coordinator
            .commit_remote(viewer_b)
            .unwrap()
            .module_snapshot();
        assert!(after_viewer_a.burned_string_bytes > before.burned_string_bytes);
        assert!(after_viewer_b.burned_string_bytes > after_viewer_a.burned_string_bytes);
        assert_eq!(
            after_viewer_b.burned_side_map_capacity_bytes,
            before.burned_side_map_capacity_bytes
        );
    }

    #[test]
    fn bytes_used_includes_empty_side_map_capacity_and_leaked_string_capacity() {
        let mut coordinator = CoordinatedStringInterner::default();
        assert_eq!(coordinator.bytes_used(), 0);
        let candidate = PreparedRemoteState::prepare(generous_limits(), TestFault::None).unwrap();
        let initialized = coordinator.install_remote(candidate).unwrap();
        assert_eq!(
            coordinator.bytes_used(),
            usize::try_from(initialized.burned_side_map_capacity_bytes).unwrap()
        );

        let mut prepared =
            prepare_local(&coordinator, &["spare-capacity"], TestFault::None).unwrap();
        let raw = prepared.census.canonical_identifiers[0]
            .raw
            .as_mut()
            .unwrap();
        let old_capacity = raw.capacity();
        raw.reserve(256);
        let leaked_capacity = raw.capacity();
        assert!(leaked_capacity > raw.len());
        let extra = u64::try_from(leaked_capacity - old_capacity).unwrap();
        prepared.census.retained_bytes += extra;
        prepared.census.candidate_peak_upper_bound += extra;

        let committed = coordinator.commit_remote(prepared).unwrap();
        assert_eq!(
            committed.module_snapshot().burned_string_bytes,
            leaked_capacity as u64
        );
        assert_eq!(
            coordinator.bytes_used(),
            usize::try_from(
                committed.module_snapshot().burned_side_map_capacity_bytes + leaked_capacity as u64
            )
            .unwrap()
        );
    }

    #[test]
    fn all_committed_handles_are_unique_and_side_map_is_bounded() {
        let mut coordinator = initialized(generous_limits());
        let raw = (0..32)
            .map(|index| format!("identifier-{index}"))
            .collect::<Vec<_>>();
        let borrowed = raw.iter().map(String::as_str).collect::<Vec<_>>();
        let prepared = prepare_local(&coordinator, &borrowed, TestFault::None).unwrap();
        let committed = coordinator.commit_remote(prepared).unwrap();
        let handles = (0..committed.len())
            .map(|index| committed.handle(index).unwrap().hash())
            .collect::<BTreeSet<_>>();
        assert_eq!(handles.len(), committed.len());
        let remote = coordinator.remote.as_ref().unwrap();
        assert!(remote.side_map.len() <= remote.max_side_map_entries.get());
        assert!(remote.side_map.capacity() >= remote.max_side_map_entries.get());
    }

    #[test]
    fn diagnostics_never_contain_identifier_values() {
        let coordinator = initialized(generous_limits());
        let prepared = prepare_local(
            &coordinator,
            &["secret-topic?token=never-print"],
            TestFault::None,
        )
        .unwrap();
        let debug = format!("{prepared:?}");
        assert!(!debug.contains("secret-topic"));
        assert!(!debug.contains("never-print"));
        for error in [
            RemoteMcapRuntimeInternError::AllocationFailed,
            RemoteMcapRuntimeInternError::StringBudgetExceeded,
            RemoteMcapRuntimeInternError::ProtocolViolation,
        ] {
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[test]
    fn production_singleton_burn_is_monotonic_across_simulated_viewers() {
        let mut coordinator = initialized(generous_limits());
        let initial = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        let viewer_a = prepare_local(
            &coordinator,
            &["production-singleton-viewer-a-identifier"],
            TestFault::None,
        )
        .unwrap();
        let viewer_a = coordinator.commit_remote(viewer_a).unwrap();
        let after_a = viewer_a.module_snapshot();
        let remote_handle = viewer_a.handle(0).unwrap();
        drop(viewer_a);

        let viewer_b = prepare_local(
            &coordinator,
            &["production-singleton-viewer-b-identifier"],
            TestFault::None,
        )
        .unwrap();
        let viewer_b = coordinator.commit_remote(viewer_b).unwrap();
        let after_b = viewer_b.module_snapshot();
        assert!(after_a.burned_string_bytes >= initial.burned_string_bytes);
        assert!(after_b.burned_string_bytes > after_a.burned_string_bytes);
        assert_eq!(
            after_b.burned_side_map_capacity_bytes,
            initial.burned_side_map_capacity_bytes
        );
        let current = coordinator.snapshot_remote(coordinator.remote.as_ref().unwrap());
        assert_eq!(current, after_b);

        let legacy_handle = coordinator.intern_legacy("production-singleton-viewer-a-identifier");
        assert_eq!(legacy_handle, remote_handle);
        assert!(std::ptr::eq(legacy_handle.as_str(), remote_handle.as_str()));
    }

    #[test]
    fn feature_on_preserves_newtype_nonempty_and_serde_behavior() {
        crate::declare_new_type!(
            /// Differential test identifier.
            struct PlainIdentifier;
        );
        crate::declare_new_type_nonempty!(
            /// Differential test nonempty identifier.
            struct NonemptyIdentifier;
        );

        let plain = PlainIdentifier::new("feature-differential");
        assert_eq!(plain.as_str(), "feature-differential");
        let interned = InternedString::new("feature-differential");
        let plain_json = serde_json::to_string(&interned).unwrap();
        assert_eq!(plain_json, "\"feature-differential\"");
        let plain_roundtrip: InternedString = serde_json::from_str(&plain_json).unwrap();
        assert_eq!(plain_roundtrip, interned);

        let nonempty = NonemptyIdentifier::try_new("nonempty-differential").unwrap();
        let nonempty_json = serde_json::to_string(&nonempty).unwrap();
        let nonempty_roundtrip: NonemptyIdentifier = serde_json::from_str(&nonempty_json).unwrap();
        assert_eq!(nonempty_roundtrip, nonempty);
        assert!(NonemptyIdentifier::try_new("").is_err());
        let nonempty_from_interned = NonemptyIdentifier::try_from_interned(interned).unwrap();
        assert_eq!(nonempty_from_interned.as_str(), interned.as_str());
        assert!(std::ptr::eq(
            nonempty_from_interned.as_str(),
            interned.as_str()
        ));
        assert!(NonemptyIdentifier::try_from_interned(InternedString::new("")).is_err());
    }
}
