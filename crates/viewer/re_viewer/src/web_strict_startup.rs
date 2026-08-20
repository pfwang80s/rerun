//! Strict `startWithRequests` bootstrap handoff orchestration for the Web Viewer.
//!
//! This module connects the pure [`re_web::strict_startup_handoff`] ownership
//! state machine to the [`crate::web::WebHandle`] lifecycle. It only serves the
//! strict `startWithRequests` lane; the compatibility [`crate::web::WebHandle::start`]
//! path is unchanged.
//!
//! The production remote-MCAP capability is deliberately disarmed. A strict
//! startup therefore completes the Rust-side spec preflight and then returns a
//! redacted `CapabilityUnavailable` error without installing a runner, DOM
//! listener, repaint callback, observer, or remote work.

const STRICT_STARTUP_MAX_FIELD_BYTES_V1: usize = 65_536;
const STRICT_STARTUP_MAX_FIELD_CHARS_V1: usize = 65_536;
const STRICT_STARTUP_MAX_TOPIC_FILTERS_V1: usize = 128;
const STRICT_STARTUP_MAX_COLLECTION_ITEM_CHARS_V1: usize = 4_096;
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(dead_code, reason = "used by the wasm-only strict startup preflight")
)]
const STRICT_STARTUP_MAX_BATCH_ITEMS_V1: usize = 64;

/// Whether the production remote-MCAP capability is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrictStartupCapabilityGateV1 {
    /// The measured capability is installed; the startup handoff may proceed.
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(
            dead_code,
            reason = "the armed variant is only reached by the future measured capability bridge"
        )
    )]
    Armed,
    /// The capability is not installed; strict startup is disarmed.
    Disarmed,
}

/// The current production-disarmed capability gate.
///
/// This returns [`StrictStartupCapabilityGateV1::Disarmed`] until the measured
/// remote-MCAP capability bridge is installed. Nothing in this module or in
/// [`crate::web::WebHandle`] arms the compatibility singleton.
pub(crate) fn strict_startup_capability_gate_v1() -> StrictStartupCapabilityGateV1 {
    StrictStartupCapabilityGateV1::Disarmed
}

/// Redacted, request-local strict-startup failure codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RedactedStrictStartupErrorCodeV1 {
    InvalidRequestShape,
    InvalidUrl,
    UnsupportedStrictOpenRoute,
    UnsupportedFormat,
    ResourceLimitExceeded,
    BatchTooLarge,
    CapabilityUnavailable,
}

impl RedactedStrictStartupErrorCodeV1 {
    pub(crate) const fn wire_code(self) -> &'static str {
        match self {
            Self::InvalidRequestShape => "invalid_request_shape",
            Self::InvalidUrl => "invalid_url",
            Self::UnsupportedStrictOpenRoute => "unsupported_strict_open_route",
            Self::UnsupportedFormat => "unsupported_format",
            Self::ResourceLimitExceeded => "resource_limit_exceeded",
            Self::BatchTooLarge => "batch_too_large",
            Self::CapabilityUnavailable => "capability_unavailable",
        }
    }

    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::InvalidRequestShape => "strict open request shape is invalid",
            Self::InvalidUrl => "strict open URL is invalid",
            Self::UnsupportedStrictOpenRoute => "strict open route is unsupported",
            Self::UnsupportedFormat => "strict open format is unsupported",
            Self::ResourceLimitExceeded => "strict open resource limit was exceeded",
            Self::BatchTooLarge => "strict open batch exceeds its item limit",
            Self::CapabilityUnavailable => "strict remote-MCAP capability is unavailable",
        }
    }
}

/// A redacted strict-startup error with its optional batch index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RedactedStrictStartupErrorV1 {
    pub code: RedactedStrictStartupErrorCodeV1,
    pub failed_index: Option<u32>,
}

impl RedactedStrictStartupErrorV1 {
    pub(crate) const fn new(
        code: RedactedStrictStartupErrorCodeV1,
        failed_index: Option<u32>,
    ) -> Self {
        Self { code, failed_index }
    }
}

/// The frozen representation-consistency admission policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrictStartupConsistencyV1 {
    RequireStrongValidator,
    AllowDeploymentAssumed,
}

/// Cross-platform validated input for one strict startup request.
#[derive(Clone, Debug)]
pub(crate) struct ValidatedStrictStartupSpecV1 {
    pub url: String,
    pub topic_filter: Box<[u8]>,
    pub decoder_allowlist_version: u64,
    pub assignment_policy_version: u64,
    pub time_type: re_log_types::TimeType,
    pub consistency: StrictStartupConsistencyV1,
}

/// Cross-platform options input after JS string-or-array normalization.
#[derive(Clone, Debug, Default)]
pub(crate) struct StrictStartupOptionsInputV1 {
    pub topic_filter: Vec<String>,
    pub decoder_selector: Vec<String>,
    pub decoder_allowlist_version: Option<String>,
    pub assignment_policy_version: Option<String>,
    pub mcap_time_type: Option<String>,
    pub representation_consistency: Option<String>,
    pub recording_open_behavior: Option<String>,
    pub allow_extensionless_sniff: Option<bool>,
}

/// Validates one strict startup spec without touching any Web or remote owner.
///
/// The validation mirrors the `TypeScript` strict preflight so the Rust boundary
/// is independently bounded and secret-safe.
pub(crate) fn validate_strict_startup_spec_v1(
    url: String,
    options: &StrictStartupOptionsInputV1,
    index: usize,
) -> Result<ValidatedStrictStartupSpecV1, RedactedStrictStartupErrorV1> {
    let failed_index = u32::try_from(index).ok();

    if url.is_empty() {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
            failed_index,
        ));
    }
    if url.chars().count() > STRICT_STARTUP_MAX_FIELD_CHARS_V1
        || url.len() > STRICT_STARTUP_MAX_FIELD_BYTES_V1
    {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
            failed_index,
        ));
    }
    let parsed_url = validate_http_url_v1(&url, failed_index)?;

    let topic_filter = canonicalize_string_collection_v1(&options.topic_filter, failed_index)?;
    validate_string_collection_v1(&options.decoder_selector, failed_index)?;

    let decoder_allowlist_version =
        parse_canonical_u64_v1(options.decoder_allowlist_version.as_deref(), failed_index)?;
    let assignment_policy_version =
        parse_canonical_u64_v1(options.assignment_policy_version.as_deref(), failed_index)?;

    let time_type = match options.mcap_time_type.as_deref().unwrap_or("timestamp_ns") {
        "timestamp_ns" => re_log_types::TimeType::TimestampNs,
        "duration_ns" => re_log_types::TimeType::DurationNs,
        _ => {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
                failed_index,
            ));
        }
    };

    let consistency = match options
        .representation_consistency
        .as_deref()
        .unwrap_or("require_strong_validator")
    {
        "require_strong_validator" => StrictStartupConsistencyV1::RequireStrongValidator,
        "allow_deployment_assumed" => StrictStartupConsistencyV1::AllowDeploymentAssumed,
        _ => {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
                failed_index,
            ));
        }
    };

    match options
        .recording_open_behavior
        .as_deref()
        .unwrap_or("open_and_select")
    {
        "open" | "open_and_select" | "background" => {}
        _ => {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
                failed_index,
            ));
        }
    }

    validate_strict_http_path_v1(&parsed_url, options.allow_extensionless_sniff, failed_index)?;

    Ok(ValidatedStrictStartupSpecV1 {
        url,
        topic_filter,
        decoder_allowlist_version,
        assignment_policy_version,
        time_type,
        consistency,
    })
}

fn validate_http_url_v1(
    url: &str,
    failed_index: Option<u32>,
) -> Result<url::Url, RedactedStrictStartupErrorV1> {
    let parsed = url::Url::parse(url).map_err(|_parse_error| {
        RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::InvalidUrl,
            failed_index,
        )
    })?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::UnsupportedStrictOpenRoute,
            failed_index,
        ));
    }

    let has_userinfo = !parsed.username().is_empty()
        || parsed
            .password()
            .is_some_and(|password| !password.is_empty());
    if parsed.host_str().is_none() || parsed.host_str() == Some("") || has_userinfo {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::InvalidUrl,
            failed_index,
        ));
    }
    Ok(parsed)
}

fn validate_strict_http_path_v1(
    parsed: &url::Url,
    allow_extensionless_sniff: Option<bool>,
    failed_index: Option<u32>,
) -> Result<(), RedactedStrictStartupErrorV1> {
    // The TypeScript preflight routes on `URL.pathname`. Use the Rust `url`
    // parser for the same WHATWG-style backslash and dot-segment normalization,
    // while retaining the original caller URL for later transport.
    let path = parsed.path();
    let explicit_mcap = path.to_ascii_lowercase().ends_with(".mcap");
    if !explicit_mcap {
        let last_segment = path.rsplit('/').next().unwrap_or(path);
        if last_segment.contains('.') {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::UnsupportedFormat,
                failed_index,
            ));
        }
        if !allow_extensionless_sniff.unwrap_or(false) {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::UnsupportedFormat,
                failed_index,
            ));
        }
    }
    Ok(())
}

fn canonicalize_string_collection_v1(
    items: &[String],
    failed_index: Option<u32>,
) -> Result<Box<[u8]>, RedactedStrictStartupErrorV1> {
    validate_string_collection_v1(items, failed_index)?;
    let mut canonical = Vec::new();
    for (position, item) in items.iter().enumerate() {
        if position > 0 {
            canonical.push(b'\n');
        }
        canonical.extend_from_slice(item.as_bytes());
    }
    Ok(canonical.into_boxed_slice())
}

fn validate_string_collection_v1(
    items: &[String],
    failed_index: Option<u32>,
) -> Result<(), RedactedStrictStartupErrorV1> {
    if items.len() > STRICT_STARTUP_MAX_TOPIC_FILTERS_V1 {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
            failed_index,
        ));
    }
    let mut total_chars = 0_usize;
    let mut total_bytes = 0_usize;
    for item in items {
        if item.chars().count() > STRICT_STARTUP_MAX_COLLECTION_ITEM_CHARS_V1
            || item.len() > STRICT_STARTUP_MAX_FIELD_BYTES_V1
        {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
                failed_index,
            ));
        }
        total_chars = total_chars
            .checked_add(item.chars().count())
            .ok_or_else(|| {
                RedactedStrictStartupErrorV1::new(
                    RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
                    failed_index,
                )
            })?;
        total_bytes = total_bytes.checked_add(item.len()).ok_or_else(|| {
            RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
                failed_index,
            )
        })?;
        if total_chars > STRICT_STARTUP_MAX_FIELD_CHARS_V1
            || total_bytes > STRICT_STARTUP_MAX_FIELD_BYTES_V1
        {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
                failed_index,
            ));
        }
    }
    Ok(())
}

fn parse_canonical_u64_v1(
    value: Option<&str>,
    failed_index: Option<u32>,
) -> Result<u64, RedactedStrictStartupErrorV1> {
    let Some(value) = value else {
        return Ok(1);
    };
    if value.is_empty()
        || !value.is_ascii()
        || value.starts_with('-')
        || value.starts_with('+')
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
            failed_index,
        ));
    }
    if value.len() > 20 {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
            failed_index,
        ));
    }
    if value.len() == 20 && value > "18446744073709551615" {
        return Err(RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
            failed_index,
        ));
    }
    value.parse::<u64>().map_err(|_parse_error| {
        RedactedStrictStartupErrorV1::new(
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
            failed_index,
        )
    })
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::JsValue;

    use re_web::remote_validator::{RepresentationConsistency, RepresentationConsistencyPolicy};
    use re_web::secret_url::HttpUrlIngress;
    use re_web::source_reuse::RemoteMcapSemanticConfigV1;
    use re_web::strict_open_batch::StrictOpenRequestSpecV1;

    use crate::web_startup::StringOrStringArray;

    use super::{
        RedactedStrictStartupErrorCodeV1, RedactedStrictStartupErrorV1,
        STRICT_STARTUP_MAX_BATCH_ITEMS_V1, StrictStartupConsistencyV1, StrictStartupOptionsInputV1,
        ValidatedStrictStartupSpecV1, validate_strict_startup_spec_v1,
    };

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictStartupSpecInputWireV1 {
        url: String,
        #[serde(default)]
        options: StrictStartupOptionsInputWireV1,
    }

    #[derive(Default, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictStartupOptionsInputWireV1 {
        #[serde(default)]
        topic_filter: Option<StringOrStringArray>,
        #[serde(default)]
        decoder_selector: Option<StringOrStringArray>,
        #[serde(default)]
        decoder_allowlist_version: Option<String>,
        #[serde(default)]
        assignment_policy_version: Option<String>,
        #[serde(default)]
        mcap_time_type: Option<String>,
        #[serde(default)]
        representation_consistency: Option<String>,
        #[serde(default)]
        recording_open_behavior: Option<String>,
        #[serde(default)]
        allow_extensionless_sniff: Option<bool>,
    }

    impl From<StrictStartupOptionsInputWireV1> for StrictStartupOptionsInputV1 {
        fn from(value: StrictStartupOptionsInputWireV1) -> Self {
            Self {
                topic_filter: value
                    .topic_filter
                    .map(StringOrStringArray::into_inner)
                    .unwrap_or_default(),
                decoder_selector: value
                    .decoder_selector
                    .map(StringOrStringArray::into_inner)
                    .unwrap_or_default(),
                decoder_allowlist_version: value.decoder_allowlist_version,
                assignment_policy_version: value.assignment_policy_version,
                mcap_time_type: value.mcap_time_type,
                representation_consistency: value.representation_consistency,
                recording_open_behavior: value.recording_open_behavior,
                allow_extensionless_sniff: value.allow_extensionless_sniff,
            }
        }
    }

    /// Rust-side strict startup preflight.
    ///
    /// The JS specs are converted into [`StrictOpenRequestSpecV1`] without
    /// installing any runner, listener, repaint, observer, handler, or remote
    /// work. The production capability gate is checked by the caller after this
    /// bounded conversion succeeds.
    pub(crate) fn preflight_strict_startup_specs_v1(
        specs: JsValue,
    ) -> Result<Vec<StrictOpenRequestSpecV1>, RedactedStrictStartupErrorV1> {
        let wire_specs: Vec<StrictStartupSpecInputWireV1> = serde_wasm_bindgen::from_value(specs)
            .map_err(|_| {
            RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
                None,
            )
        })?;
        if wire_specs.is_empty() {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
                None,
            ));
        }
        if wire_specs.len() > STRICT_STARTUP_MAX_BATCH_ITEMS_V1 {
            return Err(RedactedStrictStartupErrorV1::new(
                RedactedStrictStartupErrorCodeV1::BatchTooLarge,
                None,
            ));
        }

        let mut request_specs = Vec::with_capacity(wire_specs.len());
        for (index, wire_spec) in wire_specs.into_iter().enumerate() {
            let options = StrictStartupOptionsInputV1::from(wire_spec.options);
            let validated = validate_strict_startup_spec_v1(wire_spec.url, &options, index)?;
            request_specs.push(convert_validated_spec_v1(validated));
        }
        Ok(request_specs)
    }

    /// Serializes a redacted strict-startup error as a wire-envelope JsValue.
    pub(crate) fn strict_startup_error_envelope_to_js_v1(
        error: &RedactedStrictStartupErrorV1,
    ) -> Result<JsValue, JsValue> {
        build_error_js(
            "admission",
            error.code.wire_code(),
            error.failed_index,
            None,
            error.code.message(),
        )
    }

    /// Returns the production-disarmed `CapabilityUnavailable` envelope.
    pub(crate) fn strict_startup_capability_unavailable_js_v1() -> Result<JsValue, JsValue> {
        build_error_js(
            "handoff",
            "capability_unavailable",
            None,
            None,
            "strict remote-MCAP capability is unavailable",
        )
    }

    fn convert_validated_spec_v1(
        validated: ValidatedStrictStartupSpecV1,
    ) -> StrictOpenRequestSpecV1 {
        let (requested_consistency_policy, actual_consistency) = match validated.consistency {
            StrictStartupConsistencyV1::RequireStrongValidator => (
                RepresentationConsistencyPolicy::RequireStrongValidator,
                RepresentationConsistency::StrongValidator,
            ),
            StrictStartupConsistencyV1::AllowDeploymentAssumed => (
                RepresentationConsistencyPolicy::AllowDeploymentAssumed,
                RepresentationConsistency::DeploymentAssumed,
            ),
        };
        let semantic = RemoteMcapSemanticConfigV1::new_v1(
            validated.topic_filter,
            validated.decoder_allowlist_version,
            validated.assignment_policy_version,
            validated.time_type,
            requested_consistency_policy,
            actual_consistency,
        );
        StrictOpenRequestSpecV1::HttpRemoteMcapCandidate {
            url: validated.url.into_bytes().into_boxed_slice(),
            ingress: HttpUrlIngress::DirectExternal,
            semantic,
        }
    }

    fn build_error_js(
        class: &'static str,
        code: &'static str,
        failed_index: Option<u32>,
        retryable: Option<bool>,
        message: &'static str,
    ) -> Result<JsValue, JsValue> {
        let object = js_sys::Object::new();
        js_sys::Reflect::set(&object, &"version".into(), &JsValue::from_f64(1.0))?;
        js_sys::Reflect::set(&object, &"class".into(), &JsValue::from_str(class))?;
        js_sys::Reflect::set(&object, &"code".into(), &JsValue::from_str(code))?;
        let failed_index_decimal = match failed_index {
            Some(index) => JsValue::from_str(&index.to_string()),
            None => JsValue::null(),
        };
        js_sys::Reflect::set(
            &object,
            &"failed_index_decimal".into(),
            &failed_index_decimal,
        )?;
        let retryable = match retryable {
            Some(value) => JsValue::from_bool(value),
            None => JsValue::null(),
        };
        js_sys::Reflect::set(&object, &"retryable".into(), &retryable)?;
        js_sys::Reflect::set(&object, &"message".into(), &JsValue::from_str(message))?;
        Ok(JsValue::from(object))
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::{
    preflight_strict_startup_specs_v1, strict_startup_capability_unavailable_js_v1,
    strict_startup_error_envelope_to_js_v1,
};

#[cfg(test)]
mod tests {
    use super::{
        RedactedStrictStartupErrorCodeV1, StrictStartupCapabilityGateV1,
        StrictStartupConsistencyV1, StrictStartupOptionsInputV1, strict_startup_capability_gate_v1,
        validate_strict_startup_spec_v1,
    };

    fn options() -> StrictStartupOptionsInputV1 {
        StrictStartupOptionsInputV1::default()
    }

    #[test]
    fn capability_gate_is_disarmed_and_never_arms_compatibility() {
        assert_eq!(
            strict_startup_capability_gate_v1(),
            StrictStartupCapabilityGateV1::Disarmed
        );
    }

    #[test]
    fn redacted_error_codes_are_fixed_and_secret_safe() {
        for code in [
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape,
            RedactedStrictStartupErrorCodeV1::InvalidUrl,
            RedactedStrictStartupErrorCodeV1::UnsupportedStrictOpenRoute,
            RedactedStrictStartupErrorCodeV1::UnsupportedFormat,
            RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded,
            RedactedStrictStartupErrorCodeV1::BatchTooLarge,
            RedactedStrictStartupErrorCodeV1::CapabilityUnavailable,
        ] {
            assert!(!code.message().contains("http"));
            assert!(!code.message().contains("example"));
            assert!(!code.wire_code().contains("secret"));
        }
    }

    #[test]
    fn valid_http_mcap_spec_validates_with_defaults() {
        let validated = validate_strict_startup_spec_v1(
            "https://example.invalid/a.mcap?token=secret".to_owned(),
            &options(),
            0,
        )
        .unwrap();
        assert_eq!(validated.url, "https://example.invalid/a.mcap?token=secret");
        assert!(validated.topic_filter.is_empty());
        assert_eq!(validated.decoder_allowlist_version, 1);
        assert_eq!(validated.assignment_policy_version, 1);
        assert_eq!(validated.time_type, re_log_types::TimeType::TimestampNs);
        assert_eq!(
            validated.consistency,
            StrictStartupConsistencyV1::RequireStrongValidator
        );
    }

    #[test]
    fn non_http_and_userinfo_urls_are_rejected() {
        let error = validate_strict_startup_spec_v1(
            "rerun+http://127.0.0.1:9876/proxy".to_owned(),
            &options(),
            0,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            RedactedStrictStartupErrorCodeV1::UnsupportedStrictOpenRoute
        );
        assert_eq!(error.failed_index, Some(0));

        let error = validate_strict_startup_spec_v1(
            "https://user:pass@example.invalid/a.mcap".to_owned(),
            &options(),
            1,
        )
        .unwrap_err();
        assert_eq!(error.code, RedactedStrictStartupErrorCodeV1::InvalidUrl);
        assert_eq!(error.failed_index, Some(1));
    }

    #[test]
    fn oversized_and_noncanonical_fields_are_rejected() {
        let error = validate_strict_startup_spec_v1(String::new(), &options(), 0).unwrap_err();
        assert_eq!(
            error.code,
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape
        );

        let oversized =
            validate_strict_startup_spec_v1("x".repeat(65_537), &options(), 0).unwrap_err();
        assert_eq!(
            oversized.code,
            RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded
        );

        let mut bad_options = options();
        bad_options.decoder_allowlist_version = Some("01".to_owned());
        let error = validate_strict_startup_spec_v1(
            "https://example.invalid/a.mcap".to_owned(),
            &bad_options,
            0,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape
        );

        let mut bad_options = options();
        bad_options.decoder_allowlist_version = Some("18446744073709551616".to_owned());
        let error = validate_strict_startup_spec_v1(
            "https://example.invalid/a.mcap".to_owned(),
            &bad_options,
            0,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            RedactedStrictStartupErrorCodeV1::ResourceLimitExceeded
        );
    }

    #[test]
    fn topic_filter_canonicalization_joins_and_bounds_items() {
        let mut with_topics = options();
        with_topics.topic_filter = vec!["a".to_owned(), "b".to_owned()];
        let validated = validate_strict_startup_spec_v1(
            "https://example.invalid/a.mcap".to_owned(),
            &with_topics,
            0,
        )
        .unwrap();
        assert_eq!(validated.topic_filter.as_ref(), b"a\nb");

        let mut too_many = options();
        too_many.topic_filter = (0..129).map(|index| index.to_string()).collect();
        let error = validate_strict_startup_spec_v1(
            "https://example.invalid/a.mcap".to_owned(),
            &too_many,
            0,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            RedactedStrictStartupErrorCodeV1::InvalidRequestShape
        );
    }

    #[test]
    fn extensionless_route_requires_opt_in_and_rejects_dotted_non_mcap_paths() {
        let error = validate_strict_startup_spec_v1(
            "https://example.invalid/no-extension".to_owned(),
            &options(),
            2,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            RedactedStrictStartupErrorCodeV1::UnsupportedFormat
        );
        assert_eq!(error.failed_index, Some(2));

        let mut opted_in = options();
        opted_in.allow_extensionless_sniff = Some(true);
        let validated = validate_strict_startup_spec_v1(
            "https://example.invalid/no-extension?token=secret".to_owned(),
            &opted_in,
            0,
        )
        .unwrap();
        assert_eq!(
            validated.url,
            "https://example.invalid/no-extension?token=secret"
        );

        let dotted = validate_strict_startup_spec_v1(
            "https://example.invalid/archive.tar.gz".to_owned(),
            &opted_in,
            1,
        )
        .unwrap_err();
        assert_eq!(
            dotted.code,
            RedactedStrictStartupErrorCodeV1::UnsupportedFormat
        );
        assert_eq!(dotted.failed_index, Some(1));
    }

    #[test]
    fn whatwg_pathname_normalization_matches_typescript_route_decisions() {
        let mut opted_in = options();
        opted_in.allow_extensionless_sniff = Some(true);

        let backslash_mcap = validate_strict_startup_spec_v1(
            "https://example.invalid\\secret.mcap".to_owned(),
            &options(),
            0,
        )
        .unwrap();
        assert_eq!(backslash_mcap.url, "https://example.invalid\\secret.mcap");

        let backslash_dotted = validate_strict_startup_spec_v1(
            "https://example.invalid\\archive.tar.gz".to_owned(),
            &opted_in,
            1,
        )
        .unwrap_err();
        assert_eq!(
            backslash_dotted.code,
            RedactedStrictStartupErrorCodeV1::UnsupportedFormat
        );
        assert_eq!(backslash_dotted.failed_index, Some(1));

        let dot_segment_extensionless = validate_strict_startup_spec_v1(
            "https://example.invalid/a.mcap/..".to_owned(),
            &opted_in,
            2,
        )
        .unwrap();
        assert_eq!(
            dot_segment_extensionless.url,
            "https://example.invalid/a.mcap/.."
        );
    }
}
