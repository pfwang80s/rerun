//! Strict Chrome Range request construction and response validation for remote MCAP.

use std::fmt;
use std::num::NonZeroU64;
use std::ops::Range;

use crate::remote_validator::{
    BoundRepresentationConsistency, ParsedEntityTag, RemoteObjectValidator, RemoteValidatorError,
    RepresentationConsistency, RepresentationConsistencyPolicy, bind_representation_consistency,
};

#[cfg(target_arch = "wasm32")]
pub(crate) const MCAP_MAGIC_BYTES: [u8; 8] = [0x89, b'M', b'C', b'A', b'P', b'0', b'\r', b'\n'];

/// A required response header whose absence is observable after Fetch succeeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequiredRangeResponseHeader {
    ContentRange,
    EntityTag,
}

/// A redacted, typed failure from the strict Chrome Range transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeRangeError {
    InvalidRequestRange,
    BrowserAdapterUnavailable,
    BrowserFetchUnavailable,
    RangeUnsupported,
    AuthorizationRejected,
    ObjectUnavailable,
    PreconditionFailed,
    RangeNotSatisfiable,
    UnexpectedHttpStatus(u16),
    RequiredResponseHeaderUnavailable(RequiredRangeResponseHeader),
    InvalidContentRange,
    InvalidContentLength,
    UnsupportedContentEncoding,
    InvalidValidatorHeader,
    StrongValidatorRequired,
    ObjectChanged,
    ResourceLimit,
    #[cfg(target_arch = "wasm32")]
    BodyPump(crate::chrome_byob::ExactLengthByobPumpError),
}

impl fmt::Display for ChromeRangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequestRange => formatter.write_str("invalid remote byte range"),
            Self::BrowserAdapterUnavailable => {
                formatter.write_str("browser Range adapter is unavailable")
            }
            Self::BrowserFetchUnavailable => formatter.write_str(
                "browser Fetch failed; check network access, CORS, and content security policy",
            ),
            Self::RangeUnsupported => formatter.write_str("remote source does not support Range"),
            Self::AuthorizationRejected => {
                formatter.write_str("remote source rejected authorization")
            }
            Self::ObjectUnavailable => formatter.write_str("remote object is unavailable"),
            Self::PreconditionFailed => formatter.write_str("remote object precondition failed"),
            Self::RangeNotSatisfiable => {
                formatter.write_str("remote byte range is not satisfiable")
            }
            Self::UnexpectedHttpStatus(status) => {
                write!(
                    formatter,
                    "remote source returned unexpected HTTP status {status}"
                )
            }
            Self::RequiredResponseHeaderUnavailable(RequiredRangeResponseHeader::ContentRange) => {
                formatter.write_str("required Content-Range response header is unavailable")
            }
            Self::RequiredResponseHeaderUnavailable(RequiredRangeResponseHeader::EntityTag) => {
                formatter.write_str("required ETag response header is unavailable")
            }
            Self::InvalidContentRange => {
                formatter.write_str("Content-Range response header is invalid")
            }
            Self::InvalidContentLength => {
                formatter.write_str("Content-Length response header is invalid")
            }
            Self::UnsupportedContentEncoding => {
                formatter.write_str("remote Range response uses unsupported content encoding")
            }
            Self::InvalidValidatorHeader => {
                formatter.write_str("remote object validator header is invalid")
            }
            Self::StrongValidatorRequired => {
                formatter.write_str("a strong remote object validator is required")
            }
            Self::ObjectChanged => formatter.write_str("remote object representation changed"),
            Self::ResourceLimit => formatter.write_str("remote Range resource limit exceeded"),
            #[cfg(target_arch = "wasm32")]
            Self::BodyPump(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ChromeRangeError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RequestedRange {
    start: u64,
    end_inclusive: u64,
    expected_bytes: NonZeroU64,
}

impl RequestedRange {
    fn new(range: Range<u64>) -> Result<Self, ChromeRangeError> {
        let expected_bytes = range
            .end
            .checked_sub(range.start)
            .and_then(NonZeroU64::new)
            .ok_or(ChromeRangeError::InvalidRequestRange)?;
        let end_inclusive = range
            .end
            .checked_sub(1)
            .ok_or(ChromeRangeError::InvalidRequestRange)?;
        Ok(Self {
            start: range.start,
            end_inclusive,
            expected_bytes,
        })
    }

    fn as_exclusive_range(self) -> Range<u64> {
        self.start..self.end_inclusive + 1
    }
}

/// A non-empty, checked byte range prepared before any browser request owner exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChromeRangeRequest {
    requested: RequestedRange,
}

impl ChromeRangeRequest {
    /// Performs the allocation-free request-range preflight.
    pub fn new(range: Range<u64>) -> Result<Self, ChromeRangeError> {
        Ok(Self {
            requested: RequestedRange::new(range)?,
        })
    }

    /// Returns the checked half-open range.
    pub fn as_range(self) -> Range<u64> {
        self.requested.as_exclusive_range()
    }
}

/// Crate-private identity reported by a sealed, fully prepared Chrome Range owner.
pub(crate) trait PreparedChromeRangeIdentity<Controller> {
    fn prepared_range(&self) -> ChromeRangeRequest;

    fn matches_abort_controller(&self, controller: &Controller) -> bool;

    fn prepared_accounting_binding(&self) -> crate::remote_limits::RangeAttemptAccountingBinding;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedContentRange {
    start: u64,
    end_inclusive: u64,
    total: NonZeroU64,
}

trait AsciiHeaderInput {
    fn len(&self) -> usize;
    fn byte_at(&self, index: usize) -> Option<u8>;
}

impl AsciiHeaderInput for &[u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn byte_at(&self, index: usize) -> Option<u8> {
        self.get(index).copied()
    }
}

const MAX_CONTENT_RANGE_CODE_UNITS: usize = 68;
const MAX_CONTENT_LENGTH_CODE_UNITS: usize = 20;
const IDENTITY_ENCODING_BYTES: &[u8] = b"identity";

fn parse_content_range(input: &impl AsciiHeaderInput) -> Option<ParsedContentRange> {
    if input.len() > MAX_CONTENT_RANGE_CODE_UNITS || input.len() < 11 {
        return None;
    }
    for (index, expected) in b"bytes ".iter().copied().enumerate() {
        let actual = input.byte_at(index)?;
        if index < 5 {
            if !actual.eq_ignore_ascii_case(&expected) {
                return None;
            }
        } else if actual != expected {
            return None;
        }
    }
    let mut cursor = 6;
    let (start, next) = parse_decimal(input, cursor, b'-')?;
    cursor = next;
    let (end_inclusive, next) = parse_decimal(input, cursor, b'/')?;
    cursor = next;
    let (total, next) = parse_decimal_to_end(input, cursor)?;
    if next != input.len() || end_inclusive < start || total <= end_inclusive {
        return None;
    }
    Some(ParsedContentRange {
        start,
        end_inclusive,
        total: NonZeroU64::new(total)?,
    })
}

fn parse_content_length(input: &impl AsciiHeaderInput) -> Option<u64> {
    if input.len() == 0 || input.len() > MAX_CONTENT_LENGTH_CODE_UNITS {
        return None;
    }
    let (value, next) = parse_decimal_to_end(input, 0)?;
    (next == input.len()).then_some(value)
}

fn is_identity_encoding(input: &impl AsciiHeaderInput) -> bool {
    input.len() == IDENTITY_ENCODING_BYTES.len()
        && IDENTITY_ENCODING_BYTES
            .iter()
            .copied()
            .enumerate()
            .all(|(index, expected)| {
                input
                    .byte_at(index)
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(&expected))
            })
}

fn parse_decimal(
    input: &impl AsciiHeaderInput,
    start: usize,
    delimiter: u8,
) -> Option<(u64, usize)> {
    let mut cursor = start;
    let mut value = 0u64;
    let mut digits = 0usize;
    while cursor < input.len() {
        let byte = input.byte_at(cursor)?;
        if byte == delimiter {
            return (digits > 0).then_some((value, cursor + 1));
        }
        let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
        value = value.checked_mul(10)?.checked_add(u64::from(digit))?;
        digits += 1;
        cursor += 1;
    }
    None
}

fn parse_decimal_to_end(input: &impl AsciiHeaderInput, start: usize) -> Option<(u64, usize)> {
    let mut cursor = start;
    let mut value = 0u64;
    let mut digits = 0usize;
    while cursor < input.len() {
        let digit = input
            .byte_at(cursor)?
            .checked_sub(b'0')
            .filter(|digit| *digit <= 9)?;
        value = value.checked_mul(10)?.checked_add(u64::from(digit))?;
        digits += 1;
        cursor += 1;
    }
    (digits > 0).then_some((value, cursor))
}

fn map_visible_status(status: u16) -> Result<(), ChromeRangeError> {
    match status {
        206 => Ok(()),
        200 => Err(ChromeRangeError::RangeUnsupported),
        401 | 403 => Err(ChromeRangeError::AuthorizationRejected),
        404 | 410 => Err(ChromeRangeError::ObjectUnavailable),
        412 => Err(ChromeRangeError::PreconditionFailed),
        416 => Err(ChromeRangeError::RangeNotSatisfiable),
        status => Err(ChromeRangeError::UnexpectedHttpStatus(status)),
    }
}

/// Object length plus the atomically-bound validator/consistency capability for one Range session.
///
/// The opening layer adds the session identity before publishing a complete `BoundRemoteObject`.
pub struct BoundChromeRangeObject {
    object_length: NonZeroU64,
    representation: BoundRepresentationConsistency,
}

/// A byte range checked against, and borrowing, one bound object capability.
#[derive(Clone, Copy)]
pub struct BoundChromeRangeRequest<'object> {
    object: &'object BoundChromeRangeObject,
    requested: RequestedRange,
}

impl BoundChromeRangeRequest<'_> {
    /// Returns the checked half-open range.
    pub fn as_range(&self) -> Range<u64> {
        self.requested.as_exclusive_range()
    }

    /// Returns the exact object capability against which this range was checked.
    pub fn object(&self) -> &BoundChromeRangeObject {
        self.object
    }
}

impl BoundChromeRangeObject {
    pub const fn object_length(&self) -> NonZeroU64 {
        self.object_length
    }

    pub fn consistency(&self) -> RepresentationConsistency {
        self.representation.consistency()
    }

    pub fn validator(&self) -> &RemoteObjectValidator {
        self.representation.validator()
    }

    /// Checks a non-empty request range against the frozen object length before a browser request
    /// owner or `AbortController` is created.
    pub fn request_range(
        &self,
        range: Range<u64>,
    ) -> Result<BoundChromeRangeRequest<'_>, ChromeRangeError> {
        let requested = RequestedRange::new(range)?;
        if requested.end_inclusive >= self.object_length.get() {
            return Err(ChromeRangeError::InvalidRequestRange);
        }
        Ok(BoundChromeRangeRequest {
            object: self,
            requested,
        })
    }
}

impl fmt::Debug for BoundChromeRangeObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundChromeRangeObject")
            .field("object_length", &self.object_length)
            .field("representation", &self.representation)
            .finish()
    }
}

fn map_validator_error(error: RemoteValidatorError) -> ChromeRangeError {
    match error {
        RemoteValidatorError::ResourceLimit | RemoteValidatorError::SizeOverflow => {
            ChromeRangeError::ResourceLimit
        }
        RemoteValidatorError::HeaderAdapterUnavailable | RemoteValidatorError::HeaderRejected => {
            ChromeRangeError::BrowserAdapterUnavailable
        }
        RemoteValidatorError::MultipleHeaderValues
        | RemoteValidatorError::InvalidHeaderValueType
        | RemoteValidatorError::InvalidEntityTag
        | RemoteValidatorError::InvalidWeakPrefix
        | RemoteValidatorError::MultipleOrTrailingEntityTags
        | RemoteValidatorError::ByteStringCodeUnitOutOfRange => {
            ChromeRangeError::InvalidValidatorHeader
        }
    }
}

fn bind_probe_validator(
    object_length: NonZeroU64,
    policy: RepresentationConsistencyPolicy,
    observed: Option<ParsedEntityTag>,
) -> Result<BoundChromeRangeObject, ChromeRangeError> {
    let representation = bind_representation_consistency(policy, observed)
        .map_err(|_error| ChromeRangeError::StrongValidatorRequired)?;
    Ok(BoundChromeRangeObject {
        object_length,
        representation,
    })
}

fn validate_bound_validator(
    bound: &BoundChromeRangeObject,
    observed: Option<ParsedEntityTag>,
) -> Result<(), ChromeRangeError> {
    match (bound.validator(), observed) {
        (RemoteObjectValidator::Strong(_), None) => {
            Err(ChromeRangeError::RequiredResponseHeaderUnavailable(
                RequiredRangeResponseHeader::EntityTag,
            ))
        }
        (RemoteObjectValidator::Strong(expected), Some(ParsedEntityTag::Strong(observed)))
            if expected.matches(&observed) =>
        {
            Ok(())
        }
        (
            RemoteObjectValidator::DeploymentAssumed {
                observed_weak: Some(expected),
            },
            Some(ParsedEntityTag::Weak(observed)),
        ) if expected.matches(&observed) => Ok(()),
        (
            RemoteObjectValidator::DeploymentAssumed {
                observed_weak: None,
            },
            None,
        ) => Ok(()),
        _ => Err(ChromeRangeError::ObjectChanged),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::fmt;
    use std::future::Future;
    use std::ops::Range;

    use js_sys::{Function, JsString, Reflect};
    use wasm_bindgen::{JsCast as _, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{
        AsciiHeaderInput, BoundChromeRangeObject, BoundChromeRangeRequest, ChromeRangeError,
        ChromeRangeRequest, NonZeroU64, ParsedEntityTag, PreparedChromeRangeIdentity,
        RemoteObjectValidator, RepresentationConsistencyPolicy, RequestedRange,
        RequiredRangeResponseHeader, bind_probe_validator, is_identity_encoding,
        map_validator_error, map_visible_status, parse_content_length, parse_content_range,
        validate_bound_validator,
    };
    use crate::chrome_byob::{
        BoundedPrefixBody, ExactLengthBodyCompletion, ExactLengthByobPumpConfig,
        ExactLengthByobPumpControl, ExactLengthByobPumpControlError, ExactLengthRangeBody,
        PreparedExactLengthRangeBodyPump, prepare_exact_range_body_pump,
        read_bounded_prefix_body_prepared, read_exact_range_body, read_exact_range_body_prepared,
        read_exact_range_body_prepared_with_completion,
    };
    use crate::range_retry::ChromeRangeAttemptAbortController;
    use crate::remote_limits::{
        ActiveRemoteValidatorEgressReservation, RangeResponseAccountingScope,
        WasmModuleLimitAccountingRoot, WorkUnitAccountingScope,
    };
    use crate::remote_validator::PrepareIfMatchHeadersFailure;
    use crate::secret_url::SecretUrl;

    struct JsAsciiHeaderInput<'a>(&'a JsString);

    impl AsciiHeaderInput for JsAsciiHeaderInput<'_> {
        fn len(&self) -> usize {
            self.0.length() as usize
        }

        fn byte_at(&self, index: usize) -> Option<u8> {
            let index = u32::try_from(index).ok()?;
            u8::try_from(self.0.char_code_at(index) as u32).ok()
        }
    }

    struct ChromeRangeRequestOwner {
        request: Option<web_sys::Request>,
        validator_egress: Option<ActiveRemoteValidatorEgressReservation>,
    }

    impl ChromeRangeRequestOwner {
        fn request(&self) -> &web_sys::Request {
            self.request
                .as_ref()
                .expect("strict Range Request owner is live")
        }
    }

    impl Drop for ChromeRangeRequestOwner {
        fn drop(&mut self) {
            drop(self.request.take());
            drop(self.validator_egress.take());
        }
    }

    fn build_request(
        url: &SecretUrl,
        requested: RequestedRange,
        validator: Option<&RemoteObjectValidator>,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        work: &WorkUnitAccountingScope,
    ) -> Result<ChromeRangeRequestOwner, ChromeRangeError> {
        let result = (|| {
            let range_value = format!("bytes={}-{}", requested.start, requested.end_inclusive);
            let build = |headers: &JsValue| {
                install_ascii_header(headers, "Range", &range_value)?;
                construct_request(url, headers, abort_controller)
            };

            if let Some(RemoteObjectValidator::Strong(tag)) = validator {
                let prepared = tag
                    .prepare_if_match_headers(root, work)
                    .map_err(map_validator_error)?;
                let bound = prepared.bind().map_err(|failure| match failure {
                    PrepareIfMatchHeadersFailure::Accounting(error) => map_validator_error(error),
                    PrepareIfMatchHeadersFailure::Installation(failure) => {
                        map_validator_error(failure.error())
                    }
                })?;
                let (request, reservation) = bound.transfer_into_request(build)?;
                Ok(ChromeRangeRequestOwner {
                    request: Some(request),
                    validator_egress: Some(reservation),
                })
            } else {
                let headers = web_sys::Headers::new()
                    .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?;
                let request = build(headers.as_ref())?;
                Ok(ChromeRangeRequestOwner {
                    request: Some(request),
                    validator_egress: None,
                })
            }
        })();
        if result.is_err() {
            abort_controller.abort();
        }
        result
    }

    fn install_ascii_header(
        headers: &JsValue,
        name: &str,
        value: &str,
    ) -> Result<(), ChromeRangeError> {
        headers
            .unchecked_ref::<web_sys::Headers>()
            .set(name, value)
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)
    }

    fn construct_request(
        url: &SecretUrl,
        headers: &JsValue,
        abort_controller: &web_sys::AbortController,
    ) -> Result<web_sys::Request, ChromeRangeError> {
        let init = web_sys::RequestInit::new();
        init.set_method("GET");
        init.set_mode(web_sys::RequestMode::Cors);
        init.set_credentials(web_sys::RequestCredentials::Omit);
        init.set_cache(web_sys::RequestCache::NoStore);
        init.set_redirect(web_sys::RequestRedirect::Error);
        init.set_referrer_policy(web_sys::ReferrerPolicy::NoReferrer);
        init.set_signal(Some(&abort_controller.signal()));
        init.set_headers(headers);
        url.expose_for_request(|url| web_sys::Request::new_with_str_and_init(url, &init))
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)
    }

    async fn finish_fetch_response_unclassified(
        request: ChromeRangeRequestOwner,
        fetch: js_sys::Promise,
        abort_controller: &web_sys::AbortController,
    ) -> Result<web_sys::Response, ChromeRangeError> {
        let response = abort_validation_failure(
            classify_fetch_settlement(JsFuture::from(fetch).await),
            abort_controller,
        )?;
        drop(request);
        let response = abort_validation_failure(cast_fetch_response(response), abort_controller)?;
        if matches!(
            response.type_(),
            web_sys::ResponseType::Opaque
                | web_sys::ResponseType::Opaqueredirect
                | web_sys::ResponseType::Error
        ) {
            abort_controller.abort();
            return Err(ChromeRangeError::BrowserFetchUnavailable);
        }
        Ok(response)
    }

    async fn finish_fetch_response(
        request: ChromeRangeRequestOwner,
        fetch: js_sys::Promise,
        abort_controller: &web_sys::AbortController,
    ) -> Result<web_sys::Response, ChromeRangeError> {
        let response = finish_fetch_response_unclassified(request, fetch, abort_controller).await?;
        if let Err(error) = map_visible_status(response.status()) {
            abort_controller.abort();
            return Err(error);
        }
        Ok(response)
    }

    pub(super) fn classify_fetch_settlement(
        result: Result<JsValue, JsValue>,
    ) -> Result<JsValue, ChromeRangeError> {
        result.map_err(|_error| ChromeRangeError::BrowserFetchUnavailable)
    }

    pub(super) fn cast_fetch_response(
        value: JsValue,
    ) -> Result<web_sys::Response, ChromeRangeError> {
        value
            .dyn_into::<web_sys::Response>()
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)
    }

    fn raw_response_header(
        response: &web_sys::Response,
        name: &str,
    ) -> Result<Option<JsString>, ChromeRangeError> {
        let headers = response.headers();
        let getter = Reflect::get(headers.as_ref(), &JsValue::from_str("get"))
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?
            .dyn_into::<Function>()
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?;
        let value = getter
            .call1(headers.as_ref(), &JsValue::from_str(name))
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?;
        if value.is_null() || value.is_undefined() {
            Ok(None)
        } else if value.is_string() {
            Ok(Some(value.unchecked_into::<JsString>()))
        } else {
            Err(ChromeRangeError::BrowserAdapterUnavailable)
        }
    }

    fn validate_common_headers(
        response: &web_sys::Response,
        requested: RequestedRange,
        bound_length: Option<NonZeroU64>,
    ) -> Result<NonZeroU64, ChromeRangeError> {
        let content_range = raw_response_header(response, "Content-Range")?.ok_or(
            ChromeRangeError::RequiredResponseHeaderUnavailable(
                RequiredRangeResponseHeader::ContentRange,
            ),
        )?;
        let parsed = parse_content_range(&JsAsciiHeaderInput(&content_range))
            .ok_or(ChromeRangeError::InvalidContentRange)?;
        if parsed.start != requested.start || parsed.end_inclusive != requested.end_inclusive {
            return Err(ChromeRangeError::InvalidContentRange);
        }
        if bound_length.is_some_and(|bound| bound != parsed.total) {
            return Err(ChromeRangeError::ObjectChanged);
        }

        if let Some(content_length) = raw_response_header(response, "Content-Length")? {
            let visible = parse_content_length(&JsAsciiHeaderInput(&content_length))
                .ok_or(ChromeRangeError::InvalidContentLength)?;
            if visible != requested.expected_bytes.get() {
                return Err(ChromeRangeError::InvalidContentLength);
            }
        }
        if let Some(content_encoding) = raw_response_header(response, "Content-Encoding")?
            && !is_identity_encoding(&JsAsciiHeaderInput(&content_encoding))
        {
            return Err(ChromeRangeError::UnsupportedContentEncoding);
        }
        Ok(parsed.total)
    }

    fn validate_full_sniff_headers(
        response: &web_sys::Response,
        root: &WasmModuleLimitAccountingRoot,
        work: &WorkUnitAccountingScope,
    ) -> Result<(), ChromeRangeError> {
        if let Some(content_length) = raw_response_header(response, "Content-Length")? {
            parse_content_length(&JsAsciiHeaderInput(&content_length))
                .ok_or(ChromeRangeError::InvalidContentLength)?;
        }
        if let Some(content_encoding) = raw_response_header(response, "Content-Encoding")?
            && !is_identity_encoding(&JsAsciiHeaderInput(&content_encoding))
        {
            return Err(ChromeRangeError::UnsupportedContentEncoding);
        }
        drop(parse_response_validator(response, root, work)?);
        Ok(())
    }

    fn parse_response_validator(
        response: &web_sys::Response,
        root: &WasmModuleLimitAccountingRoot,
        work: &WorkUnitAccountingScope,
    ) -> Result<Option<ParsedEntityTag>, ChromeRangeError> {
        let value = raw_response_header(response, "ETag")?;
        match value {
            None => crate::remote_validator::parse_chrome_entity_tag(&[], root, work),
            Some(value) => {
                crate::remote_validator::parse_chrome_entity_tag(&[value.into()], root, work)
            }
        }
        .map_err(map_validator_error)
    }

    fn abort_validation_failure<T>(
        result: Result<T, ChromeRangeError>,
        abort_controller: &web_sys::AbortController,
    ) -> Result<T, ChromeRangeError> {
        if result.is_err() {
            abort_controller.abort();
        }
        result
    }

    struct PreparedChromeRangeTransport {
        requested: RequestedRange,
        request: ChromeRangeRequestOwner,
        body_pump: PreparedExactLengthRangeBodyPump,
        window: web_sys::Window,
        abort_controller: web_sys::AbortController,
        root: WasmModuleLimitAccountingRoot,
        work_scope: WorkUnitAccountingScope,
    }

    struct BorrowedPumpControl<'control, C>(&'control mut C);

    #[async_trait::async_trait(?Send)]
    impl<C> ExactLengthByobPumpControl for BorrowedPumpControl<'_, C>
    where
        C: ExactLengthByobPumpControl,
    {
        async fn wait_until_read_allowed(&mut self) -> Result<(), ExactLengthByobPumpControlError> {
            self.0.wait_until_read_allowed().await
        }
    }

    impl fmt::Debug for PreparedChromeRangeTransport {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("PreparedChromeRangeTransport")
                .field("requested", &self.requested)
                .finish_non_exhaustive()
        }
    }

    fn prepare_transport(
        url: &SecretUrl,
        requested: RequestedRange,
        validator: Option<&RemoteObjectValidator>,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        pump_slice_bytes: NonZeroU64,
    ) -> Result<PreparedChromeRangeTransport, ChromeRangeError> {
        let result = (|| {
            let window = web_sys::window().ok_or(ChromeRangeError::BrowserAdapterUnavailable)?;
            let config =
                ExactLengthByobPumpConfig::new(requested.as_exclusive_range(), pump_slice_bytes)
                    .map_err(ChromeRangeError::BodyPump)?;
            let request = build_request(
                url,
                requested,
                validator,
                abort_controller,
                root,
                work_scope,
            )?;
            let body_pump = prepare_exact_range_body_pump(root, range_scope, work_scope, config)
                .map_err(ChromeRangeError::BodyPump)?;
            Ok(PreparedChromeRangeTransport {
                requested,
                request,
                body_pump,
                window,
                abort_controller: abort_controller.clone(),
                root: root.clone(),
                work_scope: work_scope.clone(),
            })
        })();
        if result.is_err() {
            abort_controller.abort();
        }
        result
    }

    /// Sealed pre-Fetch owner for a strict metadata probe.
    pub(crate) struct PreparedChromeProbeRangeAttempt {
        transport: PreparedChromeRangeTransport,
        consistency_policy: RepresentationConsistencyPolicy,
    }

    /// Response shape accepted only by the strict extensionless format sniffer.
    pub(crate) enum ChromeFormatSniffTransportOutcome {
        PartialContent(ProbedChromeRangeBody),
        FullContent(BoundedPrefixBody),
    }

    impl fmt::Debug for ChromeFormatSniffTransportOutcome {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::PartialContent(_) => formatter
                    .debug_tuple("PartialContent")
                    .field(&"<validated>")
                    .finish(),
                Self::FullContent(prefix) => {
                    formatter.debug_tuple("FullContent").field(prefix).finish()
                }
            }
        }
    }

    /// Sealed pre-Fetch owner for the strict-only extensionless 8-byte probe.
    pub(crate) struct PreparedChromeFormatSniffRangeAttempt {
        transport: PreparedChromeRangeTransport,
        consistency_policy: RepresentationConsistencyPolicy,
    }

    impl PreparedChromeRangeIdentity<ChromeRangeAttemptAbortController>
        for PreparedChromeFormatSniffRangeAttempt
    {
        fn prepared_range(&self) -> ChromeRangeRequest {
            ChromeRangeRequest {
                requested: self.transport.requested,
            }
        }

        fn matches_abort_controller(&self, controller: &ChromeRangeAttemptAbortController) -> bool {
            js_sys::Object::is(
                self.transport.abort_controller.as_ref(),
                controller.controller().as_ref(),
            )
        }

        fn prepared_accounting_binding(
            &self,
        ) -> crate::remote_limits::RangeAttemptAccountingBinding {
            self.transport.body_pump.accounting_binding()
        }
    }

    /// Prepares the strict-only extensionless sniffer without starting Fetch.
    #[expect(
        clippy::too_many_arguments,
        reason = "the transport boundary keeps every capability and policy input explicit"
    )]
    pub(crate) fn prepare_format_sniff_range_attempt(
        url: &SecretUrl,
        request_range: ChromeRangeRequest,
        consistency_policy: RepresentationConsistencyPolicy,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        pump_slice_bytes: NonZeroU64,
    ) -> Result<PreparedChromeFormatSniffRangeAttempt, ChromeRangeError> {
        Ok(PreparedChromeFormatSniffRangeAttempt {
            transport: prepare_transport(
                url,
                request_range.requested,
                None,
                abort_controller,
                root,
                range_scope,
                work_scope,
                pump_slice_bytes,
            )?,
            consistency_policy,
        })
    }

    impl PreparedChromeFormatSniffRangeAttempt {
        /// Starts the one physical sniff Fetch after retry accounting has committed.
        pub(crate) fn start<T, C>(
            self,
            mut control: C,
            timeout: T,
        ) -> impl Future<Output = Result<ChromeFormatSniffTransportOutcome, ChromeRangeError>> + use<T, C>
        where
            T: Future<Output = ()>,
            C: ExactLengthByobPumpControl,
        {
            let Self {
                transport,
                consistency_policy,
            } = self;
            let PreparedChromeRangeTransport {
                requested,
                request,
                body_pump,
                window,
                abort_controller,
                root,
                work_scope,
            } = transport;
            let fetch = window.fetch_with_request(request.request());
            async move {
                let response =
                    finish_fetch_response_unclassified(request, fetch, &abort_controller).await?;
                match response.status() {
                    206 => {
                        let object = abort_validation_failure(
                            (|| {
                                let object_length =
                                    validate_common_headers(&response, requested, None)?;
                                let observed =
                                    parse_response_validator(&response, &root, &work_scope)?;
                                bind_probe_validator(object_length, consistency_policy, observed)
                            })(),
                            &abort_controller,
                        )?;
                        let body = read_exact_range_body_prepared_with_completion(
                            &response,
                            &abort_controller,
                            body_pump,
                            &mut control,
                            timeout,
                            |body| {
                                if body.as_slice() == super::MCAP_MAGIC_BYTES {
                                    ExactLengthBodyCompletion::ReleaseReader
                                } else {
                                    ExactLengthBodyCompletion::CancelAndAbort
                                }
                            },
                        )
                        .await
                        .map_err(ChromeRangeError::BodyPump)?;
                        Ok(ChromeFormatSniffTransportOutcome::PartialContent(
                            ProbedChromeRangeBody { body, object },
                        ))
                    }
                    200 => {
                        abort_validation_failure(
                            validate_full_sniff_headers(&response, &root, &work_scope),
                            &abort_controller,
                        )?;
                        let prefix = read_bounded_prefix_body_prepared(
                            &response,
                            &abort_controller,
                            body_pump,
                            &mut control,
                            timeout,
                        )
                        .await
                        .map_err(ChromeRangeError::BodyPump)?;
                        Ok(ChromeFormatSniffTransportOutcome::FullContent(prefix))
                    }
                    status => {
                        abort_controller.abort();
                        Err(map_visible_status(status)
                            .expect_err("format sniffer handles only visible 200/206 success"))
                    }
                }
            }
        }
    }

    impl fmt::Debug for PreparedChromeProbeRangeAttempt {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("PreparedChromeProbeRangeAttempt")
                .field("transport", &self.transport)
                .field("consistency_policy", &self.consistency_policy)
                .finish_non_exhaustive()
        }
    }

    impl PreparedChromeRangeIdentity<ChromeRangeAttemptAbortController>
        for PreparedChromeProbeRangeAttempt
    {
        fn prepared_range(&self) -> ChromeRangeRequest {
            ChromeRangeRequest {
                requested: self.transport.requested,
            }
        }

        fn matches_abort_controller(&self, controller: &ChromeRangeAttemptAbortController) -> bool {
            js_sys::Object::is(
                self.transport.abort_controller.as_ref(),
                controller.controller().as_ref(),
            )
        }

        fn prepared_accounting_binding(
            &self,
        ) -> crate::remote_limits::RangeAttemptAccountingBinding {
            self.transport.body_pump.accounting_binding()
        }
    }

    /// Completes all fallible Request/Header/output/reservation preparation without starting Fetch.
    #[expect(
        clippy::too_many_arguments,
        reason = "the transport boundary keeps every capability and policy input explicit"
    )]
    pub(crate) fn prepare_probe_exact_range_attempt(
        url: &SecretUrl,
        request_range: ChromeRangeRequest,
        consistency_policy: RepresentationConsistencyPolicy,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        pump_slice_bytes: NonZeroU64,
    ) -> Result<PreparedChromeProbeRangeAttempt, ChromeRangeError> {
        Ok(PreparedChromeProbeRangeAttempt {
            transport: prepare_transport(
                url,
                request_range.requested,
                None,
                abort_controller,
                root,
                range_scope,
                work_scope,
                pump_slice_bytes,
            )?,
            consistency_policy,
        })
    }

    impl PreparedChromeProbeRangeAttempt {
        /// Consumes the sealed owner and synchronously crosses the irreversible Fetch boundary.
        pub(crate) fn start<T, C>(
            self,
            mut control: C,
            timeout: T,
        ) -> impl Future<Output = Result<ProbedChromeRangeBody, ChromeRangeError>>
        where
            T: Future<Output = ()>,
            C: ExactLengthByobPumpControl,
        {
            let Self {
                transport,
                consistency_policy,
            } = self;
            let PreparedChromeRangeTransport {
                requested,
                request,
                body_pump,
                window,
                abort_controller,
                root,
                work_scope,
            } = transport;
            // This is the single irreversible transport boundary. All typed failures and generic
            // ownership preparation completed before the caller committed its attempt burn.
            let fetch = window.fetch_with_request(request.request());
            async move {
                let response = finish_fetch_response(request, fetch, &abort_controller).await?;
                let object = abort_validation_failure(
                    (|| {
                        let object_length = validate_common_headers(&response, requested, None)?;
                        let observed = parse_response_validator(&response, &root, &work_scope)?;
                        bind_probe_validator(object_length, consistency_policy, observed)
                    })(),
                    &abort_controller,
                )?;
                let body = read_exact_range_body_prepared(
                    &response,
                    &abort_controller,
                    body_pump,
                    &mut control,
                    timeout,
                )
                .await
                .map_err(ChromeRangeError::BodyPump)?;
                Ok(ProbedChromeRangeBody { body, object })
            }
        }
    }

    /// Sealed pre-Fetch owner for a Range against one already-bound object capability.
    pub(crate) struct PreparedChromeBoundRangeAttempt<'object> {
        transport: PreparedChromeRangeTransport,
        object: &'object BoundChromeRangeObject,
    }

    impl PreparedChromeRangeIdentity<ChromeRangeAttemptAbortController>
        for PreparedChromeBoundRangeAttempt<'_>
    {
        fn prepared_range(&self) -> ChromeRangeRequest {
            ChromeRangeRequest {
                requested: self.transport.requested,
            }
        }

        fn matches_abort_controller(&self, controller: &ChromeRangeAttemptAbortController) -> bool {
            js_sys::Object::is(
                self.transport.abort_controller.as_ref(),
                controller.controller().as_ref(),
            )
        }

        fn prepared_accounting_binding(
            &self,
        ) -> crate::remote_limits::RangeAttemptAccountingBinding {
            self.transport.body_pump.accounting_binding()
        }
    }

    /// Completes all fallible preparation for a bound Range without starting Fetch.
    pub(crate) fn prepare_bound_exact_range_attempt<'object>(
        url: &SecretUrl,
        request_range: BoundChromeRangeRequest<'object>,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        pump_slice_bytes: NonZeroU64,
    ) -> Result<PreparedChromeBoundRangeAttempt<'object>, ChromeRangeError> {
        let BoundChromeRangeRequest { object, requested } = request_range;
        Ok(PreparedChromeBoundRangeAttempt {
            transport: prepare_transport(
                url,
                requested,
                Some(object.validator()),
                abort_controller,
                root,
                range_scope,
                work_scope,
                pump_slice_bytes,
            )?,
            object,
        })
    }

    impl<'object> PreparedChromeBoundRangeAttempt<'object> {
        /// Consumes the sealed owner and synchronously crosses the irreversible Fetch boundary.
        pub(crate) fn start<T, C>(
            self,
            mut control: C,
            timeout: T,
        ) -> impl Future<Output = Result<ExactLengthRangeBody, ChromeRangeError>> + 'object
        where
            T: Future<Output = ()> + 'object,
            C: ExactLengthByobPumpControl + 'object,
        {
            let Self { transport, object } = self;
            let PreparedChromeRangeTransport {
                requested,
                request,
                body_pump,
                window,
                abort_controller,
                root,
                work_scope,
            } = transport;
            // See the probe path: this call is intentionally synchronous and infallible at the
            // Rust type boundary after the prepared owner has been consumed.
            let fetch = window.fetch_with_request(request.request());
            async move {
                let response = finish_fetch_response(request, fetch, &abort_controller).await?;
                abort_validation_failure(
                    (|| {
                        validate_common_headers(
                            &response,
                            requested,
                            Some(object.object_length()),
                        )?;
                        let observed = parse_response_validator(&response, &root, &work_scope)?;
                        validate_bound_validator(object, observed)
                    })(),
                    &abort_controller,
                )?;
                read_exact_range_body_prepared(
                    &response,
                    &abort_controller,
                    body_pump,
                    &mut control,
                    timeout,
                )
                .await
                .map_err(ChromeRangeError::BodyPump)
            }
        }
    }

    /// A successfully probed body and the validator/length capability bound by its headers.
    pub struct ProbedChromeRangeBody {
        body: ExactLengthRangeBody,
        object: BoundChromeRangeObject,
    }

    impl ProbedChromeRangeBody {
        pub fn body(&self) -> &ExactLengthRangeBody {
            &self.body
        }

        pub fn object(&self) -> &BoundChromeRangeObject {
            &self.object
        }

        pub fn into_parts(self) -> (ExactLengthRangeBody, BoundChromeRangeObject) {
            (self.body, self.object)
        }
    }

    /// Performs one strict probe Range and atomically binds its object length and validator.
    #[expect(
        clippy::too_many_arguments,
        reason = "the transport boundary keeps every capability and policy input explicit"
    )]
    pub async fn probe_exact_range<T, C>(
        url: &SecretUrl,
        request_range: ChromeRangeRequest,
        consistency_policy: RepresentationConsistencyPolicy,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        control: &mut C,
        timeout: T,
        pump_slice_bytes: NonZeroU64,
    ) -> Result<ProbedChromeRangeBody, ChromeRangeError>
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        let prepared = prepare_probe_exact_range_attempt(
            url,
            request_range,
            consistency_policy,
            abort_controller,
            root,
            range_scope,
            work_scope,
            pump_slice_bytes,
        )?;
        prepared.start(BorrowedPumpControl(control), timeout).await
    }

    /// Performs one strict Range against an already-bound object capability.
    #[expect(
        clippy::too_many_arguments,
        reason = "the transport boundary keeps every capability and policy input explicit"
    )]
    pub async fn read_bound_exact_range<T, C>(
        url: &SecretUrl,
        request_range: BoundChromeRangeRequest<'_>,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        control: &mut C,
        timeout: T,
        pump_slice_bytes: NonZeroU64,
    ) -> Result<ExactLengthRangeBody, ChromeRangeError>
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        let prepared = prepare_bound_exact_range_attempt(
            url,
            request_range,
            abort_controller,
            root,
            range_scope,
            work_scope,
            pump_slice_bytes,
        )?;
        prepared.start(BorrowedPumpControl(control), timeout).await
    }

    #[cfg(rerun_mcap_phase_a_proof_v1)]
    pub struct PhaseAExactRangeMeasurementV1 {
        body: ExactLengthRangeBody,
        retained_high_water_bytes: u64,
        overflowed: bool,
    }

    #[cfg(rerun_mcap_phase_a_proof_v1)]
    impl PhaseAExactRangeMeasurementV1 {
        pub fn into_parts_v1(self) -> (ExactLengthRangeBody, u64, bool) {
            (self.body, self.retained_high_water_bytes, self.overflowed)
        }
    }

    #[cfg(rerun_mcap_phase_a_proof_v1)]
    pub async fn fetch_exact_phase_a_measurement_v1(
        url: &str,
        range: Range<u64>,
    ) -> Result<PhaseAExactRangeMeasurementV1, ChromeRangeError> {
        struct AlwaysVisible;

        #[async_trait::async_trait(?Send)]
        impl ExactLengthByobPumpControl for AlwaysVisible {
            async fn wait_until_read_allowed(
                &mut self,
            ) -> Result<(), ExactLengthByobPumpControlError> {
                Ok(())
            }
        }

        let requested = RequestedRange::new(range.clone())?;
        let root =
            crate::remote_limits::ProductionWebRemoteLimitsV1::start_phase_a_measurement_root_v1()
                .map_err(|_error| ChromeRangeError::ResourceLimit)?;
        let viewer = root
            .create_viewer_scope()
            .map_err(|_error| ChromeRangeError::ResourceLimit)?;
        let source = viewer
            .create_source_scope()
            .map_err(|_error| ChromeRangeError::ResourceLimit)?;
        let session = source
            .create_session_scope()
            .map_err(|_error| ChromeRangeError::ResourceLimit)?;
        let range_scope = session
            .create_range_response_scope()
            .map_err(|_error| ChromeRangeError::ResourceLimit)?;
        let work_scope = range_scope
            .create_work_unit_scope()
            .map_err(|_error| ChromeRangeError::ResourceLimit)?;

        let headers = web_sys::Headers::new()
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?;
        headers
            .set(
                "Range",
                &format!("bytes={}-{}", requested.start, requested.end_inclusive),
            )
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?;
        let init = web_sys::RequestInit::new();
        init.set_method("GET");
        init.set_mode(web_sys::RequestMode::Cors);
        init.set_headers(&headers);
        let request = web_sys::Request::new_with_str_and_init(url, &init)
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?;
        let abort_controller = web_sys::AbortController::new()
            .map_err(|_error| ChromeRangeError::BrowserAdapterUnavailable)?;
        let response = JsFuture::from(
            web_sys::window()
                .ok_or(ChromeRangeError::BrowserAdapterUnavailable)?
                .fetch_with_request(&request),
        )
        .await
        .map_err(|_error| ChromeRangeError::BrowserFetchUnavailable)?
        .dyn_into::<web_sys::Response>()
        .map_err(|_error| ChromeRangeError::BrowserFetchUnavailable)?;
        map_visible_status(response.status())?;
        let content_range = raw_response_header(&response, "Content-Range")?.ok_or(
            ChromeRangeError::RequiredResponseHeaderUnavailable(
                RequiredRangeResponseHeader::ContentRange,
            ),
        )?;
        let parsed =
            parse_content_range(&content_range).ok_or(ChromeRangeError::InvalidContentRange)?;
        if parsed.start != requested.start || parsed.end_inclusive != requested.end_inclusive {
            return Err(ChromeRangeError::InvalidContentRange);
        }
        let content_length = raw_response_header(&response, "Content-Length")?
            .and_then(|value| parse_content_length(&value))
            .ok_or(ChromeRangeError::InvalidContentLength)?;
        if content_length != requested.expected_bytes.get() {
            return Err(ChromeRangeError::InvalidContentLength);
        }
        let config = ExactLengthByobPumpConfig::new(
            range,
            NonZeroU64::new(64 * 1024).expect("the proof BYOB slice is non-zero"),
        )
        .map_err(ChromeRangeError::BodyPump)?;
        let body = read_exact_range_body(
            &response,
            &abort_controller,
            &root,
            &range_scope,
            &work_scope,
            config,
            &mut AlwaysVisible,
            std::future::pending(),
        )
        .await
        .map_err(ChromeRangeError::BodyPump)?;
        let (retained_high_water_bytes, overflowed) = root.phase_a_byte_ledger_v1();
        Ok(PhaseAExactRangeMeasurementV1 {
            body,
            retained_high_water_bytes,
            overflowed,
        })
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::*;

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use std::future;
    use std::num::NonZeroU64;

    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{
        ChromeRangeError, ChromeRangeRequest, RepresentationConsistency,
        RepresentationConsistencyPolicy, RequiredRangeResponseHeader,
        prepare_probe_exact_range_attempt, probe_exact_range, read_bound_exact_range,
    };
    use crate::range_retry::{
        ChromeRangeAttemptAbortController, ChromeRangeAttemptAbortFactory,
        MetadataOpeningFailureSignal, MetadataOpeningRetryCoordinator,
        RangeAttemptCompletionDisposition, RangeAttemptStartError, RemoteRangeLiveIdentity,
        RemoteRangeRetryOperation,
    };
    use crate::remote_limits::WebRemoteLimitKey;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
        const RANGE_FIXTURE_PROTOCOL = "mcap-range-fixture-v1";
        let requestInstrumentation;
        let bodyInstrumentation;
        let preparationFailureInstrumentation;

        function fixturePort() {
            const raw = new URLSearchParams(window.location.search).get("mcap_fixture_page_port");
            if (raw === null || !/^\d+$/.test(raw)) {
                throw new Error("missing Range fixture port");
            }
            const port = Number(raw);
            if (!Number.isSafeInteger(port) || port <= 0 || port > 65_535) {
                throw new Error("invalid Range fixture port");
            }
            return port;
        }

        function baseSpec() {
            return {
                object_length: 64,
                object_seed: 10,
                request: {
                    range: { mode: "exact", value: "bytes=0-3" },
                    if_match: { mode: "absent" },
                    head: "mirror_get",
                },
                status: 206,
                content_range: { type: "fixed", start: 0, end: 3, total: 64 },
                content_length: { type: "body_length" },
                content_encoding: "omit",
                etag: { type: "strong", value: "v1" },
                cors: {
                    allow_origin: "any",
                    preflight: "allow_required_headers",
                    expose: "required_range_headers",
                },
                redirect: "none",
                csp_connect: "self_and_object_origin",
                body: {
                    type: "finite",
                    chunks: [{ length: 1 }, { length: 1 }, { length: 1 }, { length: 1 }],
                },
                service_worker: { type: "disabled" },
                response_revisions: [],
            };
        }

        function scenarioSpec(kind, status) {
            const spec = baseSpec();
            if (status !== 0) {
                spec.status = status;
                spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
            }
            switch (kind) {
                case "exact_strong":
                    break;
                case "strong_read":
                    spec.request.if_match = { mode: "exact", value: "\"v1\"" };
                    break;
                case "weak":
                    spec.etag = { type: "weak", value: "v1" };
                    break;
                case "absent":
                    spec.etag = { type: "absent" };
                    break;
                case "content_range_omit":
                    spec.content_range = { type: "omit" };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "content_range_wrong":
                    spec.content_range = { type: "fixed", start: 1, end: 4, total: 64 };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "content_length_wrong":
                    spec.content_length = { type: "fixed", value: 5 };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "gzip":
                    spec.content_encoding = "gzip";
                    spec.cors.expose = "required_range_headers_and_content_encoding";
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "etag_invalid":
                    spec.etag = { type: "raw", value: "not-an-entity-tag" };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "etag_duplicate":
                    spec.etag = { type: "duplicate", value: ["\"v1\"", "\"v2\""] };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "etag_hidden":
                    spec.cors.expose = "content_range_only";
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "total_changed":
                    spec.content_range = { type: "fixed", start: 0, end: 3, total: 65 };
                    spec.request.if_match = { mode: "exact", value: "\"v1\"" };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "etag_changed":
                    spec.etag = { type: "strong", value: "v2" };
                    spec.request.if_match = { mode: "exact", value: "\"v1\"" };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "etag_shape_changed":
                    spec.etag = { type: "weak", value: "v1" };
                    spec.request.if_match = { mode: "exact", value: "\"v1\"" };
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "bound_etag_hidden":
                    spec.request.if_match = { mode: "exact", value: "\"v1\"" };
                    spec.cors.expose = "content_range_only";
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "cors_reject":
                    spec.cors.allow_origin = "omit";
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                case "redirect":
                    spec.redirect = "same_origin_temporary";
                    spec.body = { type: "infinite", chunk: { length: 1, delay_ms: 1 } };
                    break;
                default:
                    if (status === 0) {
                        throw new Error("unknown Range scenario");
                    }
            }
            return spec;
        }

        export function isChromeRangeRuntime() {
            return /(?:Chrome|Chromium)/.test(navigator.userAgent);
        }

        export async function registerRangeScenario(kind, status) {
            const response = await fetch(
                `http://127.0.0.1:${fixturePort()}/__mcap_range_fixture/v1/bootstrap`,
                { cache: "no-store" },
            );
            const bootstrap = await response.json();
            if (bootstrap.protocol !== RANGE_FIXTURE_PROTOCOL) {
                throw new Error("Range fixture protocol mismatch");
            }
            const controlBase = `${bootstrap.page_origin}${bootstrap.control_root}/control/scenarios`;
            const registered = await fetch(controlBase, {
                method: "POST",
                cache: "no-store",
                headers: { "content-type": "application/json" },
                body: JSON.stringify(scenarioSpec(kind, status)),
            });
            if (!registered.ok) {
                throw new Error("Range fixture registration failed");
            }
            const descriptor = await registered.json();
            return {
                crossOriginUrl: descriptor.cross_origin_object_url,
                pageUrl: descriptor.page_url,
                snapshotUrl: `${controlBase}/${descriptor.id}`,
                cleanupUrl: `${controlBase}/${descriptor.id}`,
            };
        }

        export function inspectSameOriginBasic(pageUrl) {
            return new Promise((resolve, reject) => {
                const pageOrigin = new URL(pageUrl).origin;
                const requestId = `same-origin-basic-${Date.now()}-${Math.random()}`;
                const iframe = document.createElement("iframe");
                iframe.hidden = true;
                let timeout;
                const cleanup = () => {
                    window.removeEventListener("message", onMessage);
                    if (timeout !== undefined) {
                        window.clearTimeout(timeout);
                    }
                    iframe.remove();
                };
                const onMessage = (message) => {
                    if (
                        message.origin !== pageOrigin
                        || message.data?.fixture !== "mcap-range-v1"
                        || message.data?.requestId !== requestId
                    ) {
                        return;
                    }
                    cleanup();
                    if (!message.data.ok) {
                        reject(new Error("same-origin fixture inspection failed"));
                        return;
                    }
                    const result = message.data.result;
                    resolve(
                        result.status === 206
                        && result.responseType === "basic"
                        && result.contentEncoding === "gzip",
                    );
                };
                window.addEventListener("message", onMessage);
                timeout = window.setTimeout(() => {
                    cleanup();
                    reject(new Error("same-origin fixture inspection timed out"));
                }, 5000);
                iframe.addEventListener("load", () => {
                    iframe.contentWindow.postMessage({
                        fixture: "mcap-range-v1",
                        requestId,
                        scenarioId: Number(new URL(pageUrl).pathname.split("/").at(-1)),
                        command: "inspectResponse",
                    }, pageOrigin);
                }, { once: true });
                iframe.src = pageUrl;
                document.body.appendChild(iframe);
            });
        }

        export async function deleteRangeScenario(url) {
            const response = await fetch(url, { method: "DELETE", cache: "no-store" });
            if (!response.ok) {
                throw new Error("Range fixture cleanup failed");
            }
        }

        export async function rangeScenarioObservedCancellation(url) {
            for (let attempt = 0; attempt < 80; attempt += 1) {
                const response = await fetch(url, { cache: "no-store" });
                const snapshot = await response.json();
                if (snapshot.events.some((event) => event.type === "body_cancelled")) {
                    return true;
                }
                await new Promise((resolve) => window.setTimeout(resolve, 25));
            }
            return false;
        }

        export function beginRangeRequestInstrumentation() {
            if (requestInstrumentation !== undefined) {
                throw new Error("Range request instrumentation already active");
            }
            const original = window.fetch;
            const observations = [];
            window.fetch = function(input, init) {
                if (input instanceof Request) {
                    observations.push({
                        method: input.method,
                        mode: input.mode,
                        credentials: input.credentials,
                        cache: input.cache,
                        redirect: input.redirect,
                        referrerPolicy: input.referrerPolicy,
                        range: input.headers.get("Range"),
                        ifMatch: input.headers.get("If-Match"),
                        signal: input.signal,
                        signalWasAborted: input.signal.aborted,
                    });
                }
                return original.call(this, input, init);
            };
            requestInstrumentation = { original, observations };
        }

        export function rangeRequestObservationCount() {
            if (requestInstrumentation === undefined) {
                throw new Error("Range request instrumentation not active");
            }
            return requestInstrumentation.observations.length;
        }

        export function finishRangeRequestInstrumentation(
            expectedIfMatch,
            expectedSignalAborted,
            expectedRequestCount,
        ) {
            if (requestInstrumentation === undefined) {
                throw new Error("Range request instrumentation not active");
            }
            window.fetch = requestInstrumentation.original;
            const observations = requestInstrumentation.observations;
            requestInstrumentation = undefined;
            if (observations.length !== expectedRequestCount) {
                throw new Error("unexpected production Fetch count");
            }
            if (expectedRequestCount === 0) {
                return true;
            }
            const expected = expectedIfMatch === "" ? null : expectedIfMatch;
            return new Set(observations.map((request) => request.signal)).size === observations.length
                && observations.every((request) => request.method === "GET"
                    && request.mode === "cors"
                    && request.credentials === "omit"
                    && request.cache === "no-store"
                    && request.redirect === "error"
                    && request.referrerPolicy === "no-referrer"
                    && request.range === "bytes=0-3"
                    && request.ifMatch === expected
                    && request.signal instanceof AbortSignal
                    && !request.signalWasAborted
                    && request.signal.aborted === expectedSignalAborted);
        }

        export function armRangePreparationFailure(stage) {
            if (preparationFailureInstrumentation !== undefined) {
                throw new Error("Range preparation failure already armed");
            }
            let hits = 0;
            if (stage === "header") {
                const original = Headers.prototype.set;
                Headers.prototype.set = function(name, value) {
                    if (hits === 0 && name === "Range") {
                        hits += 1;
                        throw new TypeError("scripted header failure");
                    }
                    return original.call(this, name, value);
                };
                preparationFailureInstrumentation = {
                    hits: () => hits,
                    restore: () => { Headers.prototype.set = original; },
                };
                return;
            }
            if (stage === "request") {
                const original = window.Request;
                window.Request = function(..._args) {
                    hits += 1;
                    throw new TypeError("scripted Request failure");
                };
                preparationFailureInstrumentation = {
                    hits: () => hits,
                    restore: () => { window.Request = original; },
                };
                return;
            }
            throw new Error("unknown Range preparation failure stage");
        }

        export function finishRangePreparationFailure() {
            if (preparationFailureInstrumentation === undefined) {
                throw new Error("Range preparation failure not armed");
            }
            const state = preparationFailureInstrumentation;
            preparationFailureInstrumentation = undefined;
            state.restore();
            return state.hits() === 1;
        }

        export function beginRangeBodyInstrumentation() {
            if (bodyInstrumentation !== undefined) {
                throw new Error("Range body instrumentation already active");
            }
            const responsePrototype = Response.prototype;
            const readerPrototype = ReadableStreamBYOBReader.prototype;
            const originalArrayBuffer = responsePrototype.arrayBuffer;
            const originalRead = readerPrototype.read;
            const stats = { arrayBuffer: 0, reads: 0 };
            responsePrototype.arrayBuffer = function(...args) {
                stats.arrayBuffer += 1;
                return originalArrayBuffer.apply(this, args);
            };
            readerPrototype.read = function(...args) {
                stats.reads += 1;
                return originalRead.apply(this, args);
            };
            bodyInstrumentation = {
                responsePrototype,
                readerPrototype,
                originalArrayBuffer,
                originalRead,
                stats,
            };
        }

        export function finishRangeBodyInstrumentation() {
            if (bodyInstrumentation === undefined) {
                throw new Error("Range body instrumentation not active");
            }
            const state = bodyInstrumentation;
            state.responsePrototype.arrayBuffer = state.originalArrayBuffer;
            state.readerPrototype.read = state.originalRead;
            bodyInstrumentation = undefined;
            return state.stats.arrayBuffer === 0 && state.stats.reads === 0;
        }
    "#)]
    extern "C" {
        fn is_chrome_range_runtime() -> bool;
        fn register_range_scenario(kind: &str, status: u16) -> js_sys::Promise;
        fn inspect_same_origin_basic(page_url: &str) -> js_sys::Promise;
        fn delete_range_scenario(url: &str) -> js_sys::Promise;
        fn range_scenario_observed_cancellation(url: &str) -> js_sys::Promise;
        fn begin_range_request_instrumentation();
        fn range_request_observation_count() -> u32;
        fn finish_range_request_instrumentation(
            expected_if_match: &str,
            expected_signal_aborted: bool,
            expected_request_count: u32,
        ) -> bool;
        fn arm_range_preparation_failure(stage: &str);
        fn finish_range_preparation_failure() -> bool;
        fn begin_range_body_instrumentation();
        fn finish_range_body_instrumentation() -> bool;
    }

    struct Scenario {
        cross_origin_url: String,
        page_url: String,
        snapshot_url: String,
        cleanup_url: String,
    }

    impl Scenario {
        async fn register(kind: &str, status: u16) -> Self {
            let descriptor = JsFuture::from(register_range_scenario(kind, status))
                .await
                .expect("controlled Range scenario registers");
            Self {
                cross_origin_url: string_field(&descriptor, "crossOriginUrl"),
                page_url: string_field(&descriptor, "pageUrl"),
                snapshot_url: string_field(&descriptor, "snapshotUrl"),
                cleanup_url: string_field(&descriptor, "cleanupUrl"),
            }
        }

        fn cross_origin_secret(&self) -> crate::secret_url::SecretUrl {
            crate::secret_url::SecretUrl::parse_for_test(&self.cross_origin_url)
                .expect("fixture URL is a valid bounded secret URL")
        }

        async fn observed_cancellation(&self) -> bool {
            JsFuture::from(range_scenario_observed_cancellation(&self.snapshot_url))
                .await
                .expect("fixture cancellation snapshot succeeds")
                .as_bool()
                .expect("fixture cancellation result is boolean")
        }

        async fn remove(self) {
            JsFuture::from(delete_range_scenario(&self.cleanup_url))
                .await
                .expect("controlled Range scenario is removed");
        }
    }

    fn string_field(object: &JsValue, field: &str) -> String {
        js_sys::Reflect::get(object, &JsValue::from_str(field))
            .expect("fixture descriptor has fixed fields")
            .as_string()
            .expect("fixture descriptor field is a string")
    }

    struct AlwaysAllowed;

    #[async_trait::async_trait(?Send)]
    impl crate::chrome_byob::ExactLengthByobPumpControl for AlwaysAllowed {
        async fn wait_until_read_allowed(
            &mut self,
        ) -> Result<(), crate::chrome_byob::ExactLengthByobPumpControlError> {
            Ok(())
        }
    }

    struct DownstreamLikeControl;

    // Compile-time compatibility fixture: downstream crates may already implement the public
    // trait specifically for their local `&mut Control` type. A public blanket impl would overlap.
    #[async_trait::async_trait(?Send)]
    impl crate::chrome_byob::ExactLengthByobPumpControl for &mut DownstreamLikeControl {
        async fn wait_until_read_allowed(
            &mut self,
        ) -> Result<(), crate::chrome_byob::ExactLengthByobPumpControlError> {
            Ok(())
        }
    }

    #[test]
    fn downstream_mut_reference_control_impl_remains_coherent() {
        let mut control = DownstreamLikeControl;
        let _borrowed: &mut DownstreamLikeControl = &mut control;
    }

    fn scopes() -> (
        crate::remote_limits::WasmModuleLimitAccountingRoot,
        crate::remote_limits::RangeResponseAccountingScope,
        crate::remote_limits::WorkUnitAccountingScope,
    ) {
        let root = crate::remote_limits::tests::complete_test_profile()
            .start_accounting_root()
            .expect("complete test profile starts");
        let viewer = root
            .create_viewer_scope()
            .expect("viewer scope is available");
        let source = viewer
            .create_source_scope()
            .expect("source scope is available");
        let session = source
            .create_session_scope()
            .expect("session scope is available");
        let range = session
            .create_range_response_scope()
            .expect("range scope is available");
        let work = range
            .create_work_unit_scope()
            .expect("work scope is available");
        (root, range, work)
    }

    async fn probe(
        scenario: &Scenario,
        policy: RepresentationConsistencyPolicy,
    ) -> (
        Result<super::ProbedChromeRangeBody, ChromeRangeError>,
        web_sys::AbortController,
        crate::remote_limits::WasmModuleLimitAccountingRoot,
    ) {
        let (root, range, work) = scopes();
        let controller = web_sys::AbortController::new().expect("Chrome exposes AbortController");
        let url = scenario.cross_origin_secret();
        let request_range = ChromeRangeRequest::new(0..4).expect("test range is valid");
        let mut control = AlwaysAllowed;
        let result = probe_exact_range(
            &url,
            request_range,
            policy,
            &controller,
            &root,
            &range,
            &work,
            &mut control,
            future::pending(),
            NonZeroU64::new(2).expect("test pump slice is non-zero"),
        )
        .await;
        (result, controller, root)
    }

    fn prepare_strict_retry_probe(
        scenario: &Scenario,
        request: ChromeRangeRequest,
        root: &crate::remote_limits::WasmModuleLimitAccountingRoot,
        range: &crate::remote_limits::RangeResponseAccountingScope,
        work: &crate::remote_limits::WorkUnitAccountingScope,
        controller: &ChromeRangeAttemptAbortController,
    ) -> Result<super::PreparedChromeProbeRangeAttempt, ChromeRangeError> {
        let url = scenario.cross_origin_secret();
        prepare_probe_exact_range_attempt(
            &url,
            request,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            controller.controller(),
            root,
            range,
            work,
            NonZeroU64::new(2).expect("test pump slice is non-zero"),
        )
    }

    async fn settle_started_task<Source, Session, Representation, Demand, Controller, Payload>(
        attempt: crate::range_retry::StartedRangeAttempt<
            Source,
            Session,
            Representation,
            Demand,
            Controller,
            Payload,
        >,
    ) -> crate::range_retry::SettledRangeAttempt<
        Source,
        Session,
        Representation,
        Demand,
        Controller,
        Payload::Output,
    >
    where
        Controller: crate::range_retry::RangeAttemptAbortController,
        Payload: std::future::Future,
    {
        let mut attempt = std::pin::pin!(attempt);
        std::future::poll_fn(|context| match attempt.as_mut().poll_settlement(context) {
            std::task::Poll::Ready(result) => {
                std::task::Poll::Ready(result.expect("a matching test task settles exactly once"))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        })
        .await
    }

    async fn exercise_strict_retry_scenario(
        kind: &str,
        status: u16,
        expected: ChromeRangeError,
        should_retry: bool,
    ) {
        let scenario = Scenario::register(kind, status).await;
        let root = crate::remote_limits::tests::test_profile_with(&[
            (
                crate::remote_limits::WebRemoteLimitKey::RangeRetryAttemptsPerOperation,
                2,
            ),
            (
                crate::remote_limits::WebRemoteLimitKey::MetadataOpeningRangeRequests,
                2,
            ),
        ])
        .start_accounting_root()
        .expect("retry test profile starts");
        let viewer = root.create_viewer_scope().expect("viewer scope");
        let source = viewer.create_source_scope().expect("source scope");
        let session = source.create_session_scope().expect("session scope");
        let range = session.create_range_response_scope().expect("range scope");
        let work = range.create_work_unit_scope().expect("work scope");
        let operation_scope = source.create_operation_scope().expect("operation scope");
        let mut issuer =
            crate::range_retry::test_execution_support::TestVisibleExecutionIssuer::new(1);
        let binding = issuer.binding();
        let live = RemoteRangeLiveIdentity::new(1_u64, 1_u64, 1_u64);
        let mut coordinator = MetadataOpeningRetryCoordinator::new(&source, live.clone(), binding)
            .expect("metadata coordinator");
        let mut operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &range,
            &work,
            live,
            ChromeRangeRequest::new(0..4).expect("exact test range"),
            1_u64,
            ChromeRangeAttemptAbortFactory,
        )
        .expect("retry operation");

        begin_range_request_instrumentation();
        let first = operation
            .start_initial(
                &mut coordinator,
                binding,
                |controller, request| {
                    prepare_strict_retry_probe(&scenario, request, &root, &range, &work, controller)
                },
                |_controller, prepared, receipt| {
                    assert_eq!(receipt.attempt().get(), 1);
                    assert_eq!(receipt.metadata_opening_range(), 1);
                    prepared.start(AlwaysAllowed, future::pending())
                },
            )
            .expect("initial strict attempt starts");
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(coordinator.range_count(), 1);
        assert_eq!(range_request_observation_count(), 1);
        let first = settle_started_task(first).await;
        assert_eq!(range_request_observation_count(), 1);
        let crate::range_retry::RangeAttemptTransportCompletion::Failed(disposition) =
            operation.complete_chrome_settlement(&mut coordinator, first)
        else {
            panic!("scripted strict attempt must fail with matching ownership");
        };

        if should_retry {
            assert_eq!(disposition, RangeAttemptCompletionDisposition::RetryPending);
            let external_turn = issuer.turn();
            let mut permit = coordinator
                .project_retry_turn(&external_turn)
                .expect("next external turn projects once");
            let second = operation
                .start_retry_on_turn(
                    &mut coordinator,
                    &mut permit,
                    |controller, request| {
                        prepare_strict_retry_probe(
                            &scenario, request, &root, &range, &work, controller,
                        )
                    },
                    |_controller, prepared, receipt| {
                        assert_eq!(receipt.attempt().get(), 2);
                        assert_eq!(receipt.metadata_opening_range(), 2);
                        prepared.start(AlwaysAllowed, future::pending())
                    },
                )
                .expect("strict retry starts on the next external turn");
            assert_eq!(range_request_observation_count(), 2);
            let second = settle_started_task(second).await;
            assert_eq!(range_request_observation_count(), 2);
            let crate::range_retry::RangeAttemptTransportCompletion::Failed(retry_disposition) =
                operation.complete_chrome_settlement(&mut coordinator, second)
            else {
                panic!("scripted strict retry must fail with matching ownership");
            };
            assert_eq!(
                retry_disposition,
                RangeAttemptCompletionDisposition::RetryPending
            );
            assert!(finish_range_request_instrumentation("", true, 2));
        } else {
            assert_eq!(
                disposition,
                RangeAttemptCompletionDisposition::OpeningFailed(
                    MetadataOpeningFailureSignal::NonRetryable(expected)
                )
            );
            assert!(finish_range_request_instrumentation("", true, 1));
        }
        if status == 503 {
            assert!(scenario.observed_cancellation().await);
        }
        scenario.remove().await;
    }

    async fn exercise_zero_burn_preparation_failure(
        stage: Option<&str>,
        expected: ChromeRangeError,
    ) {
        let scenario = Scenario::register("exact_strong", 0).await;
        let mut overrides = vec![
            (WebRemoteLimitKey::RangeRetryAttemptsPerOperation, 2),
            (WebRemoteLimitKey::MetadataOpeningRangeRequests, 2),
        ];
        if stage.is_none() {
            overrides.push((WebRemoteLimitKey::RequestedRangeBytes, 3));
        }
        let root = crate::remote_limits::tests::test_profile_with(&overrides)
            .start_accounting_root()
            .expect("preparation failure profile starts");
        let viewer = root.create_viewer_scope().expect("viewer scope");
        let source = viewer.create_source_scope().expect("source scope");
        let session = source.create_session_scope().expect("session scope");
        let range = session.create_range_response_scope().expect("range scope");
        let work = range.create_work_unit_scope().expect("work scope");
        let operation_scope = source.create_operation_scope().expect("operation scope");
        let mut issuer =
            crate::range_retry::test_execution_support::TestVisibleExecutionIssuer::new(1);
        let binding = issuer.binding();
        let live = RemoteRangeLiveIdentity::new(1_u64, 1_u64, 1_u64);
        let mut coordinator = MetadataOpeningRetryCoordinator::new(&source, live.clone(), binding)
            .expect("metadata coordinator");
        let mut operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &range,
            &work,
            live,
            ChromeRangeRequest::new(0..4).expect("exact test range"),
            1_u64,
            ChromeRangeAttemptAbortFactory,
        )
        .expect("retry operation");

        begin_range_request_instrumentation();
        if let Some(stage) = stage {
            arm_range_preparation_failure(stage);
        }
        let result = operation.start_initial(
            &mut coordinator,
            binding,
            |controller, request| {
                prepare_strict_retry_probe(&scenario, request, &root, &range, &work, controller)
            },
            |_controller, _prepared, _receipt| {
                panic!("Fetch boundary must remain disarmed after preparation failure")
            },
        );
        let Err(crate::range_retry::RangeAttemptStartFailure::Request(
            RangeAttemptStartError::OpeningFailed(MetadataOpeningFailureSignal::NonRetryable(
                observed,
            )),
        )) = result
        else {
            panic!("preparation failure must be typed and request-local");
        };
        assert_eq!(observed, expected);
        assert_eq!(operation.attempts_burned(), 0);
        assert_eq!(coordinator.range_count(), 0);
        assert_eq!(range_request_observation_count(), 0);
        if stage.is_some() {
            assert!(finish_range_preparation_failure());
        }
        assert!(finish_range_request_instrumentation("", true, 0));
        // The preparation path must not consume a future external retry turn either.
        let turn = issuer.turn();
        let _permit = coordinator
            .project_retry_turn(&turn)
            .expect("zero-burn failure leaves the external turn unclaimed");
        scenario.remove().await;
    }

    #[wasm_bindgen_test]
    async fn strict_preparation_failure_matrix_is_zero_burn_and_zero_fetch() {
        if !is_chrome_range_runtime() {
            return;
        }
        for stage in ["header", "request"] {
            exercise_zero_burn_preparation_failure(
                Some(stage),
                ChromeRangeError::BrowserAdapterUnavailable,
            )
            .await;
        }
        exercise_zero_burn_preparation_failure(
            None,
            ChromeRangeError::BodyPump(
                crate::chrome_byob::ExactLengthByobPumpError::ResourceLimitExceeded,
            ),
        )
        .await;
    }

    #[wasm_bindgen_test]
    async fn strict_controlled_transport_drives_retry_classifier_and_burn_boundary() {
        if !is_chrome_range_runtime() {
            return;
        }
        for status in [429, 500, 502, 503, 504] {
            exercise_strict_retry_scenario(
                "status",
                status,
                ChromeRangeError::UnexpectedHttpStatus(status),
                true,
            )
            .await;
        }
        for status in [501, 505] {
            exercise_strict_retry_scenario(
                "status",
                status,
                ChromeRangeError::UnexpectedHttpStatus(status),
                false,
            )
            .await;
        }
        exercise_strict_retry_scenario(
            "cors_reject",
            0,
            ChromeRangeError::BrowserFetchUnavailable,
            true,
        )
        .await;
    }

    #[wasm_bindgen_test]
    async fn strict_success_then_injected_byob_read_failure_remains_nonretryable() {
        if !is_chrome_range_runtime() {
            return;
        }
        // The controlled fixture cannot force Chrome's BYOB reader promise to reject after a
        // valid strict response. Drive one real production request first, then inject only the
        // typed pump settlement at the retry-classifier boundary.
        let scenario = Scenario::register("exact_strong", 0).await;
        let root = crate::remote_limits::tests::test_profile_with(&[
            (WebRemoteLimitKey::RangeRetryAttemptsPerOperation, 2),
            (WebRemoteLimitKey::MetadataOpeningRangeRequests, 2),
        ])
        .start_accounting_root()
        .expect("BYOB classifier profile starts");
        let viewer = root.create_viewer_scope().expect("viewer scope");
        let source = viewer.create_source_scope().expect("source scope");
        let session = source.create_session_scope().expect("session scope");
        let range = session.create_range_response_scope().expect("range scope");
        let work = range.create_work_unit_scope().expect("work scope");
        let operation_scope = source.create_operation_scope().expect("operation scope");
        let mut issuer =
            crate::range_retry::test_execution_support::TestVisibleExecutionIssuer::new(1);
        let binding = issuer.binding();
        let live = RemoteRangeLiveIdentity::new(1_u64, 1_u64, 1_u64);
        let mut coordinator = MetadataOpeningRetryCoordinator::new(&source, live.clone(), binding)
            .expect("metadata coordinator");
        let mut operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &range,
            &work,
            live,
            ChromeRangeRequest::new(0..4).expect("exact test range"),
            1_u64,
            ChromeRangeAttemptAbortFactory,
        )
        .expect("retry operation");

        begin_range_request_instrumentation();
        let started = operation
            .start_initial(
                &mut coordinator,
                binding,
                |controller, request| {
                    prepare_strict_retry_probe(&scenario, request, &root, &range, &work, controller)
                },
                |_controller, prepared, receipt| {
                    assert_eq!(receipt.attempt().get(), 1);
                    assert_eq!(receipt.metadata_opening_range(), 1);
                    let strict = prepared.start(AlwaysAllowed, future::pending());
                    async move {
                        let probed = strict.await?;
                        drop(probed);
                        Err::<super::ProbedChromeRangeBody, _>(ChromeRangeError::BodyPump(
                            crate::chrome_byob::ExactLengthByobPumpError::ReadFailed,
                        ))
                    }
                },
            )
            .expect("strict request starts");
        assert_eq!(range_request_observation_count(), 1);
        let settled = settle_started_task(started).await;
        assert!(matches!(
            operation.complete_chrome_settlement(&mut coordinator, settled),
            crate::range_retry::RangeAttemptTransportCompletion::Failed(
                RangeAttemptCompletionDisposition::OpeningFailed(
                    MetadataOpeningFailureSignal::NonRetryable(ChromeRangeError::BodyPump(
                        crate::chrome_byob::ExactLengthByobPumpError::ReadFailed,
                    ))
                )
            )
        ));
        let turn = issuer.turn();
        let mut permit = coordinator
            .project_retry_turn(&turn)
            .expect("future visible turn projects after terminal settlement");
        assert!(matches!(
            operation.start_retry_on_turn(
                &mut coordinator,
                &mut permit,
                |_controller, _request| -> Result<super::PreparedChromeProbeRangeAttempt, _> {
                    panic!("terminal BYOB failure must not prepare a retry")
                },
                |_controller, _prepared, _receipt| (),
            ),
            Err(crate::range_retry::RangeAttemptStartFailure::Request(
                RangeAttemptStartError::Protocol(
                    crate::range_retry::RetryProtocolError::OperationAlreadyTerminal
                )
            ))
        ));
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(coordinator.range_count(), 1);
        assert!(finish_range_request_instrumentation("", true, 1));
        scenario.remove().await;
    }

    #[wasm_bindgen_test]
    async fn strict_success_settles_with_output_and_controller_in_one_transition() {
        if !is_chrome_range_runtime() {
            return;
        }
        let scenario = Scenario::register("exact_strong", 0).await;
        let root = crate::remote_limits::tests::test_profile_with(&[
            (WebRemoteLimitKey::RangeRetryAttemptsPerOperation, 1),
            (WebRemoteLimitKey::MetadataOpeningRangeRequests, 1),
        ])
        .start_accounting_root()
        .expect("success settlement profile starts");
        let viewer = root.create_viewer_scope().expect("viewer scope");
        let source = viewer.create_source_scope().expect("source scope");
        let session = source.create_session_scope().expect("session scope");
        let range = session.create_range_response_scope().expect("range scope");
        let work = range.create_work_unit_scope().expect("work scope");
        let operation_scope = source.create_operation_scope().expect("operation scope");
        let issuer = crate::range_retry::test_execution_support::TestVisibleExecutionIssuer::new(1);
        let binding = issuer.binding();
        let live = RemoteRangeLiveIdentity::new(1_u64, 1_u64, 1_u64);
        let mut coordinator = MetadataOpeningRetryCoordinator::new(&source, live.clone(), binding)
            .expect("metadata coordinator");
        let mut operation = RemoteRangeRetryOperation::new(
            &operation_scope,
            &range,
            &work,
            live,
            ChromeRangeRequest::new(0..4).expect("exact test range"),
            1_u64,
            ChromeRangeAttemptAbortFactory,
        )
        .expect("retry operation");

        begin_range_request_instrumentation();
        let started = operation
            .start_initial(
                &mut coordinator,
                binding,
                |controller, request| {
                    prepare_strict_retry_probe(&scenario, request, &root, &range, &work, controller)
                },
                |_controller, prepared, receipt| {
                    assert_eq!(receipt.attempt().get(), 1);
                    assert_eq!(receipt.metadata_opening_range(), 1);
                    prepared.start(AlwaysAllowed, future::pending())
                },
            )
            .expect("strict success starts");
        let settled = settle_started_task(started).await;
        let crate::range_retry::RangeAttemptTransportCompletion::Succeeded(probed) =
            operation.complete_chrome_settlement(&mut coordinator, settled)
        else {
            panic!("successful transport must atomically publish its output");
        };
        assert_eq!(probed.body().as_slice(), &[10, 11, 12, 13]);
        assert_eq!(operation.attempts_burned(), 1);
        assert_eq!(coordinator.range_count(), 1);
        assert!(finish_range_request_instrumentation("", false, 1));
        drop(probed);
        scenario.remove().await;
    }

    #[wasm_bindgen_test]
    fn fetch_rejection_and_resolved_non_response_have_opposite_typed_boundaries() {
        let Err(error) = super::web::classify_fetch_settlement(Err(JsValue::from_str("redacted")))
        else {
            panic!("a rejected Fetch promise must fail transport classification");
        };
        assert_eq!(error, ChromeRangeError::BrowserFetchUnavailable);
        assert!(super::web::classify_fetch_settlement(Ok(JsValue::NULL)).is_ok());
        let Err(error) = super::web::cast_fetch_response(JsValue::NULL) else {
            panic!("a resolved non-Response value must fail the adapter cast");
        };
        assert_eq!(error, ChromeRangeError::BrowserAdapterUnavailable);
        let response = web_sys::Response::new().expect("browser constructs an empty Response");
        assert!(super::web::cast_fetch_response(response.into()).is_ok());
    }

    #[wasm_bindgen_test]
    async fn strict_request_options_probe_and_if_match_read_use_production_transport() {
        if !is_chrome_range_runtime() {
            return;
        }

        let probe_scenario = Scenario::register("exact_strong", 0).await;
        begin_range_request_instrumentation();
        let (probed, controller, root) = probe(
            &probe_scenario,
            RepresentationConsistencyPolicy::RequireStrongValidator,
        )
        .await;
        assert!(finish_range_request_instrumentation("", false, 1));
        let probed = probed.expect("strong probe succeeds");
        assert!(!controller.signal().aborted());
        assert_eq!(probed.body().as_slice(), &[10, 11, 12, 13]);
        assert_eq!(
            probed.object().consistency(),
            RepresentationConsistency::StrongValidator
        );
        assert_eq!(probed.object().object_length().get(), 64);
        let (body, object) = probed.into_parts();
        drop(body);
        probe_scenario.remove().await;

        let read_scenario = Scenario::register("strong_read", 0).await;
        let (read_root, read_range, read_work) = scopes();
        let read_controller =
            web_sys::AbortController::new().expect("Chrome exposes AbortController");
        let read_url = read_scenario.cross_origin_secret();
        let request_range = object.request_range(0..4).expect("test range is valid");
        let mut control = AlwaysAllowed;
        begin_range_request_instrumentation();
        let body = read_bound_exact_range(
            &read_url,
            request_range,
            &read_controller,
            &read_root,
            &read_range,
            &read_work,
            &mut control,
            future::pending(),
            NonZeroU64::new(2).expect("test pump slice is non-zero"),
        )
        .await
        .expect("bound strong Range succeeds");
        assert!(finish_range_request_instrumentation("\"v1\"", false, 1));
        assert_eq!(body.as_slice(), &[10, 11, 12, 13]);
        assert!(!read_controller.signal().aborted());
        drop(body);
        drop(object);
        drop(root);
        read_scenario.remove().await;
    }

    #[wasm_bindgen_test]
    async fn weak_and_absent_probe_never_emit_if_match() {
        if !is_chrome_range_runtime() {
            return;
        }

        for (kind, expected_consistency) in [
            ("weak", RepresentationConsistency::DeploymentAssumed),
            ("absent", RepresentationConsistency::DeploymentAssumed),
        ] {
            let scenario = Scenario::register(kind, 0).await;
            begin_range_request_instrumentation();
            let (result, controller, _root) = probe(
                &scenario,
                RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            )
            .await;
            assert!(finish_range_request_instrumentation("", false, 1));
            let result = result.expect("deployment-assumed probe succeeds");
            assert_eq!(result.object().consistency(), expected_consistency);
            assert!(!controller.signal().aborted());
            drop(result);
            scenario.remove().await;
        }
    }

    #[wasm_bindgen_test]
    async fn invalid_ranges_fail_before_abort_request_or_accounting_ownership_exists() {
        if !is_chrome_range_runtime() {
            return;
        }

        let (root, _range, _work) = scopes();
        let baseline = root.accounting_scalar_snapshot();
        begin_range_request_instrumentation();
        begin_range_body_instrumentation();
        for range in [0..0, std::ops::Range { start: 8, end: 7 }] {
            assert_eq!(
                ChromeRangeRequest::new(range),
                Err(ChromeRangeError::InvalidRequestRange)
            );
        }
        assert_eq!(root.accounting_scalar_snapshot(), baseline);
        assert!(finish_range_request_instrumentation("", false, 0));
        assert!(finish_range_body_instrumentation());

        let scenario = Scenario::register("exact_strong", 0).await;
        let (probed, _controller, bound_root) = probe(
            &scenario,
            RepresentationConsistencyPolicy::RequireStrongValidator,
        )
        .await;
        let (body, object) = probed.expect("initial strong probe succeeds").into_parts();
        drop(body);
        scenario.remove().await;
        let bound_baseline = bound_root.accounting_scalar_snapshot();
        begin_range_request_instrumentation();
        begin_range_body_instrumentation();
        for range in [0..0, std::ops::Range { start: 8, end: 7 }, 0..65] {
            let Err(error) = object.request_range(range) else {
                panic!("invalid bound range unexpectedly succeeded");
            };
            assert_eq!(error, ChromeRangeError::InvalidRequestRange);
        }
        assert_eq!(bound_root.accounting_scalar_snapshot(), bound_baseline);
        assert!(finish_range_request_instrumentation("", false, 0));
        assert!(finish_range_body_instrumentation());
    }

    #[wasm_bindgen_test]
    async fn controlled_page_proves_its_fixture_object_is_truly_same_origin_basic() {
        if !is_chrome_range_runtime() {
            return;
        }

        let scenario = Scenario::register("gzip", 0).await;
        let is_basic = JsFuture::from(inspect_same_origin_basic(&scenario.page_url))
            .await
            .expect("controlled page same-origin inspection succeeds")
            .as_bool()
            .expect("same-origin inspection result is boolean");
        assert!(is_basic);
        assert!(scenario.observed_cancellation().await);
        scenario.remove().await;
    }

    #[wasm_bindgen_test]
    async fn status_and_header_failures_abort_before_any_body_api() {
        if !is_chrome_range_runtime() {
            return;
        }

        let cases = [
            ("status", 200, ChromeRangeError::RangeUnsupported),
            ("status", 401, ChromeRangeError::AuthorizationRejected),
            ("status", 403, ChromeRangeError::AuthorizationRejected),
            ("status", 404, ChromeRangeError::ObjectUnavailable),
            ("status", 410, ChromeRangeError::ObjectUnavailable),
            ("status", 412, ChromeRangeError::PreconditionFailed),
            ("status", 416, ChromeRangeError::RangeNotSatisfiable),
            ("status", 429, ChromeRangeError::UnexpectedHttpStatus(429)),
            ("status", 500, ChromeRangeError::UnexpectedHttpStatus(500)),
            ("status", 501, ChromeRangeError::UnexpectedHttpStatus(501)),
            ("status", 502, ChromeRangeError::UnexpectedHttpStatus(502)),
            ("status", 503, ChromeRangeError::UnexpectedHttpStatus(503)),
            ("status", 504, ChromeRangeError::UnexpectedHttpStatus(504)),
            ("status", 505, ChromeRangeError::UnexpectedHttpStatus(505)),
            (
                "content_range_omit",
                0,
                ChromeRangeError::RequiredResponseHeaderUnavailable(
                    RequiredRangeResponseHeader::ContentRange,
                ),
            ),
            (
                "content_range_wrong",
                0,
                ChromeRangeError::InvalidContentRange,
            ),
            (
                "content_length_wrong",
                0,
                ChromeRangeError::InvalidContentLength,
            ),
            ("gzip", 0, ChromeRangeError::UnsupportedContentEncoding),
            ("etag_invalid", 0, ChromeRangeError::InvalidValidatorHeader),
            (
                "etag_duplicate",
                0,
                ChromeRangeError::InvalidValidatorHeader,
            ),
            ("etag_hidden", 0, ChromeRangeError::StrongValidatorRequired),
        ];
        for (kind, status, expected) in cases {
            let scenario = Scenario::register(kind, status).await;
            begin_range_request_instrumentation();
            begin_range_body_instrumentation();
            let (result, controller, _root) = probe(
                &scenario,
                RepresentationConsistencyPolicy::RequireStrongValidator,
            )
            .await;
            let Err(actual) = result else {
                panic!("scenario {kind}/{status} unexpectedly succeeded");
            };
            assert_eq!(actual, expected, "scenario {kind}/{status}");
            assert!(controller.signal().aborted());
            assert!(finish_range_request_instrumentation("", true, 1));
            assert!(finish_range_body_instrumentation());
            assert!(
                scenario.observed_cancellation().await,
                "fixture did not observe cancellation for {kind}/{status}"
            );
            scenario.remove().await;
        }
    }

    #[wasm_bindgen_test]
    async fn bound_length_validator_shape_and_value_changes_fail_before_body() {
        if !is_chrome_range_runtime() {
            return;
        }

        let initial = Scenario::register("exact_strong", 0).await;
        let (probed, _controller, _root) = probe(
            &initial,
            RepresentationConsistencyPolicy::RequireStrongValidator,
        )
        .await;
        let (body, object) = probed.expect("initial strong probe succeeds").into_parts();
        drop(body);
        initial.remove().await;

        for (kind, expected) in [
            ("total_changed", ChromeRangeError::ObjectChanged),
            ("etag_changed", ChromeRangeError::ObjectChanged),
            ("etag_shape_changed", ChromeRangeError::ObjectChanged),
            (
                "bound_etag_hidden",
                ChromeRangeError::RequiredResponseHeaderUnavailable(
                    RequiredRangeResponseHeader::EntityTag,
                ),
            ),
        ] {
            let scenario = Scenario::register(kind, 0).await;
            let (root, range, work) = scopes();
            let controller =
                web_sys::AbortController::new().expect("Chrome exposes AbortController");
            let url = scenario.cross_origin_secret();
            let request_range = object.request_range(0..4).expect("test range is valid");
            let mut control = AlwaysAllowed;
            begin_range_request_instrumentation();
            begin_range_body_instrumentation();
            let result = read_bound_exact_range(
                &url,
                request_range,
                &controller,
                &root,
                &range,
                &work,
                &mut control,
                future::pending(),
                NonZeroU64::new(2).expect("test pump slice is non-zero"),
            )
            .await;
            let Err(actual) = result else {
                panic!("scenario {kind} unexpectedly succeeded");
            };
            assert_eq!(actual, expected, "scenario {kind}");
            assert!(controller.signal().aborted());
            assert!(finish_range_request_instrumentation("\"v1\"", true, 1));
            assert!(finish_range_body_instrumentation());
            assert!(scenario.observed_cancellation().await);
            scenario.remove().await;
        }
    }

    #[wasm_bindgen_test]
    async fn cors_and_redirect_fetch_rejection_have_one_redacted_classification() {
        if !is_chrome_range_runtime() {
            return;
        }

        for kind in ["cors_reject", "redirect"] {
            let scenario = Scenario::register(kind, 0).await;
            begin_range_request_instrumentation();
            begin_range_body_instrumentation();
            let (result, controller, _root) = probe(
                &scenario,
                RepresentationConsistencyPolicy::RequireStrongValidator,
            )
            .await;
            let Err(actual) = result else {
                panic!("scenario {kind} unexpectedly succeeded");
            };
            assert_eq!(actual, ChromeRangeError::BrowserFetchUnavailable);
            assert!(controller.signal().aborted());
            assert!(finish_range_request_instrumentation("", true, 1));
            assert!(finish_range_body_instrumentation());
            if kind == "cors_reject" {
                assert!(scenario.observed_cancellation().await);
            }
            scenario.remove().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_is_typed_and_policy_free() {
        assert_eq!(map_visible_status(206), Ok(()));
        for (status, expected) in [
            (200, ChromeRangeError::RangeUnsupported),
            (401, ChromeRangeError::AuthorizationRejected),
            (403, ChromeRangeError::AuthorizationRejected),
            (404, ChromeRangeError::ObjectUnavailable),
            (410, ChromeRangeError::ObjectUnavailable),
            (412, ChromeRangeError::PreconditionFailed),
            (416, ChromeRangeError::RangeNotSatisfiable),
            (429, ChromeRangeError::UnexpectedHttpStatus(429)),
            (500, ChromeRangeError::UnexpectedHttpStatus(500)),
            (501, ChromeRangeError::UnexpectedHttpStatus(501)),
            (502, ChromeRangeError::UnexpectedHttpStatus(502)),
            (503, ChromeRangeError::UnexpectedHttpStatus(503)),
            (504, ChromeRangeError::UnexpectedHttpStatus(504)),
            (505, ChromeRangeError::UnexpectedHttpStatus(505)),
        ] {
            let actual = map_visible_status(status).unwrap_err();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn content_range_parser_is_strict_checked_and_allocation_free() {
        for (wire, expected) in [
            (b"bytes 0-3/64".as_slice(), Some((0, 3, 64))),
            (b"Bytes 10-19/20".as_slice(), Some((10, 19, 20))),
            (b"bytes 0-0/1".as_slice(), Some((0, 0, 1))),
            (b"bytes 0-3/*".as_slice(), None),
            (b"bytes */64".as_slice(), None),
            (b"bytes 3-0/64".as_slice(), None),
            (b"bytes 0-64/64".as_slice(), None),
            (b"bytes 0-3/18446744073709551616".as_slice(), None),
            (b"bytes 0-3/64, bytes 4-7/64".as_slice(), None),
            (b"bytes\t0-3/64".as_slice(), None),
        ] {
            let parsed = parse_content_range(&wire);
            assert_eq!(
                parsed.map(|value| { (value.start, value.end_inclusive, value.total.get(),) }),
                expected
            );
        }
        let oversized = vec![b'0'; MAX_CONTENT_RANGE_CODE_UNITS + 1];
        assert_eq!(parse_content_range(&oversized.as_slice()), None);
    }

    #[test]
    fn content_length_encoding_and_request_ranges_are_exact() {
        assert_eq!(parse_content_length(&b"0".as_slice()), Some(0));
        assert_eq!(
            parse_content_length(&b"18446744073709551615".as_slice()),
            Some(u64::MAX)
        );
        for invalid in [
            b"".as_slice(),
            b"+1".as_slice(),
            b"1, 1".as_slice(),
            b"18446744073709551616".as_slice(),
        ] {
            assert_eq!(parse_content_length(&invalid), None);
        }
        assert!(is_identity_encoding(&b"identity".as_slice()));
        assert!(is_identity_encoding(&b"IDENTITY".as_slice()));
        assert!(!is_identity_encoding(&b"gzip".as_slice()));
        assert!(!is_identity_encoding(&b"identity, gzip".as_slice()));

        assert_eq!(
            RequestedRange::new(3..7).unwrap(),
            RequestedRange {
                start: 3,
                end_inclusive: 6,
                expected_bytes: NonZeroU64::new(4).unwrap(),
            }
        );
        assert_eq!(
            RequestedRange::new(7..7).unwrap_err(),
            ChromeRangeError::InvalidRequestRange
        );
        assert_eq!(
            RequestedRange::new(Range { start: 8, end: 7 }).unwrap_err(),
            ChromeRangeError::InvalidRequestRange
        );
    }

    #[test]
    fn errors_and_bound_debug_are_secret_free() {
        for error in [
            ChromeRangeError::BrowserAdapterUnavailable,
            ChromeRangeError::BrowserFetchUnavailable,
            ChromeRangeError::InvalidContentRange,
            ChromeRangeError::InvalidContentLength,
            ChromeRangeError::UnsupportedContentEncoding,
            ChromeRangeError::RequiredResponseHeaderUnavailable(
                RequiredRangeResponseHeader::ContentRange,
            ),
            ChromeRangeError::RequiredResponseHeaderUnavailable(
                RequiredRangeResponseHeader::EntityTag,
            ),
            ChromeRangeError::ObjectChanged,
            ChromeRangeError::UnexpectedHttpStatus(599),
        ] {
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("https://"));
            assert!(!rendered.contains("etag-secret"));
        }
    }

    #[test]
    fn probe_binding_and_bound_revalidation_follow_the_frozen_truth_table() {
        let parse = |wire: &[u8]| {
            crate::remote_validator::parse_wire_for_test(&[wire])
                .unwrap()
                .unwrap()
        };

        let strong = bind_probe_validator(
            NonZeroU64::new(64).unwrap(),
            RepresentationConsistencyPolicy::RequireStrongValidator,
            Some(parse(b"\"v1\"")),
        )
        .unwrap();
        assert_eq!(
            strong.consistency(),
            RepresentationConsistency::StrongValidator
        );
        assert_eq!(strong.object_length().get(), 64);
        let bound_request = strong.request_range(3..7).unwrap();
        assert_eq!(bound_request.as_range(), 3..7);
        assert_eq!(
            bound_request.object().object_length(),
            strong.object_length()
        );
        assert!(matches!(
            strong.request_range(0..65),
            Err(ChromeRangeError::InvalidRequestRange)
        ));
        assert_eq!(
            validate_bound_validator(&strong, Some(parse(b"\"v1\""))),
            Ok(())
        );
        assert_eq!(
            validate_bound_validator(&strong, Some(parse(b"\"v2\""))),
            Err(ChromeRangeError::ObjectChanged)
        );
        assert_eq!(
            validate_bound_validator(&strong, None),
            Err(ChromeRangeError::RequiredResponseHeaderUnavailable(
                RequiredRangeResponseHeader::EntityTag
            ))
        );

        let weak = bind_probe_validator(
            NonZeroU64::new(64).unwrap(),
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            Some(parse(b"W/\"v1\"")),
        )
        .unwrap();
        assert_eq!(
            weak.consistency(),
            RepresentationConsistency::DeploymentAssumed
        );
        assert_eq!(
            validate_bound_validator(&weak, Some(parse(b"W/\"v1\""))),
            Ok(())
        );
        assert_eq!(
            validate_bound_validator(&weak, Some(parse(b"W/\"v2\""))),
            Err(ChromeRangeError::ObjectChanged)
        );

        let absent = bind_probe_validator(
            NonZeroU64::new(64).unwrap(),
            RepresentationConsistencyPolicy::AllowDeploymentAssumed,
            None,
        )
        .unwrap();
        assert_eq!(validate_bound_validator(&absent, None), Ok(()));
        assert_eq!(
            validate_bound_validator(&absent, Some(parse(b"\"v1\""))),
            Err(ChromeRangeError::ObjectChanged)
        );
        assert_eq!(
            bind_probe_validator(
                NonZeroU64::new(64).unwrap(),
                RepresentationConsistencyPolicy::RequireStrongValidator,
                None,
            )
            .unwrap_err(),
            ChromeRangeError::StrongValidatorRequired
        );
    }

    #[test]
    fn typed_validator_error_mapping_is_secret_free() {
        for (source, expected) in [
            (
                RemoteValidatorError::ResourceLimit,
                ChromeRangeError::ResourceLimit,
            ),
            (
                RemoteValidatorError::InvalidEntityTag,
                ChromeRangeError::InvalidValidatorHeader,
            ),
            (
                RemoteValidatorError::HeaderRejected,
                ChromeRangeError::BrowserAdapterUnavailable,
            ),
        ] {
            assert_eq!(map_validator_error(source), expected);
        }
        assert_eq!(ChromeRangeRequest::new(3..7).unwrap().as_range(), 3..7);
    }
}
