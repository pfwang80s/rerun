//! Secret-safe ownership and identity normalization for Web HTTP URLs.
//!
//! This module is additive: it does not replace `re_uri`, `ViewerOpenUrl`, or any native or
//! compatibility parser.

use std::fmt;
use std::io::Write as _;
use std::mem::size_of;
use std::ops::{Deref, Range};

use url::Host;
use zeroize::{Zeroize as _, Zeroizing};

/// Identifies how an HTTP URL entered the Web Viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpUrlIngress {
    /// The URL came directly from an embedder API or an explicit Open URL action.
    DirectExternal,

    /// The URL came from the Web Viewer page's own nested `?url=` query.
    NestedWebViewerQuery,
}

/// The accepted HTTP scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpScheme {
    Http,
    Https,
}

impl HttpScheme {
    fn parse(raw_scheme: &[u8]) -> Result<Self, SecretUrlError> {
        if raw_scheme.eq_ignore_ascii_case(b"http") {
            Ok(Self::Http)
        } else if raw_scheme.eq_ignore_ascii_case(b"https") {
            Ok(Self::Https)
        } else {
            Err(SecretUrlError::UnsupportedScheme)
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

impl fmt::Display for HttpScheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Fixed, non-secret failure reasons for [`SecretUrl::parse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretUrlError {
    InputTooLong,
    InvalidUtf8,
    ForbiddenUrlCodePoint,
    InvalidPercentEncoding,
    UnsupportedScheme,
    NonHierarchicalUrl,
    UserInfoNotAllowed,
    MissingHost,
    InvalidUrl,
    NestedQueryNotAllowed,
    FragmentTooLong,
    TooManyFragmentFields,
    FragmentFieldTooLong,
    EmptyFragmentField,
    EmptyFragmentKey,
    CanonicalUrlTooLong,
    RetainedBytesExceeded,
    PeakBytesExceeded,
    SizeOverflow,
}

impl fmt::Display for SecretUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InputTooLong => "URL input exceeds its byte limit",
            Self::InvalidUtf8 => "URL input is not valid UTF-8",
            Self::ForbiddenUrlCodePoint => "URL contains a forbidden code point",
            Self::InvalidPercentEncoding => "URL contains an invalid percent escape",
            Self::UnsupportedScheme => "URL scheme is not HTTP or HTTPS",
            Self::NonHierarchicalUrl => "URL is not a hierarchical HTTP URL",
            Self::UserInfoNotAllowed => "URL user information is not allowed",
            Self::MissingHost => "URL has no host",
            Self::InvalidUrl => "URL is invalid",
            Self::NestedQueryNotAllowed => "nested Web Viewer URLs cannot contain a query",
            Self::FragmentTooLong => "URL fragment exceeds its byte limit",
            Self::TooManyFragmentFields => "URL fragment has too many fields",
            Self::FragmentFieldTooLong => "URL fragment field exceeds its byte limit",
            Self::EmptyFragmentField => "URL fragment contains an empty field",
            Self::EmptyFragmentKey => "URL fragment contains an empty key",
            Self::CanonicalUrlTooLong => "canonical URL exceeds its byte limit",
            Self::RetainedBytesExceeded => "URL ownership exceeds its retained-byte limit",
            Self::PeakBytesExceeded => "URL parsing exceeds its peak-byte limit",
            Self::SizeOverflow => "URL size accounting overflowed",
        })
    }
}

impl std::error::Error for SecretUrlError {}

/// Limits required to parse and retain one secret HTTP URL.
///
/// This type deliberately has no public constructor while the corresponding production values in
/// the remote-limits profile remain unfrozen.
///
/// ```compile_fail
/// use re_web::secret_url::SecretUrlParserLimits;
///
/// let _ = SecretUrlParserLimits {
///     max_input_bytes: 1,
///     max_canonical_bytes: 1,
///     max_fragment_bytes: 1,
///     max_fragment_fields: 1,
///     max_fragment_field_bytes: 1,
///     max_retained_bytes: 1,
///     max_peak_bytes: 1,
/// };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretUrlParserLimits {
    max_input_bytes: usize,
    max_canonical_bytes: usize,
    max_fragment_bytes: usize,
    max_fragment_fields: usize,
    max_fragment_field_bytes: usize,
    max_retained_bytes: usize,
    max_peak_bytes: usize,
}

impl SecretUrlParserLimits {
    #[cfg(test)]
    const fn explicit_for_test(
        max_input_bytes: usize,
        max_canonical_bytes: usize,
        max_fragment_bytes: usize,
        max_fragment_fields: usize,
        max_fragment_field_bytes: usize,
        max_retained_bytes: usize,
        max_peak_bytes: usize,
    ) -> Self {
        Self {
            max_input_bytes,
            max_canonical_bytes,
            max_fragment_bytes,
            max_fragment_fields,
            max_fragment_field_bytes,
            max_retained_bytes,
            max_peak_bytes,
        }
    }
}

/// Secret-bearing bytes which are overwritten before their allocation is released.
struct SecretBytes(Box<[u8]>);

impl SecretBytes {
    fn new(bytes: Box<[u8]>) -> Self {
        Self(bytes)
    }

    fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn as_str(&self) -> &str {
        // Construction validates the complete input and only derives bytes from valid UTF-8.
        std::str::from_utf8(self.as_bytes()).expect("SecretBytes must contain validated UTF-8")
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
        record_test_drop(self.0.iter().all(|byte| *byte == 0));
    }
}

/// Makes the domain allocation inside `url::Host` explicit and zeroizing on every post-parse exit.
struct ZeroizingParsedHost(Option<Host<String>>);

impl ZeroizingParsedHost {
    fn new(host: Host<String>) -> Self {
        Self(Some(host))
    }
}

impl Deref for ZeroizingParsedHost {
    type Target = Host<String>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("parsed host is live")
    }
}

impl Drop for ZeroizingParsedHost {
    fn drop(&mut self) {
        if let Some(Host::Domain(mut domain)) = self.0.take() {
            domain.zeroize();
        }
    }
}

/// Opaque, bounded fragment bytes.
///
/// The outer parser validates only field boundaries and percent escapes.
/// It does not percent-decode, treat `+` specially, or construct domain identifiers.
pub struct OpaqueRawFragment {
    bytes: SecretBytes,
    field_count: usize,
}

impl OpaqueRawFragment {
    /// Returns the number of literal `&`-separated fields.
    pub fn field_count(&self) -> usize {
        self.field_count
    }

    /// Returns the retained raw fragment byte count.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether this fragment is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.len() == 0
    }

    /// Temporarily exposes the opaque bytes to the later bounded semantic parser.
    ///
    /// The higher-ranked closure prevents returning a borrow of the secret bytes.
    pub fn expose_for_bounded_parse<R>(&self, parse: impl for<'url> FnOnce(&'url [u8]) -> R) -> R {
        parse(self.bytes.as_bytes())
    }
}

impl fmt::Debug for OpaqueRawFragment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueRawFragment(<redacted>)")
    }
}

/// Fragment-free route identity bytes.
///
/// These bytes may still contain a secret query and therefore have no general `Display`, `Hash`,
/// serialization, or byte-slice accessor.
pub struct RouteCanonicalUrl {
    bytes: SecretBytes,
}

impl RouteCanonicalUrl {
    /// Returns the retained canonical byte count.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether the canonical identity is empty.
    ///
    /// A successfully parsed HTTP URL is never empty; this complements [`Self::len`] for generic
    /// bounded-owner code.
    pub fn is_empty(&self) -> bool {
        self.bytes.len() == 0
    }

    /// Compares exact canonical bytes without branching on byte contents.
    ///
    /// Length is not hidden; the loop duration depends on the larger input length.
    pub fn matches(&self, other: &Self) -> bool {
        constant_time_eq::constant_time_eq(self.bytes.as_bytes(), other.bytes.as_bytes())
    }

    /// Temporarily exposes the bytes to a keyed fingerprint implementation.
    ///
    /// The higher-ranked closure prevents returning a borrow of the secret bytes.
    pub fn expose_for_fingerprint<R>(
        &self,
        fingerprint: impl for<'url> FnOnce(&'url [u8]) -> R,
    ) -> R {
        fingerprint(self.bytes.as_bytes())
    }
}

impl fmt::Debug for RouteCanonicalUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RouteCanonicalUrl(<redacted>)")
    }
}

/// A secret-bearing HTTP URL with redacted formatting and explicit exposure points.
///
/// The raw URL, route identity, and optional raw fragment are separate zeroizing owners so later
/// state transitions can move or release them independently.
///
/// `SecretUrl` intentionally does not implement `Clone`, `Hash`, or serialization:
///
/// ```compile_fail
/// use re_web::secret_url::SecretUrl;
///
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<SecretUrl>();
/// ```
pub struct SecretUrl {
    raw: SecretBytes,
    canonical: RouteCanonicalUrl,
    fragment: Option<OpaqueRawFragment>,
    scheme: HttpScheme,
    host: SecretBytes,
    port: Option<u16>,
    retained_bytes: usize,
}

impl SecretUrl {
    /// Parses one owned UTF-8 HTTP URL without exposing it through an error value.
    ///
    /// The input is exact-sized so a tiny view cannot retain an unaccounted large backing buffer.
    pub fn parse(
        input: Box<[u8]>,
        ingress: HttpUrlIngress,
        limits: &SecretUrlParserLimits,
    ) -> Result<Self, SecretUrlError> {
        let raw = SecretBytes::new(input);
        if raw.len() > limits.max_input_bytes {
            return Err(SecretUrlError::InputTooLong);
        }

        let raw_str = std::str::from_utf8(raw.as_bytes())
            .map_err(|_utf8_error| SecretUrlError::InvalidUtf8)?;
        if raw
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, b'\t' | b'\n' | b'\r'))
        {
            return Err(SecretUrlError::ForbiddenUrlCodePoint);
        }
        let parts = RawUrlParts::scan(raw.as_bytes())?;
        if ingress == HttpUrlIngress::NestedWebViewerQuery && parts.query.is_some() {
            return Err(SecretUrlError::NestedQueryNotAllowed);
        }
        validate_percent_escapes(&raw.as_bytes()[parts.host.clone()])?;
        validate_percent_escapes(&raw.as_bytes()[parts.path.clone()])?;
        let fragment_preflight = preflight_fragment(raw.as_bytes(), &parts, limits)?;

        // The conservative plan is checked before any derived allocation. In particular, a small
        // canonical/retained/peak cap rejects while `raw` is still the only owned buffer.
        let allocation_plan = SecretUrlAllocationPlan::preflight(
            raw.len(),
            &parts,
            fragment_preflight.as_ref(),
            limits,
        )?;

        // Only the design-approved raw host reaches `url::Host`. Secret path, query, and fragment
        // bytes remain exclusively in the exact-sized zeroizing `raw` owner. Parsing the port here
        // also avoids `url::Url`'s independently growing serialization buffer.
        let parsed_host = ZeroizingParsedHost::new(
            Host::parse(&raw_str[parts.host.clone()])
                .map_err(|_parse_error| SecretUrlError::InvalidUrl)?,
        );
        record_test_derived_allocation();
        let mut host_temporary = Zeroizing::new(Vec::with_capacity(allocation_plan.host_upper));
        write!(&mut *host_temporary, "{}", &*parsed_host)
            .map_err(|_write_error| SecretUrlError::SizeOverflow)?;
        if host_temporary.len() > allocation_plan.host_upper {
            return Err(SecretUrlError::SizeOverflow);
        }
        drop(parsed_host);

        record_test_derived_allocation();
        let host = SecretBytes::new(host_temporary.as_slice().into());
        host_temporary.zeroize();
        drop(host_temporary);

        record_test_derived_allocation();
        let mut canonical_temporary =
            Zeroizing::new(Vec::with_capacity(allocation_plan.canonical_upper));
        extend_canonical(
            &mut canonical_temporary,
            parts.scheme.as_str().as_bytes(),
            allocation_plan.canonical_upper,
        )?;
        extend_canonical(
            &mut canonical_temporary,
            b"://",
            allocation_plan.canonical_upper,
        )?;
        extend_canonical(
            &mut canonical_temporary,
            host.as_bytes(),
            allocation_plan.canonical_upper,
        )?;
        if let Some(port) = &parts.port {
            push_canonical(
                &mut canonical_temporary,
                b':',
                allocation_plan.canonical_upper,
            )?;
            write!(&mut *canonical_temporary, "{}", port.number)
                .map_err(|_write_error| SecretUrlError::SizeOverflow)?;
            if canonical_temporary.len() > allocation_plan.canonical_upper {
                return Err(SecretUrlError::SizeOverflow);
            }
        }
        append_canonical_path(
            &mut canonical_temporary,
            &raw.as_bytes()[parts.path.clone()],
            allocation_plan.canonical_upper,
        )?;
        if let Some(query) = &parts.query {
            push_canonical(
                &mut canonical_temporary,
                b'?',
                allocation_plan.canonical_upper,
            )?;
            extend_canonical(
                &mut canonical_temporary,
                &raw.as_bytes()[query.clone()],
                allocation_plan.canonical_upper,
            )?;
        }

        let fragment_len = fragment_preflight
            .as_ref()
            .map_or(0, |preflight| preflight.range.len());
        let retained_bytes = size_of::<Self>()
            .checked_add(raw.len())
            .and_then(|bytes| bytes.checked_add(canonical_temporary.len()))
            .and_then(|bytes| bytes.checked_add(fragment_len))
            .and_then(|bytes| bytes.checked_add(host.len()))
            .ok_or(SecretUrlError::SizeOverflow)?;
        if retained_bytes > limits.max_retained_bytes {
            // The conservative preflight already proved this bound before allocation.
            return Err(SecretUrlError::SizeOverflow);
        }

        // Copy into an exact-sized owner while the temporary remains zeroizing. Moving the `Vec`
        // through `into_boxed_slice` could let an allocator shrink/reallocate its secret-bearing
        // buffer without first clearing the old allocation.
        record_test_derived_allocation();
        let canonical: Box<[u8]> = canonical_temporary.as_slice().into();
        canonical_temporary.zeroize();
        drop(canonical_temporary);

        // The opaque fragment owner is allocated only after every plan check succeeds.
        let fragment = if let Some(preflight) = fragment_preflight {
            record_test_derived_allocation();
            Some(OpaqueRawFragment {
                bytes: SecretBytes::new(raw.as_bytes()[preflight.range].into()),
                field_count: preflight.field_count,
            })
        } else {
            None
        };
        Ok(Self {
            raw,
            canonical: RouteCanonicalUrl {
                bytes: SecretBytes::new(canonical),
            },
            fragment,
            scheme: parts.scheme,
            host,
            port: parts.port.as_ref().map(|port| port.number),
            retained_bytes,
        })
    }

    /// Returns the URL scheme without exposing path, query, or fragment bytes.
    pub fn scheme(&self) -> HttpScheme {
        self.scheme
    }

    /// Returns the URL host without exposing path, query, or fragment bytes.
    pub fn host(&self) -> &str {
        self.host.as_str()
    }

    /// Returns the explicit, non-default port, if present.
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Returns fragment-free route identity material.
    pub fn route_canonical(&self) -> &RouteCanonicalUrl {
        &self.canonical
    }

    /// Returns the opaque raw fragment, if one was present.
    pub fn raw_fragment(&self) -> Option<&OpaqueRawFragment> {
        self.fragment.as_ref()
    }

    /// Returns the logical byte ownership charged to this value.
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// Temporarily exposes the complete URL to an HTTP request builder.
    ///
    /// The higher-ranked closure prevents returning a borrow of the secret URL.
    pub fn expose_for_request<R>(&self, request: impl for<'url> FnOnce(&'url str) -> R) -> R {
        request(self.raw.as_str())
    }
}

impl fmt::Display for SecretUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}://{}", self.scheme, self.host())?;
        if let Some(port) = self.port {
            write!(formatter, ":{port}")?;
        }
        formatter.write_str("/<redacted>")
    }
}

impl fmt::Debug for SecretUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SecretUrl({self})")
    }
}

struct RawUrlParts {
    scheme: HttpScheme,
    host: Range<usize>,
    port: Option<ParsedPort>,
    authority_is_plain_ascii: bool,
    path: Range<usize>,
    query: Option<Range<usize>>,
    fragment: Option<Range<usize>>,
}

#[derive(Clone, Copy)]
struct ParsedPort {
    number: u16,
}

impl RawUrlParts {
    fn scan(raw: &[u8]) -> Result<Self, SecretUrlError> {
        let scheme_end = raw
            .iter()
            .position(|byte| *byte == b':')
            .ok_or(SecretUrlError::UnsupportedScheme)?;
        let scheme = HttpScheme::parse(&raw[..scheme_end])?;

        let authority_start = scheme_end
            .checked_add(3)
            .ok_or(SecretUrlError::SizeOverflow)?;
        if raw.get(scheme_end + 1..authority_start) != Some(b"//") {
            return Err(SecretUrlError::NonHierarchicalUrl);
        }
        let authority_end = raw[authority_start..]
            .iter()
            .position(|byte| matches!(byte, b'/' | b'\\' | b'?' | b'#'))
            .map_or(raw.len(), |offset| authority_start + offset);
        let authority = &raw[authority_start..authority_end];
        if authority.is_empty() {
            return Err(SecretUrlError::MissingHost);
        }
        if authority.contains(&b'@') {
            return Err(SecretUrlError::UserInfoNotAllowed);
        }
        if authority.iter().any(|byte| *byte <= b' ' || *byte == 0x7F) {
            return Err(SecretUrlError::ForbiddenUrlCodePoint);
        }
        let (relative_host, port) = split_host_and_port(authority, scheme)?;
        let host = authority_start + relative_host.start..authority_start + relative_host.end;
        let raw_host = &raw[host.clone()];
        let authority_is_plain_ascii = raw_host.is_ascii() && !raw_host.contains(&b'%');

        let fragment_delimiter = raw.iter().position(|byte| *byte == b'#');
        let before_fragment = fragment_delimiter.unwrap_or(raw.len());
        let query_delimiter = raw[..before_fragment].iter().position(|byte| *byte == b'?');
        let query = query_delimiter.map(|start| start + 1..before_fragment);
        let fragment = fragment_delimiter.map(|start| start + 1..raw.len());
        let path = authority_end..query_delimiter.unwrap_or(before_fragment);

        Ok(Self {
            scheme,
            host,
            port,
            authority_is_plain_ascii,
            path,
            query,
            fragment,
        })
    }
}

fn split_host_and_port(
    authority: &[u8],
    scheme: HttpScheme,
) -> Result<(Range<usize>, Option<ParsedPort>), SecretUrlError> {
    let (host_end, port_bytes) = if authority.first() == Some(&b'[') {
        let bracket = authority
            .iter()
            .position(|byte| *byte == b']')
            .ok_or(SecretUrlError::InvalidUrl)?;
        let host_end = bracket + 1;
        match &authority[host_end..] {
            [] => (host_end, None),
            [b':', port @ ..] => (host_end, Some(port)),
            _ => return Err(SecretUrlError::InvalidUrl),
        }
    } else if let Some(colon) = authority.iter().position(|byte| *byte == b':') {
        if authority[colon + 1..].contains(&b':') {
            return Err(SecretUrlError::InvalidUrl);
        }
        (colon, Some(&authority[colon + 1..]))
    } else {
        (authority.len(), None)
    };

    if host_end == 0 {
        return Err(SecretUrlError::MissingHost);
    }

    let port = port_bytes
        .filter(|port| !port.is_empty())
        .map(|port| parse_port(port, scheme))
        .transpose()?
        .flatten();
    Ok((0..host_end, port))
}

fn parse_port(port: &[u8], scheme: HttpScheme) -> Result<Option<ParsedPort>, SecretUrlError> {
    if !port.iter().all(u8::is_ascii_digit) {
        return Err(SecretUrlError::InvalidUrl);
    }

    let mut number = 0u16;
    for digit in port {
        number = number
            .checked_mul(10)
            .and_then(|value| value.checked_add(u16::from(*digit - b'0')))
            .ok_or(SecretUrlError::InvalidUrl)?;
    }
    let is_default = matches!(
        (scheme, number),
        (HttpScheme::Http, 80) | (HttpScheme::Https, 443)
    );
    Ok((!is_default).then_some(ParsedPort { number }))
}

struct FragmentPreflight {
    range: Range<usize>,
    field_count: usize,
}

struct SecretUrlAllocationPlan {
    host_upper: usize,
    canonical_upper: usize,
}

impl SecretUrlAllocationPlan {
    // This proof was audited against the exact dependency set in `Cargo.lock`: `url` 2.5.8,
    // `percent-encoding` 2.3.2, `idna` 1.1.0, `idna_adapter` 1.2.1, `icu_normalizer` 2.2.0, and
    // `smallvec` 1.15.1. Updating any of them requires re-auditing every owner in
    // `AuthorityParserHeapUpper` before accepting the new lockfile. These are structural bounds,
    // not empirical allocation factors:
    //
    // * an ASCII domain cannot grow, an IPv4 serialization is at most 15 bytes, and an IPv6
    //   serialization is at most eight four-digit groups, seven colons, and two brackets;
    // * UTS #46 mapping/normalization expands one input scalar to at most 18 mapped scalars in the
    //   Unicode data used by `idna` 1.1;
    // * `idna` caps every non-ASCII mapped label at 1,000 scalars, so its internal `u32` Punycode
    //   delta emits at most ten continuation digits and one terminating digit per scalar;
    // * charging the four-byte Punycode prefix and optional delimiter to every mapped scalar adds
    //   at most five more bytes per scalar.
    //
    // `url::Host` is used directly so there is no independently growing `url::Url` parser
    // serialization. The post-parse checks deliberately remain in place as defense in depth; they
    // are not a substitute for the dependency-upgrade audit because parsing happens after this
    // preflight.
    const MAX_IPV6_HOST_SERIALIZATION_BYTES: usize = 8 * 4 + 7 + 2;
    const MAX_UTS46_MAPPED_SCALARS_PER_INPUT_SCALAR: usize = 18;
    const MAX_PUNYCODE_BYTES_PER_MAPPED_SCALAR: usize = 10 + 1 + 5;
    const MAX_SERIALIZED_PORT_SUFFIX_BYTES: usize = 1 + 5;

    fn preflight(
        raw_len: usize,
        parts: &RawUrlParts,
        fragment: Option<&FragmentPreflight>,
        limits: &SecretUrlParserLimits,
    ) -> Result<Self, SecretUrlError> {
        let raw_host_bytes = parts.host.len();
        let host_upper = if parts.authority_is_plain_ascii {
            raw_host_bytes.max(Self::MAX_IPV6_HOST_SERIALIZATION_BYTES)
        } else {
            raw_host_bytes
                .checked_mul(Self::MAX_UTS46_MAPPED_SCALARS_PER_INPUT_SCALAR)
                .and_then(|scalars| scalars.checked_mul(Self::MAX_PUNYCODE_BYTES_PER_MAPPED_SCALAR))
                .ok_or(SecretUrlError::SizeOverflow)?
        };
        let authority_serialization_upper = parts
            .scheme
            .as_str()
            .len()
            .checked_add(3)
            .and_then(|bytes| bytes.checked_add(host_upper))
            .and_then(|bytes| bytes.checked_add(Self::MAX_SERIALIZED_PORT_SUFFIX_BYTES))
            .ok_or(SecretUrlError::SizeOverflow)?;
        let path_upper = parts
            .path
            .len()
            .checked_mul(3)
            .map(|bytes| bytes.max(1))
            .ok_or(SecretUrlError::SizeOverflow)?;
        let query_suffix = parts.query.as_ref().map_or(Ok(0), |query| {
            query
                .len()
                .checked_add(1)
                .ok_or(SecretUrlError::SizeOverflow)
        })?;
        let canonical_upper = authority_serialization_upper
            .checked_add(path_upper)
            .and_then(|bytes| bytes.checked_add(query_suffix))
            .ok_or(SecretUrlError::SizeOverflow)?;
        if canonical_upper > limits.max_canonical_bytes {
            return Err(SecretUrlError::CanonicalUrlTooLong);
        }

        let fragment_len = fragment.map_or(0, |preflight| preflight.range.len());
        let retained_upper = size_of::<SecretUrl>()
            .checked_add(raw_len)
            .and_then(|bytes| bytes.checked_add(canonical_upper))
            .and_then(|bytes| bytes.checked_add(fragment_len))
            .and_then(|bytes| bytes.checked_add(host_upper))
            .ok_or(SecretUrlError::SizeOverflow)?;
        if retained_upper > limits.max_retained_bytes {
            return Err(SecretUrlError::RetainedBytesExceeded);
        }

        let authority_heap = AuthorityParserHeapUpper::preflight(raw_host_bytes, host_upper)?;

        // The final retained graph is charged even while it is not fully constructed. The
        // authority ledger then charges every Host/IDNA heap owner at its own realloc peak, the
        // exact host-serialization scratch overlaps the parsed Host, and canonical scratch
        // overlaps its exact-sized final owner. Summing individually impossible realloc peaks is
        // intentionally conservative and makes concurrent-lifetime reasoning monotonic.
        let peak_upper = retained_upper
            .checked_add(authority_heap.total_bytes)
            .and_then(|bytes| bytes.checked_add(host_upper))
            .and_then(|bytes| bytes.checked_add(canonical_upper))
            .ok_or(SecretUrlError::SizeOverflow)?;
        if peak_upper > limits.max_peak_bytes {
            return Err(SecretUrlError::PeakBytesExceeded);
        }

        Ok(Self {
            host_upper,
            canonical_upper,
        })
    }
}

/// Heap bytes which may coexist while `url::Host` parses one bounded raw host.
///
/// The fields mirror the locked implementation rather than hiding it behind a multiplier:
///
/// * `percent-encoding` may retain an owned decoded `Cow<[u8]>`;
/// * IDNA retains its output `String`, whole-domain `SmallVec<char>`, and label-classification
///   `SmallVec` together;
/// * existing Punycode labels temporarily retain both decoder insertions and a decoded label;
/// * ICU's decomposition `SmallVec<[CharacterAndClass; 17]>` can retain a whole run of following
///   non-starters in `gather_and_sort_combining`, so both its spill/realloc peak and stable-sort
///   scratch are charged against the whole checked mapped-scalar upper.
///
/// For a growing `Vec`/`String`, three times maximum logical length covers the current Rust
/// amortized capacity and the old plus new allocations during `realloc`; the eight-byte floor is
/// `RawVec<u8>`'s locked minimum non-zero allocation. For `smallvec` 1.15.1, a spill grows to
/// `next_power_of_two(required)` and the prior heap capacity is at most half of the new capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuthorityParserHeapUpper {
    percent_decoded_cow_bytes: usize,
    idna_output_string_bytes: usize,
    uts46_domain_buffer_bytes: usize,
    uts46_label_classification_bytes: usize,
    punycode_decoder_insertions_bytes: usize,
    punycode_decoded_label_bytes: usize,
    punycode_sort_scratch_bytes: usize,
    icu_normalization_spill_bytes: usize,
    icu_sort_scratch_bytes: usize,
    total_bytes: usize,
}

impl AuthorityParserHeapUpper {
    const UTS46_DOMAIN_INLINE_SCALARS: usize = 253;
    const UTS46_LABEL_CLASSIFICATION_INLINE_ITEMS: usize = 8;
    const PUNYCODE_INLINE_ITEMS: usize = 59;
    const ICU_NORMALIZATION_INLINE_SCALARS: usize = 17;
    const MAX_UTS46_MAPPED_SCALARS_PER_INPUT_SCALAR: usize = 18;
    const MAX_LABEL_CLASSIFICATION_WORDS: usize = 3;
    const PUNYCODE_DECODER_INSERTION_WORDS: usize = 2;
    const RAW_VEC_U8_MIN_NON_ZERO_BYTES: usize = 8;

    fn preflight(raw_host_bytes: usize, host_upper: usize) -> Result<Self, SecretUrlError> {
        let mapped_scalars = raw_host_bytes
            .checked_mul(Self::MAX_UTS46_MAPPED_SCALARS_PER_INPUT_SCALAR)
            .ok_or(SecretUrlError::SizeOverflow)?;
        let label_count = mapped_scalars
            .checked_add(1)
            .ok_or(SecretUrlError::SizeOverflow)?;

        let percent_decoded_cow_bytes = vec_u8_reallocation_peak(raw_host_bytes)?;
        let idna_output_string_bytes = vec_u8_reallocation_peak(host_upper)?;
        let uts46_domain_buffer_bytes = smallvec_reallocation_peak(
            mapped_scalars,
            Self::UTS46_DOMAIN_INLINE_SCALARS,
            size_of::<char>(),
        )?;
        let uts46_label_classification_bytes = smallvec_reallocation_peak(
            label_count,
            Self::UTS46_LABEL_CLASSIFICATION_INLINE_ITEMS,
            size_of::<usize>()
                .checked_mul(Self::MAX_LABEL_CLASSIFICATION_WORDS)
                .ok_or(SecretUrlError::SizeOverflow)?,
        )?;
        // A decoded Punycode label cannot contain more scalars than encoded input bytes.
        let punycode_decoder_insertions_bytes = smallvec_reallocation_peak(
            raw_host_bytes,
            Self::PUNYCODE_INLINE_ITEMS,
            size_of::<usize>()
                .checked_mul(Self::PUNYCODE_DECODER_INSERTION_WORDS)
                .ok_or(SecretUrlError::SizeOverflow)?,
        )?;
        let punycode_decoded_label_bytes = smallvec_reallocation_peak(
            raw_host_bytes,
            Self::PUNYCODE_INLINE_ITEMS,
            size_of::<char>(),
        )?;
        // `Decoder::decode` uses stable `sort_by_key`; charge a full insertion array even though
        // the locked standard-library implementation needs no more than a fraction of it.
        let punycode_sort_scratch_bytes = raw_host_bytes
            .checked_mul(
                size_of::<usize>()
                    .checked_mul(Self::PUNYCODE_DECODER_INSERTION_WORDS)
                    .ok_or(SecretUrlError::SizeOverflow)?,
            )
            .ok_or(SecretUrlError::SizeOverflow)?;
        let icu_normalization_spill_bytes = smallvec_reallocation_peak(
            mapped_scalars,
            Self::ICU_NORMALIZATION_INLINE_SCALARS,
            size_of::<u32>(),
        )?;
        // `sort_slice_by_ccc` can sort the complete combining run. Charge one full
        // `CharacterAndClass` array, which bounds the locked stable-sort scratch.
        let icu_sort_scratch_bytes = mapped_scalars
            .checked_mul(size_of::<u32>())
            .ok_or(SecretUrlError::SizeOverflow)?;

        let total_bytes = [
            percent_decoded_cow_bytes,
            idna_output_string_bytes,
            uts46_domain_buffer_bytes,
            uts46_label_classification_bytes,
            punycode_decoder_insertions_bytes,
            punycode_decoded_label_bytes,
            punycode_sort_scratch_bytes,
            icu_normalization_spill_bytes,
            icu_sort_scratch_bytes,
        ]
        .into_iter()
        .try_fold(0usize, |total, bytes| total.checked_add(bytes))
        .ok_or(SecretUrlError::SizeOverflow)?;

        Ok(Self {
            percent_decoded_cow_bytes,
            idna_output_string_bytes,
            uts46_domain_buffer_bytes,
            uts46_label_classification_bytes,
            punycode_decoder_insertions_bytes,
            punycode_decoded_label_bytes,
            punycode_sort_scratch_bytes,
            icu_normalization_spill_bytes,
            icu_sort_scratch_bytes,
            total_bytes,
        })
    }
}

fn vec_u8_reallocation_peak(max_len: usize) -> Result<usize, SecretUrlError> {
    if max_len == 0 {
        return Ok(0);
    }
    max_len
        .checked_mul(3)
        .map(|bytes| bytes.max(AuthorityParserHeapUpper::RAW_VEC_U8_MIN_NON_ZERO_BYTES))
        .ok_or(SecretUrlError::SizeOverflow)
}

fn smallvec_reallocation_peak(
    max_items: usize,
    inline_items: usize,
    item_bytes: usize,
) -> Result<usize, SecretUrlError> {
    if max_items <= inline_items {
        return Ok(0);
    }
    let new_capacity = max_items
        .checked_next_power_of_two()
        .ok_or(SecretUrlError::SizeOverflow)?;
    let prior_capacity = new_capacity / 2;
    new_capacity
        .checked_add(prior_capacity)
        .and_then(|items| items.checked_mul(item_bytes))
        .ok_or(SecretUrlError::SizeOverflow)
}

fn preflight_fragment(
    raw: &[u8],
    parts: &RawUrlParts,
    limits: &SecretUrlParserLimits,
) -> Result<Option<FragmentPreflight>, SecretUrlError> {
    let Some(range) = parts.fragment.clone() else {
        return Ok(None);
    };
    if range.len() > limits.max_fragment_bytes {
        return Err(SecretUrlError::FragmentTooLong);
    }

    let fragment = &raw[range.clone()];
    validate_percent_escapes(fragment)?;
    let mut field_count = 0usize;
    for field in fragment.split(|byte| *byte == b'&') {
        field_count = field_count
            .checked_add(1)
            .ok_or(SecretUrlError::SizeOverflow)?;
        if field_count > limits.max_fragment_fields {
            return Err(SecretUrlError::TooManyFragmentFields);
        }
        if field.len() > limits.max_fragment_field_bytes {
            return Err(SecretUrlError::FragmentFieldTooLong);
        }
        if field.is_empty() {
            return Err(SecretUrlError::EmptyFragmentField);
        }
        let key_end = field
            .iter()
            .position(|byte| *byte == b'=')
            .unwrap_or(field.len());
        if key_end == 0 {
            return Err(SecretUrlError::EmptyFragmentKey);
        }
    }

    Ok(Some(FragmentPreflight { range, field_count }))
}

fn validate_percent_escapes(bytes: &[u8]) -> Result<(), SecretUrlError> {
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(digits) = bytes.get(index + 1..index + 3) else {
                return Err(SecretUrlError::InvalidPercentEncoding);
            };
            if !digits.iter().all(u8::is_ascii_hexdigit) {
                return Err(SecretUrlError::InvalidPercentEncoding);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn append_canonical_path(
    output: &mut Vec<u8>,
    raw_path: &[u8],
    limit: usize,
) -> Result<(), SecretUrlError> {
    let path_start = output.len();
    push_canonical(output, b'/', limit)?;
    if raw_path.is_empty() {
        return Ok(());
    }
    assert!(matches!(raw_path[0], b'/' | b'\\'));

    let mut raw_segment_start = 1usize;
    loop {
        let next_separator = raw_path[raw_segment_start..]
            .iter()
            .position(|byte| matches!(byte, b'/' | b'\\'))
            .map(|offset| raw_segment_start + offset);
        let raw_segment_end = next_separator.unwrap_or(raw_path.len());
        let output_segment_start = output.len();
        append_canonical_segment(output, &raw_path[raw_segment_start..raw_segment_end], limit)?;

        match &output[output_segment_start..] {
            b"." => output.truncate(output_segment_start),
            b".." => {
                output.truncate(output_segment_start);
                shorten_canonical_path(output, path_start);
            }
            _ => {
                if next_separator.is_some() {
                    push_canonical(output, b'/', limit)?;
                }
            }
        }

        let Some(separator) = next_separator else {
            break;
        };
        raw_segment_start = separator + 1;
    }

    Ok(())
}

fn append_canonical_segment(
    output: &mut Vec<u8>,
    raw_segment: &[u8],
    limit: usize,
) -> Result<(), SecretUrlError> {
    let mut index = 0usize;
    while index < raw_segment.len() {
        let byte = raw_segment[index];
        if byte == b'%' {
            let high = decode_hex(raw_segment[index + 1]);
            let low = decode_hex(raw_segment[index + 2]);
            let decoded = (high << 4) | low;
            if is_unreserved(decoded) {
                push_canonical(output, decoded, limit)?;
            } else {
                append_percent_encoded(output, decoded, limit)?;
            }
            index += 3;
        } else if byte >= 0x80 || needs_path_percent_encoding(byte) {
            append_percent_encoded(output, byte, limit)?;
            index += 1;
        } else {
            push_canonical(output, byte, limit)?;
            index += 1;
        }
    }

    Ok(())
}

fn shorten_canonical_path(output: &mut Vec<u8>, path_start: usize) {
    assert_eq!(output.last(), Some(&b'/'));
    if output.len() == path_start + 1 {
        return;
    }

    output.truncate(output.len() - 1);
    let previous_separator = output[path_start..]
        .iter()
        .rposition(|byte| *byte == b'/')
        .map_or(path_start, |offset| path_start + offset);
    output.truncate(previous_separator + 1);
}

fn push_canonical(output: &mut Vec<u8>, byte: u8, limit: usize) -> Result<(), SecretUrlError> {
    if output.len() == limit {
        return Err(SecretUrlError::SizeOverflow);
    }
    output.push(byte);
    Ok(())
}

fn extend_canonical(
    output: &mut Vec<u8>,
    bytes: &[u8],
    limit: usize,
) -> Result<(), SecretUrlError> {
    let new_len = output
        .len()
        .checked_add(bytes.len())
        .ok_or(SecretUrlError::SizeOverflow)?;
    if new_len > limit {
        return Err(SecretUrlError::SizeOverflow);
    }
    output.extend_from_slice(bytes);
    Ok(())
}

fn append_percent_encoded(
    output: &mut Vec<u8>,
    byte: u8,
    limit: usize,
) -> Result<(), SecretUrlError> {
    push_canonical(output, b'%', limit)?;
    push_canonical(output, encode_hex(byte >> 4), limit)?;
    push_canonical(output, encode_hex(byte & 0x0F), limit)
}

fn needs_path_percent_encoding(byte: u8) -> bool {
    byte <= 0x1F || byte == 0x7F || matches!(byte, b' ' | b'"' | b'<' | b'>' | b'`' | b'{' | b'}')
}

fn decode_hex(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("URL percent escapes were validated before canonicalization"),
    }
}

fn encode_hex(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        10..=15 => b'A' + nibble - 10,
        _ => unreachable!("a decoded hexadecimal nibble is at most 15"),
    }
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(test)]
thread_local! {
    static TEST_DROP_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static TEST_DERIVED_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_test_drop(was_zeroized: bool) {
    TEST_DROP_COUNTS.with(|counts| {
        let (total, zeroized) = counts.get();
        counts.set((total + 1, zeroized + usize::from(was_zeroized)));
    });
}

#[cfg(not(test))]
fn record_test_drop(_was_zeroized: bool) {}

#[cfg(test)]
fn record_test_derived_allocation() {
    TEST_DERIVED_ALLOCATIONS.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_test_derived_allocation() {}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_LIMITS: SecretUrlParserLimits =
        SecretUrlParserLimits::explicit_for_test(4_096, 65_536, 1_024, 16, 256, 262_144, 1_048_576);
    const LARGE_AUTHORITY_TEST_LIMITS: SecretUrlParserLimits =
        SecretUrlParserLimits::explicit_for_test(
            4_096, 1_048_576, 1_024, 16, 256, 2_097_152, 33_554_432,
        );

    fn parse(input: &str) -> Result<SecretUrl, SecretUrlError> {
        SecretUrl::parse(
            input.as_bytes().to_vec().into_boxed_slice(),
            HttpUrlIngress::DirectExternal,
            &TEST_LIMITS,
        )
    }

    fn canonical(input: &str) -> Vec<u8> {
        parse(input)
            .unwrap()
            .route_canonical()
            .expose_for_fingerprint(<[u8]>::to_vec)
    }

    fn reset_drop_counts() {
        TEST_DROP_COUNTS.with(|counts| counts.set((0, 0)));
    }

    fn drop_counts() -> (usize, usize) {
        TEST_DROP_COUNTS.with(std::cell::Cell::get)
    }

    fn reset_derived_allocations() {
        TEST_DERIVED_ALLOCATIONS.with(|count| count.set(0));
    }

    fn derived_allocations() -> usize {
        TEST_DERIVED_ALLOCATIONS.with(std::cell::Cell::get)
    }

    #[test]
    fn normalizes_authority_path_and_percent_encoding() {
        assert_eq!(
            canonical("HTTP://EXAMPLE.COM:80/a/%7e/%41/../c"),
            canonical("http://example.com/a/~/c")
        );
        assert_eq!(
            canonical("https://EXAMPLE.com:443/a/%2f/b"),
            canonical("https://example.com/a/%2F/b")
        );
        assert_eq!(
            canonical("https://example.com/a/%2e/b/%2E%2e/c"),
            canonical("https://example.com/a/c")
        );
        assert_eq!(
            canonical("https://example.com/caf%C3%a9"),
            canonical("https://example.com/café")
        );
        assert_ne!(
            canonical("https://example.com/a/%252e/b"),
            canonical("https://example.com/a/b")
        );
        assert_ne!(
            canonical("https://example.com/a/%2F/b"),
            canonical("https://example.com/a///b")
        );
        assert_eq!(
            canonical("https://example.com/a/%5c/b"),
            canonical("https://example.com/a/%5C/b")
        );
        assert_ne!(
            canonical("https://example.com/a/%5C/b"),
            canonical("https://example.com/a\\b")
        );
    }

    #[test]
    fn preserves_path_structure_and_case() {
        assert_ne!(
            canonical("https://example.com/A//b"),
            canonical("https://example.com/a//b")
        );
        assert_ne!(
            canonical("https://example.com/a//b"),
            canonical("https://example.com/a/b")
        );
        assert_ne!(
            canonical("https://example.com/a/b/"),
            canonical("https://example.com/a/b")
        );
    }

    #[test]
    fn removes_dot_segments_at_every_path_boundary_without_recursive_decoding() {
        for (input, expected) in [
            ("https://example.com/a/..", "https://example.com/"),
            ("https://example.com/a/.", "https://example.com/a/"),
            ("https://example.com/../a", "https://example.com/a"),
            ("https://example.com/a//../b", "https://example.com/a/b"),
            ("https://example.com/%2e/", "https://example.com/"),
            ("https://example.com/%2e%2e/a", "https://example.com/a"),
            ("https://example.com/%252e/a", "https://example.com/%252e/a"),
        ] {
            assert_eq!(canonical(input), expected.as_bytes(), "input: {input}");
        }
    }

    #[test]
    fn percent_encodes_raw_path_code_points_but_not_reserved_percent_escapes() {
        for (input, expected) in [
            (
                "https://example.com/a b/\"c\"/{d}`e`",
                "https://example.com/a%20b/%22c%22/%7Bd%7D%60e%60",
            ),
            ("https://example.com/café", "https://example.com/caf%C3%A9"),
            (
                "https://example.com/a/%2f/%5c",
                "https://example.com/a/%2F/%5C",
            ),
        ] {
            assert_eq!(canonical(input), expected.as_bytes(), "input: {input}");
        }
    }

    #[test]
    fn authority_only_normalization_covers_idna_ipv6_empty_path_and_backslash() {
        assert_eq!(
            canonical("https://bücher.example/café"),
            canonical("https://xn--bcher-kva.example/caf%C3%A9")
        );
        assert_eq!(
            canonical("HTTP://[2001:0DB8::1]:80"),
            canonical("http://[2001:db8::1]/")
        );
        assert_eq!(
            canonical("https://example.com"),
            canonical("https://example.com/")
        );
        assert_eq!(
            canonical("https://example.com\\a\\b"),
            canonical("https://example.com/a/b")
        );
        assert_eq!(
            canonical("https://%62%C3%BCcher.example/a"),
            canonical("https://xn--bcher-kva.example/a")
        );
        assert_eq!(
            canonical("http://[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:00081/a"),
            b"http://[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:81/a"
        );
        assert_eq!(
            canonical("https://example.com:/a"),
            canonical("https://example.com/a")
        );
    }

    #[test]
    fn accepts_the_audited_idna_label_and_domain_boundaries() {
        let maximum_mapped_label = "é".repeat(1_000);
        let input = format!("https://{maximum_mapped_label}.example/a");
        let parsed = SecretUrl::parse(
            input.into_bytes().into_boxed_slice(),
            HttpUrlIngress::DirectExternal,
            &LARGE_AUTHORITY_TEST_LIMITS,
        )
        .unwrap();
        assert!(parsed.host().starts_with("xn--"));

        let many_labels = std::iter::repeat_n("é", 127).collect::<Vec<_>>().join(".");
        let input = format!("https://{many_labels}/a");
        let parsed = SecretUrl::parse(
            input.into_bytes().into_boxed_slice(),
            HttpUrlIngress::DirectExternal,
            &LARGE_AUTHORITY_TEST_LIMITS,
        )
        .unwrap();
        assert_eq!(parsed.host().split('.').count(), 127);

        // `icu_normalizer` gathers the entire following non-starter run before sorting it. This
        // exceeds `Decomposition`'s 17-item inline buffer and exercises the audited spill path.
        let long_combining_run = format!("a{}", "\u{0301}".repeat(32));
        let input = format!("https://{long_combining_run}.example/a");
        let parsed = SecretUrl::parse(
            input.into_bytes().into_boxed_slice(),
            HttpUrlIngress::DirectExternal,
            &LARGE_AUTHORITY_TEST_LIMITS,
        )
        .unwrap();
        assert!(parsed.host().starts_with("xn--"));
    }

    #[test]
    fn preserves_exact_query_bytes_and_ignores_fragment() {
        assert_ne!(
            canonical("https://example.com/a?q=%7e"),
            canonical("https://example.com/a?q=~")
        );
        assert_ne!(
            canonical("https://example.com/a?q=%7e"),
            canonical("https://example.com/a?q=%7E")
        );
        assert_eq!(
            canonical("https://example.com/a?sig=%&q=%zz&short=%2"),
            b"https://example.com/a?sig=%&q=%zz&short=%2"
        );
        assert_ne!(
            canonical("https://example.com/a?q=a+b"),
            canonical("https://example.com/a?q=a%20b")
        );
        assert_ne!(
            canonical("https://example.com/a?q=1&q=2"),
            canonical("https://example.com/a?q=2&q=1")
        );
        assert_ne!(
            canonical("https://example.com/a"),
            canonical("https://example.com/a?")
        );
        assert_eq!(
            canonical("https://example.com/a?q=secret#view=one"),
            canonical("https://example.com/a?q=secret#view=two")
        );
    }

    #[test]
    fn nested_ingress_rejects_any_query_but_not_question_mark_in_fragment() {
        for input in ["https://example.com/a?", "https://example.com/a?q=secret"] {
            assert_eq!(
                SecretUrl::parse(
                    input.as_bytes().to_vec().into_boxed_slice(),
                    HttpUrlIngress::NestedWebViewerQuery,
                    &TEST_LIMITS,
                )
                .unwrap_err(),
                SecretUrlError::NestedQueryNotAllowed
            );
        }

        let malformed_non_query_escapes = "https://example.com/%zz?q=%#view=%zz";
        let error = SecretUrl::parse(
            malformed_non_query_escapes
                .as_bytes()
                .to_vec()
                .into_boxed_slice(),
            HttpUrlIngress::NestedWebViewerQuery,
            &TEST_LIMITS,
        )
        .unwrap_err();
        assert_eq!(error, SecretUrlError::NestedQueryNotAllowed);
        assert!(!format!("{error:?} {error}").contains(malformed_non_query_escapes));

        SecretUrl::parse(
            b"https://example.com/a#view?not-a-query"
                .to_vec()
                .into_boxed_slice(),
            HttpUrlIngress::NestedWebViewerQuery,
            &TEST_LIMITS,
        )
        .unwrap();
        SecretUrl::parse(
            b"https://example.com/a%3Fb#view=x"
                .to_vec()
                .into_boxed_slice(),
            HttpUrlIngress::NestedWebViewerQuery,
            &TEST_LIMITS,
        )
        .unwrap();
    }

    #[test]
    fn fragment_is_bounded_opaque_and_preserves_exact_bytes() {
        let url = parse("https://example.com/a#when=frame%2f1&flag&empty=&when=x=y").unwrap();
        let fragment = url.raw_fragment().unwrap();
        assert_eq!(fragment.field_count(), 4);
        assert_eq!(
            fragment.expose_for_bounded_parse(<[u8]>::to_vec),
            b"when=frame%2f1&flag&empty=&when=x=y"
        );
    }

    #[test]
    fn rejects_invalid_fragment_grammar_without_allocating_a_fragment_owner() {
        for (input, expected) in [
            ("https://example.com/a#", SecretUrlError::EmptyFragmentField),
            (
                "https://example.com/a#a&&b",
                SecretUrlError::EmptyFragmentField,
            ),
            (
                "https://example.com/a#=value",
                SecretUrlError::EmptyFragmentKey,
            ),
            (
                "https://example.com/a#key=%xy",
                SecretUrlError::InvalidPercentEncoding,
            ),
        ] {
            reset_drop_counts();
            assert_eq!(parse(input).unwrap_err(), expected);
            assert_eq!(drop_counts(), (1, 1));
        }

        assert_eq!(
            parse("https://example.com/a%zz?sig=%").unwrap_err(),
            SecretUrlError::InvalidPercentEncoding
        );
    }

    #[test]
    fn rejects_invalid_url_shapes_without_echoing_input() {
        for (input, expected) in [
            ("ftp://example.com/a", SecretUrlError::UnsupportedScheme),
            ("http:example.com/a", SecretUrlError::NonHierarchicalUrl),
            ("http:///a", SecretUrlError::MissingHost),
            ("http://example.com:65536/a", SecretUrlError::InvalidUrl),
            (
                "http://user:secret@example.com/a",
                SecretUrlError::UserInfoNotAllowed,
            ),
            ("http://@example.com/a", SecretUrlError::UserInfoNotAllowed),
        ] {
            let error = SecretUrl::parse(
                input.as_bytes().to_vec().into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &TEST_LIMITS,
            )
            .unwrap_err();
            assert_eq!(error, expected);
            assert!(!format!("{error:?} {error}").contains(input));
        }
    }

    #[test]
    fn rejects_invalid_utf8_and_enforces_all_limits() {
        assert_eq!(
            SecretUrl::parse(
                vec![0xFF].into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &TEST_LIMITS,
            )
            .unwrap_err(),
            SecretUrlError::InvalidUtf8
        );

        let tiny = SecretUrlParserLimits::explicit_for_test(8, 8, 1, 1, 1, 1, 1);
        assert_eq!(
            SecretUrl::parse(
                b"https://example.com".to_vec().into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &tiny,
            )
            .unwrap_err(),
            SecretUrlError::InputTooLong
        );

        let fragment_count =
            SecretUrlParserLimits::explicit_for_test(128, 128, 32, 1, 32, 512, 2_048);
        assert_eq!(
            SecretUrl::parse(
                b"https://example.com#a&b".to_vec().into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &fragment_count,
            )
            .unwrap_err(),
            SecretUrlError::TooManyFragmentFields
        );

        let fragment_bytes =
            SecretUrlParserLimits::explicit_for_test(128, 128, 1, 4, 32, 512, 2_048);
        assert_eq!(
            SecretUrl::parse(
                b"https://example.com#ab".to_vec().into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &fragment_bytes,
            )
            .unwrap_err(),
            SecretUrlError::FragmentTooLong
        );

        let field_bytes = SecretUrlParserLimits::explicit_for_test(128, 128, 32, 4, 1, 512, 2_048);
        assert_eq!(
            SecretUrl::parse(
                b"https://example.com#ab".to_vec().into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &field_bytes,
            )
            .unwrap_err(),
            SecretUrlError::FragmentFieldTooLong
        );

        let canonical_bytes =
            SecretUrlParserLimits::explicit_for_test(128, 8, 32, 4, 32, 512, 2_048);
        assert_eq!(
            SecretUrl::parse(
                b"https://example.com".to_vec().into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &canonical_bytes,
            )
            .unwrap_err(),
            SecretUrlError::CanonicalUrlTooLong
        );

        let retained_bytes =
            SecretUrlParserLimits::explicit_for_test(128, 512, 32, 4, 32, 1, 2_048);
        assert_eq!(
            SecretUrl::parse(
                b"https://example.com".to_vec().into_boxed_slice(),
                HttpUrlIngress::DirectExternal,
                &retained_bytes,
            )
            .unwrap_err(),
            SecretUrlError::RetainedBytesExceeded
        );
    }

    #[test]
    fn display_debug_and_errors_are_redacted() {
        let secret = "path-secret?token=query-secret#fragment-secret=value";
        let url = parse(&format!("https://EXAMPLE.com:8443/{secret}")).unwrap();

        assert_eq!(url.to_string(), "https://example.com:8443/<redacted>");
        assert_eq!(
            format!("{url:?}"),
            "SecretUrl(https://example.com:8443/<redacted>)"
        );
        assert!(!url.to_string().contains(secret));
        assert!(!format!("{:?}", url.route_canonical()).contains(secret));
        assert!(!format!("{:?}", url.raw_fragment()).contains(secret));
    }

    #[test]
    fn request_exposure_preserves_the_exact_input() {
        for input in [
            "https://example.com/%7e?q=%7e#view=x",
            "https://example.com/a?sig=%&q=%zz&short=%2",
        ] {
            let url = parse(input).unwrap();
            assert_eq!(url.expose_for_request(str::to_owned), input);
        }
    }

    #[test]
    fn all_secret_owners_are_zeroized_before_release() {
        reset_drop_counts();
        let url = parse("https://example.com/path?token=secret#view=secret").unwrap();
        assert!(url.retained_bytes() >= size_of::<SecretUrl>());
        drop(url);

        // raw URL, canonical route, raw fragment, and redacted host owner.
        assert_eq!(drop_counts(), (4, 4));
    }

    #[test]
    fn capacity_preflight_rejects_before_every_derived_allocation() {
        for (limits, expected) in [
            (
                SecretUrlParserLimits::explicit_for_test(128, 8, 32, 4, 32, 4_096, 16_384),
                SecretUrlError::CanonicalUrlTooLong,
            ),
            (
                SecretUrlParserLimits::explicit_for_test(128, 4_096, 32, 4, 32, 1, 16_384),
                SecretUrlError::RetainedBytesExceeded,
            ),
            (
                SecretUrlParserLimits::explicit_for_test(128, 4_096, 32, 4, 32, 4_096, 1),
                SecretUrlError::PeakBytesExceeded,
            ),
        ] {
            reset_drop_counts();
            reset_derived_allocations();
            assert_eq!(
                SecretUrl::parse(
                    b"https://example.com/secret?token=secret#view=secret"
                        .to_vec()
                        .into_boxed_slice(),
                    HttpUrlIngress::DirectExternal,
                    &limits,
                )
                .unwrap_err(),
                expected
            );

            assert_eq!(derived_allocations(), 0);
            assert_eq!(drop_counts(), (1, 1));
        }
    }

    #[test]
    fn parsing_does_not_grow_the_domain_string_interner() {
        let before = re_string_interner::bytes_used();
        let url =
            parse("https://example.com/data#when=unique_timeline_mcap_007&entity=/unique/entity")
                .unwrap();
        assert_eq!(url.raw_fragment().unwrap().field_count(), 2);
        assert_eq!(re_string_interner::bytes_used(), before);
    }

    #[test]
    fn canonical_match_compares_contents_not_only_lengths() {
        let first = parse("https://example.com/aa?q=1").unwrap();
        let same = parse("HTTPS://EXAMPLE.COM:443/aa?q=1#fragment=x").unwrap();
        let other = parse("https://example.com/bb?q=1").unwrap();

        assert!(first.route_canonical().matches(same.route_canonical()));
        assert!(!first.route_canonical().matches(other.route_canonical()));

        let short = parse("https://example.com/a").unwrap();
        let long = parse("https://example.com/longer").unwrap();
        assert!(!short.route_canonical().matches(long.route_canonical()));

        for input in [
            "https://example.com/ba?q=1",
            "https://example.com/ab?q=2",
            "https://example.net/ab?q=1",
        ] {
            let collision = parse(input).unwrap();
            assert_eq!(
                collision.route_canonical().len(),
                first.route_canonical().len()
            );
            assert!(!first.route_canonical().matches(collision.route_canonical()));
        }
    }

    #[test]
    fn authority_heap_ledger_covers_reallocation_and_inline_boundaries() {
        assert_eq!(vec_u8_reallocation_peak(1).unwrap(), 8);
        assert_eq!(vec_u8_reallocation_peak(1_000).unwrap(), 3_000);
        assert_eq!(smallvec_reallocation_peak(253, 253, 4).unwrap(), 0);
        assert_eq!(smallvec_reallocation_peak(254, 253, 4).unwrap(), 1_536);

        let ledger = AuthorityParserHeapUpper::preflight(2_000, 576_000).unwrap();
        assert!(ledger.percent_decoded_cow_bytes >= 2_000);
        assert!(ledger.idna_output_string_bytes >= 576_000);
        assert!(ledger.uts46_domain_buffer_bytes > 36_000 * size_of::<char>());
        assert!(ledger.uts46_label_classification_bytes > 36_001 * size_of::<usize>());
        assert!(ledger.punycode_decoder_insertions_bytes > 2_000 * size_of::<usize>());
        assert!(ledger.punycode_decoded_label_bytes > 2_000 * size_of::<char>());
        assert!(ledger.punycode_sort_scratch_bytes >= 2_000 * size_of::<usize>());
        assert!(ledger.icu_normalization_spill_bytes > 36_000 * size_of::<u32>());
        assert_eq!(ledger.icu_sort_scratch_bytes, 36_000 * size_of::<u32>());
        assert_eq!(
            ledger.total_bytes,
            ledger.percent_decoded_cow_bytes
                + ledger.idna_output_string_bytes
                + ledger.uts46_domain_buffer_bytes
                + ledger.uts46_label_classification_bytes
                + ledger.punycode_decoder_insertions_bytes
                + ledger.punycode_decoded_label_bytes
                + ledger.punycode_sort_scratch_bytes
                + ledger.icu_normalization_spill_bytes
                + ledger.icu_sort_scratch_bytes
        );
    }
}
