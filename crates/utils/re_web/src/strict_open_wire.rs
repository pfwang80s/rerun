//! Versioned, bounded wire primitives for future strict Web open handoffs.
//!
//! This module defines codecs only.
//! It is not connected to public open, release, abort, lifecycle, or Viewer teardown paths.
//!
//! Raw shape assertions cannot be constructed outside this module:
//!
//! ```compile_fail
//! use re_web::strict_open_wire::StrictOpenWireErrorEnvelopeInputV1;
//! ```
//!
//! Arbitrary strings cannot be asserted to be redacted:
//!
//! ```compile_fail
//! use re_web::strict_open_wire::BoundedRedactedWireString;
//! let _message = BoundedRedactedWireString("https://secret.invalid/?token=secret".to_owned());
//! ```
//!
//! Recording labels likewise have no caller-provided string constructor:
//!
//! ```compile_fail
//! use re_web::strict_open_wire::BoundedRedactedRecordingLabelV1;
//! let _label = BoundedRedactedRecordingLabelV1("https://secret.invalid/?token=secret".to_owned());
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::remote_limits::{AtomicBatchAccountingScope, ScopeAccountingError};

pub const STRICT_OPEN_WIRE_VERSION_V1: u8 = 1;
pub const STRICT_OPEN_REDACTED_MESSAGE_MAX_BYTES_V1: usize = 192;

const STRICT_OPAQUE_ID_PREFIX_V1: &str = "rso1";
const STRICT_OPAQUE_ID_MAX_BYTES_V1: usize = 96;
const JAVASCRIPT_MAX_SAFE_INTEGER_U64: u64 = (1_u64 << 53) - 1;

/// The Viewer-instance namespace embedded into every strict opaque identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StrictOpenWireInstanceId(u128);

impl StrictOpenWireInstanceId {
    pub const fn new(nonce: u128) -> Self {
        Self(nonce)
    }
}

impl fmt::Debug for StrictOpenWireInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StrictOpenWireInstanceId(<opaque>)")
    }
}

macro_rules! define_internal_identity {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(u128);

        impl $name {
            pub const fn new(nonce: u128) -> Self {
                Self(nonce)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(<opaque>)"))
            }
        }
    };
}

define_internal_identity!(StrictOpenHandoffIdentity);
define_internal_identity!(OpenOperationIdentity);
define_internal_identity!(StrictOpenInstallationIdentity);
define_internal_identity!(PublicOpenRequestIdentity);
define_internal_identity!(PublicRecordingIdentity);

macro_rules! define_wire_identity {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn wire_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(<opaque>)"))
            }
        }
    };
}

define_wire_identity!(StrictOpenHandoffWireId);
define_wire_identity!(OpenOperationWireId);
define_wire_identity!(StrictOpenInstallationWireId);
define_wire_identity!(PublicOpenRequestWireId);
define_wire_identity!(PublicRecordingWireId);

/// One canonical unsigned decimal string.
#[derive(Clone, PartialEq, Eq)]
pub struct CanonicalU64DecimalWire(String);

impl CanonicalU64DecimalWire {
    pub fn wire_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CanonicalU64DecimalWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CanonicalU64DecimalWire")
            .field(&self.0)
            .finish()
    }
}

/// A codec-created fixed message with a V1 byte bound.
#[derive(Clone, PartialEq, Eq)]
pub struct BoundedRedactedWireString(String);

impl BoundedRedactedWireString {
    pub fn wire_str(&self) -> &str {
        &self.0
    }

    fn from_static(message: &'static str) -> Self {
        Self(message.to_owned())
    }
}

impl fmt::Debug for BoundedRedactedWireString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoundedRedactedWireString(<redacted>)")
    }
}

/// A fixed, bounded recording label that cannot carry source-provided secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct BoundedRedactedRecordingLabelV1(BoundedRedactedWireString);

impl BoundedRedactedRecordingLabelV1 {
    pub fn generic() -> Self {
        Self(BoundedRedactedWireString::from_static("Recording"))
    }

    pub fn wire_str(&self) -> &str {
        self.0.wire_str()
    }
}

impl fmt::Debug for BoundedRedactedRecordingLabelV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoundedRedactedRecordingLabelV1(<redacted>)")
    }
}

/// Primitive input types observed at the `JavaScript` wire boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrictWirePrimitive<'a> {
    String(&'a str),
    JavaScriptNumber(f64),
    Boolean(bool),
    Null,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecimalDecodeError {
    WrongWireType,
    Empty,
    Sign,
    LeadingZero,
    NonAscii,
    NonDigit,
    Overflow,
}

fn encode_canonical_u64(value: u64) -> CanonicalU64DecimalWire {
    CanonicalU64DecimalWire(value.to_string())
}

fn decode_canonical_u64(input: StrictWirePrimitive<'_>) -> Result<u64, DecimalDecodeError> {
    let StrictWirePrimitive::String(decimal) = input else {
        return Err(DecimalDecodeError::WrongWireType);
    };
    if decimal.is_empty() {
        return Err(DecimalDecodeError::Empty);
    }
    if matches!(decimal.as_bytes().first(), Some(b'-' | b'+')) {
        return Err(DecimalDecodeError::Sign);
    }
    if !decimal.is_ascii() {
        return Err(DecimalDecodeError::NonAscii);
    }
    if decimal.len() > 1 && decimal.starts_with('0') {
        return Err(DecimalDecodeError::LeadingZero);
    }

    let mut value = 0_u64;
    for byte in decimal.bytes() {
        if !byte.is_ascii_digit() {
            return Err(DecimalDecodeError::NonDigit);
        }
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .ok_or(DecimalDecodeError::Overflow)?;
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpaqueIdentityDecodeError {
    Malformed,
    WrongDomain,
    WrongInstance,
}

fn encode_opaque_identity(
    instance: StrictOpenWireInstanceId,
    domain: &'static str,
    nonce: u128,
) -> String {
    format!(
        "{STRICT_OPAQUE_ID_PREFIX_V1}:{domain}:{:032x}:{nonce:032x}",
        instance.0
    )
}

fn decode_opaque_identity(
    input: StrictWirePrimitive<'_>,
    expected_instance: StrictOpenWireInstanceId,
    expected_domain: &'static str,
) -> Result<u128, OpaqueIdentityDecodeError> {
    let StrictWirePrimitive::String(value) = input else {
        return Err(OpaqueIdentityDecodeError::Malformed);
    };
    if value.len() > STRICT_OPAQUE_ID_MAX_BYTES_V1 || !value.is_ascii() {
        return Err(OpaqueIdentityDecodeError::Malformed);
    }
    let mut parts = value.split(':');
    let (Some(prefix), Some(domain), Some(instance), Some(nonce), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(OpaqueIdentityDecodeError::Malformed);
    };
    if prefix != STRICT_OPAQUE_ID_PREFIX_V1
        || !is_canonical_lower_hex_u128(instance)
        || !is_canonical_lower_hex_u128(nonce)
    {
        return Err(OpaqueIdentityDecodeError::Malformed);
    }
    if domain != expected_domain {
        return Err(OpaqueIdentityDecodeError::WrongDomain);
    }
    let instance = u128::from_str_radix(instance, 16)
        .map_err(|_error| OpaqueIdentityDecodeError::Malformed)?;
    if instance != expected_instance.0 {
        return Err(OpaqueIdentityDecodeError::WrongInstance);
    }
    u128::from_str_radix(nonce, 16).map_err(|_error| OpaqueIdentityDecodeError::Malformed)
}

fn is_canonical_lower_hex_u128(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

macro_rules! define_wire_code {
    ($name:ident { $($variant:ident => ($wire:literal, $message:literal)),+ $(,)? }) => {
        $(const _: () = assert!($message.len() <= STRICT_OPEN_REDACTED_MESSAGE_MAX_BYTES_V1);)+

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const fn wire_code(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }

            const fn message(self) -> &'static str {
                match self {
                    $(Self::$variant => $message),+
                }
            }

            fn parse(wire: &str) -> Option<Self> {
                match wire {
                    $($wire => Some(Self::$variant)),+,
                    _ => None,
                }
            }
        }
    };
}

define_wire_code!(StrictOpenAdmissionCodeV1 {
    InvalidRequestShape => ("invalid_request_shape", "strict open request shape is invalid"),
    InvalidUrl => ("invalid_url", "strict open URL is invalid"),
    InvalidRemoteMcapOptions => ("invalid_remote_mcap_options", "remote MCAP options are invalid"),
    UnsupportedStrictOpenRoute => ("unsupported_strict_open_route", "strict open route is unsupported"),
    BatchTooLarge => ("batch_too_large", "strict open batch exceeds its item limit"),
    ResourceLimitExceeded => ("resource_limit_exceeded", "strict open resource limit was exceeded"),
    RemoteSessionLimitReached => ("remote_session_limit_reached", "remote MCAP session limit was reached"),
    ExistingSourceOptionsConflict => ("existing_source_options_conflict", "existing source options conflict"),
    ConflictingStartupSources => ("conflicting_startup_sources", "startup sources conflict"),
    BrowserIngressDraining => ("browser_ingress_draining", "browser ingress is draining"),
    BrowserExecutionSuspended => ("browser_execution_suspended", "browser execution is suspended"),
});

define_wire_code!(StrictOpenCancelledCodeV1 {
    ViewerStopped => ("viewer_stopped", "Viewer stopped before strict open completed"),
    HandoffCancelled => ("handoff_cancelled", "strict open handoff was cancelled"),
});

define_wire_code!(StrictOpenStateChangedCodeV1 {
    HandoffStateChanged => ("handoff_state_changed", "strict open handoff state changed"),
});

define_wire_code!(StrictOpenProtocolCodeV1 {
    WrongOrStaleHandoffToken => ("wrong_or_stale_handoff_token", "strict open handoff token is wrong or stale"),
    WrongIdentityDomain => ("wrong_identity_domain", "strict open identity domain or instance is wrong"),
    NonCanonicalDecimal => ("non_canonical_decimal", "strict open decimal is not canonical"),
    InstallationAckMismatch => ("installation_ack_mismatch", "strict open installation acknowledgement does not match"),
    DuplicateOrMissingAck => ("duplicate_or_missing_ack", "strict open installation acknowledgement is duplicate or missing"),
});

define_wire_code!(StrictOpenFatalAbiCodeV1 {
    UnknownWireVersionOrShape => ("unknown_wire_version_or_shape", "strict open wire version or shape is unknown"),
    WasmPanic => ("wasm_panic", "strict open Wasm execution failed"),
    CodecInvariantBroken => ("codec_invariant_broken", "strict open codec invariant was broken"),
    CleanupCouldNotBeProved => ("cleanup_could_not_be_proved", "strict open cleanup could not be proved"),
    DispatcherInvariantBroken => ("dispatcher_invariant_broken", "strict open dispatcher invariant was broken"),
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenAdmissionErrorV1 {
    pub code: StrictOpenAdmissionCodeV1,
    pub failed_index: Option<u32>,
    pub message: BoundedRedactedWireString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenCancelledErrorV1 {
    pub code: StrictOpenCancelledCodeV1,
    pub message: BoundedRedactedWireString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenStateChangedErrorV1 {
    pub code: StrictOpenStateChangedCodeV1,
    pub message: BoundedRedactedWireString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenProtocolErrorV1 {
    pub code: StrictOpenProtocolCodeV1,
    pub message: BoundedRedactedWireString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenFatalAbiErrorV1 {
    pub code: StrictOpenFatalAbiCodeV1,
    pub message: BoundedRedactedWireString,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StrictOpenWireErrorV1 {
    Admission(StrictOpenAdmissionErrorV1),
    Cancelled(StrictOpenCancelledErrorV1),
    HandoffStateChanged(StrictOpenStateChangedErrorV1),
    ProtocolViolation(StrictOpenProtocolErrorV1),
    FatalAbi(StrictOpenFatalAbiErrorV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrictOpenViewerPolicyV1 {
    KeepRunning,
    TeardownViewer,
}

impl StrictOpenWireErrorV1 {
    pub fn admission(code: StrictOpenAdmissionCodeV1, failed_index: Option<u32>) -> Self {
        Self::Admission(StrictOpenAdmissionErrorV1 {
            code,
            failed_index,
            message: BoundedRedactedWireString::from_static(code.message()),
        })
    }

    pub fn cancelled(code: StrictOpenCancelledCodeV1) -> Self {
        Self::Cancelled(StrictOpenCancelledErrorV1 {
            code,
            message: BoundedRedactedWireString::from_static(code.message()),
        })
    }

    pub fn handoff_state_changed() -> Self {
        let code = StrictOpenStateChangedCodeV1::HandoffStateChanged;
        Self::HandoffStateChanged(StrictOpenStateChangedErrorV1 {
            code,
            message: BoundedRedactedWireString::from_static(code.message()),
        })
    }

    pub fn protocol_violation(code: StrictOpenProtocolCodeV1) -> Self {
        protocol_error(code)
    }

    pub fn fatal_abi(code: StrictOpenFatalAbiCodeV1) -> Self {
        fatal_error(code)
    }

    pub fn viewer_policy(&self) -> StrictOpenViewerPolicyV1 {
        match self {
            Self::Admission(_)
            | Self::Cancelled(_)
            | Self::HandoffStateChanged(_)
            | Self::ProtocolViolation(_) => StrictOpenViewerPolicyV1::KeepRunning,
            Self::FatalAbi(_) => StrictOpenViewerPolicyV1::TeardownViewer,
        }
    }
}

fn protocol_error(code: StrictOpenProtocolCodeV1) -> StrictOpenWireErrorV1 {
    StrictOpenWireErrorV1::ProtocolViolation(StrictOpenProtocolErrorV1 {
        code,
        message: BoundedRedactedWireString::from_static(code.message()),
    })
}

fn fatal_error(code: StrictOpenFatalAbiCodeV1) -> StrictOpenWireErrorV1 {
    StrictOpenWireErrorV1::FatalAbi(StrictOpenFatalAbiErrorV1 {
        code,
        message: BoundedRedactedWireString::from_static(code.message()),
    })
}

fn code_and_message(error: &StrictOpenWireErrorV1) -> (&'static str, &'static str, &'static str) {
    match error {
        StrictOpenWireErrorV1::Admission(error) => {
            ("admission", error.code.wire_code(), error.code.message())
        }
        StrictOpenWireErrorV1::Cancelled(error) => {
            ("cancelled", error.code.wire_code(), error.code.message())
        }
        StrictOpenWireErrorV1::HandoffStateChanged(error) => (
            "handoff_state_changed",
            error.code.wire_code(),
            error.code.message(),
        ),
        StrictOpenWireErrorV1::ProtocolViolation(error) => (
            "protocol_violation",
            error.code.wire_code(),
            error.code.message(),
        ),
        StrictOpenWireErrorV1::FatalAbi(error) => {
            ("fatal_abi", error.code.wire_code(), error.code.message())
        }
    }
}

/// The serialized V1 error envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenWireErrorEnvelopeV1 {
    pub version: u8,
    pub class: &'static str,
    pub code: &'static str,
    pub failed_index_decimal: Option<CanonicalU64DecimalWire>,
    pub retryable: Option<bool>,
    pub message: BoundedRedactedWireString,
}

/// Borrowed, shape-validated primitive inputs for decoding one V1 error envelope.
#[derive(Clone, Copy, Debug, PartialEq)]
enum StrictOpenWireErrorEnvelopeInputV1<'a> {
    Exact {
        version: StrictWirePrimitive<'a>,
        class: StrictWirePrimitive<'a>,
        code: StrictWirePrimitive<'a>,
        failed_index_decimal: Option<StrictWirePrimitive<'a>>,
        retryable: Option<StrictWirePrimitive<'a>>,
        message: StrictWirePrimitive<'a>,
    },
    UnknownShape,
}

/// Internal recording metadata accepted by the foundational success codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicRecordingDescriptorV1 {
    pub request_id: PublicOpenRequestIdentity,
    pub recording_handle_id: PublicRecordingIdentity,
    pub display_name: BoundedRedactedRecordingLabelV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedRecordingAttachmentV1 {
    pub public_recording_id: PublicRecordingIdentity,
    pub lifecycle_revision: u64,
    pub descriptor: PublicRecordingDescriptorV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedOpenDescriptorV1 {
    pub operation_token: OpenOperationIdentity,
    pub public_request_id: PublicOpenRequestIdentity,
    pub installation_nonce: StrictOpenInstallationIdentity,
    pub preexisting_recordings: Vec<CommittedRecordingAttachmentV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenSuccessV1 {
    pub handoff_token: StrictOpenHandoffIdentity,
    pub descriptors: Vec<CommittedOpenDescriptorV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicRecordingDescriptorWireV1 {
    pub request_id: PublicOpenRequestWireId,
    pub recording_handle_id: PublicRecordingWireId,
    pub display_name: BoundedRedactedRecordingLabelV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictRecordingAttachmentWireV1 {
    pub public_recording_id: PublicRecordingWireId,
    pub lifecycle_revision_decimal: CanonicalU64DecimalWire,
    pub descriptor: PublicRecordingDescriptorWireV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenDescriptorWireV1 {
    pub operation_token: OpenOperationWireId,
    pub public_request_id: PublicOpenRequestWireId,
    pub installation_nonce: StrictOpenInstallationWireId,
    pub preexisting_recordings: Vec<StrictRecordingAttachmentWireV1>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenWireV1 {
    pub version: u8,
    pub handoff_token: StrictOpenHandoffWireId,
    pub descriptors: Vec<StrictOpenDescriptorWireV1>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StrictRecordingActivationAckInputV1<'a> {
    Exact {
        public_recording_id: StrictWirePrimitive<'a>,
        lifecycle_revision_decimal: StrictWirePrimitive<'a>,
    },
    UnknownShape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedRecordingActivationAckV1 {
    pub public_recording_id: PublicRecordingIdentity,
    pub lifecycle_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenInstallationAckExpectationV1 {
    pub operation_token: OpenOperationIdentity,
    pub installation_nonce: StrictOpenInstallationIdentity,
    pub recordings: Vec<ExpectedRecordingActivationAckV1>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StrictOpenInstallationAckEnvelopeInputV1<'a> {
    Exact {
        version: StrictWirePrimitive<'a>,
        operation_token: StrictWirePrimitive<'a>,
        installation_nonce: StrictWirePrimitive<'a>,
        recordings: &'a [StrictRecordingActivationAckInputV1<'a>],
    },
    UnknownShape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingActivationDeliveryAckV1 {
    pub instance: StrictOpenWireInstanceId,
    pub operation_token: OpenOperationIdentity,
    pub public_recording_id: PublicRecordingIdentity,
    pub lifecycle_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrictOpenJsInstallationAckV1 {
    pub operation_token: OpenOperationIdentity,
    pub installation_nonce: StrictOpenInstallationIdentity,
    pub recordings: Vec<RecordingActivationDeliveryAckV1>,
}

/// The sole V1 identity, decimal, success, error, and acknowledgement codec.
pub struct StrictOpenWireCodecV1 {
    instance: StrictOpenWireInstanceId,
    max_batch_urls: u64,
    max_preexisting_recording_attachments: u64,
    max_installation_acks: u64,
}

impl fmt::Debug for StrictOpenWireCodecV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StrictOpenWireCodecV1")
            .field("instance", &"<opaque>")
            .field("max_batch_urls", &self.max_batch_urls)
            .field(
                "max_preexisting_recording_attachments",
                &self.max_preexisting_recording_attachments,
            )
            .field("max_installation_acks", &self.max_installation_acks)
            .finish()
    }
}

impl StrictOpenWireCodecV1 {
    pub fn for_batch(
        instance: StrictOpenWireInstanceId,
        batch: &AtomicBatchAccountingScope,
    ) -> Result<Self, StrictOpenWireErrorV1> {
        let limits = batch
            .strict_open_wire_batch_limits()
            .map_err(map_accounting_error_to_wire_error)?;
        let max_batch_urls = limits.open_batch_urls.get();
        if max_batch_urls > u64::from(u32::MAX) + 1
            || max_batch_urls - 1 > JAVASCRIPT_MAX_SAFE_INTEGER_U64
        {
            return Err(fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken));
        }
        Ok(Self {
            instance,
            max_batch_urls,
            max_preexisting_recording_attachments: limits.preexisting_recording_attachments.get(),
            max_installation_acks: limits.installation_acks.get(),
        })
    }

    pub fn encode_success(
        &self,
        success: StrictOpenSuccessV1,
    ) -> Result<StrictOpenWireV1, StrictOpenWireErrorV1> {
        let descriptor_count = u64::try_from(success.descriptors.len())
            .map_err(|_overflow| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))?;
        if descriptor_count > self.max_batch_urls || descriptor_count > self.max_installation_acks {
            return Err(fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken));
        }

        let mut total_attachments = 0_u64;
        let mut operation_tokens = BTreeSet::new();
        let mut public_request_ids = BTreeSet::new();
        let mut installation_nonces = BTreeSet::new();
        let mut shared_recording_snapshots = BTreeMap::new();
        for descriptor in &success.descriptors {
            let recording_count = u64::try_from(descriptor.preexisting_recordings.len())
                .map_err(|_overflow| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))?;
            total_attachments = total_attachments
                .checked_add(recording_count)
                .ok_or_else(|| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))?;
            if total_attachments > self.max_preexisting_recording_attachments
                || !operation_tokens.insert(descriptor.operation_token)
                || !public_request_ids.insert(descriptor.public_request_id)
                || !installation_nonces.insert(descriptor.installation_nonce)
            {
                return Err(fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken));
            }

            let mut descriptor_recordings = BTreeSet::new();
            for recording in &descriptor.preexisting_recordings {
                if !descriptor_recordings.insert(recording.public_recording_id)
                    || recording.public_recording_id != recording.descriptor.recording_handle_id
                    || descriptor.public_request_id != recording.descriptor.request_id
                {
                    return Err(fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken));
                }
                let snapshot = (
                    recording.lifecycle_revision,
                    recording.descriptor.display_name.clone(),
                );
                if shared_recording_snapshots
                    .insert(recording.public_recording_id, snapshot.clone())
                    .is_some_and(|existing| existing != snapshot)
                {
                    return Err(fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken));
                }
            }
        }

        let mut descriptors = Vec::with_capacity(success.descriptors.len());
        for descriptor in success.descriptors {
            let mut recordings = Vec::with_capacity(descriptor.preexisting_recordings.len());
            for recording in descriptor.preexisting_recordings {
                recordings.push(StrictRecordingAttachmentWireV1 {
                    public_recording_id: self
                        .encode_public_recording(recording.public_recording_id),
                    lifecycle_revision_decimal: encode_canonical_u64(recording.lifecycle_revision),
                    descriptor: PublicRecordingDescriptorWireV1 {
                        request_id: self.encode_public_request(recording.descriptor.request_id),
                        recording_handle_id: self
                            .encode_public_recording(recording.descriptor.recording_handle_id),
                        display_name: recording.descriptor.display_name,
                    },
                });
            }
            descriptors.push(StrictOpenDescriptorWireV1 {
                operation_token: self.encode_operation(descriptor.operation_token),
                public_request_id: self.encode_public_request(descriptor.public_request_id),
                installation_nonce: self.encode_installation(descriptor.installation_nonce),
                preexisting_recordings: recordings,
            });
        }
        Ok(StrictOpenWireV1 {
            version: STRICT_OPEN_WIRE_VERSION_V1,
            handoff_token: self.encode_handoff(success.handoff_token),
            descriptors,
        })
    }

    pub fn encode_error(&self, error: StrictOpenWireErrorV1) -> StrictOpenWireErrorEnvelopeV1 {
        let (_, _, expected_message) = code_and_message(&error);
        let supplied_message = match &error {
            StrictOpenWireErrorV1::Admission(error) => &error.message,
            StrictOpenWireErrorV1::Cancelled(error) => &error.message,
            StrictOpenWireErrorV1::HandoffStateChanged(error) => &error.message,
            StrictOpenWireErrorV1::ProtocolViolation(error) => &error.message,
            StrictOpenWireErrorV1::FatalAbi(error) => &error.message,
        };
        let failed_index_in_range = match &error {
            StrictOpenWireErrorV1::Admission(error) => error
                .failed_index
                .is_none_or(|index| u64::from(index) < self.max_batch_urls),
            StrictOpenWireErrorV1::Cancelled(_)
            | StrictOpenWireErrorV1::HandoffStateChanged(_)
            | StrictOpenWireErrorV1::ProtocolViolation(_)
            | StrictOpenWireErrorV1::FatalAbi(_) => true,
        };
        let error = if supplied_message.wire_str() == expected_message && failed_index_in_range {
            error
        } else {
            fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken)
        };
        let (class, code, _) = code_and_message(&error);
        let (failed_index_decimal, retryable, message) = match error {
            StrictOpenWireErrorV1::Admission(error) => (
                error.failed_index.map(u64::from).map(encode_canonical_u64),
                None,
                error.message,
            ),
            StrictOpenWireErrorV1::Cancelled(error) => (None, None, error.message),
            StrictOpenWireErrorV1::HandoffStateChanged(error) => (None, Some(true), error.message),
            StrictOpenWireErrorV1::ProtocolViolation(error) => (None, None, error.message),
            StrictOpenWireErrorV1::FatalAbi(error) => (None, None, error.message),
        };
        StrictOpenWireErrorEnvelopeV1 {
            version: STRICT_OPEN_WIRE_VERSION_V1,
            class,
            code,
            failed_index_decimal,
            retryable,
            message,
        }
    }

    fn decode_error(&self, input: StrictOpenWireErrorEnvelopeInputV1<'_>) -> StrictOpenWireErrorV1 {
        let StrictOpenWireErrorEnvelopeInputV1::Exact {
            version,
            class,
            code,
            failed_index_decimal,
            retryable,
            message,
        } = input
        else {
            return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
        };
        if !is_v1_version(version) {
            return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
        }
        let (
            StrictWirePrimitive::String(class),
            StrictWirePrimitive::String(code),
            StrictWirePrimitive::String(message),
        ) = (class, code, message)
        else {
            return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
        };
        if message.len() > STRICT_OPEN_REDACTED_MESSAGE_MAX_BYTES_V1 {
            return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
        }

        let decoded = match class {
            "admission" => {
                let Some(code) = StrictOpenAdmissionCodeV1::parse(code) else {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                };
                if retryable.is_some() {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                }
                let failed_index = match failed_index_decimal {
                    Some(decimal) => match self.decode_failed_index(decimal) {
                        Ok(index) => Some(index),
                        Err(error) => return error,
                    },
                    None => None,
                };
                StrictOpenWireErrorV1::Admission(StrictOpenAdmissionErrorV1 {
                    code,
                    failed_index,
                    message: BoundedRedactedWireString::from_static(code.message()),
                })
            }
            "cancelled" => {
                let Some(code) = StrictOpenCancelledCodeV1::parse(code) else {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                };
                if failed_index_decimal.is_some() || retryable.is_some() {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                }
                StrictOpenWireErrorV1::Cancelled(StrictOpenCancelledErrorV1 {
                    code,
                    message: BoundedRedactedWireString::from_static(code.message()),
                })
            }
            "handoff_state_changed" => {
                let Some(code) = StrictOpenStateChangedCodeV1::parse(code) else {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                };
                let Some(StrictWirePrimitive::Boolean(true)) = retryable else {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                };
                if failed_index_decimal.is_some() {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                }
                StrictOpenWireErrorV1::HandoffStateChanged(StrictOpenStateChangedErrorV1 {
                    code,
                    message: BoundedRedactedWireString::from_static(code.message()),
                })
            }
            "protocol_violation" => {
                let Some(code) = StrictOpenProtocolCodeV1::parse(code) else {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                };
                if failed_index_decimal.is_some() || retryable.is_some() {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                }
                protocol_error(code)
            }
            "fatal_abi" => {
                let Some(code) = StrictOpenFatalAbiCodeV1::parse(code) else {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                };
                if failed_index_decimal.is_some() || retryable.is_some() {
                    return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
                }
                fatal_error(code)
            }
            _ => return fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape),
        };

        let (_, _, expected_message) = code_and_message(&decoded);
        if message != expected_message {
            return fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken);
        }
        decoded
    }

    fn decode_installation_acks(
        &self,
        inputs: &[StrictOpenInstallationAckEnvelopeInputV1<'_>],
        expected: &[StrictOpenInstallationAckExpectationV1],
    ) -> Result<Vec<StrictOpenJsInstallationAckV1>, StrictOpenWireErrorV1> {
        let input_count = u64::try_from(inputs.len())
            .map_err(|_overflow| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))?;
        let expected_count = u64::try_from(expected.len())
            .map_err(|_overflow| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))?;
        if expected_count > self.max_batch_urls || expected_count > self.max_installation_acks {
            return Err(fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken));
        }
        if input_count != expected_count
            || input_count > self.max_batch_urls
            || input_count > self.max_installation_acks
        {
            return Err(protocol_error(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        }

        let mut expected_total = 0_u64;
        let mut input_total = 0_u64;
        for (input, expected) in inputs.iter().zip(expected) {
            let StrictOpenInstallationAckEnvelopeInputV1::Exact { recordings, .. } = input else {
                return Err(fatal_error(
                    StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape,
                ));
            };
            expected_total = expected_total
                .checked_add(
                    u64::try_from(expected.recordings.len()).map_err(|_overflow| {
                        fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken)
                    })?,
                )
                .ok_or_else(|| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))?;
            input_total = input_total
                .checked_add(u64::try_from(recordings.len()).map_err(|_overflow| {
                    fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken)
                })?)
                .ok_or_else(|| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))?;
        }
        if expected_total > self.max_preexisting_recording_attachments {
            return Err(fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken));
        }
        if input_total != expected_total || input_total > self.max_preexisting_recording_attachments
        {
            return Err(protocol_error(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        }

        let mut decoded = Vec::with_capacity(inputs.len());
        for (input, expected) in inputs.iter().copied().zip(expected) {
            decoded.push(self.decode_one_installation_ack(input, expected)?);
        }
        Ok(decoded)
    }

    fn decode_one_installation_ack(
        &self,
        input: StrictOpenInstallationAckEnvelopeInputV1<'_>,
        expected: &StrictOpenInstallationAckExpectationV1,
    ) -> Result<StrictOpenJsInstallationAckV1, StrictOpenWireErrorV1> {
        let StrictOpenInstallationAckEnvelopeInputV1::Exact {
            version,
            operation_token,
            installation_nonce,
            recordings: input_recordings,
        } = input
        else {
            return Err(fatal_error(
                StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape,
            ));
        };
        if !is_v1_version(version) {
            return Err(fatal_error(
                StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape,
            ));
        }
        if input_recordings.len() != expected.recordings.len() {
            return Err(protocol_error(
                StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
            ));
        }

        let operation_token = self
            .decode_operation(operation_token)
            .map_err(map_non_handoff_identity_error)?;
        let installation_nonce = self
            .decode_installation(installation_nonce)
            .map_err(map_non_handoff_identity_error)?;
        if operation_token != expected.operation_token
            || installation_nonce != expected.installation_nonce
        {
            return Err(protocol_error(
                StrictOpenProtocolCodeV1::InstallationAckMismatch,
            ));
        }

        let mut seen = BTreeSet::new();
        let mut recordings = Vec::with_capacity(input_recordings.len());
        for (input, expected_recording) in input_recordings.iter().zip(&expected.recordings) {
            let StrictRecordingActivationAckInputV1::Exact {
                public_recording_id,
                lifecycle_revision_decimal,
            } = input
            else {
                return Err(fatal_error(
                    StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape,
                ));
            };
            let public_recording_id = self
                .decode_public_recording(*public_recording_id)
                .map_err(map_non_handoff_identity_error)?;
            if !seen.insert(public_recording_id) {
                return Err(protocol_error(
                    StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
                ));
            }
            let lifecycle_revision = decode_canonical_u64(*lifecycle_revision_decimal)
                .map_err(|_error| protocol_error(StrictOpenProtocolCodeV1::NonCanonicalDecimal))?;
            if public_recording_id != expected_recording.public_recording_id
                || lifecycle_revision != expected_recording.lifecycle_revision
            {
                return Err(protocol_error(
                    StrictOpenProtocolCodeV1::InstallationAckMismatch,
                ));
            }
            recordings.push(RecordingActivationDeliveryAckV1 {
                instance: self.instance,
                operation_token,
                public_recording_id,
                lifecycle_revision,
            });
        }
        Ok(StrictOpenJsInstallationAckV1 {
            operation_token,
            installation_nonce,
            recordings,
        })
    }

    #[cfg(test)]
    fn decode_installation_ack(
        &self,
        input: StrictOpenInstallationAckEnvelopeInputV1<'_>,
        expected: &StrictOpenInstallationAckExpectationV1,
    ) -> Result<StrictOpenJsInstallationAckV1, StrictOpenWireErrorV1> {
        let mut decoded =
            self.decode_installation_acks(&[input], std::slice::from_ref(expected))?;
        decoded
            .pop()
            .ok_or_else(|| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))
    }

    pub fn decode_handoff_token(
        &self,
        input: StrictWirePrimitive<'_>,
    ) -> Result<StrictOpenHandoffIdentity, StrictOpenWireErrorV1> {
        self.decode_handoff(input).map_err(|error| match error {
            OpaqueIdentityDecodeError::WrongDomain => {
                protocol_error(StrictOpenProtocolCodeV1::WrongIdentityDomain)
            }
            OpaqueIdentityDecodeError::Malformed | OpaqueIdentityDecodeError::WrongInstance => {
                protocol_error(StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken)
            }
        })
    }

    fn decode_failed_index(
        &self,
        input: StrictWirePrimitive<'_>,
    ) -> Result<u32, StrictOpenWireErrorV1> {
        let index = decode_canonical_u64(input)
            .map_err(|_error| fatal_error(StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape))?;
        if index >= self.max_batch_urls || index > JAVASCRIPT_MAX_SAFE_INTEGER_U64 {
            return Err(fatal_error(
                StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape,
            ));
        }
        u32::try_from(index)
            .map_err(|_error| fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken))
    }

    fn encode_handoff(&self, identity: StrictOpenHandoffIdentity) -> StrictOpenHandoffWireId {
        StrictOpenHandoffWireId(encode_opaque_identity(self.instance, "handoff", identity.0))
    }

    fn encode_operation(&self, identity: OpenOperationIdentity) -> OpenOperationWireId {
        OpenOperationWireId(encode_opaque_identity(
            self.instance,
            "operation",
            identity.0,
        ))
    }

    fn encode_installation(
        &self,
        identity: StrictOpenInstallationIdentity,
    ) -> StrictOpenInstallationWireId {
        StrictOpenInstallationWireId(encode_opaque_identity(
            self.instance,
            "installation",
            identity.0,
        ))
    }

    fn encode_public_request(
        &self,
        identity: PublicOpenRequestIdentity,
    ) -> PublicOpenRequestWireId {
        PublicOpenRequestWireId(encode_opaque_identity(
            self.instance,
            "public_request",
            identity.0,
        ))
    }

    fn encode_public_recording(&self, identity: PublicRecordingIdentity) -> PublicRecordingWireId {
        PublicRecordingWireId(encode_opaque_identity(
            self.instance,
            "public_recording",
            identity.0,
        ))
    }

    fn decode_handoff(
        &self,
        input: StrictWirePrimitive<'_>,
    ) -> Result<StrictOpenHandoffIdentity, OpaqueIdentityDecodeError> {
        decode_opaque_identity(input, self.instance, "handoff").map(StrictOpenHandoffIdentity)
    }

    fn decode_operation(
        &self,
        input: StrictWirePrimitive<'_>,
    ) -> Result<OpenOperationIdentity, OpaqueIdentityDecodeError> {
        decode_opaque_identity(input, self.instance, "operation").map(OpenOperationIdentity)
    }

    fn decode_installation(
        &self,
        input: StrictWirePrimitive<'_>,
    ) -> Result<StrictOpenInstallationIdentity, OpaqueIdentityDecodeError> {
        decode_opaque_identity(input, self.instance, "installation")
            .map(StrictOpenInstallationIdentity)
    }

    fn decode_public_recording(
        &self,
        input: StrictWirePrimitive<'_>,
    ) -> Result<PublicRecordingIdentity, OpaqueIdentityDecodeError> {
        decode_opaque_identity(input, self.instance, "public_recording")
            .map(PublicRecordingIdentity)
    }

    #[cfg(test)]
    fn decode_public_request(
        &self,
        input: StrictWirePrimitive<'_>,
    ) -> Result<PublicOpenRequestIdentity, OpaqueIdentityDecodeError> {
        decode_opaque_identity(input, self.instance, "public_request")
            .map(PublicOpenRequestIdentity)
    }
}

fn is_v1_version(input: StrictWirePrimitive<'_>) -> bool {
    matches!(input, StrictWirePrimitive::JavaScriptNumber(version) if version == f64::from(STRICT_OPEN_WIRE_VERSION_V1))
}

fn map_non_handoff_identity_error(error: OpaqueIdentityDecodeError) -> StrictOpenWireErrorV1 {
    match error {
        OpaqueIdentityDecodeError::Malformed
        | OpaqueIdentityDecodeError::WrongDomain
        | OpaqueIdentityDecodeError::WrongInstance => {
            protocol_error(StrictOpenProtocolCodeV1::WrongIdentityDomain)
        }
    }
}

fn map_accounting_error_to_wire_error(error: ScopeAccountingError) -> StrictOpenWireErrorV1 {
    match error {
        ScopeAccountingError::RootStopped => {
            StrictOpenWireErrorV1::cancelled(StrictOpenCancelledCodeV1::ViewerStopped)
        }
        _ => fatal_error(StrictOpenFatalAbiCodeV1::CodecInvariantBroken),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_limits::{WebRemoteLimitKey, tests::test_profile_with};

    const INSTANCE: StrictOpenWireInstanceId = StrictOpenWireInstanceId::new(0x1234);

    fn codec_with_limits(
        max_batch_urls: u64,
        max_attachments: u64,
        max_installation_acks: u64,
    ) -> StrictOpenWireCodecV1 {
        let root = test_profile_with(&[
            (WebRemoteLimitKey::OpenBatchUrls, max_batch_urls),
            (
                WebRemoteLimitKey::PreexistingRecordingAttachments,
                max_attachments,
            ),
            (WebRemoteLimitKey::InstallationAcks, max_installation_acks),
        ])
        .start_accounting_root()
        .unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let batch = viewer.create_atomic_batch_scope().unwrap();
        StrictOpenWireCodecV1::for_batch(INSTANCE, &batch).unwrap()
    }

    fn codec_with_batch_limit(max_batch_urls: u64) -> StrictOpenWireCodecV1 {
        codec_with_limits(max_batch_urls, 8, 8)
    }

    fn codec() -> StrictOpenWireCodecV1 {
        codec_with_batch_limit(4)
    }

    fn version() -> StrictWirePrimitive<'static> {
        StrictWirePrimitive::JavaScriptNumber(f64::from(STRICT_OPEN_WIRE_VERSION_V1))
    }

    fn exact_error_input<'a>(
        version: StrictWirePrimitive<'a>,
        class: StrictWirePrimitive<'a>,
        code: StrictWirePrimitive<'a>,
        failed_index_decimal: Option<StrictWirePrimitive<'a>>,
        retryable: Option<StrictWirePrimitive<'a>>,
        message: StrictWirePrimitive<'a>,
    ) -> StrictOpenWireErrorEnvelopeInputV1<'a> {
        StrictOpenWireErrorEnvelopeInputV1::Exact {
            version,
            class,
            code,
            failed_index_decimal,
            retryable,
            message,
        }
    }

    fn exact_recording_ack<'a>(
        public_recording_id: StrictWirePrimitive<'a>,
        lifecycle_revision_decimal: StrictWirePrimitive<'a>,
    ) -> StrictRecordingActivationAckInputV1<'a> {
        StrictRecordingActivationAckInputV1::Exact {
            public_recording_id,
            lifecycle_revision_decimal,
        }
    }

    fn exact_installation_ack<'a>(
        version: StrictWirePrimitive<'a>,
        operation_token: StrictWirePrimitive<'a>,
        installation_nonce: StrictWirePrimitive<'a>,
        recordings: &'a [StrictRecordingActivationAckInputV1<'a>],
    ) -> StrictOpenInstallationAckEnvelopeInputV1<'a> {
        StrictOpenInstallationAckEnvelopeInputV1::Exact {
            version,
            operation_token,
            installation_nonce,
            recordings,
        }
    }

    fn attachment(
        request_id: PublicOpenRequestIdentity,
        recording_id: PublicRecordingIdentity,
        lifecycle_revision: u64,
    ) -> CommittedRecordingAttachmentV1 {
        CommittedRecordingAttachmentV1 {
            public_recording_id: recording_id,
            lifecycle_revision,
            descriptor: PublicRecordingDescriptorV1 {
                request_id,
                recording_handle_id: recording_id,
                display_name: BoundedRedactedRecordingLabelV1::generic(),
            },
        }
    }

    fn descriptor(
        operation: u128,
        request: u128,
        installation: u128,
        recording_ids: &[u128],
    ) -> CommittedOpenDescriptorV1 {
        let public_request_id = PublicOpenRequestIdentity::new(request);
        CommittedOpenDescriptorV1 {
            operation_token: OpenOperationIdentity::new(operation),
            public_request_id,
            installation_nonce: StrictOpenInstallationIdentity::new(installation),
            preexisting_recordings: recording_ids
                .iter()
                .enumerate()
                .map(|(revision, recording)| {
                    attachment(
                        public_request_id,
                        PublicRecordingIdentity::new(*recording),
                        u64::try_from(revision).unwrap(),
                    )
                })
                .collect(),
        }
    }

    fn assert_fatal(error: &StrictOpenWireErrorV1, code: StrictOpenFatalAbiCodeV1) {
        assert!(matches!(
            error,
            StrictOpenWireErrorV1::FatalAbi(StrictOpenFatalAbiErrorV1 {
                code: actual,
                ..
            }) if *actual == code
        ));
        assert_eq!(
            error.viewer_policy(),
            StrictOpenViewerPolicyV1::TeardownViewer
        );
    }

    fn assert_protocol(error: &StrictOpenWireErrorV1, code: StrictOpenProtocolCodeV1) {
        assert!(matches!(
            error,
            StrictOpenWireErrorV1::ProtocolViolation(StrictOpenProtocolErrorV1 {
                code: actual,
                ..
            }) if *actual == code
        ));
        assert_eq!(error.viewer_policy(), StrictOpenViewerPolicyV1::KeepRunning);
    }

    fn decode_encoded_error(
        codec: &StrictOpenWireCodecV1,
        envelope: &StrictOpenWireErrorEnvelopeV1,
    ) -> StrictOpenWireErrorV1 {
        codec.decode_error(exact_error_input(
            StrictWirePrimitive::JavaScriptNumber(f64::from(envelope.version)),
            StrictWirePrimitive::String(envelope.class),
            StrictWirePrimitive::String(envelope.code),
            envelope
                .failed_index_decimal
                .as_ref()
                .map(|index| StrictWirePrimitive::String(index.wire_str())),
            envelope.retryable.map(StrictWirePrimitive::Boolean),
            StrictWirePrimitive::String(envelope.message.wire_str()),
        ))
    }

    #[test]
    fn canonical_u64_decimal_rejects_every_noncanonical_form() {
        for value in [0, 1, u64::MAX] {
            let encoded = encode_canonical_u64(value);
            assert_eq!(
                decode_canonical_u64(StrictWirePrimitive::String(encoded.wire_str())),
                Ok(value)
            );
        }
        assert_eq!(
            encode_canonical_u64(u64::MAX).wire_str(),
            "18446744073709551615"
        );

        for (input, expected) in [
            (StrictWirePrimitive::String(""), DecimalDecodeError::Empty),
            (StrictWirePrimitive::String("-1"), DecimalDecodeError::Sign),
            (StrictWirePrimitive::String("+1"), DecimalDecodeError::Sign),
            (
                StrictWirePrimitive::String("00"),
                DecimalDecodeError::LeadingZero,
            ),
            (
                StrictWirePrimitive::String("01"),
                DecimalDecodeError::LeadingZero,
            ),
            (
                StrictWirePrimitive::String("18446744073709551616"),
                DecimalDecodeError::Overflow,
            ),
            (
                StrictWirePrimitive::String("１２"),
                DecimalDecodeError::NonAscii,
            ),
            (
                StrictWirePrimitive::String("1a"),
                DecimalDecodeError::NonDigit,
            ),
            (
                StrictWirePrimitive::JavaScriptNumber(1.0),
                DecimalDecodeError::WrongWireType,
            ),
        ] {
            assert_eq!(decode_canonical_u64(input), Err(expected));
        }
    }

    #[test]
    fn every_identity_has_an_instance_scoped_domain() {
        let codec = codec();
        let handoff = StrictOpenHandoffIdentity::new(1);
        let operation = OpenOperationIdentity::new(2);
        let installation = StrictOpenInstallationIdentity::new(3);
        let request = PublicOpenRequestIdentity::new(4);
        let recording = PublicRecordingIdentity::new(5);

        let handoff_wire = codec.encode_handoff(handoff);
        let operation_wire = codec.encode_operation(operation);
        let installation_wire = codec.encode_installation(installation);
        let request_wire = codec.encode_public_request(request);
        let recording_wire = codec.encode_public_recording(recording);

        assert_eq!(
            codec.decode_handoff(StrictWirePrimitive::String(handoff_wire.wire_str())),
            Ok(handoff)
        );
        assert_eq!(
            codec.decode_operation(StrictWirePrimitive::String(operation_wire.wire_str())),
            Ok(operation)
        );
        assert_eq!(
            codec.decode_installation(StrictWirePrimitive::String(installation_wire.wire_str())),
            Ok(installation)
        );
        assert_eq!(
            codec.decode_public_request(StrictWirePrimitive::String(request_wire.wire_str())),
            Ok(request)
        );
        assert_eq!(
            codec.decode_public_recording(StrictWirePrimitive::String(recording_wire.wire_str())),
            Ok(recording)
        );

        assert_eq!(
            codec.decode_installation(StrictWirePrimitive::String(operation_wire.wire_str())),
            Err(OpaqueIdentityDecodeError::WrongDomain)
        );
        let other_instance_codec = {
            let root = test_profile_with(&[(WebRemoteLimitKey::OpenBatchUrls, 4)])
                .start_accounting_root()
                .unwrap();
            let viewer = root.create_viewer_scope().unwrap();
            let batch = viewer.create_atomic_batch_scope().unwrap();
            StrictOpenWireCodecV1::for_batch(StrictOpenWireInstanceId::new(0x5678), &batch).unwrap()
        };
        assert_eq!(
            other_instance_codec
                .decode_operation(StrictWirePrimitive::String(operation_wire.wire_str())),
            Err(OpaqueIdentityDecodeError::WrongInstance)
        );
    }

    #[test]
    fn handoff_decode_classifies_wrong_domain_instance_and_shape() {
        let codec = codec();
        let operation_wire = codec.encode_operation(OpenOperationIdentity::new(1));
        let error = codec
            .decode_handoff_token(StrictWirePrimitive::String(operation_wire.wire_str()))
            .unwrap_err();
        assert_protocol(&error, StrictOpenProtocolCodeV1::WrongIdentityDomain);

        let other_instance_wire =
            encode_opaque_identity(StrictOpenWireInstanceId::new(0x9999), "handoff", 1);
        for input in [
            StrictWirePrimitive::String(&other_instance_wire),
            StrictWirePrimitive::String("not-an-identity"),
            StrictWirePrimitive::JavaScriptNumber(1.0),
        ] {
            let error = codec.decode_handoff_token(input).unwrap_err();
            assert_protocol(&error, StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken);
        }
    }

    #[test]
    fn success_encoding_preserves_identity_and_max_u64_revision() {
        let codec = codec();
        let request_id = PublicOpenRequestIdentity::new(4);
        let recording_id = PublicRecordingIdentity::new(5);
        let wire = codec
            .encode_success(StrictOpenSuccessV1 {
                handoff_token: StrictOpenHandoffIdentity::new(1),
                descriptors: vec![CommittedOpenDescriptorV1 {
                    operation_token: OpenOperationIdentity::new(2),
                    public_request_id: request_id,
                    installation_nonce: StrictOpenInstallationIdentity::new(3),
                    preexisting_recordings: vec![CommittedRecordingAttachmentV1 {
                        public_recording_id: recording_id,
                        lifecycle_revision: u64::MAX,
                        descriptor: PublicRecordingDescriptorV1 {
                            request_id,
                            recording_handle_id: recording_id,
                            display_name: BoundedRedactedRecordingLabelV1::generic(),
                        },
                    }],
                }],
            })
            .unwrap();

        assert_eq!(wire.version, STRICT_OPEN_WIRE_VERSION_V1);
        assert_eq!(wire.descriptors.len(), 1);
        let descriptor = &wire.descriptors[0];
        let attachment = &descriptor.preexisting_recordings[0];
        assert_eq!(
            attachment.lifecycle_revision_decimal.wire_str(),
            "18446744073709551615"
        );
        assert_eq!(
            attachment.public_recording_id,
            attachment.descriptor.recording_handle_id
        );
        assert_eq!(
            descriptor.public_request_id,
            attachment.descriptor.request_id
        );
    }

    #[test]
    fn success_encoding_rejects_descriptor_mismatch_and_batch_overflow() {
        let codec = codec_with_batch_limit(1);
        let mismatch = codec.encode_success(StrictOpenSuccessV1 {
            handoff_token: StrictOpenHandoffIdentity::new(1),
            descriptors: vec![CommittedOpenDescriptorV1 {
                operation_token: OpenOperationIdentity::new(2),
                public_request_id: PublicOpenRequestIdentity::new(3),
                installation_nonce: StrictOpenInstallationIdentity::new(4),
                preexisting_recordings: vec![CommittedRecordingAttachmentV1 {
                    public_recording_id: PublicRecordingIdentity::new(5),
                    lifecycle_revision: 0,
                    descriptor: PublicRecordingDescriptorV1 {
                        request_id: PublicOpenRequestIdentity::new(6),
                        recording_handle_id: PublicRecordingIdentity::new(5),
                        display_name: BoundedRedactedRecordingLabelV1::generic(),
                    },
                }],
            }],
        });
        assert_fatal(
            &mismatch.unwrap_err(),
            StrictOpenFatalAbiCodeV1::CodecInvariantBroken,
        );

        let overflow = codec.encode_success(StrictOpenSuccessV1 {
            handoff_token: StrictOpenHandoffIdentity::new(1),
            descriptors: vec![
                CommittedOpenDescriptorV1 {
                    operation_token: OpenOperationIdentity::new(2),
                    public_request_id: PublicOpenRequestIdentity::new(3),
                    installation_nonce: StrictOpenInstallationIdentity::new(4),
                    preexisting_recordings: Vec::new(),
                },
                CommittedOpenDescriptorV1 {
                    operation_token: OpenOperationIdentity::new(5),
                    public_request_id: PublicOpenRequestIdentity::new(6),
                    installation_nonce: StrictOpenInstallationIdentity::new(7),
                    preexisting_recordings: Vec::new(),
                },
            ],
        });
        assert_fatal(
            &overflow.unwrap_err(),
            StrictOpenFatalAbiCodeV1::CodecInvariantBroken,
        );
    }

    #[test]
    fn success_uses_attachment_cap_across_the_whole_batch() {
        let codec = codec_with_limits(1, 3, 3);
        let one_operation_with_three_recordings = codec.encode_success(StrictOpenSuccessV1 {
            handoff_token: StrictOpenHandoffIdentity::new(1),
            descriptors: vec![descriptor(2, 3, 4, &[10, 11, 12])],
        });
        assert_eq!(
            one_operation_with_three_recordings.unwrap().descriptors[0]
                .preexisting_recordings
                .len(),
            3
        );

        let codec = codec_with_limits(2, 3, 4);
        let error = codec
            .encode_success(StrictOpenSuccessV1 {
                handoff_token: StrictOpenHandoffIdentity::new(1),
                descriptors: vec![
                    descriptor(2, 3, 4, &[10, 11]),
                    descriptor(5, 6, 7, &[12, 13]),
                ],
            })
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::CodecInvariantBroken);

        let error = codec_with_limits(2, 2, 1)
            .encode_success(StrictOpenSuccessV1 {
                handoff_token: StrictOpenHandoffIdentity::new(1),
                descriptors: vec![descriptor(2, 3, 4, &[]), descriptor(5, 6, 7, &[])],
            })
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::CodecInvariantBroken);
    }

    #[test]
    fn success_rejects_duplicate_descriptor_identities_before_encoding() {
        for duplicate in ["operation", "request", "installation"] {
            let first = descriptor(1, 2, 3, &[10]);
            let mut second = descriptor(4, 5, 6, &[11]);
            match duplicate {
                "operation" => second.operation_token = first.operation_token,
                "request" => {
                    second.public_request_id = first.public_request_id;
                    second.preexisting_recordings[0].descriptor.request_id =
                        first.public_request_id;
                }
                "installation" => second.installation_nonce = first.installation_nonce,
                _ => unreachable!(),
            }
            let error = codec_with_limits(2, 2, 2)
                .encode_success(StrictOpenSuccessV1 {
                    handoff_token: StrictOpenHandoffIdentity::new(7),
                    descriptors: vec![first, second],
                })
                .unwrap_err();
            assert_fatal(&error, StrictOpenFatalAbiCodeV1::CodecInvariantBroken);
        }

        let error = codec_with_limits(1, 2, 2)
            .encode_success(StrictOpenSuccessV1 {
                handoff_token: StrictOpenHandoffIdentity::new(7),
                descriptors: vec![descriptor(1, 2, 3, &[10, 10])],
            })
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::CodecInvariantBroken);
    }

    #[test]
    fn compatible_aliases_may_share_a_recording_across_descriptors() {
        let wire = codec_with_limits(2, 2, 2)
            .encode_success(StrictOpenSuccessV1 {
                handoff_token: StrictOpenHandoffIdentity::new(7),
                descriptors: vec![descriptor(1, 2, 3, &[10]), descriptor(4, 5, 6, &[10])],
            })
            .unwrap();
        assert_eq!(wire.descriptors.len(), 2);
        assert_eq!(
            wire.descriptors[0].preexisting_recordings[0].public_recording_id,
            wire.descriptors[1].preexisting_recordings[0].public_recording_id
        );
    }

    #[test]
    fn compatible_aliases_require_one_shared_recording_snapshot() {
        for mismatch in ["revision", "label"] {
            let first = descriptor(1, 2, 3, &[10]);
            let mut second = descriptor(4, 5, 6, &[10]);
            match mismatch {
                "revision" => second.preexisting_recordings[0].lifecycle_revision = 42,
                "label" => {
                    second.preexisting_recordings[0].descriptor.display_name =
                        BoundedRedactedRecordingLabelV1(BoundedRedactedWireString(
                            "Other recording".to_owned(),
                        ));
                }
                _ => unreachable!(),
            }
            let error = codec_with_limits(2, 2, 2)
                .encode_success(StrictOpenSuccessV1 {
                    handoff_token: StrictOpenHandoffIdentity::new(7),
                    descriptors: vec![first, second],
                })
                .unwrap_err();
            assert_fatal(&error, StrictOpenFatalAbiCodeV1::CodecInvariantBroken);
        }
    }

    #[test]
    fn installation_ack_roundtrips_exact_expected_values() {
        let codec = codec();
        let operation = OpenOperationIdentity::new(1);
        let installation = StrictOpenInstallationIdentity::new(2);
        let recording = PublicRecordingIdentity::new(3);
        let operation_wire = codec.encode_operation(operation);
        let installation_wire = codec.encode_installation(installation);
        let recording_wire = codec.encode_public_recording(recording);
        let recording_input = [exact_recording_ack(
            StrictWirePrimitive::String(recording_wire.wire_str()),
            StrictWirePrimitive::String("18446744073709551615"),
        )];
        let decoded = codec
            .decode_installation_ack(
                exact_installation_ack(
                    version(),
                    StrictWirePrimitive::String(operation_wire.wire_str()),
                    StrictWirePrimitive::String(installation_wire.wire_str()),
                    &recording_input,
                ),
                &StrictOpenInstallationAckExpectationV1 {
                    operation_token: operation,
                    installation_nonce: installation,
                    recordings: vec![ExpectedRecordingActivationAckV1 {
                        public_recording_id: recording,
                        lifecycle_revision: u64::MAX,
                    }],
                },
            )
            .unwrap();
        assert_eq!(decoded.operation_token, operation);
        assert_eq!(decoded.installation_nonce, installation);
        assert_eq!(decoded.recordings[0].instance, INSTANCE);
        assert_eq!(decoded.recordings[0].lifecycle_revision, u64::MAX);
    }

    #[test]
    fn installation_ack_uses_its_own_full_batch_cap() {
        let codec = codec_with_limits(1, 3, 1);
        let operation = OpenOperationIdentity::new(1);
        let installation = StrictOpenInstallationIdentity::new(2);
        let recording_ids = [
            PublicRecordingIdentity::new(3),
            PublicRecordingIdentity::new(4),
            PublicRecordingIdentity::new(5),
        ];
        let operation_wire = codec.encode_operation(operation);
        let installation_wire = codec.encode_installation(installation);
        let recording_wires =
            recording_ids.map(|recording| codec.encode_public_recording(recording));
        let recordings = [
            exact_recording_ack(
                StrictWirePrimitive::String(recording_wires[0].wire_str()),
                StrictWirePrimitive::String("1"),
            ),
            exact_recording_ack(
                StrictWirePrimitive::String(recording_wires[1].wire_str()),
                StrictWirePrimitive::String("2"),
            ),
            exact_recording_ack(
                StrictWirePrimitive::String(recording_wires[2].wire_str()),
                StrictWirePrimitive::String("3"),
            ),
        ];
        let expected = StrictOpenInstallationAckExpectationV1 {
            operation_token: operation,
            installation_nonce: installation,
            recordings: recording_ids
                .into_iter()
                .enumerate()
                .map(
                    |(revision, public_recording_id)| ExpectedRecordingActivationAckV1 {
                        public_recording_id,
                        lifecycle_revision: u64::try_from(revision + 1).unwrap(),
                    },
                )
                .collect(),
        };
        let decoded = codec
            .decode_installation_ack(
                exact_installation_ack(
                    version(),
                    StrictWirePrimitive::String(operation_wire.wire_str()),
                    StrictWirePrimitive::String(installation_wire.wire_str()),
                    &recordings,
                ),
                &expected,
            )
            .unwrap();
        assert_eq!(decoded.recordings.len(), 3);

        let codec = codec_with_limits(2, 3, 2);
        let operation_a = codec.encode_operation(OpenOperationIdentity::new(10));
        let operation_b = codec.encode_operation(OpenOperationIdentity::new(11));
        let installation_a = codec.encode_installation(StrictOpenInstallationIdentity::new(12));
        let installation_b = codec.encode_installation(StrictOpenInstallationIdentity::new(13));
        let recording_wires = [20_u128, 21, 22, 23].map(|recording| {
            codec.encode_public_recording(PublicRecordingIdentity::new(recording))
        });
        let recordings_a = [
            exact_recording_ack(
                StrictWirePrimitive::String(recording_wires[0].wire_str()),
                StrictWirePrimitive::String("1"),
            ),
            exact_recording_ack(
                StrictWirePrimitive::String(recording_wires[1].wire_str()),
                StrictWirePrimitive::String("2"),
            ),
        ];
        let recordings_b = [
            exact_recording_ack(
                StrictWirePrimitive::String(recording_wires[2].wire_str()),
                StrictWirePrimitive::String("3"),
            ),
            exact_recording_ack(
                StrictWirePrimitive::String(recording_wires[3].wire_str()),
                StrictWirePrimitive::String("4"),
            ),
        ];
        let inputs = [
            exact_installation_ack(
                version(),
                StrictWirePrimitive::String(operation_a.wire_str()),
                StrictWirePrimitive::String(installation_a.wire_str()),
                &recordings_a,
            ),
            exact_installation_ack(
                version(),
                StrictWirePrimitive::String(operation_b.wire_str()),
                StrictWirePrimitive::String(installation_b.wire_str()),
                &recordings_b,
            ),
        ];
        let expectations = [
            StrictOpenInstallationAckExpectationV1 {
                operation_token: OpenOperationIdentity::new(10),
                installation_nonce: StrictOpenInstallationIdentity::new(12),
                recordings: vec![
                    ExpectedRecordingActivationAckV1 {
                        public_recording_id: PublicRecordingIdentity::new(20),
                        lifecycle_revision: 1,
                    },
                    ExpectedRecordingActivationAckV1 {
                        public_recording_id: PublicRecordingIdentity::new(21),
                        lifecycle_revision: 2,
                    },
                ],
            },
            StrictOpenInstallationAckExpectationV1 {
                operation_token: OpenOperationIdentity::new(11),
                installation_nonce: StrictOpenInstallationIdentity::new(13),
                recordings: vec![
                    ExpectedRecordingActivationAckV1 {
                        public_recording_id: PublicRecordingIdentity::new(22),
                        lifecycle_revision: 3,
                    },
                    ExpectedRecordingActivationAckV1 {
                        public_recording_id: PublicRecordingIdentity::new(23),
                        lifecycle_revision: 4,
                    },
                ],
            },
        ];
        let error = codec
            .decode_installation_acks(&inputs, &expectations)
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::CodecInvariantBroken);

        let codec = codec_with_limits(2, 1, 1);
        let operation_a = codec.encode_operation(OpenOperationIdentity::new(30));
        let operation_b = codec.encode_operation(OpenOperationIdentity::new(31));
        let installation_a = codec.encode_installation(StrictOpenInstallationIdentity::new(32));
        let installation_b = codec.encode_installation(StrictOpenInstallationIdentity::new(33));
        let no_recordings = [];
        let inputs = [
            exact_installation_ack(
                version(),
                StrictWirePrimitive::String(operation_a.wire_str()),
                StrictWirePrimitive::String(installation_a.wire_str()),
                &no_recordings,
            ),
            exact_installation_ack(
                version(),
                StrictWirePrimitive::String(operation_b.wire_str()),
                StrictWirePrimitive::String(installation_b.wire_str()),
                &no_recordings,
            ),
        ];
        let expectations = [
            StrictOpenInstallationAckExpectationV1 {
                operation_token: OpenOperationIdentity::new(30),
                installation_nonce: StrictOpenInstallationIdentity::new(32),
                recordings: Vec::new(),
            },
            StrictOpenInstallationAckExpectationV1 {
                operation_token: OpenOperationIdentity::new(31),
                installation_nonce: StrictOpenInstallationIdentity::new(33),
                recordings: Vec::new(),
            },
        ];
        let error = codec
            .decode_installation_acks(&inputs, &expectations)
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::CodecInvariantBroken);
    }

    #[test]
    fn installation_ack_rejects_noncanonical_decimal_as_protocol_error() {
        let codec = codec();
        let operation = OpenOperationIdentity::new(1);
        let installation = StrictOpenInstallationIdentity::new(2);
        let recording = PublicRecordingIdentity::new(3);
        let operation_wire = codec.encode_operation(operation);
        let installation_wire = codec.encode_installation(installation);
        let recording_wire = codec.encode_public_recording(recording);
        let expectation = StrictOpenInstallationAckExpectationV1 {
            operation_token: operation,
            installation_nonce: installation,
            recordings: vec![ExpectedRecordingActivationAckV1 {
                public_recording_id: recording,
                lifecycle_revision: 1,
            }],
        };

        for decimal in [
            StrictWirePrimitive::String(""),
            StrictWirePrimitive::String("-1"),
            StrictWirePrimitive::String("+1"),
            StrictWirePrimitive::String("01"),
            StrictWirePrimitive::String("18446744073709551616"),
            StrictWirePrimitive::String("１２"),
            StrictWirePrimitive::JavaScriptNumber(1.0),
        ] {
            let recordings = [exact_recording_ack(
                StrictWirePrimitive::String(recording_wire.wire_str()),
                decimal,
            )];
            let error = codec
                .decode_installation_ack(
                    exact_installation_ack(
                        version(),
                        StrictWirePrimitive::String(operation_wire.wire_str()),
                        StrictWirePrimitive::String(installation_wire.wire_str()),
                        &recordings,
                    ),
                    &expectation,
                )
                .unwrap_err();
            assert_protocol(&error, StrictOpenProtocolCodeV1::NonCanonicalDecimal);
        }
    }

    #[test]
    fn installation_ack_rejects_wrong_shape_identity_and_membership() {
        let codec = codec();
        let operation = OpenOperationIdentity::new(1);
        let installation = StrictOpenInstallationIdentity::new(2);
        let recording_a = PublicRecordingIdentity::new(3);
        let recording_b = PublicRecordingIdentity::new(4);
        let operation_wire = codec.encode_operation(operation);
        let installation_wire = codec.encode_installation(installation);
        let recording_a_wire = codec.encode_public_recording(recording_a);
        let recording_b_wire = codec.encode_public_recording(recording_b);
        let expectation = StrictOpenInstallationAckExpectationV1 {
            operation_token: operation,
            installation_nonce: installation,
            recordings: vec![
                ExpectedRecordingActivationAckV1 {
                    public_recording_id: recording_a,
                    lifecycle_revision: 1,
                },
                ExpectedRecordingActivationAckV1 {
                    public_recording_id: recording_b,
                    lifecycle_revision: 2,
                },
            ],
        };

        let exact = [
            exact_recording_ack(
                StrictWirePrimitive::String(recording_a_wire.wire_str()),
                StrictWirePrimitive::String("1"),
            ),
            exact_recording_ack(
                StrictWirePrimitive::String(recording_b_wire.wire_str()),
                StrictWirePrimitive::String("2"),
            ),
        ];
        let envelope = |recordings| {
            exact_installation_ack(
                version(),
                StrictWirePrimitive::String(operation_wire.wire_str()),
                StrictWirePrimitive::String(installation_wire.wire_str()),
                recordings,
            )
        };

        let error = codec
            .decode_installation_ack(
                exact_installation_ack(
                    StrictWirePrimitive::JavaScriptNumber(2.0),
                    StrictWirePrimitive::String(operation_wire.wire_str()),
                    StrictWirePrimitive::String(installation_wire.wire_str()),
                    &exact,
                ),
                &expectation,
            )
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);

        let error = codec
            .decode_installation_ack(
                StrictOpenInstallationAckEnvelopeInputV1::UnknownShape,
                &expectation,
            )
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);

        let nested_unknown_shape = [StrictRecordingActivationAckInputV1::UnknownShape, exact[1]];
        let error = codec
            .decode_installation_ack(envelope(&nested_unknown_shape), &expectation)
            .unwrap_err();
        assert_fatal(&error, StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);

        let duplicate = [exact[0], exact[0]];
        let error = codec
            .decode_installation_ack(envelope(&duplicate), &expectation)
            .unwrap_err();
        assert_protocol(&error, StrictOpenProtocolCodeV1::DuplicateOrMissingAck);

        let error = codec
            .decode_installation_ack(envelope(&exact[..1]), &expectation)
            .unwrap_err();
        assert_protocol(&error, StrictOpenProtocolCodeV1::DuplicateOrMissingAck);

        let error = codec
            .decode_installation_ack(
                exact_installation_ack(
                    version(),
                    StrictWirePrimitive::String(operation_wire.wire_str()),
                    StrictWirePrimitive::String(operation_wire.wire_str()),
                    &exact,
                ),
                &expectation,
            )
            .unwrap_err();
        assert_protocol(&error, StrictOpenProtocolCodeV1::WrongIdentityDomain);

        let other_instance_installation =
            encode_opaque_identity(StrictOpenWireInstanceId::new(0x9999), "installation", 2);
        let error = codec
            .decode_installation_ack(
                exact_installation_ack(
                    version(),
                    StrictWirePrimitive::String(operation_wire.wire_str()),
                    StrictWirePrimitive::String(&other_instance_installation),
                    &exact,
                ),
                &expectation,
            )
            .unwrap_err();
        assert_protocol(&error, StrictOpenProtocolCodeV1::WrongIdentityDomain);
    }

    #[test]
    fn every_error_code_has_a_stable_roundtrip() {
        let codec = codec();
        for (code, wire) in [
            (
                StrictOpenAdmissionCodeV1::InvalidRequestShape,
                "invalid_request_shape",
            ),
            (StrictOpenAdmissionCodeV1::InvalidUrl, "invalid_url"),
            (
                StrictOpenAdmissionCodeV1::InvalidRemoteMcapOptions,
                "invalid_remote_mcap_options",
            ),
            (
                StrictOpenAdmissionCodeV1::UnsupportedStrictOpenRoute,
                "unsupported_strict_open_route",
            ),
            (StrictOpenAdmissionCodeV1::BatchTooLarge, "batch_too_large"),
            (
                StrictOpenAdmissionCodeV1::ResourceLimitExceeded,
                "resource_limit_exceeded",
            ),
            (
                StrictOpenAdmissionCodeV1::RemoteSessionLimitReached,
                "remote_session_limit_reached",
            ),
            (
                StrictOpenAdmissionCodeV1::ExistingSourceOptionsConflict,
                "existing_source_options_conflict",
            ),
            (
                StrictOpenAdmissionCodeV1::ConflictingStartupSources,
                "conflicting_startup_sources",
            ),
            (
                StrictOpenAdmissionCodeV1::BrowserIngressDraining,
                "browser_ingress_draining",
            ),
            (
                StrictOpenAdmissionCodeV1::BrowserExecutionSuspended,
                "browser_execution_suspended",
            ),
        ] {
            assert_eq!(code.wire_code(), wire);
            let error = StrictOpenWireErrorV1::Admission(StrictOpenAdmissionErrorV1 {
                code,
                failed_index: Some(0),
                message: BoundedRedactedWireString::from_static(code.message()),
            });
            let envelope = codec.encode_error(error.clone());
            assert_eq!(decode_encoded_error(&codec, &envelope), error);
        }

        for code in [
            StrictOpenCancelledCodeV1::ViewerStopped,
            StrictOpenCancelledCodeV1::HandoffCancelled,
        ] {
            let error = StrictOpenWireErrorV1::Cancelled(StrictOpenCancelledErrorV1 {
                code,
                message: BoundedRedactedWireString::from_static(code.message()),
            });
            let envelope = codec.encode_error(error.clone());
            assert_eq!(decode_encoded_error(&codec, &envelope), error);
        }

        let error = StrictOpenWireErrorV1::handoff_state_changed();
        let envelope = codec.encode_error(error.clone());
        assert_eq!(envelope.retryable, Some(true));
        assert_eq!(decode_encoded_error(&codec, &envelope), error);

        for code in [
            StrictOpenProtocolCodeV1::WrongOrStaleHandoffToken,
            StrictOpenProtocolCodeV1::WrongIdentityDomain,
            StrictOpenProtocolCodeV1::NonCanonicalDecimal,
            StrictOpenProtocolCodeV1::InstallationAckMismatch,
            StrictOpenProtocolCodeV1::DuplicateOrMissingAck,
        ] {
            let error = protocol_error(code);
            let envelope = codec.encode_error(error.clone());
            assert_eq!(decode_encoded_error(&codec, &envelope), error);
        }

        for code in [
            StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape,
            StrictOpenFatalAbiCodeV1::WasmPanic,
            StrictOpenFatalAbiCodeV1::CodecInvariantBroken,
            StrictOpenFatalAbiCodeV1::CleanupCouldNotBeProved,
            StrictOpenFatalAbiCodeV1::DispatcherInvariantBroken,
        ] {
            let error = fatal_error(code);
            let envelope = codec.encode_error(error.clone());
            assert_eq!(decode_encoded_error(&codec, &envelope), error);
        }
    }

    #[test]
    fn handoff_state_changed_is_always_retryable_in_v1() {
        let codec = codec();
        let error = StrictOpenWireErrorV1::handoff_state_changed();
        let encoded = codec.encode_error(error.clone());
        assert_eq!(encoded.retryable, Some(true));
        assert_eq!(decode_encoded_error(&codec, &encoded), error);

        let decoded = codec.decode_error(exact_error_input(
            version(),
            StrictWirePrimitive::String("handoff_state_changed"),
            StrictWirePrimitive::String(
                StrictOpenStateChangedCodeV1::HandoffStateChanged.wire_code(),
            ),
            None,
            Some(StrictWirePrimitive::Boolean(false)),
            StrictWirePrimitive::String(
                StrictOpenStateChangedCodeV1::HandoffStateChanged.message(),
            ),
        ));
        assert_fatal(
            &decoded,
            StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape,
        );
    }

    #[test]
    fn error_decoder_rejects_unknown_shape_code_and_failed_index_forms() {
        let codec = codec_with_batch_limit(2);
        let class = StrictWirePrimitive::String("admission");
        let code = StrictWirePrimitive::String("invalid_url");
        let index = Some(StrictWirePrimitive::String("1"));
        let message = StrictWirePrimitive::String(StrictOpenAdmissionCodeV1::InvalidUrl.message());

        for input in [
            exact_error_input(
                StrictWirePrimitive::JavaScriptNumber(2.0),
                class,
                code,
                index,
                None,
                message,
            ),
            StrictOpenWireErrorEnvelopeInputV1::UnknownShape,
            exact_error_input(
                version(),
                StrictWirePrimitive::String("unknown"),
                code,
                index,
                None,
                message,
            ),
            exact_error_input(
                version(),
                class,
                StrictWirePrimitive::String("unknown"),
                index,
                None,
                message,
            ),
            exact_error_input(
                version(),
                class,
                code,
                Some(StrictWirePrimitive::String("01")),
                None,
                message,
            ),
            exact_error_input(
                version(),
                class,
                code,
                Some(StrictWirePrimitive::String("2")),
                None,
                message,
            ),
            exact_error_input(
                version(),
                class,
                code,
                Some(StrictWirePrimitive::JavaScriptNumber(1.0)),
                None,
                message,
            ),
        ] {
            let error = codec.decode_error(input);
            assert_fatal(&error, StrictOpenFatalAbiCodeV1::UnknownWireVersionOrShape);
        }
    }

    #[test]
    fn error_messages_are_bounded_fixed_and_secret_safe() {
        let codec = codec();
        let secret = "https://secret.example/?token=do-not-serialize";
        let envelope = codec.encode_error(StrictOpenWireErrorV1::Admission(
            StrictOpenAdmissionErrorV1 {
                code: StrictOpenAdmissionCodeV1::InvalidUrl,
                failed_index: None,
                message: BoundedRedactedWireString(secret.to_owned()),
            },
        ));
        assert_eq!(envelope.class, "fatal_abi");
        assert_eq!(
            envelope.code,
            StrictOpenFatalAbiCodeV1::CodecInvariantBroken.wire_code()
        );
        assert!(!envelope.message.wire_str().contains(secret));
        assert!(
            !BoundedRedactedRecordingLabelV1::generic()
                .wire_str()
                .contains(secret)
        );

        let too_long = codec.encode_error(StrictOpenWireErrorV1::Admission(
            StrictOpenAdmissionErrorV1 {
                code: StrictOpenAdmissionCodeV1::InvalidUrl,
                failed_index: None,
                message: BoundedRedactedWireString(
                    "x".repeat(STRICT_OPEN_REDACTED_MESSAGE_MAX_BYTES_V1 + 1),
                ),
            },
        ));
        assert_eq!(
            too_long.code,
            StrictOpenFatalAbiCodeV1::CodecInvariantBroken.wire_code()
        );
    }

    #[test]
    fn error_encoder_rejects_out_of_range_failed_index() {
        let codec = codec_with_batch_limit(1);
        let code = StrictOpenAdmissionCodeV1::InvalidUrl;
        let envelope = codec.encode_error(StrictOpenWireErrorV1::Admission(
            StrictOpenAdmissionErrorV1 {
                code,
                failed_index: Some(1),
                message: BoundedRedactedWireString::from_static(code.message()),
            },
        ));
        assert_eq!(envelope.class, "fatal_abi");
        assert_eq!(
            envelope.code,
            StrictOpenFatalAbiCodeV1::CodecInvariantBroken.wire_code()
        );
        assert!(envelope.failed_index_decimal.is_none());
    }

    #[test]
    fn stopped_accounting_root_is_cancelled_not_fatal() {
        let root = test_profile_with(&[]).start_accounting_root().unwrap();
        let viewer = root.create_viewer_scope().unwrap();
        let batch = viewer.create_atomic_batch_scope().unwrap();
        drop(root);

        let error = StrictOpenWireCodecV1::for_batch(INSTANCE, &batch).unwrap_err();
        assert!(matches!(
            error,
            StrictOpenWireErrorV1::Cancelled(StrictOpenCancelledErrorV1 {
                code: StrictOpenCancelledCodeV1::ViewerStopped,
                ..
            })
        ));
        assert_eq!(error.viewer_policy(), StrictOpenViewerPolicyV1::KeepRunning);
    }

    #[test]
    fn only_fatal_abi_requests_viewer_teardown() {
        let errors = [
            StrictOpenWireErrorV1::admission(StrictOpenAdmissionCodeV1::InvalidUrl, None),
            StrictOpenWireErrorV1::cancelled(StrictOpenCancelledCodeV1::ViewerStopped),
            StrictOpenWireErrorV1::handoff_state_changed(),
            protocol_error(StrictOpenProtocolCodeV1::InstallationAckMismatch),
        ];
        for error in errors {
            assert_eq!(error.viewer_policy(), StrictOpenViewerPolicyV1::KeepRunning);
        }
        assert_eq!(
            fatal_error(StrictOpenFatalAbiCodeV1::WasmPanic).viewer_policy(),
            StrictOpenViewerPolicyV1::TeardownViewer
        );
    }
}
