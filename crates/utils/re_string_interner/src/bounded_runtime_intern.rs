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
use std::fmt::Write as _;
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
    pub unique_identifiers: u64,
    pub raw_bytes: u64,
    pub retained_bytes: u64,
    pub existing_legacy: u64,
    pub existing_remote: u64,
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
/// Construction validates the raw shape for the requested family. Entity paths use the same
/// forgiving unescaping and canonical display form as the local parser, but without constructing
/// an `EntityPath` or interning any part.
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

#[derive(Debug)]
struct OwnedRawIdentifier {
    hash: u64,
    raw: Option<String>,
}

/// A complete, canonical, owned raw identifier census.
///
/// The fields are private and the value is move-only. It cannot be assembled from caller-selected
/// internals, and it owns every temporary string until the interner transaction consumes it.
pub struct BoundedRemoteIdentifierCensus {
    identifiers: Vec<OwnedRawIdentifier>,
    raw_bytes: u64,
    retained_bytes: u64,
    candidate_peak_upper_bound: u64,
}

static_assertions::assert_not_impl_any!(BoundedRemoteIdentifierCensus: Clone, Copy);

impl fmt::Debug for BoundedRemoteIdentifierCensus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedRemoteIdentifierCensus")
            .field("identifier_count", &self.identifiers.len())
            .field("raw_bytes", &self.raw_bytes)
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
    telemetry: RemoteInternBatchTelemetry,
}

impl fmt::Debug for PreparedRemoteInternBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedRemoteInternBatch")
            .field("unique_identifiers", &self.telemetry.unique_identifiers)
            .field("raw_bytes", &self.telemetry.raw_bytes)
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
pub struct CommittedRemoteInternBatch {
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
            .finish()
    }
}

impl CommittedRemoteInternBatch {
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Returns the handle at its first-seen unique-census position.
    pub fn handle(&self, index: usize) -> Option<InternedString> {
        self.handles.get(index).copied().flatten()
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
    raw_bytes: u64,
    identifiers: usize,
) -> Result<u64, RemoteMcapRuntimeInternError> {
    let with_identifier_vec = raw_bytes
        .checked_add(checked_vec_bytes::<OwnedRawIdentifier>(identifiers)?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    let dedup_capacity = conservative_hash_capacity_for_entries(identifiers)?;
    with_identifier_vec
        .checked_add(checked_capacity_bytes(dedup_capacity)?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)
}

fn checked_candidate_peak_upper_bound(
    raw_bytes: u64,
    identifiers: usize,
) -> Result<u64, RemoteMcapRuntimeInternError> {
    checked_census_capacity_upper_bound(raw_bytes, identifiers)?
        .checked_add(checked_vec_bytes::<Option<InternedString>>(identifiers)?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)
}

fn checked_census_retained_bytes(
    string_capacity_bytes: u64,
    identifier_capacity: usize,
    dedup_capacity: usize,
) -> Result<u64, RemoteMcapRuntimeInternError> {
    let retained = string_capacity_bytes
        .checked_add(checked_vec_bytes::<OwnedRawIdentifier>(
            identifier_capacity,
        )?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
    retained
        .checked_add(checked_capacity_bytes(dedup_capacity)?)
        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)
}

fn push_escaped_character(output: &mut String, character: char) {
    if character.is_alphanumeric() || matches!(character, '_' | '-' | '.') {
        output.push(character);
        return;
    }
    match character {
        '\n' => output.push_str("\\n"),
        '\r' => output.push_str("\\r"),
        '\t' => output.push_str("\\t"),
        character if character.is_ascii_punctuation() || character == ' ' => {
            output.push('\\');
            output.push(character);
        }
        character => {
            write!(output, "\\u{{{:04X}}}", u32::from(character)).ok();
        }
    }
}

struct InvalidUnicodeEscape {
    consumed: [char; 6],
    length: usize,
}

fn parse_unicode_escape(
    input: &mut impl Iterator<Item = char>,
) -> Result<char, InvalidUnicodeEscape> {
    let mut consumed = ['\0'; 6];
    let mut length = 0;
    for character in input {
        consumed[length] = character;
        length += 1;
        if character == '}' || length == consumed.len() {
            break;
        }
    }

    let body = &consumed[..length];
    if body.last() != Some(&'}') {
        return Err(InvalidUnicodeEscape { consumed, length });
    }
    let digits = &body[..length - 1];
    if digits.len() != 5 || digits[0] != '{' {
        return Err(InvalidUnicodeEscape { consumed, length });
    }
    let mut codepoint = 0_u32;
    for digit in &digits[1..] {
        let Some(digit) = digit.to_digit(16) else {
            return Err(InvalidUnicodeEscape { consumed, length });
        };
        codepoint = codepoint * 16 + digit;
    }
    char::from_u32(codepoint).ok_or(InvalidUnicodeEscape { consumed, length })
}

fn push_unescaped_character(
    input: &mut impl Iterator<Item = char>,
    first: char,
    output: &mut String,
) {
    if first != '\\' {
        push_escaped_character(output, first);
        return;
    }

    let Some(next) = input.next() else {
        push_escaped_character(output, '\\');
        return;
    };
    match next {
        'n' => push_escaped_character(output, '\n'),
        'r' => push_escaped_character(output, '\r'),
        't' => push_escaped_character(output, '\t'),
        'u' => match parse_unicode_escape(input) {
            Ok(character) => push_escaped_character(output, character),
            Err(invalid) => {
                push_escaped_character(output, '\\');
                output.push('u');
                for character in &invalid.consumed[..invalid.length] {
                    push_escaped_character(output, *character);
                }
            }
        },
        character => push_escaped_character(output, character),
    }
}

fn canonicalize_entity_path_part_into(raw: &str, output: &mut String) {
    let mut input = raw.chars();
    while let Some(first) = input.next() {
        push_unescaped_character(&mut input, first, output);
    }
}

fn canonicalize_entity_path_into(raw: &str, output: &mut String) {
    let mut input = raw.chars();
    let mut part_has_content = false;
    while let Some(first) = input.next() {
        if first == '/' {
            part_has_content = false;
            continue;
        }
        if !part_has_content {
            output.push('/');
            part_has_content = true;
        }
        push_unescaped_character(&mut input, first, output);
    }
    if output.is_empty() {
        output.push('/');
    }
}

fn canonicalize_identifier_into(
    identifier: RemoteMcapRawIdentifier<'_>,
    output: &mut String,
) -> Result<(), RemoteMcapRuntimeInternError> {
    if identifier.kind == RemoteMcapRawIdentifierKind::EntityPathPart && identifier.raw.is_empty() {
        return Err(RemoteMcapRuntimeInternError::CensusCanonicalizationFailed);
    }
    match identifier.kind {
        RemoteMcapRawIdentifierKind::Timeline | RemoteMcapRawIdentifierKind::Component => {
            output.push_str(identifier.raw);
        }
        RemoteMcapRawIdentifierKind::EntityPathPart => {
            canonicalize_entity_path_part_into(identifier.raw, output);
        }
        RemoteMcapRawIdentifierKind::EntityPath => {
            canonicalize_entity_path_into(identifier.raw, output);
        }
    }
    Ok(())
}

fn canonical_byte_upper_bound(identifier: RemoteMcapRawIdentifier<'_>) -> Option<u64> {
    let raw_bytes = u64::try_from(identifier.raw.len()).ok()?;
    if matches!(
        identifier.kind,
        RemoteMcapRawIdentifierKind::EntityPath | RemoteMcapRawIdentifierKind::EntityPathPart
    ) {
        let characters = u64::try_from(identifier.raw.chars().count()).ok()?;
        // Every input character expands to at most 12 canonical bytes. The extra byte covers the
        // canonical entity-path slash.
        characters.checked_mul(12)?.checked_add(1)
    } else {
        Some(raw_bytes)
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

        // Validate the complete borrowed input before hashing or allocating anything.
        // This intentionally counts duplicate positions: an adversarial duplicate list must not
        // obtain unbounded pre-dedup CPU or byte work merely because its canonical result is small.
        let mut input_count = 0_usize;
        let mut input_raw_bytes = 0_u64;
        let mut canonical_bytes_upper_bound = 0_u64;
        for identifier in input_identifiers.clone() {
            input_count = input_count
                .checked_add(1)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            if input_count > max_identifiers {
                return Err(RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded);
            }
            if identifier.kind == RemoteMcapRawIdentifierKind::EntityPathPart
                && identifier.raw.is_empty()
            {
                return Err(RemoteMcapRuntimeInternError::CensusCanonicalizationFailed);
            }
            input_raw_bytes =
                input_raw_bytes
                    .checked_add(u64::try_from(identifier.raw.len()).map_err(
                        |_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow,
                    )?)
                    .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            canonical_bytes_upper_bound = canonical_bytes_upper_bound
                .checked_add(
                    canonical_byte_upper_bound(identifier)
                        .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?,
                )
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        }
        if input_raw_bytes > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRawBytesExceeded);
        }
        if canonical_bytes_upper_bound > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        let census_bytes_upper_bound = input_raw_bytes.max(canonical_bytes_upper_bound);

        if input_count > max_identifiers {
            return Err(RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded);
        }

        let census_capacity_upper_bound =
            checked_census_capacity_upper_bound(census_bytes_upper_bound, input_count)?;
        if census_capacity_upper_bound > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
        }
        let candidate_peak_upper_bound =
            checked_candidate_peak_upper_bound(census_bytes_upper_bound, input_count)?;
        if candidate_peak_upper_bound > limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }

        let mut owned_identifiers = Vec::new();
        let mut dedup = IntMap::<u64, usize>::default();
        if fault == TestFault::CensusStructures {
            return Err(RemoteMcapRuntimeInternError::AllocationFailed);
        }
        owned_identifiers
            .try_reserve_exact(input_count)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        dedup
            .try_reserve(input_count)
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        if dedup.capacity() > conservative_hash_capacity_for_entries(input_count)? {
            return Err(RemoteMcapRuntimeInternError::InvalidLimitProfile);
        }

        let mut raw_bytes = 0_u64;
        let mut string_capacity_bytes = 0_u64;

        for identifier in input_identifiers {
            let canonical_capacity_bound = canonical_byte_upper_bound(identifier)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            let temporary_peak = checked_census_retained_bytes(
                string_capacity_bytes,
                owned_identifiers.capacity(),
                dedup.capacity(),
            )?
            .checked_add(canonical_capacity_bound)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            if temporary_peak > limits.max_census_retained_bytes.get() {
                return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
            }
            if temporary_peak > limits.max_candidate_peak_bytes.get() {
                return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
            }

            if fault == TestFault::CensusString {
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
            canonicalize_identifier_into(identifier, &mut canonical)?;
            if u64::try_from(canonical.len())
                .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?
                > canonical_capacity_bound
            {
                return Err(RemoteMcapRuntimeInternError::ProtocolViolation);
            }

            let raw_hash = hash(&canonical);
            if let Some(index) = dedup.get(&raw_hash).copied() {
                let existing = owned_identifiers
                    .get(index)
                    .and_then(|entry: &OwnedRawIdentifier| entry.raw.as_deref())
                    .ok_or(RemoteMcapRuntimeInternError::ProtocolViolation)?;
                if existing != canonical {
                    return Err(RemoteMcapRuntimeInternError::IdentifierHashCollision);
                }
                continue;
            }

            if owned_identifiers.len() >= max_identifiers {
                return Err(RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded);
            }
            let raw_len = u64::try_from(canonical.len())
                .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
            raw_bytes = raw_bytes
                .checked_add(raw_len)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;

            string_capacity_bytes = string_capacity_bytes
                .checked_add(actual_canonical_capacity)
                .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;

            let retained_bytes = checked_census_retained_bytes(
                string_capacity_bytes,
                owned_identifiers.capacity(),
                dedup.capacity(),
            )?;
            if retained_bytes > limits.max_census_retained_bytes.get() {
                return Err(RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded);
            }
            if retained_bytes > limits.max_candidate_peak_bytes.get() {
                return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
            }

            let index = owned_identifiers.len();
            owned_identifiers.push(OwnedRawIdentifier {
                hash: raw_hash,
                raw: Some(canonical),
            });
            dedup.insert(raw_hash, index);
        }

        let retained_bytes = checked_census_retained_bytes(
            string_capacity_bytes,
            owned_identifiers.capacity(),
            dedup.capacity(),
        )?;
        Ok(Self {
            identifiers: owned_identifiers,
            raw_bytes,
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
        for (index, identifier) in census.identifiers.iter().enumerate() {
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
            checked_vec_bytes::<Option<InternedString>>(census.identifiers.len())?;
        let candidate_peak_before_result_allocation = census
            .retained_bytes
            .checked_add(resolved_capacity_upper_bound)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if candidate_peak_before_result_allocation > remote.limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }
        let mut resolved = Vec::new();
        resolved
            .try_reserve_exact(census.identifiers.len())
            .map_err(|_reserve_error| RemoteMcapRuntimeInternError::AllocationFailed)?;
        let actual_candidate_peak = census
            .retained_bytes
            .checked_add(checked_vec_bytes::<Option<InternedString>>(
                resolved.capacity(),
            )?)
            .ok_or(RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if actual_candidate_peak > remote.limits.max_candidate_peak_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CandidatePeakExceeded);
        }
        resolved.resize(census.identifiers.len(), None);
        let telemetry = self.telemetry(&census, plan, false);
        Ok(PreparedRemoteInternBatch {
            census,
            resolved,
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
        for (index, mut identifier) in prepared.census.identifiers.into_iter().enumerate() {
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
            handles: prepared.resolved,
            telemetry,
            module_snapshot,
        })
    }

    fn validate_census_again(
        census: &BoundedRemoteIdentifierCensus,
        limits: RemoteMcapRuntimeInternLimits,
    ) -> Result<(), RemoteMcapRuntimeInternError> {
        let count = u64::try_from(census.identifiers.len())
            .map_err(|_conversion_error| RemoteMcapRuntimeInternError::ArithmeticOverflow)?;
        if count > limits.max_census_identifiers.get() {
            return Err(RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded);
        }
        if census.raw_bytes > limits.max_census_retained_bytes.get() {
            return Err(RemoteMcapRuntimeInternError::CensusRawBytesExceeded);
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
            unique_identifiers: census.identifiers.len() as u64,
            raw_bytes: census.raw_bytes,
            retained_bytes: census.retained_bytes,
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

    fn canonicalize_entity_path(raw: &str) -> String {
        let mut canonical = String::new();
        canonicalize_entity_path_into(raw, &mut canonical);
        canonical
    }

    fn canonicalize_entity_path_part(raw: &str) -> String {
        let mut canonical = String::new();
        canonicalize_entity_path_part_into(raw, &mut canonical);
        canonical
    }

    #[test]
    fn entity_path_canonicalization_matches_forgiving_display_shape() {
        for (raw, expected) in [
            ("", "/"),
            ("/", "/"),
            ("///", "/"),
            ("foo///bar/", "/foo/bar"),
            (r"foo\/bar", r"/foo\/bar"),
            (r"foo\ bar\!", r"/foo\ bar\!"),
            (r"foo\bar", "/foobar"),
            (r"foo\", r"/foo\\"),
            (r"\u{00E5}", "/å"),
            (r"\u{apa}", r"/\\u\{apa\}"),
        ] {
            assert_eq!(canonicalize_entity_path(raw), expected, "raw: {raw:?}");
        }

        assert_eq!(canonicalize_entity_path_part(r"\u{apa}"), r"\\u\{apa\}");
        assert_eq!(canonicalize_entity_path_part(r"foo\!"), r"foo\!");
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
        let committed = coordinator.commit_remote(prepared).unwrap();

        assert_eq!(committed.len(), 5);
        assert_eq!(committed.handle(0).unwrap().as_str(), "message_log_time");
        assert_eq!(committed.handle(1).unwrap().as_str(), "McapMessage");
        assert_eq!(committed.handle(2).unwrap().as_str(), "/foo/bar");
        assert_eq!(
            committed.handle(3).unwrap().as_str(),
            r"/foo/remote\ path\!"
        );
        assert_eq!(committed.handle(4).unwrap().as_str(), r"remote\ path\!");
        assert!((0..committed.len()).all(|index| committed.handle(index).is_some()));
    }

    #[test]
    fn raw_census_failures_have_zero_intern_effect() {
        initialize_remote_mcap_runtime_intern(generous_limits()).unwrap();
        let before = remote_mcap_runtime_intern_snapshot().unwrap();

        assert_eq!(
            RemoteMcapRawIdentifier::timeline("")
                .and_then(|identifier| BoundedRemoteIdentifierCensus::try_new_v1([identifier]))
                .unwrap_err(),
            RemoteMcapRuntimeInternError::CensusCanonicalizationFailed
        );
        assert_eq!(
            RemoteMcapRawIdentifier::component("")
                .and_then(|identifier| BoundedRemoteIdentifierCensus::try_new_v1([identifier]))
                .unwrap_err(),
            RemoteMcapRuntimeInternError::CensusCanonicalizationFailed
        );
        assert_eq!(
            RemoteMcapRawIdentifier::entity_path_part("")
                .and_then(|identifier| BoundedRemoteIdentifierCensus::try_new_v1([identifier]))
                .unwrap_err(),
            RemoteMcapRuntimeInternError::CensusCanonicalizationFailed
        );

        let oversized_raw = "x".repeat(32 * 1024 + 1);
        assert_eq!(
            BoundedRemoteIdentifierCensus::try_new_v1([RemoteMcapRawIdentifier::timeline(
                &oversized_raw
            )
            .unwrap()])
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
            BoundedRemoteIdentifierCensus::try_new_v1(too_many).unwrap_err(),
            RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded
        );
        assert_eq!(remote_mcap_runtime_intern_snapshot().unwrap(), before);
    }

    #[test]
    fn canonical_expansion_is_owned_and_counted_before_domain_construction() {
        let coordinator = initialized(generous_limits());
        let raw = r"\u{262E}";
        let canonical = r"/\u{262E}";
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

        assert!(census.raw_bytes > raw.len() as u64);
        assert_eq!(census.identifiers.len(), 2);
        assert_eq!(census.identifiers[0].raw.as_deref(), Some(canonical));
        assert_eq!(census.identifiers[1].raw.as_deref(), Some("shared"));

        let token = census.into_domain_construction_token();
        let debug = format!("{token:?}");
        assert!(!debug.contains(raw));
        assert!(!debug.contains("shared"));
    }

    #[test]
    fn canonical_expansion_counts_toward_candidate_preflight() {
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
        let complete_peak =
            checked_candidate_peak_upper_bound(canonical_bytes_upper_bound, identifiers.len())
                .unwrap();
        assert!(complete_peak > checked_candidate_peak_upper_bound(raw_bytes, 2).unwrap());

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
        initialize_remote_mcap_runtime_intern(generous_limits()).unwrap();
        let census = BoundedRemoteIdentifierCensus::try_new_v1([
            RemoteMcapRawIdentifier::timeline("token-timeline").unwrap(),
            RemoteMcapRawIdentifier::entity_path("//token/path"),
            RemoteMcapRawIdentifier::entity_path_part("token part").unwrap(),
            RemoteMcapRawIdentifier::component("TokenComponent").unwrap(),
        ])
        .unwrap();
        let token = census.into_domain_construction_token();
        let prepared =
            prepare_remote_mcap_runtime_intern_from_domain_construction_token(token).unwrap();
        let committed = prepared.commit().unwrap();

        assert_eq!(committed.telemetry().unique_identifiers, 4);
        assert_eq!(committed.len(), 4);
        assert_eq!(committed.handle(1).unwrap().as_str(), "/token/path");
        assert!((0..committed.len()).all(|index| committed.handle(index).is_some()));
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
        let complete_peak =
            checked_candidate_peak_upper_bound(raw_bytes, duplicate_positions.len()).unwrap();
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

        let without_result = checked_census_capacity_upper_bound(1, 1).unwrap();
        let with_result = checked_candidate_peak_upper_bound(1, 1).unwrap();
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
        let raw = prepared.census.identifiers[0].raw.as_mut().unwrap();
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
        let initial = initialize_remote_mcap_runtime_intern(generous_limits()).unwrap();
        let viewer_a =
            prepare_remote_mcap_runtime_intern(&["production-singleton-viewer-a-identifier"])
                .unwrap()
                .commit()
                .unwrap();
        let after_a = viewer_a.module_snapshot();
        let remote_handle = viewer_a.handle(0).unwrap();
        drop(viewer_a);

        let viewer_b =
            prepare_remote_mcap_runtime_intern(&["production-singleton-viewer-b-identifier"])
                .unwrap()
                .commit()
                .unwrap();
        let after_b = viewer_b.module_snapshot();
        assert!(after_a.burned_string_bytes >= initial.burned_string_bytes);
        assert!(after_b.burned_string_bytes > after_a.burned_string_bytes);
        assert_eq!(
            after_b.burned_side_map_capacity_bytes,
            initial.burned_side_map_capacity_bytes
        );
        assert_eq!(remote_mcap_runtime_intern_snapshot().unwrap(), after_b);

        let legacy_handle = InternedString::new("production-singleton-viewer-a-identifier");
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
    }
}
