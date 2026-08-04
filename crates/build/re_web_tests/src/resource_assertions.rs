//! Bounded-resource and redaction assertions shared by native and Wasm tests.
//!
//! These helpers deliberately model ownership and observations, not Viewer product state.
//! They never include caller-provided labels or secret values in assertion diagnostics.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

use zeroize::Zeroize as _;

/// An opaque, low-cardinality resource class selected by a test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceKey(u16);

impl ResourceKey {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Checked count and byte limits for one resource class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_count: u64,
    pub max_bytes: u64,
}

impl ResourceLimits {
    pub const UNLIMITED: Self = Self {
        max_count: u64::MAX,
        max_bytes: u64::MAX,
    };
}

/// Current and historical usage for one resource class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceUsage {
    pub current_count: u64,
    pub current_bytes: u64,
    pub high_water_count: u64,
    pub high_water_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReservationRequest {
    pub key: ResourceKey,
    pub count: u64,
    pub bytes: u64,
}

impl ReservationRequest {
    pub const fn new(key: ResourceKey, count: u64, bytes: u64) -> Self {
        Self { key, count, bytes }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResourceBucket {
    limits: ResourceLimits,
    usage: ResourceUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResourceRegistryState {
    revision: u64,
    buckets: BTreeMap<ResourceKey, ResourceBucket>,
}

/// An exact, comparable snapshot of all registry state, including high-water marks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceRegistrySnapshot(ResourceRegistryState);

impl ResourceRegistrySnapshot {
    pub fn revision(&self) -> u64 {
        self.0.revision
    }

    pub fn usage(&self, key: ResourceKey) -> Option<ResourceUsage> {
        self.0.buckets.get(&key).map(|bucket| bucket.usage)
    }

    pub fn is_live_empty(&self) -> bool {
        self.0
            .buckets
            .values()
            .all(|bucket| bucket.usage.current_count == 0 && bucket.usage.current_bytes == 0)
    }
}

/// Failure from checked resource accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceReservationError {
    UnknownResource { key: ResourceKey },
    DuplicateResourceDefinition { key: ResourceKey },
    CountOverflow { key: ResourceKey },
    ByteOverflow { key: ResourceKey },
    CountLimitExceeded { key: ResourceKey },
    ByteLimitExceeded { key: ResourceKey },
    RegistryRevisionExhausted,
    PreparedRevisionMismatch,
}

impl fmt::Display for ResourceReservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownResource { key } => {
                write!(formatter, "unknown resource class {}", key.get())
            }
            Self::DuplicateResourceDefinition { key } => {
                write!(formatter, "duplicate resource class {}", key.get())
            }
            Self::CountOverflow { key } => {
                write!(formatter, "resource count overflow for class {}", key.get())
            }
            Self::ByteOverflow { key } => {
                write!(formatter, "resource byte overflow for class {}", key.get())
            }
            Self::CountLimitExceeded { key } => {
                write!(
                    formatter,
                    "resource count limit exceeded for class {}",
                    key.get()
                )
            }
            Self::ByteLimitExceeded { key } => {
                write!(
                    formatter,
                    "resource byte limit exceeded for class {}",
                    key.get()
                )
            }
            Self::RegistryRevisionExhausted => {
                formatter.write_str("resource registry revision exhausted")
            }
            Self::PreparedRevisionMismatch => {
                formatter.write_str("prepared resource registry revision mismatch")
            }
        }
    }
}

impl std::error::Error for ResourceReservationError {}

/// A checked resource registry for deterministic ownership tests.
///
/// The prepare method performs all fallible arithmetic against a snapshot without mutating the
/// registry.
/// Dropping or aborting the returned prepared value is therefore an exact rollback, including
/// revision and high-water state.
#[derive(Clone, Debug, Default)]
pub struct CheckedResourceRegistry(Rc<RefCell<ResourceRegistryState>>);

impl CheckedResourceRegistry {
    pub fn new(
        resources: impl IntoIterator<Item = (ResourceKey, ResourceLimits)>,
    ) -> Result<Self, ResourceReservationError> {
        let mut buckets = BTreeMap::new();
        for (key, limits) in resources {
            if buckets
                .insert(
                    key,
                    ResourceBucket {
                        limits,
                        usage: ResourceUsage::default(),
                    },
                )
                .is_some()
            {
                return Err(ResourceReservationError::DuplicateResourceDefinition { key });
            }
        }
        Ok(Self(Rc::new(RefCell::new(ResourceRegistryState {
            revision: 0,
            buckets,
        }))))
    }

    pub fn snapshot(&self) -> ResourceRegistrySnapshot {
        ResourceRegistrySnapshot(self.0.borrow().clone())
    }

    pub fn prepare(
        &self,
        requests: impl IntoIterator<Item = ReservationRequest>,
    ) -> Result<PreparedReservations, ResourceReservationError> {
        let mut totals = BTreeMap::<ResourceKey, (u64, u64)>::new();
        for request in requests {
            let total = totals.entry(request.key).or_default();
            total.0 = total
                .0
                .checked_add(request.count)
                .ok_or(ResourceReservationError::CountOverflow { key: request.key })?;
            total.1 = total
                .1
                .checked_add(request.bytes)
                .ok_or(ResourceReservationError::ByteOverflow { key: request.key })?;
        }
        let state = self.0.borrow();
        let committed_revision = state
            .revision
            .checked_add(1)
            .ok_or(ResourceReservationError::RegistryRevisionExhausted)?;
        for (key, (count, bytes)) in &totals {
            let Some(bucket) = state.buckets.get(key) else {
                return Err(ResourceReservationError::UnknownResource { key: *key });
            };
            let next_count = bucket
                .usage
                .current_count
                .checked_add(*count)
                .ok_or(ResourceReservationError::CountOverflow { key: *key })?;
            let next_bytes = bucket
                .usage
                .current_bytes
                .checked_add(*bytes)
                .ok_or(ResourceReservationError::ByteOverflow { key: *key })?;
            if next_count > bucket.limits.max_count {
                return Err(ResourceReservationError::CountLimitExceeded { key: *key });
            }
            if next_bytes > bucket.limits.max_bytes {
                return Err(ResourceReservationError::ByteLimitExceeded { key: *key });
            }
        }
        Ok(PreparedReservations {
            registry: self.clone(),
            expected_revision: state.revision,
            committed_revision,
            totals,
        })
    }
}

/// A fully checked but disarmed resource reservation.
#[derive(Debug)]
pub struct PreparedReservations {
    registry: CheckedResourceRegistry,
    expected_revision: u64,
    committed_revision: u64,
    totals: BTreeMap<ResourceKey, (u64, u64)>,
}

impl PreparedReservations {
    /// Commits a prepared reservation if no intervening registry mutation occurred.
    ///
    /// A revision mismatch leaves the registry exactly unchanged.
    pub fn commit(self) -> Result<ResourceReservation, ResourceReservationError> {
        let mut state = self.registry.0.borrow_mut();
        if state.revision != self.expected_revision {
            return Err(ResourceReservationError::PreparedRevisionMismatch);
        }
        for (key, (count, bytes)) in &self.totals {
            let bucket = state
                .buckets
                .get_mut(key)
                .expect("prepared resource class remains installed");
            bucket.usage.current_count += count;
            bucket.usage.current_bytes += bytes;
            bucket.usage.high_water_count = bucket
                .usage
                .high_water_count
                .max(bucket.usage.current_count);
            bucket.usage.high_water_bytes = bucket
                .usage
                .high_water_bytes
                .max(bucket.usage.current_bytes);
        }
        state.revision = self.committed_revision;
        drop(state);
        Ok(ResourceReservation {
            registry: self.registry,
            totals: self.totals,
            active: true,
        })
    }
}

/// A non-cloneable RAII owner for a committed reservation batch.
#[derive(Debug)]
pub struct ResourceReservation {
    registry: CheckedResourceRegistry,
    totals: BTreeMap<ResourceKey, (u64, u64)>,
    active: bool,
}

impl ResourceReservation {
    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.registry.0.borrow_mut();
        for (key, (count, bytes)) in &self.totals {
            let bucket = state
                .buckets
                .get_mut(key)
                .expect("reserved resource class remains installed");
            bucket.usage.current_count = bucket
                .usage
                .current_count
                .checked_sub(*count)
                .expect("resource count ownership balances");
            bucket.usage.current_bytes = bucket
                .usage
                .current_bytes
                .checked_sub(*bytes)
                .expect("resource byte ownership balances");
        }
        self.active = false;
    }
}

impl Drop for ResourceReservation {
    fn drop(&mut self) {
        self.release_inner();
    }
}

/// Captures any cloneable registry or state-machine value for exact rollback assertions.
#[derive(Clone)]
pub struct ExactSnapshot<T> {
    value: T,
}

impl<T: Clone> ExactSnapshot<T> {
    pub fn capture(value: &T) -> Self {
        Self {
            value: value.clone(),
        }
    }
}

impl<T: PartialEq> ExactSnapshot<T> {
    pub fn is_unchanged(&self, actual: &T) -> bool {
        &self.value == actual
    }

    #[track_caller]
    pub fn assert_unchanged(&self, actual: &T) {
        assert!(
            self.is_unchanged(actual),
            "exact rollback assertion failed: state changed"
        );
    }
}

#[track_caller]
pub fn assert_registry_unchanged(
    before: &ResourceRegistrySnapshot,
    after: &ResourceRegistrySnapshot,
) {
    assert_eq!(
        before, after,
        "resource registry changed across rollback boundary"
    );
}

/// A sensitive-value category that is safe to include in diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SensitiveKind {
    Url,
    Query,
    Etag,
    Topic,
    EntityPath,
    StoreId,
    InternalToken,
}

impl SensitiveKind {
    pub const ALL: [Self; 7] = [
        Self::Url,
        Self::Query,
        Self::Etag,
        Self::Topic,
        Self::EntityPath,
        Self::StoreId,
        Self::InternalToken,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Query => "query",
            Self::Etag => "etag",
            Self::Topic => "topic",
            Self::EntityPath => "entity_path",
            Self::StoreId => "store_id",
            Self::InternalToken => "internal_token",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "url" => Some(Self::Url),
            "query" => Some(Self::Query),
            "etag" => Some(Self::Etag),
            "topic" => Some(Self::Topic),
            "entity_path" => Some(Self::EntityPath),
            "store_id" => Some(Self::StoreId),
            "internal_token" => Some(Self::InternalToken),
            _ => None,
        }
    }
}

/// Invalid sensitive-value registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensitiveValueError {
    EmptyValue { kind: SensitiveKind },
    InvalidCorpusLine { line: usize },
    UnknownCorpusKind { line: usize },
    DuplicateCorpusKind { kind: SensitiveKind },
    IncompleteCorpus,
}

impl fmt::Display for SensitiveValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue { kind } => {
                write!(formatter, "empty sensitive {} sentinel", kind.as_str())
            }
            Self::InvalidCorpusLine { line } => {
                write!(formatter, "invalid redaction corpus line {line}")
            }
            Self::UnknownCorpusKind { line } => {
                write!(formatter, "unknown redaction corpus kind on line {line}")
            }
            Self::DuplicateCorpusKind { kind } => {
                write!(
                    formatter,
                    "duplicate {} redaction corpus entry",
                    kind.as_str()
                )
            }
            Self::IncompleteCorpus => formatter.write_str("redaction corpus is incomplete"),
        }
    }
}

impl std::error::Error for SensitiveValueError {}

#[derive(Clone)]
struct SensitiveValue {
    kind: SensitiveKind,
    bytes: Box<[u8]>,
}

impl SensitiveValue {
    fn zeroize(&mut self) {
        self.bytes.zeroize();
    }
}

impl fmt::Debug for SensitiveValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SensitiveValue")
            .field("kind", &self.kind)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

impl Drop for SensitiveValue {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// A caller-populated set of exact values which must not appear in observable output.
#[derive(Clone, Default)]
pub struct LeakDetector {
    values: Vec<SensitiveValue>,
}

impl fmt::Debug for LeakDetector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LeakDetector")
            .field("registered_values", &self.values.len())
            .finish()
    }
}

impl LeakDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        kind: SensitiveKind,
        value: impl AsRef<[u8]>,
    ) -> Result<(), SensitiveValueError> {
        let bytes = value.as_ref();
        if bytes.is_empty() {
            return Err(SensitiveValueError::EmptyValue { kind });
        }
        self.values.push(SensitiveValue {
            kind,
            bytes: bytes.into(),
        });
        Ok(())
    }

    pub fn inspect(&self, observable: impl AsRef<[u8]>) -> Result<(), LeakError> {
        let observable = observable.as_ref();
        for sensitive in &self.values {
            if sensitive.bytes.len() > observable.len() {
                continue;
            }
            if let Some(offset) = observable
                .windows(sensitive.bytes.len())
                .position(|window| window == sensitive.bytes.as_ref())
            {
                return Err(LeakError {
                    kind: sensitive.kind,
                    offset,
                });
            }
        }
        Ok(())
    }

    /// Zeroes and removes every registered sensitive value.
    pub fn dispose(&mut self) {
        for sensitive in &mut self.values {
            sensitive.zeroize();
        }
        self.values.clear();
    }

    #[track_caller]
    pub fn assert_redacted(&self, observable: impl AsRef<[u8]>) {
        if let Err(error) = self.inspect(observable) {
            panic!("redaction assertion failed: {error}");
        }
    }
}

impl Drop for LeakDetector {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// Loads the single shared set of cross-language redaction sentinels.
pub fn redaction_leak_corpus_v1() -> Result<LeakDetector, SensitiveValueError> {
    const CORPUS: &str = include_str!("../test_data/redaction_leak_corpus_v1.tsv");
    let mut detector = LeakDetector::new();
    let mut seen = BTreeSet::<SensitiveKind>::new();
    for (index, line) in CORPUS.lines().enumerate() {
        let line_number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((kind, value)) = line.split_once('\t') else {
            return Err(SensitiveValueError::InvalidCorpusLine { line: line_number });
        };
        let Some(kind) = SensitiveKind::parse(kind) else {
            return Err(SensitiveValueError::UnknownCorpusKind { line: line_number });
        };
        if !seen.insert(kind) {
            return Err(SensitiveValueError::DuplicateCorpusKind { kind });
        }
        detector.register(kind, value)?;
    }
    if seen.len() != SensitiveKind::ALL.len() {
        return Err(SensitiveValueError::IncompleteCorpus);
    }
    Ok(detector)
}

/// A leak location that reports only a safe category and byte offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeakError {
    pub kind: SensitiveKind,
    pub offset: usize,
}

impl fmt::Display for LeakError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "sensitive {} value at byte offset {}",
            self.kind.as_str(),
            self.offset
        )
    }
}

impl std::error::Error for LeakError {}

/// A bounded, redaction-checked diagnostic label.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundedLabel(Box<str>);

impl BoundedLabel {
    pub fn try_new(
        value: impl Into<Box<str>>,
        max_bytes: usize,
        detector: &LeakDetector,
    ) -> Result<Self, BoundedLabelError> {
        let value = value.into();
        if value.len() > max_bytes {
            return Err(BoundedLabelError::TooLong {
                actual_bytes: value.len(),
                max_bytes,
            });
        }
        detector
            .inspect(value.as_bytes())
            .map_err(BoundedLabelError::Leak)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BoundedLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedLabel")
            .field("byte_len", &self.0.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundedLabelError {
    TooLong {
        actual_bytes: usize,
        max_bytes: usize,
    },
    Leak(LeakError),
}

impl fmt::Display for BoundedLabelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong {
                actual_bytes,
                max_bytes,
            } => write!(
                formatter,
                "bounded label has {actual_bytes} bytes, limit is {max_bytes}"
            ),
            Self::Leak(error) => write!(formatter, "bounded label contains {error}"),
        }
    }
}

impl std::error::Error for BoundedLabelError {}

/// Reports whether an inspected byte owner has been completely overwritten with zeroes.
pub fn check_zeroized(bytes: &[u8]) -> Result<(), ZeroizeError> {
    match bytes.iter().position(|byte| *byte != 0) {
        Some(offset) => Err(ZeroizeError { offset }),
        None => Ok(()),
    }
}

#[track_caller]
pub fn assert_zeroized(bytes: &[u8]) {
    if let Err(error) = check_zeroized(bytes) {
        panic!("zeroize assertion failed: {error}");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroizeError {
    pub offset: usize,
}

impl fmt::Display for ZeroizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "non-zero byte at offset {}", self.offset)
    }
}

impl std::error::Error for ZeroizeError {}

/// Creates an inspectable, test-only secret owner whose drop path zeroes its backing bytes.
pub fn zeroize_probe(secret: impl Into<Vec<u8>>) -> (ZeroizingTestBytes, ZeroizeObserver) {
    let shared = Rc::new(RefCell::new(secret.into()));
    (
        ZeroizingTestBytes {
            shared: Rc::clone(&shared),
        },
        ZeroizeObserver { shared },
    )
}

pub struct ZeroizingTestBytes {
    shared: Rc<RefCell<Vec<u8>>>,
}

impl ZeroizingTestBytes {
    pub fn with_bytes<R>(&self, inspect: impl FnOnce(&[u8]) -> R) -> R {
        inspect(&self.shared.borrow())
    }
}

impl fmt::Debug for ZeroizingTestBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ZeroizingTestBytes")
            .field("byte_len", &self.shared.borrow().len())
            .finish()
    }
}

impl Drop for ZeroizingTestBytes {
    fn drop(&mut self) {
        self.shared.borrow_mut().zeroize();
    }
}

#[derive(Clone)]
pub struct ZeroizeObserver {
    shared: Rc<RefCell<Vec<u8>>>,
}

impl ZeroizeObserver {
    pub fn check(&self) -> Result<(), ZeroizeError> {
        check_zeroized(&self.shared.borrow())
    }

    #[track_caller]
    pub fn assert_zeroized(&self) {
        assert_zeroized(&self.shared.borrow());
    }
}

impl fmt::Debug for ZeroizeObserver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ZeroizeObserver")
            .field("byte_len", &self.shared.borrow().len())
            .finish()
    }
}

const PUBLIC_EFFECT_KIND_COUNT: usize = 15;

/// Publicly observable effects which safety-boundary tests commonly forbid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PublicEffectKind {
    Event,
    Store,
    Route,
    Selection,
    Panel,
    Subscriber,
    Cache,
    Callback,
    Fetch,
    Connection,
    Mutation,
    Query,
    AnimationFrame,
    DomHandler,
    Observer,
}

impl PublicEffectKind {
    pub const ALL: [Self; PUBLIC_EFFECT_KIND_COUNT] = [
        Self::Event,
        Self::Store,
        Self::Route,
        Self::Selection,
        Self::Panel,
        Self::Subscriber,
        Self::Cache,
        Self::Callback,
        Self::Fetch,
        Self::Connection,
        Self::Mutation,
        Self::Query,
        Self::AnimationFrame,
        Self::DomHandler,
        Self::Observer,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublicEffectSnapshot {
    counts: [u64; PUBLIC_EFFECT_KIND_COUNT],
}

impl PublicEffectSnapshot {
    pub fn count(&self, kind: PublicEffectKind) -> u64 {
        self.counts[kind.index()]
    }
}

#[derive(Clone, Debug, Default)]
pub struct PublicEffectProbe(Rc<RefCell<PublicEffectSnapshot>>);

impl PublicEffectProbe {
    pub fn record(&self, kind: PublicEffectKind) -> Result<(), PublicEffectOverflow> {
        let mut snapshot = self.0.borrow_mut();
        let count = &mut snapshot.counts[kind.index()];
        *count = count.checked_add(1).ok_or(PublicEffectOverflow { kind })?;
        Ok(())
    }

    pub fn snapshot(&self) -> PublicEffectSnapshot {
        self.0.borrow().clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicEffectOverflow {
    pub kind: PublicEffectKind,
}

impl fmt::Display for PublicEffectOverflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "public effect counter overflow for {:?}",
            self.kind
        )
    }
}

impl std::error::Error for PublicEffectOverflow {}

#[track_caller]
pub fn assert_no_public_effects(before: &PublicEffectSnapshot, after: &PublicEffectSnapshot) {
    for kind in PublicEffectKind::ALL {
        let before_count = before.count(kind);
        let after_count = after.count(kind);
        assert_eq!(
            before_count, after_count,
            "unexpected public effect: kind={kind:?}"
        );
    }
}

/// A snapshot of the production interner's permanent byte counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InternerSnapshot {
    bytes_used: u64,
}

impl InternerSnapshot {
    pub fn capture(read_bytes_used: impl FnOnce() -> usize) -> Self {
        Self {
            bytes_used: u64::try_from(read_bytes_used()).unwrap_or(u64::MAX),
        }
    }

    pub const fn from_bytes_used(bytes_used: u64) -> Self {
        Self { bytes_used }
    }

    pub const fn bytes_used(self) -> u64 {
        self.bytes_used
    }
}

#[track_caller]
pub fn assert_no_interner_delta(before: InternerSnapshot, after: InternerSnapshot) {
    assert_eq!(
        before.bytes_used, after.bytes_used,
        "process-global interner changed across a zero-delta boundary"
    );
}

/// Checked conversion used by cross-language tests that represent counters as wider integers.
pub fn checked_u64(value: u128) -> Option<u64> {
    u64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: ResourceKey = ResourceKey::new(1);
    const BUFFER: ResourceKey = ResourceKey::new(2);

    fn registry() -> CheckedResourceRegistry {
        CheckedResourceRegistry::new([
            (
                OWNER,
                ResourceLimits {
                    max_count: 2,
                    max_bytes: 16,
                },
            ),
            (
                BUFFER,
                ResourceLimits {
                    max_count: 2,
                    max_bytes: 32,
                },
            ),
        ])
        .expect("unique resource classes")
    }

    struct ReleaseOnIntoIter {
        reservation: ResourceReservation,
        request: ReservationRequest,
    }

    impl IntoIterator for ReleaseOnIntoIter {
        type Item = ReservationRequest;
        type IntoIter = std::iter::Once<ReservationRequest>;

        fn into_iter(self) -> Self::IntoIter {
            let Self {
                reservation,
                request,
            } = self;
            drop(reservation);
            std::iter::once(request)
        }
    }

    struct CommitOnNext {
        prepared: Option<PreparedReservations>,
        retained: Rc<RefCell<Option<ResourceReservation>>>,
        request: Option<ReservationRequest>,
    }

    impl Iterator for CommitOnNext {
        type Item = ReservationRequest;

        fn next(&mut self) -> Option<Self::Item> {
            if let Some(prepared) = self.prepared.take() {
                let reservation = prepared.commit().expect("reentrant commit");
                self.retained.replace(Some(reservation));
            }
            self.request.take()
        }
    }

    struct CommitReleaseThenOverflow {
        prepared: Option<PreparedReservations>,
        reservation: Option<ResourceReservation>,
        step: u8,
    }

    impl Iterator for CommitReleaseThenOverflow {
        type Item = ReservationRequest;

        fn next(&mut self) -> Option<Self::Item> {
            match self.step {
                0 => {
                    self.step = 1;
                    self.reservation = Some(
                        self.prepared
                            .take()
                            .expect("prepared reentrant reservation")
                            .commit()
                            .expect("reentrant commit"),
                    );
                    Some(ReservationRequest::new(OWNER, u64::MAX, 0))
                }
                1 => {
                    self.step = 2;
                    Some(ReservationRequest::new(OWNER, 1, 0))
                }
                _ => None,
            }
        }
    }

    #[test]
    fn caller_iterator_never_runs_under_registry_borrow() {
        let actual = registry();
        let existing = actual
            .prepare([ReservationRequest::new(OWNER, 1, 8)])
            .expect("prepare existing reservation")
            .commit()
            .expect("commit existing reservation");
        let expected_after_release = {
            let control = registry();
            let reservation = control
                .prepare([ReservationRequest::new(OWNER, 1, 8)])
                .expect("prepare control reservation")
                .commit()
                .expect("commit control reservation");
            drop(reservation);
            control.snapshot()
        };
        let prepared = actual
            .prepare(ReleaseOnIntoIter {
                reservation: existing,
                request: ReservationRequest::new(OWNER, 1, 1),
            })
            .expect("IntoIterator may release registry ownership");
        drop(prepared);
        assert_registry_unchanged(&expected_after_release, &actual.snapshot());

        let reentrant = actual
            .prepare([ReservationRequest::new(BUFFER, 1, 2)])
            .expect("prepare reservation committed from next");
        let retained = Rc::new(RefCell::new(None));
        let outer = actual
            .prepare(CommitOnNext {
                prepared: Some(reentrant),
                retained: Rc::clone(&retained),
                request: Some(ReservationRequest::new(OWNER, 1, 1)),
            })
            .expect("Iterator::next may commit registry ownership");
        drop(outer);
        assert_eq!(
            actual.snapshot().usage(BUFFER),
            Some(ResourceUsage {
                current_count: 1,
                current_bytes: 2,
                high_water_count: 1,
                high_water_bytes: 2,
            })
        );
        drop(retained.borrow_mut().take());
        assert!(actual.snapshot().is_live_empty());
    }

    #[test]
    fn early_iterator_error_drops_reentrant_owner_and_preserves_exact_state() {
        let expected = {
            let control = registry();
            let reservation = control
                .prepare([ReservationRequest::new(BUFFER, 1, 2)])
                .expect("prepare control reservation")
                .commit()
                .expect("commit control reservation");
            drop(reservation);
            control.snapshot()
        };
        let registry = registry();
        let reentrant = registry
            .prepare([ReservationRequest::new(BUFFER, 1, 2)])
            .expect("prepare reentrant reservation");
        assert_eq!(
            registry
                .prepare(CommitReleaseThenOverflow {
                    prepared: Some(reentrant),
                    reservation: None,
                    step: 0,
                })
                .expect_err("request aggregation must overflow"),
            ResourceReservationError::CountOverflow { key: OWNER }
        );
        assert_registry_unchanged(&expected, &registry.snapshot());
    }

    #[test]
    fn failed_and_aborted_prepare_leave_exact_snapshot() {
        let registry = registry();
        let before = registry.snapshot();
        let error = registry
            .prepare([
                ReservationRequest::new(OWNER, 1, 8),
                ReservationRequest::new(BUFFER, 1, 33),
            ])
            .expect_err("byte limit must reject batch");
        assert_eq!(
            error,
            ResourceReservationError::ByteLimitExceeded { key: BUFFER }
        );
        assert_registry_unchanged(&before, &registry.snapshot());

        let prepared = registry
            .prepare([ReservationRequest::new(OWNER, 1, 8)])
            .expect("prepare within limits");
        drop(prepared);
        assert_registry_unchanged(&before, &registry.snapshot());
    }

    #[test]
    fn reservation_batches_sum_duplicates_and_release_once() {
        let registry = registry();
        let reservation = registry
            .prepare([
                ReservationRequest::new(OWNER, 1, 3),
                ReservationRequest::new(OWNER, 1, 5),
            ])
            .expect("prepare within aggregate limits")
            .commit()
            .expect("matching revision");
        assert_eq!(
            registry.snapshot().usage(OWNER),
            Some(ResourceUsage {
                current_count: 2,
                current_bytes: 8,
                high_water_count: 2,
                high_water_bytes: 8,
            })
        );
        drop(reservation);
        let released = registry.snapshot();
        assert!(released.is_live_empty());
        assert_eq!(
            released.usage(OWNER).expect("owner usage").high_water_bytes,
            8
        );
    }

    #[test]
    fn concurrent_reservations_release_in_either_order() {
        let registry = registry();
        let first = registry
            .prepare([ReservationRequest::new(OWNER, 1, 3)])
            .expect("prepare first reservation")
            .commit()
            .expect("commit first reservation");
        let second = registry
            .prepare([ReservationRequest::new(BUFFER, 1, 5)])
            .expect("prepare second reservation")
            .commit()
            .expect("commit second reservation");
        drop(first);
        assert_eq!(
            registry.snapshot().usage(BUFFER),
            Some(ResourceUsage {
                current_count: 1,
                current_bytes: 5,
                high_water_count: 1,
                high_water_bytes: 5,
            })
        );
        drop(second);
        assert!(registry.snapshot().is_live_empty());
    }

    #[test]
    fn overflow_and_stale_commit_do_not_mutate_registry() {
        let registry = registry();
        let before = registry.snapshot();
        let error = registry
            .prepare([
                ReservationRequest::new(OWNER, u64::MAX, 0),
                ReservationRequest::new(OWNER, 1, 0),
            ])
            .expect_err("aggregate overflow");
        assert_eq!(
            error,
            ResourceReservationError::CountOverflow { key: OWNER }
        );
        assert_registry_unchanged(&before, &registry.snapshot());
        let error = registry
            .prepare([
                ReservationRequest::new(BUFFER, 0, u64::MAX),
                ReservationRequest::new(BUFFER, 0, 1),
            ])
            .expect_err("aggregate byte overflow");
        assert_eq!(
            error,
            ResourceReservationError::ByteOverflow { key: BUFFER }
        );
        assert_registry_unchanged(&before, &registry.snapshot());

        let stale = registry
            .prepare([ReservationRequest::new(OWNER, 1, 1)])
            .expect("prepare stale reservation");
        let current = registry
            .prepare([ReservationRequest::new(BUFFER, 1, 1)])
            .expect("prepare current reservation")
            .commit()
            .expect("commit current reservation");
        let before_stale_commit = registry.snapshot();
        assert_eq!(
            stale.commit().expect_err("revision mismatch"),
            ResourceReservationError::PreparedRevisionMismatch
        );
        assert_registry_unchanged(&before_stale_commit, &registry.snapshot());
        drop(current);
    }

    #[test]
    fn exact_snapshot_does_not_require_debug_output() {
        struct SecretState(&'static str);
        impl Clone for SecretState {
            fn clone(&self) -> Self {
                Self(self.0)
            }
        }
        impl PartialEq for SecretState {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }
        let state = SecretState("must-not-appear-in-diagnostics");
        ExactSnapshot::capture(&state).assert_unchanged(&state);
    }

    #[test]
    fn shared_corpus_detects_every_sensitive_category_without_echoing_values() {
        let detector = redaction_leak_corpus_v1().expect("valid shared corpus");
        assert_eq!(detector.values.len(), SensitiveKind::ALL.len());
        for sensitive in &detector.values {
            let error = detector
                .inspect(&sensitive.bytes)
                .expect_err("sentinel must be detected");
            assert_eq!(error.kind, sensitive.kind);
            assert_eq!(error.offset, 0);
            assert!(
                !error
                    .to_string()
                    .as_bytes()
                    .windows(sensitive.bytes.len())
                    .any(|window| window == sensitive.bytes.as_ref())
            );
        }
        detector.assert_redacted("code=remote_open_failed stage=activation");
    }

    #[test]
    fn leak_detector_zeroizes_independent_clones() {
        let secret = b"clone-specific-secret";
        let mut sensitive = SensitiveValue {
            kind: SensitiveKind::Query,
            bytes: secret.as_slice().into(),
        };
        sensitive.zeroize();
        assert!(sensitive.bytes.iter().all(|byte| *byte == 0));

        let mut detector = LeakDetector::new();
        detector
            .register(SensitiveKind::Query, secret)
            .expect("non-empty sentinel");
        let mut clone = detector.clone();
        detector.dispose();
        assert!(detector.values.is_empty());
        assert_eq!(
            clone
                .inspect(secret)
                .expect_err("clone has independent sentinel storage")
                .kind,
            SensitiveKind::Query
        );
        clone.dispose();
        assert!(clone.values.is_empty());
    }

    #[test]
    fn bounded_labels_reject_size_and_secret_leaks() {
        let detector = redaction_leak_corpus_v1().expect("valid shared corpus");
        let label = BoundedLabel::try_new("remote_open_failed", 32, &detector)
            .expect("bounded redacted label");
        assert_eq!(label.as_str(), "remote_open_failed");
        assert!(matches!(
            BoundedLabel::try_new("12345", 4, &detector),
            Err(BoundedLabelError::TooLong { .. })
        ));
        for sensitive in &detector.values {
            assert!(matches!(
                BoundedLabel::try_new(
                    String::from_utf8_lossy(&sensitive.bytes).into_owned(),
                    usize::MAX,
                    &detector
                ),
                Err(BoundedLabelError::Leak(LeakError { kind, .. }))
                    if kind == sensitive.kind
            ));
        }
    }

    #[test]
    fn zeroize_probe_is_observable_without_exposing_bytes() {
        let (owner, observer) = zeroize_probe(b"sensitive bytes".to_vec());
        assert!(observer.check().is_err());
        owner.with_bytes(|bytes| assert_eq!(bytes.len(), 15));
        drop(owner);
        observer.assert_zeroized();
        assert!(check_zeroized(&[0, 0, 1]).is_err());
    }

    #[test]
    fn public_effect_and_interner_snapshots_detect_deltas() {
        let effects = PublicEffectProbe::default();
        let before = effects.snapshot();
        assert_no_public_effects(&before, &effects.snapshot());
        effects
            .record(PublicEffectKind::Store)
            .expect("effect counter has capacity");
        let after = effects.snapshot();
        assert_eq!(after.count(PublicEffectKind::Store), 1);

        let interner_before = InternerSnapshot::capture(|| 42);
        let interner_after = InternerSnapshot::from_bytes_used(42);
        assert_no_interner_delta(interner_before, interner_after);
    }

    #[test]
    fn u64_conversion_is_checked() {
        assert_eq!(checked_u64(u64::MAX as u128), Some(u64::MAX));
        assert_eq!(checked_u64(u64::MAX as u128 + 1), None);
    }
}
