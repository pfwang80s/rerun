//! Production-disarmed HTTP-only strict open batch preparation.
//!
//! The transaction validates all inputs into local staging first.
//! Only after the whole batch succeeds does it allocate source/status/operation identities.
//! It does not start Fetch, connect to gRPC/Redap, mutate the compatibility route dispatcher, or
//! evict terminal history.

use std::fmt;

use crate::external_string_ingress::{CombinedCopyPermit, OpaqueJsString};
use crate::open_source_terminal::{OpenSourceStatusOwnerV1, OpenSourceToken};
use crate::secret_url::{HttpUrlIngress, SecretUrl, SecretUrlParserLimits};
use crate::source_reuse::RemoteMcapSemanticConfigV1;
use crate::strict_open_wire::{
    OpenOperationIdentity, PublicOpenRequestIdentity, PublicRecordingIdentity,
    StrictOpenAdmissionCodeV1, StrictOpenWireErrorV1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedStrictOpenRouteV1 {
    KnownRemoteMcap,
    ExtensionlessSniff,
}

pub enum StrictOpenRequestSpecV1 {
    HttpRemoteMcapCandidate {
        url: Box<[u8]>,
        ingress: HttpUrlIngress,
        semantic: RemoteMcapSemanticConfigV1,
    },
    UnsupportedRoute,
}

pub struct PreparedStrictOpenOperationV1 {
    pub operation_id: OpenOperationIdentity,
    pub public_request_id: PublicOpenRequestIdentity,
    pub public_recording_id: PublicRecordingIdentity,
    pub source_token: OpenSourceToken,
    pub status_owner: OpenSourceStatusOwnerV1,
    pub route: PreparedStrictOpenRouteV1,
    pub url: SecretUrl,
    pub semantic: RemoteMcapSemanticConfigV1,
    #[expect(dead_code, reason = "retained by the future handoff release adapter")]
    pub(crate) ingress_permit: CombinedCopyPermit,
}

impl fmt::Debug for PreparedStrictOpenOperationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedStrictOpenOperationV1")
            .field("operation_id", &self.operation_id)
            .field("public_request_id", &self.public_request_id)
            .field("public_recording_id", &self.public_recording_id)
            .field("status_owner", &"<owned>")
            .field("route", &self.route)
            .field("url", &"<redacted>")
            .field("semantic", &"<owned>")
            .field("ingress_permit", &"<owned>")
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct PreparedStrictOpenBatchV1 {
    operations: Vec<PreparedStrictOpenOperationV1>,
}

impl PreparedStrictOpenBatchV1 {
    pub fn operations_v1(&self) -> &[PreparedStrictOpenOperationV1] {
        &self.operations
    }

    pub fn extensionless_sniff_count_v1(&self) -> usize {
        self.operations
            .iter()
            .filter(|operation| operation.route == PreparedStrictOpenRouteV1::ExtensionlessSniff)
            .count()
    }
}

pub struct StrictOpenBatchPrepareContextV1 {
    next_identity: u128,
    max_batch_items: usize,
}

impl StrictOpenBatchPrepareContextV1 {
    pub fn new_v1(max_batch_items: usize) -> Self {
        Self {
            next_identity: 1,
            max_batch_items,
        }
    }

    pub fn prepare_batch_v1(
        &mut self,
        specs: Vec<StrictOpenRequestSpecV1>,
        url_limits: &SecretUrlParserLimits,
    ) -> Result<PreparedStrictOpenBatchV1, StrictOpenWireErrorV1> {
        if specs.len() > self.max_batch_items {
            return Err(StrictOpenWireErrorV1::admission(
                StrictOpenAdmissionCodeV1::BatchTooLarge,
                None,
            ));
        }

        let mut staged = Vec::with_capacity(specs.len());
        for (index, spec) in specs.into_iter().enumerate() {
            let staged_operation = match spec {
                StrictOpenRequestSpecV1::HttpRemoteMcapCandidate {
                    url,
                    ingress,
                    semantic,
                } => {
                    let route = classify_http_remote_mcap_route_v1(&url)
                        .map_err(|code| admission_error_v1(code, index))?;
                    let url_string = OpaqueJsString::from_utf8(&url).map_err(|_err| {
                        admission_error_v1(StrictOpenAdmissionCodeV1::ResourceLimitExceeded, index)
                    })?;
                    let topic_string = OpaqueJsString::from_utf8(semantic.topic_filter_bytes_v1())
                        .map_err(|_err| {
                            admission_error_v1(
                                StrictOpenAdmissionCodeV1::ResourceLimitExceeded,
                                index,
                            )
                        })?;
                    let graph_bytes = url
                        .len()
                        .checked_add(semantic.retained_bytes_v1())
                        .and_then(|bytes| u64::try_from(bytes).ok())
                        .ok_or_else(|| {
                            admission_error_v1(
                                StrictOpenAdmissionCodeV1::ResourceLimitExceeded,
                                index,
                            )
                        })?;
                    let ingress_permit =
                        CombinedCopyPermit::prepare([url_string, topic_string], graph_bytes)
                            .map_err(|_err| {
                                admission_error_v1(
                                    StrictOpenAdmissionCodeV1::ResourceLimitExceeded,
                                    index,
                                )
                            })?;
                    let url = SecretUrl::parse(url, ingress, url_limits).map_err(|_err| {
                        admission_error_v1(StrictOpenAdmissionCodeV1::InvalidUrl, index)
                    })?;
                    StagedStrictOpenOperationV1 {
                        route,
                        url,
                        semantic,
                        ingress_permit,
                    }
                }
                StrictOpenRequestSpecV1::UnsupportedRoute => {
                    return Err(admission_error_v1(
                        StrictOpenAdmissionCodeV1::UnsupportedStrictOpenRoute,
                        index,
                    ));
                }
            };
            staged.push(staged_operation);
        }

        let mut operations = Vec::with_capacity(staged.len());
        for staged_operation in staged {
            let operation_id = self.allocate_identity_v1()?;
            let public_request_id = self.allocate_identity_v1()?;
            let public_recording_id = self.allocate_identity_v1()?;
            let status_owner = OpenSourceStatusOwnerV1::new_fresh_v1();
            let source_token = status_owner.source_token();
            operations.push(PreparedStrictOpenOperationV1 {
                operation_id: OpenOperationIdentity::new(operation_id),
                public_request_id: PublicOpenRequestIdentity::new(public_request_id),
                public_recording_id: PublicRecordingIdentity::new(public_recording_id),
                source_token,
                status_owner,
                route: staged_operation.route,
                url: staged_operation.url,
                semantic: staged_operation.semantic,
                ingress_permit: staged_operation.ingress_permit,
            });
        }

        Ok(PreparedStrictOpenBatchV1 { operations })
    }

    fn allocate_identity_v1(&mut self) -> Result<u128, StrictOpenWireErrorV1> {
        let identity = self.next_identity;
        self.next_identity = self.next_identity.checked_add(1).ok_or_else(|| {
            StrictOpenWireErrorV1::admission(StrictOpenAdmissionCodeV1::ResourceLimitExceeded, None)
        })?;
        Ok(identity)
    }
}

struct StagedStrictOpenOperationV1 {
    route: PreparedStrictOpenRouteV1,
    url: SecretUrl,
    semantic: RemoteMcapSemanticConfigV1,
    ingress_permit: CombinedCopyPermit,
}

fn admission_error_v1(code: StrictOpenAdmissionCodeV1, index: usize) -> StrictOpenWireErrorV1 {
    StrictOpenWireErrorV1::admission(
        code,
        Some(u32::try_from(index).expect("strict open batch index fits u32")),
    )
}

fn classify_http_remote_mcap_route_v1(
    raw_url: &[u8],
) -> Result<PreparedStrictOpenRouteV1, StrictOpenAdmissionCodeV1> {
    if !(starts_with_ignore_ascii_case_v1(raw_url, b"http://")
        || starts_with_ignore_ascii_case_v1(raw_url, b"https://"))
    {
        return Err(StrictOpenAdmissionCodeV1::UnsupportedStrictOpenRoute);
    }

    let path_end = raw_url
        .iter()
        .position(|byte| matches!(byte, b'?' | b'#'))
        .unwrap_or(raw_url.len());
    let scheme_end = raw_url
        .windows(3)
        .position(|window| window == b"://")
        .ok_or(StrictOpenAdmissionCodeV1::InvalidUrl)?;
    let authority_start = scheme_end + 3;
    let path_start = raw_url[authority_start..path_end]
        .iter()
        .position(|byte| *byte == b'/')
        .map_or(path_end, |relative| authority_start + relative);
    let path = &raw_url[path_start..path_end];
    if ends_with_ignore_ascii_case_v1(path, b".mcap") {
        return Ok(PreparedStrictOpenRouteV1::KnownRemoteMcap);
    }

    let last_segment = path
        .iter()
        .rposition(|byte| *byte == b'/')
        .map_or(path, |index| &path[index + 1..]);
    if last_segment.contains(&b'.') {
        return Err(StrictOpenAdmissionCodeV1::UnsupportedStrictOpenRoute);
    }
    Ok(PreparedStrictOpenRouteV1::ExtensionlessSniff)
}

fn starts_with_ignore_ascii_case_v1(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack.len() >= prefix.len() && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn ends_with_ignore_ascii_case_v1(haystack: &[u8], suffix: &[u8]) -> bool {
    haystack.len() >= suffix.len()
        && haystack[haystack.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

#[cfg(test)]
mod tests {
    use re_log_types::TimeType;

    use crate::remote_validator::{RepresentationConsistency, RepresentationConsistencyPolicy};

    use super::*;

    fn limits() -> SecretUrlParserLimits {
        SecretUrlParserLimits::explicit_for_internal_v1(
            4_096, 65_536, 1_024, 16, 256, 262_144, 1_048_576,
        )
    }

    fn semantic() -> RemoteMcapSemanticConfigV1 {
        RemoteMcapSemanticConfigV1::new_v1(
            b"topic:*".to_vec().into_boxed_slice(),
            1,
            1,
            TimeType::Sequence,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistency::StrongValidator,
        )
    }

    fn http(url: &str) -> StrictOpenRequestSpecV1 {
        StrictOpenRequestSpecV1::HttpRemoteMcapCandidate {
            url: url.as_bytes().to_vec().into_boxed_slice(),
            ingress: HttpUrlIngress::DirectExternal,
            semantic: semantic(),
        }
    }

    fn admission_code(error: StrictOpenWireErrorV1) -> (StrictOpenAdmissionCodeV1, Option<u32>) {
        let StrictOpenWireErrorV1::Admission(error) = error else {
            panic!("expected admission error");
        };
        (error.code, error.failed_index)
    }

    #[test]
    fn successful_batch_prepares_known_mcap_and_extensionless_without_starting_work() {
        let mut context = StrictOpenBatchPrepareContextV1::new_v1(4);
        let prepared = context
            .prepare_batch_v1(
                vec![
                    http("https://example.invalid/a.mcap?token=secret"),
                    http("https://example.invalid/no-extension"),
                ],
                &limits(),
            )
            .unwrap();
        assert_eq!(prepared.operations_v1().len(), 2);
        assert_eq!(prepared.extensionless_sniff_count_v1(), 1);
        assert_eq!(
            prepared.operations_v1()[0].route,
            PreparedStrictOpenRouteV1::KnownRemoteMcap
        );
        assert_eq!(
            prepared.operations_v1()[1].route,
            PreparedStrictOpenRouteV1::ExtensionlessSniff
        );
        assert_ne!(
            prepared.operations_v1()[0].operation_id,
            prepared.operations_v1()[1].operation_id
        );
        assert_ne!(
            prepared.operations_v1()[0].public_request_id,
            prepared.operations_v1()[1].public_request_id
        );
        assert_ne!(
            prepared.operations_v1()[0].public_recording_id,
            prepared.operations_v1()[1].public_recording_id
        );
    }

    #[test]
    fn invalid_later_item_returns_indexed_error_and_no_prepared_prefix() {
        let mut context = StrictOpenBatchPrepareContextV1::new_v1(4);
        let error = context
            .prepare_batch_v1(
                vec![
                    http("https://example.invalid/a.mcap"),
                    http("https://example.invalid/not-mcap.rrd"),
                ],
                &limits(),
            )
            .unwrap_err();
        assert_eq!(
            admission_code(error),
            (
                StrictOpenAdmissionCodeV1::UnsupportedStrictOpenRoute,
                Some(1)
            )
        );
    }

    #[test]
    fn unsupported_non_http_route_never_falls_back_to_compatibility_dispatch() {
        let mut context = StrictOpenBatchPrepareContextV1::new_v1(4);
        let error = context
            .prepare_batch_v1(vec![StrictOpenRequestSpecV1::UnsupportedRoute], &limits())
            .unwrap_err();
        assert_eq!(
            admission_code(error),
            (
                StrictOpenAdmissionCodeV1::UnsupportedStrictOpenRoute,
                Some(0)
            )
        );

        let error = context
            .prepare_batch_v1(vec![http("rerun+http://127.0.0.1:9876/proxy")], &limits())
            .unwrap_err();
        assert_eq!(
            admission_code(error),
            (
                StrictOpenAdmissionCodeV1::UnsupportedStrictOpenRoute,
                Some(0)
            )
        );
    }

    #[test]
    fn batch_too_large_fails_before_any_identity_is_allocated() {
        let mut context = StrictOpenBatchPrepareContextV1::new_v1(1);
        let error = context
            .prepare_batch_v1(
                vec![
                    http("https://example.invalid/a.mcap"),
                    http("https://example.invalid/b.mcap"),
                ],
                &limits(),
            )
            .unwrap_err();
        assert_eq!(
            admission_code(error),
            (StrictOpenAdmissionCodeV1::BatchTooLarge, None)
        );
        let prepared = context
            .prepare_batch_v1(vec![http("https://example.invalid/c.mcap")], &limits())
            .unwrap();
        assert_eq!(
            prepared.operations_v1()[0].operation_id,
            OpenOperationIdentity::new(1)
        );
        let first = &prepared.operations_v1()[0];
        assert_eq!(first.status_owner.source_token(), first.source_token);
        assert_eq!(
            first.semantic.retained_bytes_v1(),
            semantic().retained_bytes_v1()
        );
        assert_eq!(first.url.scheme().to_string(), "https");
        assert_eq!(first.public_recording_id, PublicRecordingIdentity::new(3));
    }

    #[test]
    fn ingress_graph_limit_fails_before_identity_or_secret_url_materialization() {
        let oversized = RemoteMcapSemanticConfigV1::new_v1(
            vec![b'x'; crate::external_string_ingress::MAX_COMBINED_UTF8 as usize]
                .into_boxed_slice(),
            1,
            1,
            TimeType::Sequence,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistency::StrongValidator,
        );
        let mut context = StrictOpenBatchPrepareContextV1::new_v1(1);
        let error = context
            .prepare_batch_v1(
                vec![StrictOpenRequestSpecV1::HttpRemoteMcapCandidate {
                    url: b"https://example.invalid/a.mcap"
                        .to_vec()
                        .into_boxed_slice(),
                    ingress: HttpUrlIngress::DirectExternal,
                    semantic: oversized,
                }],
                &limits(),
            )
            .unwrap_err();
        assert_eq!(
            admission_code(error),
            (StrictOpenAdmissionCodeV1::ResourceLimitExceeded, Some(0))
        );
        let prepared = context
            .prepare_batch_v1(vec![http("https://example.invalid/ok.mcap")], &limits())
            .unwrap();
        assert_eq!(
            prepared.operations_v1()[0].operation_id,
            OpenOperationIdentity::new(1)
        );
    }
}
