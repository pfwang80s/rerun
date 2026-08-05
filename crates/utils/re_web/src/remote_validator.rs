//! Typed, bounded HTTP entity-tag handling for Web remote-MCAP sources.
//!
//! Entity-tags are opaque RFC 9110 wire bytes.
//! This module never approximates `obs-text` as UTF-8 and is not connected to Fetch, Range,
//! native HTTP, or compatibility paths.

use std::fmt;
use std::mem::size_of;
use std::num::NonZeroU64;
use std::rc::Rc;

use zeroize::{Zeroize as _, Zeroizing};

use crate::remote_limits::{ActiveRemoteValidatorRetainedReservation, ScopeAccountingError};

#[cfg(target_arch = "wasm32")]
use crate::remote_limits::{
    ActiveRemoteValidatorEgressReservation, PreparedRemoteValidatorEgressReservation,
    WasmModuleLimitAccountingRoot, WorkUnitAccountingScope,
};
#[cfg(target_arch = "wasm32")]
use js_sys::{Array, Function, JsString, Reflect};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast as _, JsValue};

/// Fixed failures from validator parsing, `ByteString` conversion, or resource ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteValidatorError {
    MultipleHeaderValues,
    InvalidHeaderValueType,
    InvalidEntityTag,
    InvalidWeakPrefix,
    MultipleOrTrailingEntityTags,
    ByteStringCodeUnitOutOfRange,
    ResourceLimit,
    SizeOverflow,
    HeaderAdapterUnavailable,
    HeaderRejected,
}

impl fmt::Display for RemoteValidatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MultipleHeaderValues => "multiple entity-tag header values are not allowed",
            Self::InvalidHeaderValueType => "entity-tag header value is not a ByteString",
            Self::InvalidEntityTag => "entity-tag header value is invalid",
            Self::InvalidWeakPrefix => "entity-tag weak prefix is invalid",
            Self::MultipleOrTrailingEntityTags => {
                "multiple or trailing entity-tag values are not allowed"
            }
            Self::ByteStringCodeUnitOutOfRange => {
                "entity-tag header value contains a code unit outside ByteString range"
            }
            Self::ResourceLimit => "entity-tag resource ownership was rejected",
            Self::SizeOverflow => "entity-tag size accounting overflowed",
            Self::HeaderAdapterUnavailable => "the raw Headers adapter is unavailable",
            Self::HeaderRejected => "the Headers implementation rejected the entity-tag",
        })
    }
}

impl std::error::Error for RemoteValidatorError {}

impl From<ScopeAccountingError> for RemoteValidatorError {
    fn from(_error: ScopeAccountingError) -> Self {
        Self::ResourceLimit
    }
}

enum RetainedReservation {
    Accounted {
        _owner: ActiveRemoteValidatorRetainedReservation,
    },
    #[cfg(test)]
    TestOnly,
}

impl fmt::Debug for RetainedReservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RetainedReservation(<opaque>)")
    }
}

struct SecretEntityTagBytes {
    wire: Box<[u8]>,
    _reservation: RetainedReservation,
}

impl SecretEntityTagBytes {
    fn as_bytes(&self) -> &[u8] {
        &self.wire
    }
}

impl Drop for SecretEntityTagBytes {
    fn drop(&mut self) {
        self.wire.zeroize();
        record_test_secret_drop(self.wire.iter().all(|byte| *byte == 0));
    }
}

/// A validated strong entity-tag retaining the complete quoted wire bytes.
///
/// It intentionally does not implement `Hash`, serialization, or a revealing formatter.
///
/// ```compile_fail
/// use re_web::remote_validator::StrongEntityTag;
///
/// fn requires_hash<T: std::hash::Hash>() {}
/// requires_hash::<StrongEntityTag>();
/// ```
#[derive(Clone)]
pub struct StrongEntityTag(Rc<SecretEntityTagBytes>);

impl StrongEntityTag {
    /// Compares the complete strong wire values without exposing them.
    pub fn matches(&self, other: &Self) -> bool {
        constant_time_eq::constant_time_eq(self.0.as_bytes(), other.0.as_bytes())
    }

    /// Returns the retained wire byte count without exposing the value.
    pub fn wire_len(&self) -> usize {
        self.0.as_bytes().len()
    }

    /// Prepares the crate-private, one-shot `If-Match` Headers owner.
    #[cfg(target_arch = "wasm32")]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "MCAP-014 will consume the disarmed If-Match owner"
        )
    )]
    pub(crate) fn prepare_if_match_headers(
        &self,
        root: &WasmModuleLimitAccountingRoot,
        work_unit: &WorkUnitAccountingScope,
    ) -> Result<PreparedIfMatchHeadersOwner, RemoteValidatorError> {
        let wire_bytes = NonZeroU64::new(
            u64::try_from(self.wire_len()).map_err(|_error| RemoteValidatorError::SizeOverflow)?,
        )
        .ok_or(RemoteValidatorError::InvalidEntityTag)?;
        let reservation = work_unit.prepare_remote_validator_egress(root, wire_bytes)?;
        if reservation.accounted_bytes() != wire_bytes {
            return Err(RemoteValidatorError::ResourceLimit);
        }
        Ok(PreparedIfMatchHeadersOwner {
            validator: self.clone(),
            reservation,
        })
    }

    #[cfg(test)]
    fn wire_bytes_for_test(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for StrongEntityTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StrongEntityTag(<redacted>)")
    }
}

impl fmt::Display for StrongEntityTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-strong-entity-tag>")
    }
}

/// A validated weak entity-tag retaining the complete `W/` plus quoted wire bytes.
///
/// Weak tags deliberately cannot create the crate-internal `If-Match` owner.
/// External callers cannot invoke the strong-tag constructor either.
/// The type distinction enforces this invariant inside the future Range implementation.
///
/// ```compile_fail
/// # use re_web::remote_validator::WeakEntityTag;
/// # use re_web::remote_limits::{WasmModuleLimitAccountingRoot, WorkUnitAccountingScope};
/// fn cannot_build_if_match(
///     tag: &WeakEntityTag,
///     root: &WasmModuleLimitAccountingRoot,
///     work: &WorkUnitAccountingScope,
/// ) {
///     let _ = tag.prepare_if_match_headers(root, work);
/// }
/// ```
#[derive(Clone)]
pub struct WeakEntityTag(Rc<SecretEntityTagBytes>);

impl WeakEntityTag {
    /// Compares the complete weak wire values without exposing them.
    pub fn matches(&self, other: &Self) -> bool {
        constant_time_eq::constant_time_eq(self.0.as_bytes(), other.0.as_bytes())
    }

    /// Returns the retained wire byte count without exposing the value.
    pub fn wire_len(&self) -> usize {
        self.0.as_bytes().len()
    }

    #[cfg(test)]
    fn wire_bytes_for_test(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for WeakEntityTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WeakEntityTag(<redacted>)")
    }
}

impl fmt::Display for WeakEntityTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-weak-entity-tag>")
    }
}

/// One successfully parsed visible entity-tag.
pub enum ParsedEntityTag {
    Strong(StrongEntityTag),
    Weak(WeakEntityTag),
}

impl fmt::Debug for ParsedEntityTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Strong(_) => "ParsedEntityTag::Strong(<redacted>)",
            Self::Weak(_) => "ParsedEntityTag::Weak(<redacted>)",
        })
    }
}

/// The caller's frozen representation-consistency admission policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RepresentationConsistencyPolicy {
    #[default]
    RequireStrongValidator,
    AllowDeploymentAssumed,
}

/// The actual representation-consistency capability observed after the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepresentationConsistency {
    StrongValidator,
    DeploymentAssumed,
}

/// The validator state retained by a bound remote object.
#[derive(Clone)]
pub enum RemoteObjectValidator {
    Strong(StrongEntityTag),
    DeploymentAssumed {
        observed_weak: Option<WeakEntityTag>,
    },
}

impl fmt::Debug for RemoteObjectValidator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Strong(_) => "RemoteObjectValidator::Strong(<redacted>)",
            Self::DeploymentAssumed {
                observed_weak: Some(_),
            } => "RemoteObjectValidator::DeploymentAssumed(observed_weak=<redacted>)",
            Self::DeploymentAssumed {
                observed_weak: None,
            } => "RemoteObjectValidator::DeploymentAssumed(observed_weak=None)",
        })
    }
}

/// An atomically constructed, internally consistent capability/validator pair.
#[derive(Clone)]
pub struct BoundRepresentationConsistency {
    consistency: RepresentationConsistency,
    validator: RemoteObjectValidator,
}

impl BoundRepresentationConsistency {
    pub fn consistency(&self) -> RepresentationConsistency {
        self.consistency
    }

    pub fn validator(&self) -> &RemoteObjectValidator {
        &self.validator
    }
}

impl fmt::Debug for BoundRepresentationConsistency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundRepresentationConsistency")
            .field("consistency", &self.consistency)
            .field("validator", &self.validator)
            .finish()
    }
}

/// A fixed, non-secret rejection from the consistency truth table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrongValidatorRequired;

impl fmt::Display for StrongValidatorRequired {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a strong representation validator is required")
    }
}

impl std::error::Error for StrongValidatorRequired {}

/// Applies the one normative policy × observed-validator truth table.
pub fn bind_representation_consistency(
    policy: RepresentationConsistencyPolicy,
    parsed: Option<ParsedEntityTag>,
) -> Result<BoundRepresentationConsistency, StrongValidatorRequired> {
    match (policy, parsed) {
        (_, Some(ParsedEntityTag::Strong(tag))) => Ok(BoundRepresentationConsistency {
            consistency: RepresentationConsistency::StrongValidator,
            validator: RemoteObjectValidator::Strong(tag),
        }),
        (
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            Some(ParsedEntityTag::Weak(tag)),
        ) => Ok(BoundRepresentationConsistency {
            consistency: RepresentationConsistency::DeploymentAssumed,
            validator: RemoteObjectValidator::DeploymentAssumed {
                observed_weak: Some(tag),
            },
        }),
        (RepresentationConsistencyPolicy::AllowDeploymentAssumed, None) => {
            Ok(BoundRepresentationConsistency {
                consistency: RepresentationConsistency::DeploymentAssumed,
                validator: RemoteObjectValidator::DeploymentAssumed {
                    observed_weak: None,
                },
            })
        }
        (
            RepresentationConsistencyPolicy::RequireStrongValidator,
            Some(ParsedEntityTag::Weak(_)) | None,
        ) => Err(StrongValidatorRequired),
    }
}

trait WireInput {
    fn len(&self) -> usize;
    fn byte_at(&self, index: usize) -> Result<u8, RemoteValidatorError>;
}

impl WireInput for &[u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn byte_at(&self, index: usize) -> Result<u8, RemoteValidatorError> {
        self.get(index)
            .copied()
            .ok_or(RemoteValidatorError::InvalidEntityTag)
    }
}

#[cfg(target_arch = "wasm32")]
struct JsByteStringInput<'a>(&'a JsString);

#[cfg(target_arch = "wasm32")]
impl WireInput for JsByteStringInput<'_> {
    fn len(&self) -> usize {
        self.0.length() as usize
    }

    fn byte_at(&self, index: usize) -> Result<u8, RemoteValidatorError> {
        let index = u32::try_from(index).map_err(|_error| RemoteValidatorError::SizeOverflow)?;
        let code_unit = self.0.char_code_at(index) as u32;
        u8::try_from(code_unit).map_err(|_error| RemoteValidatorError::ByteStringCodeUnitOutOfRange)
    }
}

#[derive(Clone, Copy)]
enum EntityTagKind {
    Strong,
    Weak,
}

struct EntityTagPlan {
    kind: EntityTagKind,
    start: usize,
    end: usize,
}

fn preflight_entity_tag(input: &impl WireInput) -> Result<EntityTagPlan, RemoteValidatorError> {
    let full_len = input.len();
    let mut start = 0usize;
    while start < full_len && is_ows(input.byte_at(start)?) {
        start += 1;
    }
    let mut end = full_len;
    while end > start && is_ows(input.byte_at(end - 1)?) {
        end -= 1;
    }
    if start == end {
        return Err(RemoteValidatorError::InvalidEntityTag);
    }

    let first = input.byte_at(start)?;
    let second_index = start
        .checked_add(1)
        .ok_or(RemoteValidatorError::SizeOverflow)?;
    let second = (second_index < end)
        .then(|| input.byte_at(second_index))
        .transpose()?;
    let (kind, quote_start) = match (first, second) {
        (b'"', _) => (EntityTagKind::Strong, start),
        (b'W', Some(b'/')) => {
            let quote_start = start
                .checked_add(2)
                .ok_or(RemoteValidatorError::SizeOverflow)?;
            if quote_start >= end || input.byte_at(quote_start)? != b'"' {
                return Err(RemoteValidatorError::InvalidEntityTag);
            }
            (EntityTagKind::Weak, quote_start)
        }
        (b'w', Some(b'/')) => return Err(RemoteValidatorError::InvalidWeakPrefix),
        _ => return Err(RemoteValidatorError::InvalidEntityTag),
    };

    let content_start = quote_start
        .checked_add(1)
        .ok_or(RemoteValidatorError::SizeOverflow)?;
    let mut closing_quote = None;
    for index in content_start..end {
        let byte = input.byte_at(index)?;
        if byte == b'"' {
            closing_quote = Some(index);
            break;
        }
        if !is_etagc(byte) {
            return Err(RemoteValidatorError::InvalidEntityTag);
        }
    }
    let closing_quote = closing_quote.ok_or(RemoteValidatorError::InvalidEntityTag)?;
    if closing_quote
        .checked_add(1)
        .ok_or(RemoteValidatorError::SizeOverflow)?
        != end
    {
        return Err(RemoteValidatorError::MultipleOrTrailingEntityTags);
    }
    Ok(EntityTagPlan { kind, start, end })
}

fn retained_bytes(plan: &EntityTagPlan) -> Result<NonZeroU64, RemoteValidatorError> {
    let wire_len = plan
        .end
        .checked_sub(plan.start)
        .ok_or(RemoteValidatorError::SizeOverflow)?;
    let retained = wire_len
        .checked_add(size_of::<SecretEntityTagBytes>())
        .and_then(|bytes| bytes.checked_add(2 * size_of::<usize>()))
        .and_then(|bytes| bytes.checked_add(retained_outer_shell_bytes()))
        .ok_or(RemoteValidatorError::SizeOverflow)?;
    NonZeroU64::new(u64::try_from(retained).map_err(|_error| RemoteValidatorError::SizeOverflow)?)
        .ok_or(RemoteValidatorError::SizeOverflow)
}

const fn retained_outer_shell_bytes() -> usize {
    let parsed = size_of::<ParsedEntityTag>();
    let bound = size_of::<BoundRepresentationConsistency>();
    if parsed > bound { parsed } else { bound }
}

fn materialize_entity_tag(
    input: &impl WireInput,
    plan: &EntityTagPlan,
    reservation: RetainedReservation,
) -> Result<ParsedEntityTag, RemoteValidatorError> {
    let wire_len = plan
        .end
        .checked_sub(plan.start)
        .ok_or(RemoteValidatorError::SizeOverflow)?;
    record_test_wire_allocation();
    let mut scratch = Zeroizing::new(Vec::with_capacity(wire_len));
    for index in plan.start..plan.end {
        scratch.push(input.byte_at(index)?);
    }
    let owner = Rc::new(SecretEntityTagBytes {
        wire: scratch.as_slice().into(),
        _reservation: reservation,
    });
    Ok(match plan.kind {
        EntityTagKind::Strong => ParsedEntityTag::Strong(StrongEntityTag(owner)),
        EntityTagKind::Weak => ParsedEntityTag::Weak(WeakEntityTag(owner)),
    })
}

fn is_ows(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn is_etagc(byte: u8) -> bool {
    byte == 0x21 || (0x23..=0x7E).contains(&byte) || byte >= 0x80
}

/// Parses browser-visible entity-tags directly from raw `JavaScript` values.
///
/// Type and UTF-16 length checks happen without conversion.
/// The typed ingress reservation is committed before the first code unit is validated or copied.
#[cfg(target_arch = "wasm32")]
pub fn parse_chrome_entity_tag(
    values: &[JsValue],
    root: &WasmModuleLimitAccountingRoot,
    work_unit: &WorkUnitAccountingScope,
) -> Result<Option<ParsedEntityTag>, RemoteValidatorError> {
    let value = match values {
        [] => {
            work_unit.validate_remote_validator_ingress_shape(0, 0)?;
            return Ok(None);
        }
        [value] => value,
        [_, _, ..] => return Err(RemoteValidatorError::MultipleHeaderValues),
    };
    let visible_values = 1;
    work_unit.validate_remote_validator_ingress_shape(visible_values, 0)?;
    if !value.is_string() {
        return Err(RemoteValidatorError::InvalidHeaderValueType);
    }
    let value = value.unchecked_ref::<JsString>();
    let wire_bytes = u64::from(value.length());
    work_unit.validate_remote_validator_ingress_shape(visible_values, wire_bytes)?;
    let wire_bytes = NonZeroU64::new(wire_bytes).ok_or(RemoteValidatorError::InvalidEntityTag)?;
    let ingress = work_unit
        .prepare_remote_validator_ingress(root, NonZeroU64::MIN, wire_bytes)?
        .commit()?;
    if ingress.accounted_bytes() != wire_bytes {
        return Err(RemoteValidatorError::ResourceLimit);
    }

    let input = JsByteStringInput(value);
    let plan = preflight_entity_tag(&input)?;
    let retained_bytes = retained_bytes(&plan)?;
    let retained = work_unit
        .prepare_remote_validator_retained(root, retained_bytes)?
        .commit()?;
    if retained.accounted_bytes() != retained_bytes {
        return Err(RemoteValidatorError::ResourceLimit);
    }
    let parsed = materialize_entity_tag(
        &input,
        &plan,
        RetainedReservation::Accounted { _owner: retained },
    )?;
    ingress.release()?;
    Ok(Some(parsed))
}

/// A checked but disarmed, crate-private `If-Match` operation.
///
/// It is unique, non-cloneable, and exposes neither Headers nor the `ByteString`.
#[cfg(target_arch = "wasm32")]
pub(crate) struct PreparedIfMatchHeadersOwner {
    validator: StrongEntityTag,
    reservation: PreparedRemoteValidatorEgressReservation,
}

#[cfg(target_arch = "wasm32")]
impl fmt::Debug for PreparedIfMatchHeadersOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedIfMatchHeadersOwner(<redacted>)")
    }
}

#[cfg(target_arch = "wasm32")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "MCAP-014 will consume the disarmed If-Match owner"
    )
)]
impl PreparedIfMatchHeadersOwner {
    /// Consumes the disarmed owner and installs the value exactly once into an internally-created
    /// Headers object.
    #[expect(
        clippy::result_large_err,
        reason = "installation failure returns the unique retry owner without another allocation"
    )]
    pub(crate) fn bind(self) -> Result<BoundIfMatchHeadersOwner, PrepareIfMatchHeadersFailure> {
        let active = self.reservation.commit().map_err(|error| {
            PrepareIfMatchHeadersFailure::Accounting(RemoteValidatorError::from(error))
        })?;
        RetryableIfMatchHeadersOwner {
            validator: self.validator,
            reservation: active,
        }
        .bind()
        .map_err(PrepareIfMatchHeadersFailure::Installation)
    }

    #[cfg(test)]
    fn arm_for_test(self) -> Result<RetryableIfMatchHeadersOwner, RemoteValidatorError> {
        Ok(RetryableIfMatchHeadersOwner {
            validator: self.validator,
            reservation: self.reservation.commit()?,
        })
    }
}

/// A bind failure which either happened before ownership activation or retains a retry owner.
#[cfg(target_arch = "wasm32")]
pub(crate) enum PrepareIfMatchHeadersFailure {
    Accounting(RemoteValidatorError),
    Installation(IfMatchHeadersInstallationFailure),
}

#[cfg(target_arch = "wasm32")]
impl fmt::Debug for PrepareIfMatchHeadersFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accounting(error) => formatter
                .debug_tuple("PrepareIfMatchHeadersFailure::Accounting")
                .field(error)
                .finish(),
            Self::Installation(failure) => formatter
                .debug_tuple("PrepareIfMatchHeadersFailure::Installation")
                .field(failure)
                .finish(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
struct RetryableIfMatchHeadersOwner {
    validator: StrongEntityTag,
    reservation: ActiveRemoteValidatorEgressReservation,
}

#[cfg(target_arch = "wasm32")]
impl RetryableIfMatchHeadersOwner {
    #[expect(
        clippy::result_large_err,
        reason = "installation failure returns the unique retry owner without another allocation"
    )]
    fn bind(self) -> Result<BoundIfMatchHeadersOwner, IfMatchHeadersInstallationFailure> {
        let headers = match create_headers() {
            Ok(headers) => headers,
            Err(error) => {
                return Err(IfMatchHeadersInstallationFailure { error, retry: self });
            }
        };
        self.bind_to_headers(headers)
    }

    #[expect(
        clippy::result_large_err,
        reason = "installation failure returns the unique retry owner without another allocation"
    )]
    fn bind_to_headers(
        self,
        headers: JsValue,
    ) -> Result<BoundIfMatchHeadersOwner, IfMatchHeadersInstallationFailure> {
        let mut code_units = Zeroizing::new(Vec::with_capacity(self.validator.wire_len()));
        record_test_egress_scratch_state(true);
        code_units.extend(self.validator.0.as_bytes().iter().copied().map(u16::from));
        record_test_egress_allocation();
        let byte_string = JsString::from_char_code(code_units.as_slice());
        record_test_standalone_byte_string_state(true);
        // The 2n scratch is zeroized before `Headers.set` can create its own retained 2n copy.
        // The active 4n reservation therefore covers either scratch + ByteString or ByteString +
        // Headers, never all three simultaneously.
        drop(code_units);
        record_test_egress_scratch_state(false);
        record_test_headers_install_phase();
        if let Err(error) = install_if_match(&headers, &byte_string) {
            drop(headers);
            drop(byte_string);
            record_test_standalone_byte_string_state(false);
            return Err(IfMatchHeadersInstallationFailure { error, retry: self });
        }
        // `Headers.set` has made the sole retained 2n copy owned by this operation.
        // Drop the standalone 2n `JsString` before publishing the bound owner so a future
        // `Headers` -> `Request` copy can coexist with the Headers copy under the same 4n credit.
        drop(byte_string);
        record_test_standalone_byte_string_state(false);
        Ok(BoundIfMatchHeadersOwner {
            headers: Some(headers),
            reservation: Some(self.reservation),
        })
    }

    #[cfg(test)]
    #[expect(
        clippy::result_large_err,
        reason = "the test failure path must retain the unique retry owner without allocation"
    )]
    fn bind_to_headers_for_test(
        self,
        headers: JsValue,
    ) -> Result<BoundIfMatchHeadersOwner, IfMatchHeadersInstallationFailure> {
        self.bind_to_headers(headers)
    }
}

#[cfg(target_arch = "wasm32")]
fn create_headers() -> Result<JsValue, RemoteValidatorError> {
    let constructor = Reflect::get(&js_sys::global(), &JsValue::from_str("Headers"))
        .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?
        .dyn_into::<Function>()
        .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?;
    Reflect::construct(&constructor, &Array::new())
        .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)
}

#[cfg(target_arch = "wasm32")]
fn install_if_match(headers: &JsValue, value: &JsString) -> Result<(), RemoteValidatorError> {
    let setter = Reflect::get(headers, &JsValue::from_str("set"))
        .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?
        .dyn_into::<Function>()
        .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?;
    setter
        .call2(headers, &JsValue::from_str("If-Match"), value.as_ref())
        .map_err(|_error| RemoteValidatorError::HeaderRejected)?;
    Ok(())
}

/// A retryable installation failure which continues to own the one active egress permit.
#[cfg(target_arch = "wasm32")]
pub(crate) struct IfMatchHeadersInstallationFailure {
    error: RemoteValidatorError,
    retry: RetryableIfMatchHeadersOwner,
}

#[cfg(target_arch = "wasm32")]
impl fmt::Debug for IfMatchHeadersInstallationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IfMatchHeadersInstallationFailure(<redacted>)")
    }
}

#[cfg(target_arch = "wasm32")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "MCAP-014 will consume the retryable If-Match installation failure"
    )
)]
impl IfMatchHeadersInstallationFailure {
    pub(crate) const fn error(&self) -> RemoteValidatorError {
        self.error
    }

    #[expect(
        clippy::result_large_err,
        reason = "installation failure returns the unique retry owner without another allocation"
    )]
    pub(crate) fn retry(self) -> Result<BoundIfMatchHeadersOwner, Self> {
        self.retry.bind()
    }
}

/// The unique, non-cloneable Headers/egress-permit ownership unit.
///
/// MCAP-014 may add only a crate-private consuming transfer into its unique Request owner.
#[cfg(target_arch = "wasm32")]
pub(crate) struct BoundIfMatchHeadersOwner {
    // During `Headers.set`, standalone ByteString + Headers copy peak at 4n.
    // Once bound, only the 2n Headers copy remains, so a future 2n Request copy also peaks at 4n.
    // Explicit `Drop` preserves the ownership order: Headers before reservation refund.
    headers: Option<JsValue>,
    reservation: Option<ActiveRemoteValidatorEgressReservation>,
}

#[cfg(target_arch = "wasm32")]
impl fmt::Debug for BoundIfMatchHeadersOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoundIfMatchHeadersOwner(<redacted>)")
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for BoundIfMatchHeadersOwner {
    fn drop(&mut self) {
        drop(self.headers.take());
        record_test_bound_headers_dropped_before_refund();
        drop(self.reservation.take());
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
impl BoundIfMatchHeadersOwner {
    fn header_wire_for_test(&self) -> Result<Vec<u8>, RemoteValidatorError> {
        let headers = self
            .headers
            .as_ref()
            .ok_or(RemoteValidatorError::HeaderAdapterUnavailable)?;
        let getter = Reflect::get(headers, &JsValue::from_str("get"))
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?
            .dyn_into::<Function>()
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?;
        let value = getter
            .call1(headers, &JsValue::from_str("If-Match"))
            .map_err(|_error| RemoteValidatorError::HeaderRejected)?
            .unchecked_into::<JsString>();
        let input = JsByteStringInput(&value);
        (0..input.len()).map(|index| input.byte_at(index)).collect()
    }

    fn copy_into_request_for_test(&self) -> Result<JsValue, RemoteValidatorError> {
        TEST_EGRESS_SCRATCH_LIVE.with(|state| assert!(!state.get()));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(!state.get()));
        let constructor = Reflect::get(&js_sys::global(), &JsValue::from_str("Request"))
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?
            .dyn_into::<Function>()
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?;
        let init = js_sys::Object::new();
        Reflect::set(
            init.as_ref(),
            &JsValue::from_str("headers"),
            self.headers
                .as_ref()
                .ok_or(RemoteValidatorError::HeaderAdapterUnavailable)?,
        )
        .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?;
        let arguments = Array::new();
        arguments.push(&JsValue::from_str("https://example.invalid/"));
        arguments.push(init.as_ref());
        Reflect::construct(&constructor, &arguments)
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)
    }
}

#[cfg(test)]
thread_local! {
    static TEST_WIRE_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_EGRESS_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_EGRESS_SCRATCH_LIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_STANDALONE_BYTE_STRING_LIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_HEADERS_INSTALLS_AFTER_SCRATCH_DROP: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_SECRET_DROPS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
fn record_test_wire_allocation() {
    TEST_WIRE_ALLOCATIONS.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_test_wire_allocation() {}

#[cfg(test)]
fn record_test_egress_allocation() {
    TEST_EGRESS_ALLOCATIONS.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_test_egress_allocation() {}

#[cfg(test)]
fn record_test_egress_scratch_state(live: bool) {
    TEST_EGRESS_SCRATCH_LIVE.with(|state| state.set(live));
}

#[cfg(not(test))]
fn record_test_egress_scratch_state(_live: bool) {}

#[cfg(test)]
fn record_test_standalone_byte_string_state(live: bool) {
    TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| state.set(live));
}

#[cfg(not(test))]
fn record_test_standalone_byte_string_state(_live: bool) {}

#[cfg(test)]
fn record_test_headers_install_phase() {
    TEST_EGRESS_SCRATCH_LIVE.with(|state| assert!(!state.get()));
    TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(state.get()));
    TEST_HEADERS_INSTALLS_AFTER_SCRATCH_DROP.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_test_headers_install_phase() {}

#[cfg(test)]
fn record_test_bound_headers_dropped_before_refund() {
    TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(!state.get()));
    TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_test_bound_headers_dropped_before_refund() {}

#[cfg(test)]
fn record_test_secret_drop(was_zeroized: bool) {
    TEST_SECRET_DROPS.with(|counts| {
        let (total, zeroized) = counts.get();
        counts.set((total + 1, zeroized + usize::from(was_zeroized)));
    });
}

#[cfg(not(test))]
fn record_test_secret_drop(_was_zeroized: bool) {}

#[cfg(test)]
fn parse_wire_for_test(values: &[&[u8]]) -> Result<Option<ParsedEntityTag>, RemoteValidatorError> {
    let value = match values {
        [] => return Ok(None),
        [value] => *value,
        [_, _, ..] => return Err(RemoteValidatorError::MultipleHeaderValues),
    };
    let plan = preflight_entity_tag(&value)?;
    materialize_entity_tag(&value, &plan, RetainedReservation::TestOnly).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strong(value: &[u8]) -> StrongEntityTag {
        match parse_wire_for_test(&[value]).unwrap().unwrap() {
            ParsedEntityTag::Strong(tag) => tag,
            ParsedEntityTag::Weak(_) => panic!("expected strong tag"),
        }
    }

    fn weak(value: &[u8]) -> WeakEntityTag {
        match parse_wire_for_test(&[value]).unwrap().unwrap() {
            ParsedEntityTag::Weak(tag) => tag,
            ParsedEntityTag::Strong(_) => panic!("expected weak tag"),
        }
    }

    #[test]
    fn accepts_complete_rfc_etagc_including_obs_text_without_utf8() {
        let mut all_obs_text = vec![b'"'];
        all_obs_text.extend(0x80..=0xFF);
        all_obs_text.push(b'"');
        let all_obs = strong(&all_obs_text);
        assert_eq!(all_obs.wire_bytes_for_test(), all_obs_text);
        assert!(std::str::from_utf8(all_obs.wire_bytes_for_test()).is_err());

        let weak_wire = [b"W/\"".as_slice(), &[0x80, 0xFF], b"\""].concat();
        let weak = weak(&weak_wire);
        assert_eq!(weak.wire_bytes_for_test(), weak_wire);

        for content in [0x21, 0x23, 0x7E, 0x80, 0xFF] {
            let wire = [b'"', content, b'"'];
            assert_eq!(strong(&wire).wire_bytes_for_test(), wire);
        }
    }

    #[test]
    fn rejects_ctl_space_quote_and_malformed_shapes() {
        for content in [0x00, 0x09, 0x20, 0x7F] {
            let wire = [b'"', content, b'"'];
            assert_eq!(
                parse_wire_for_test(&[&wire]).unwrap_err(),
                RemoteValidatorError::InvalidEntityTag
            );
        }
        for wire in [
            b"v1".as_slice(),
            b"\"v1".as_slice(),
            b"W/v1".as_slice(),
            b"\"v1\"\"".as_slice(),
        ] {
            assert!(matches!(
                parse_wire_for_test(&[wire]),
                Err(RemoteValidatorError::InvalidEntityTag
                    | RemoteValidatorError::MultipleOrTrailingEntityTags)
            ));
        }
        assert_eq!(
            parse_wire_for_test(&[b"w/\"v1\""]).unwrap_err(),
            RemoteValidatorError::InvalidWeakPrefix
        );
    }

    #[test]
    fn distinguishes_quoted_comma_from_lists_and_duplicate_values() {
        assert_eq!(strong(b"\"a,b\"").wire_bytes_for_test(), b"\"a,b\"");
        for wire in [b"\"a\",\"b\"".as_slice(), b"W/\"a\", W/\"b\"".as_slice()] {
            assert_eq!(
                parse_wire_for_test(&[wire]).unwrap_err(),
                RemoteValidatorError::MultipleOrTrailingEntityTags
            );
        }
        assert_eq!(
            parse_wire_for_test(&[b"\"a\"", b"\"b\""]).unwrap_err(),
            RemoteValidatorError::MultipleHeaderValues
        );
    }

    #[test]
    fn opaque_case_and_strong_weak_wire_forms_are_exact() {
        let lower = strong(b"\"v1\"");
        let upper = strong(b"\"V1\"");
        assert!(!lower.matches(&upper));
        assert_eq!(lower.wire_bytes_for_test(), b"\"v1\"");
        assert_eq!(weak(b"W/\"v1\"").wire_bytes_for_test(), b"W/\"v1\"");
    }

    #[test]
    fn all_policy_observation_combinations_follow_one_truth_table() {
        assert_eq!(
            RepresentationConsistencyPolicy::default(),
            RepresentationConsistencyPolicy::RequireStrongValidator
        );
        for policy in [
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
        ] {
            let bound = bind_representation_consistency(
                policy,
                Some(ParsedEntityTag::Strong(strong(b"\"v1\""))),
            )
            .unwrap();
            assert_eq!(
                bound.consistency(),
                RepresentationConsistency::StrongValidator
            );
            assert!(matches!(
                bound.validator(),
                RemoteObjectValidator::Strong(_)
            ));
        }
        assert_eq!(
            bind_representation_consistency(
                RepresentationConsistencyPolicy::RequireStrongValidator,
                Some(ParsedEntityTag::Weak(weak(b"W/\"v1\""))),
            )
            .unwrap_err(),
            StrongValidatorRequired
        );
        assert_eq!(
            bind_representation_consistency(
                RepresentationConsistencyPolicy::RequireStrongValidator,
                None,
            )
            .unwrap_err(),
            StrongValidatorRequired
        );
        let weak_bound = bind_representation_consistency(
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            Some(ParsedEntityTag::Weak(weak(b"W/\"v1\""))),
        )
        .unwrap();
        assert_eq!(
            weak_bound.consistency(),
            RepresentationConsistency::DeploymentAssumed
        );
        assert!(matches!(
            weak_bound.validator(),
            RemoteObjectValidator::DeploymentAssumed {
                observed_weak: Some(_)
            }
        ));
        let absent = bind_representation_consistency(
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            None,
        )
        .unwrap();
        assert!(matches!(
            absent.validator(),
            RemoteObjectValidator::DeploymentAssumed {
                observed_weak: None
            }
        ));
    }

    #[test]
    fn retained_formula_covers_parsed_and_every_bound_truth_table_shell() {
        assert!(retained_outer_shell_bytes() >= size_of::<ParsedEntityTag>());
        assert!(retained_outer_shell_bytes() >= size_of::<BoundRepresentationConsistency>());

        for wire in [b"\"strong\"".as_slice(), b"W/\"weak\"".as_slice()] {
            let input = wire;
            let plan = preflight_entity_tag(&input).unwrap();
            let expected = wire
                .len()
                .checked_add(size_of::<SecretEntityTagBytes>())
                .and_then(|bytes| bytes.checked_add(2 * size_of::<usize>()))
                .and_then(|bytes| bytes.checked_add(retained_outer_shell_bytes()))
                .unwrap();
            assert_eq!(
                retained_bytes(&plan).unwrap().get(),
                u64::try_from(expected).unwrap()
            );
        }

        TEST_SECRET_DROPS.with(|counts| counts.set((0, 0)));
        let strong_bound = bind_representation_consistency(
            RepresentationConsistencyPolicy::RequireStrongValidator,
            parse_wire_for_test(&[b"\"strong\""]).unwrap(),
        )
        .unwrap();
        assert!(size_of_val(&strong_bound) <= retained_outer_shell_bytes());
        TEST_SECRET_DROPS.with(|counts| assert_eq!(counts.get(), (0, 0)));
        drop(strong_bound);
        TEST_SECRET_DROPS.with(|counts| assert_eq!(counts.get(), (1, 1)));

        let weak_bound = bind_representation_consistency(
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            parse_wire_for_test(&[b"W/\"weak\""]).unwrap(),
        )
        .unwrap();
        assert!(size_of_val(&weak_bound) <= retained_outer_shell_bytes());
        TEST_SECRET_DROPS.with(|counts| assert_eq!(counts.get(), (1, 1)));
        drop(weak_bound);
        TEST_SECRET_DROPS.with(|counts| assert_eq!(counts.get(), (2, 2)));

        let absent_bound = bind_representation_consistency(
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            None,
        )
        .unwrap();
        assert!(size_of_val(&absent_bound) <= retained_outer_shell_bytes());
        drop(absent_bound);
        TEST_SECRET_DROPS.with(|counts| assert_eq!(counts.get(), (2, 2)));
    }

    #[test]
    fn shared_owner_zeroizes_only_after_last_clone_and_formatting_is_redacted() {
        TEST_SECRET_DROPS.with(|counts| counts.set((0, 0)));
        let secret = strong(b"\"\x80\xff\"");
        let clone = secret.clone();
        assert_eq!(format!("{secret:?}"), "StrongEntityTag(<redacted>)");
        assert_eq!(secret.to_string(), "<redacted-strong-entity-tag>");
        drop(secret);
        TEST_SECRET_DROPS.with(|counts| assert_eq!(counts.get(), (0, 0)));
        drop(clone);
        TEST_SECRET_DROPS.with(|counts| assert_eq!(counts.get(), (1, 1)));
    }

    #[test]
    fn errors_and_debug_never_echo_wire_values() {
        let secret = b"\"secret-validator\"";
        let parsed = parse_wire_for_test(&[secret]).unwrap().unwrap();
        assert!(!format!("{parsed:?}").contains("secret-validator"));
        let bound = bind_representation_consistency(
            RepresentationConsistencyPolicy::RequireStrongValidator,
            Some(parsed),
        )
        .unwrap();
        assert!(!format!("{bound:?}").contains("secret-validator"));
        let error = parse_wire_for_test(&[b"\"secret-validator\" trailing"]).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret-validator"));
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    #![expect(
        clippy::unwrap_used,
        reason = "browser contract tests treat setup or assertion failures as test failures"
    )]

    use super::*;
    use crate::remote_limits::{self, WebRemoteLimitKey};
    use js_sys::Object;
    use wasm_bindgen_test::wasm_bindgen_test;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    const TEST_VALIDATOR_RETAINED_CAP: u64 = 512;

    fn copy_js_byte_string_for_test(value: &JsString) -> Result<Vec<u8>, RemoteValidatorError> {
        let input = JsByteStringInput(value);
        (0..input.len()).map(|index| input.byte_at(index)).collect()
    }

    fn make_js_byte_string_for_test(wire: &[u8]) -> JsString {
        let units = wire.iter().copied().map(u16::from).collect::<Vec<_>>();
        JsString::from_char_code(&units)
    }

    fn read_raw_header_value_for_test(
        name: &str,
        value: &JsString,
    ) -> Result<JsValue, RemoteValidatorError> {
        let headers = create_headers()?;
        let setter = Reflect::get(&headers, &JsValue::from_str("set"))
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?
            .dyn_into::<Function>()
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?;
        setter
            .call2(&headers, &JsValue::from_str(name), value.as_ref())
            .map_err(|_error| RemoteValidatorError::HeaderRejected)?;
        let getter = Reflect::get(&headers, &JsValue::from_str("get"))
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?
            .dyn_into::<Function>()
            .map_err(|_error| RemoteValidatorError::HeaderAdapterUnavailable)?;
        getter
            .call1(&headers, &JsValue::from_str(name))
            .map_err(|_error| RemoteValidatorError::HeaderRejected)
    }

    fn obs_text_tag_for_test() -> StrongEntityTag {
        let Some(ParsedEntityTag::Strong(tag)) = parse_wire_for_test(&[b"\"\x80\xff\""]).unwrap()
        else {
            panic!("expected a strong test validator");
        };
        tag
    }

    fn accounting_for_test() -> (WasmModuleLimitAccountingRoot, WorkUnitAccountingScope) {
        let root = remote_limits::tests::test_profile_with(&[
            (WebRemoteLimitKey::RemoteValidatorEgressValues, 1),
            (WebRemoteLimitKey::RemoteValidatorEgressWireBytes, 256),
            (
                WebRemoteLimitKey::RemoteValidatorEgressJsWasmOverlapBytes,
                512,
            ),
            (WebRemoteLimitKey::RemoteValidatorEgressScratchBytes, 512),
            (
                WebRemoteLimitKey::RemoteValidatorRetainedBytes,
                TEST_VALIDATOR_RETAINED_CAP,
            ),
            (WebRemoteLimitKey::RemoteInternalRetainedBytes, 1_024),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let source = viewer.create_source_scope().unwrap();
        let session = source.create_session_scope().unwrap();
        let range = session.create_range_response_scope().unwrap();
        let work = range.create_work_unit_scope().unwrap();
        (root, work)
    }

    fn warm_accounting_for_valid_chain(
        root: &WasmModuleLimitAccountingRoot,
        work: &WorkUnitAccountingScope,
        wire: &[u8],
    ) {
        let input = wire;
        let plan = preflight_entity_tag(&input).unwrap();
        let retained_bytes = retained_bytes(&plan).unwrap();
        assert!(retained_bytes.get() <= TEST_VALIDATOR_RETAINED_CAP);
        let wire_bytes = NonZeroU64::new(u64::try_from(wire.len()).unwrap()).unwrap();
        let ingress = work
            .prepare_remote_validator_ingress(root, NonZeroU64::MIN, wire_bytes)
            .unwrap()
            .commit()
            .unwrap();
        let retained = work
            .prepare_remote_validator_retained(root, retained_bytes)
            .unwrap()
            .commit()
            .unwrap();
        drop(ingress);
        let egress = work
            .prepare_remote_validator_egress(root, wire_bytes)
            .unwrap()
            .commit()
            .unwrap();
        drop(egress);
        drop(retained);
    }

    fn warm_accounting_for_rejected_ingress(
        root: &WasmModuleLimitAccountingRoot,
        work: &WorkUnitAccountingScope,
        wire_bytes: NonZeroU64,
    ) {
        let ingress = work
            .prepare_remote_validator_ingress(root, NonZeroU64::MIN, wire_bytes)
            .unwrap()
            .commit()
            .unwrap();
        drop(ingress);
    }

    #[wasm_bindgen_test]
    fn chrome_byte_string_roundtrips_obs_text_without_utf8() {
        let wire = [b'"', 0x80, 0xFF, b'"'];
        let byte_string = make_js_byte_string_for_test(&wire);
        assert_eq!(copy_js_byte_string_for_test(&byte_string).unwrap(), wire);
        assert_eq!(
            (0..byte_string.length())
                .map(|index| byte_string.char_code_at(index) as u16)
                .collect::<Vec<_>>(),
            wire.map(u16::from)
        );
    }

    #[wasm_bindgen_test]
    fn chrome_public_parser_rejects_non_byte_code_unit_without_partial_ownership() {
        let (root, work) = accounting_for_test();
        warm_accounting_for_rejected_ingress(&root, &work, NonZeroU64::new(3).unwrap());
        let before = root.snapshot();
        let before_scalar = root.accounting_scalar_snapshot();
        TEST_WIRE_ALLOCATIONS.with(|count| count.set(0));
        let value = JsString::from_char_code(&[u16::from(b'"'), 0x0100, u16::from(b'"')]);
        let Err(error) = parse_chrome_entity_tag(&[value.into()], &root, &work) else {
            panic!("out-of-range ByteString code unit must fail");
        };
        assert_eq!(error, RemoteValidatorError::ByteStringCodeUnitOutOfRange);
        TEST_WIRE_ALLOCATIONS.with(|count| assert_eq!(count.get(), 0));
        assert_eq!(root.accounting_scalar_snapshot(), before_scalar);
        assert!(
            root.snapshot()
                .same_except_revision_and_reservation_sequence(&before)
        );
    }

    #[wasm_bindgen_test]
    fn chrome_public_parser_rejects_duplicate_values_semantically_before_accounting() {
        let (root, work) = accounting_for_test();
        let before = root.snapshot();
        let before_scalar = root.accounting_scalar_snapshot();
        let first = make_js_byte_string_for_test(b"\"first\"");
        let second = make_js_byte_string_for_test(b"\"second\"");
        assert_eq!(
            parse_chrome_entity_tag(&[first.into(), second.into()], &root, &work).unwrap_err(),
            RemoteValidatorError::MultipleHeaderValues
        );
        assert_eq!(root.accounting_scalar_snapshot(), before_scalar);
        assert!(
            root.snapshot()
                .same_except_revision_and_reservation_sequence(&before)
        );
    }

    #[wasm_bindgen_test]
    fn chrome_headers_parser_to_if_match_is_byte_exact_and_fully_reclaims_accounting() {
        let wire = [b'"', 0x80, 0xFF, b'"'];
        let (root, work) = accounting_for_test();
        warm_accounting_for_valid_chain(&root, &work, &wire);
        let before = root.snapshot();
        let before_scalar = root.accounting_scalar_snapshot();
        TEST_EGRESS_SCRATCH_LIVE.with(|state| state.set(false));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| state.set(false));
        TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND.with(|count| count.set(0));

        let raw_value =
            read_raw_header_value_for_test("ETag", &make_js_byte_string_for_test(&wire)).unwrap();
        let parsed = parse_chrome_entity_tag(&[raw_value], &root, &work)
            .unwrap()
            .unwrap();
        let bound_consistency = bind_representation_consistency(
            RepresentationConsistencyPolicy::RequireStrongValidator,
            Some(parsed),
        )
        .unwrap();
        let RemoteObjectValidator::Strong(tag) = bound_consistency.validator() else {
            panic!("expected a strong bound entity-tag from Chrome Headers");
        };
        let tag = tag.clone();
        assert_eq!(tag.wire_bytes_for_test(), wire);
        let retained_scalar = root.accounting_scalar_snapshot();
        assert_eq!(
            retained_scalar.active_reservation_records,
            before_scalar.active_reservation_records + 1
        );
        assert_eq!(
            retained_scalar.prepared_reservation_records,
            before_scalar.prepared_reservation_records
        );

        let bound = tag
            .prepare_if_match_headers(&root, &work)
            .unwrap()
            .bind()
            .unwrap();
        TEST_EGRESS_SCRATCH_LIVE.with(|state| assert!(!state.get()));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(!state.get()));
        assert_eq!(bound.header_wire_for_test().unwrap(), wire);
        let bound_scalar = root.accounting_scalar_snapshot();
        assert_eq!(
            bound_scalar.active_reservation_records,
            before_scalar.active_reservation_records + 2
        );

        // This is the future consuming transfer's browser copy phase: the bound owner retains only
        // Headers (2n), and Request construction creates the other 2n copy under the active 4n
        // reservation. No standalone ByteString or Wasm scratch is live in this phase.
        let request = bound.copy_into_request_for_test().unwrap();
        TEST_EGRESS_SCRATCH_LIVE.with(|state| assert!(!state.get()));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(!state.get()));
        drop(request);

        drop(bound);
        TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND.with(|count| assert_eq!(count.get(), 1));
        assert_eq!(
            root.accounting_scalar_snapshot().active_reservation_records,
            before_scalar.active_reservation_records + 1
        );
        drop(tag);
        assert_eq!(
            root.accounting_scalar_snapshot().active_reservation_records,
            before_scalar.active_reservation_records + 1
        );
        drop(bound_consistency);
        assert_eq!(root.accounting_scalar_snapshot(), before_scalar);
        assert!(
            root.snapshot()
                .same_except_revision_and_reservation_sequence(&before)
        );
    }

    #[wasm_bindgen_test]
    fn chrome_truth_table_keeps_weak_owner_and_absent_has_no_partial_reservation() {
        let wire = b"W/\"weak\"";
        let (root, work) = accounting_for_test();
        warm_accounting_for_valid_chain(&root, &work, wire);
        work.validate_remote_validator_ingress_shape(0, 0).unwrap();
        let before = root.snapshot();
        let before_scalar = root.accounting_scalar_snapshot();

        let parsed =
            parse_chrome_entity_tag(&[make_js_byte_string_for_test(wire).into()], &root, &work)
                .unwrap();
        let weak_bound = bind_representation_consistency(
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            parsed,
        )
        .unwrap();
        assert!(matches!(
            weak_bound.validator(),
            RemoteObjectValidator::DeploymentAssumed {
                observed_weak: Some(_)
            }
        ));
        assert!(size_of_val(&weak_bound) <= retained_outer_shell_bytes());
        assert_eq!(
            root.accounting_scalar_snapshot().active_reservation_records,
            before_scalar.active_reservation_records + 1
        );
        drop(weak_bound);
        assert_eq!(root.accounting_scalar_snapshot(), before_scalar);

        let absent = parse_chrome_entity_tag(&[], &root, &work).unwrap();
        let absent_bound = bind_representation_consistency(
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            absent,
        )
        .unwrap();
        assert!(matches!(
            absent_bound.validator(),
            RemoteObjectValidator::DeploymentAssumed {
                observed_weak: None
            }
        ));
        assert!(size_of_val(&absent_bound) <= retained_outer_shell_bytes());
        assert_eq!(root.accounting_scalar_snapshot(), before_scalar);
        drop(absent_bound);
        assert!(
            root.snapshot()
                .same_except_revision_and_reservation_sequence(&before)
        );
    }

    #[wasm_bindgen_test]
    fn one_shot_headers_owner_bounds_count_and_preserves_obs_text() {
        TEST_HEADERS_INSTALLS_AFTER_SCRATCH_DROP.with(|count| count.set(0));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| state.set(false));
        TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND.with(|count| count.set(0));
        let (root, work) = accounting_for_test();
        let tag = obs_text_tag_for_test();

        let prepared = tag.prepare_if_match_headers(&root, &work).unwrap();
        drop(prepared);
        let bound = tag
            .prepare_if_match_headers(&root, &work)
            .unwrap()
            .bind()
            .unwrap();
        assert_eq!(
            bound.header_wire_for_test().unwrap(),
            [b'"', 0x80, 0xFF, b'"']
        );
        TEST_EGRESS_SCRATCH_LIVE.with(|state| assert!(!state.get()));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(!state.get()));
        TEST_HEADERS_INSTALLS_AFTER_SCRATCH_DROP.with(|count| assert_eq!(count.get(), 1));
        assert_eq!(
            tag.prepare_if_match_headers(&root, &work).unwrap_err(),
            RemoteValidatorError::ResourceLimit
        );
        drop(bound);
        TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND.with(|count| assert_eq!(count.get(), 1));
        drop(tag.prepare_if_match_headers(&root, &work).unwrap());
    }

    #[wasm_bindgen_test]
    fn failed_install_keeps_the_unique_permit_and_can_retry() {
        TEST_HEADERS_INSTALLS_AFTER_SCRATCH_DROP.with(|count| count.set(0));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| state.set(false));
        TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND.with(|count| count.set(0));
        let (root, work) = accounting_for_test();
        let tag = obs_text_tag_for_test();
        let retry = tag
            .prepare_if_match_headers(&root, &work)
            .unwrap()
            .arm_for_test()
            .unwrap();
        let headers = Object::new();
        let throwing_setter = Function::new_no_args("throw new Error('expected test failure')");
        Reflect::set(
            headers.as_ref(),
            &JsValue::from_str("set"),
            throwing_setter.as_ref(),
        )
        .unwrap();
        let failure = retry.bind_to_headers_for_test(headers.into()).unwrap_err();
        assert_eq!(failure.error(), RemoteValidatorError::HeaderRejected);
        TEST_EGRESS_SCRATCH_LIVE.with(|state| assert!(!state.get()));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(!state.get()));
        TEST_HEADERS_INSTALLS_AFTER_SCRATCH_DROP.with(|count| assert_eq!(count.get(), 1));
        assert_eq!(
            tag.prepare_if_match_headers(&root, &work).unwrap_err(),
            RemoteValidatorError::ResourceLimit
        );
        let bound = failure.retry().unwrap();
        TEST_HEADERS_INSTALLS_AFTER_SCRATCH_DROP.with(|count| assert_eq!(count.get(), 2));
        TEST_STANDALONE_BYTE_STRING_LIVE.with(|state| assert!(!state.get()));
        assert_eq!(
            bound.header_wire_for_test().unwrap(),
            [b'"', 0x80, 0xFF, b'"']
        );
        drop(bound);
        TEST_BOUND_HEADERS_DROPPED_BEFORE_REFUND.with(|count| assert_eq!(count.get(), 1));
        drop(tag.prepare_if_match_headers(&root, &work).unwrap());
    }
}
