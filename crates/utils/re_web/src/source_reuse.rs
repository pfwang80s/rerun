//! Production-disarmed secret URL reuse classification for Web remote sources.
//!
//! This module only models secret-safe fingerprint bucketing and exact semantic reuse.
//! It does not hook into native viewer routing, compatibility `open()`, or live transport.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::mem::size_of;

use hmac::{Hmac, Mac};
use re_log_types::TimeType;
use sha2::Sha256;

use crate::open_source_terminal::OpenSourceToken;
use crate::remote_validator::{RepresentationConsistency, RepresentationConsistencyPolicy};
use crate::secret_url::{RouteCanonicalUrl, SecretUrl};

type HmacSha256 = Hmac<Sha256>;

/// One keyed fingerprint over a canonical URL.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SecretUrlFingerprintV1([u8; 32]);

impl fmt::Debug for SecretUrlFingerprintV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretUrlFingerprintV1(<opaque>)")
    }
}

/// A policy that maps a canonical URL to a secret-safe fingerprint.
pub trait SourceReuseFingerprintPolicyV1 {
    fn fingerprint_v1(&self, canonical: &RouteCanonicalUrl) -> SecretUrlFingerprintV1;
}

/// Instance-keyed HMAC-SHA256 fingerprinting.
#[derive(Clone)]
pub struct InstanceKeyedHmacFingerprintPolicyV1 {
    key: [u8; 32],
}

impl InstanceKeyedHmacFingerprintPolicyV1 {
    pub fn new_v1(key: [u8; 32]) -> Self {
        Self { key }
    }
}

impl SourceReuseFingerprintPolicyV1 for InstanceKeyedHmacFingerprintPolicyV1 {
    fn fingerprint_v1(&self, canonical: &RouteCanonicalUrl) -> SecretUrlFingerprintV1 {
        let mut mac =
            HmacSha256::new_from_slice(&self.key).expect("a 32-byte HMAC key is always valid");
        canonical.expose_for_fingerprint(|bytes| mac.update(bytes));
        let digest = mac.finalize().into_bytes();
        SecretUrlFingerprintV1(digest.into())
    }
}

/// Exact semantic identity for one remote source request.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteMcapSemanticConfigV1 {
    topic_filter_canonical_bytes: Box<[u8]>,
    decoder_allowlist_version: u64,
    assignment_policy_version: u64,
    mcap_time_type: TimeType,
    requested_consistency_policy: RepresentationConsistencyPolicy,
    actual_consistency: RepresentationConsistency,
}

impl RemoteMcapSemanticConfigV1 {
    pub(crate) fn topic_filter_bytes_v1(&self) -> &[u8] {
        &self.topic_filter_canonical_bytes
    }
    pub fn new_v1(
        topic_filter_canonical_bytes: Box<[u8]>,
        decoder_allowlist_version: u64,
        assignment_policy_version: u64,
        mcap_time_type: TimeType,
        requested_consistency_policy: RepresentationConsistencyPolicy,
        actual_consistency: RepresentationConsistency,
    ) -> Self {
        Self {
            topic_filter_canonical_bytes,
            decoder_allowlist_version,
            assignment_policy_version,
            mcap_time_type,
            requested_consistency_policy,
            actual_consistency,
        }
    }

    pub fn retained_bytes_v1(&self) -> usize {
        size_of::<Self>() + self.topic_filter_canonical_bytes.len()
    }
}

impl fmt::Debug for RemoteMcapSemanticConfigV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteMcapSemanticConfigV1")
            .field("topic_filter_canonical_bytes", &"<redacted>")
            .field("decoder_allowlist_version", &self.decoder_allowlist_version)
            .field("assignment_policy_version", &self.assignment_policy_version)
            .field("mcap_time_type", &self.mcap_time_type)
            .field(
                "requested_consistency_policy",
                &self.requested_consistency_policy,
            )
            .field("actual_consistency", &self.actual_consistency)
            .finish()
    }
}

/// One source entry retained under a fingerprint bucket.
struct SourceReuseEntryV1 {
    source_token: OpenSourceToken,
    url: SecretUrl,
    semantic: RemoteMcapSemanticConfigV1,
}

impl SourceReuseEntryV1 {
    fn retained_bytes_v1(&self) -> usize {
        self.url.retained_bytes() + self.semantic.retained_bytes_v1() + size_of::<Self>()
    }
}

/// The decision returned when a request is checked against the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceReuseDecisionV1 {
    ReusedExisting { source_token: OpenSourceToken },
    InsertedFresh { source_token: OpenSourceToken },
}

/// Fixed failures from fingerprint lookup, semantic mismatch, or registry limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceReuseRegistryErrorV1 {
    ExistingSourceOptionsConflict,
    SourceLimitReached,
    RetainedBytesExceeded,
    TokenExhausted,
}

/// A bounded multimap keyed by secret-safe URL fingerprints.
pub struct SourceReuseRegistryV1<P> {
    policy: P,
    buckets: HashMap<SecretUrlFingerprintV1, Vec<SourceReuseEntryV1>>,
    next_source_token: u64,
    max_fingerprint_buckets: usize,
    max_entries_per_bucket: usize,
    max_retained_bytes: usize,
    retained_bytes: usize,
}

impl<P: SourceReuseFingerprintPolicyV1> SourceReuseRegistryV1<P> {
    pub fn new_v1(
        policy: P,
        max_fingerprint_buckets: usize,
        max_entries_per_bucket: usize,
        max_retained_bytes: usize,
    ) -> Self {
        Self {
            policy,
            buckets: HashMap::new(),
            next_source_token: 1,
            max_fingerprint_buckets,
            max_entries_per_bucket,
            max_retained_bytes,
            retained_bytes: 0,
        }
    }

    fn allocate_source_token_v1(&mut self) -> Result<OpenSourceToken, SourceReuseRegistryErrorV1> {
        let token = OpenSourceToken::new_v1(self.next_source_token)
            .ok_or(SourceReuseRegistryErrorV1::TokenExhausted)?;
        self.next_source_token = self
            .next_source_token
            .checked_add(1)
            .ok_or(SourceReuseRegistryErrorV1::TokenExhausted)?;
        Ok(token)
    }

    fn fingerprint_v1(&self, url: &SecretUrl) -> SecretUrlFingerprintV1 {
        self.policy.fingerprint_v1(url.route_canonical())
    }

    pub fn bucket_count_v1(&self) -> usize {
        self.buckets.len()
    }

    pub fn retained_bytes_v1(&self) -> usize {
        self.retained_bytes
    }

    pub fn classify_or_insert_v1(
        &mut self,
        url: SecretUrl,
        semantic: RemoteMcapSemanticConfigV1,
    ) -> Result<SourceReuseDecisionV1, SourceReuseRegistryErrorV1> {
        let fingerprint = self.fingerprint_v1(&url);
        let bucket_exists = self.buckets.contains_key(&fingerprint);
        let bucket_count_before = self.buckets.len();

        if let Some(existing) = self.buckets.get(&fingerprint).and_then(|bucket| {
            bucket
                .iter()
                .find(|entry| entry.url.route_canonical().matches(url.route_canonical()))
        }) {
            if existing.semantic == semantic {
                return Ok(SourceReuseDecisionV1::ReusedExisting {
                    source_token: existing.source_token,
                });
            }
            return Err(SourceReuseRegistryErrorV1::ExistingSourceOptionsConflict);
        }

        let bucket_len = self.buckets.get(&fingerprint).map_or(0, Vec::len);
        if bucket_len >= self.max_entries_per_bucket
            || (!bucket_exists && bucket_count_before >= self.max_fingerprint_buckets)
        {
            return Err(SourceReuseRegistryErrorV1::SourceLimitReached);
        }

        let added_bytes =
            url.retained_bytes() + semantic.retained_bytes_v1() + size_of::<SourceReuseEntryV1>();
        let Some(next_retained_bytes) = self.retained_bytes.checked_add(added_bytes) else {
            return Err(SourceReuseRegistryErrorV1::RetainedBytesExceeded);
        };
        if next_retained_bytes > self.max_retained_bytes {
            return Err(SourceReuseRegistryErrorV1::RetainedBytesExceeded);
        }
        let source_token = self.allocate_source_token_v1()?;
        let entry = SourceReuseEntryV1 {
            source_token,
            semantic,
            url,
        };
        self.retained_bytes = next_retained_bytes;
        self.buckets.entry(fingerprint).or_default().push(entry);
        Ok(SourceReuseDecisionV1::InsertedFresh { source_token })
    }

    pub fn remove_by_source_token_v1(&mut self, source_token: OpenSourceToken) -> bool {
        let mut removed_bytes = 0usize;
        let mut removed = false;
        let mut empty_buckets = Vec::new();

        for (fingerprint, bucket) in &mut self.buckets {
            if let Some(index) = bucket
                .iter()
                .position(|entry| entry.source_token == source_token)
            {
                let entry = bucket.remove(index);
                removed_bytes = entry.retained_bytes_v1();
                removed = true;
                if bucket.is_empty() {
                    empty_buckets.push(*fingerprint);
                }
                break;
            }
        }

        for fingerprint in empty_buckets {
            self.buckets.remove(&fingerprint);
        }

        if removed {
            self.retained_bytes = self.retained_bytes.saturating_sub(removed_bytes);
        }
        removed
    }
}

impl fmt::Debug for SourceReuseRegistryV1<InstanceKeyedHmacFingerprintPolicyV1> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceReuseRegistryV1")
            .field("bucket_count", &self.bucket_count_v1())
            .field("retained_bytes", &self.retained_bytes_v1())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct ConstantFingerprintPolicyV1(SecretUrlFingerprintV1);

    impl SourceReuseFingerprintPolicyV1 for ConstantFingerprintPolicyV1 {
        fn fingerprint_v1(&self, _canonical: &RouteCanonicalUrl) -> SecretUrlFingerprintV1 {
            self.0
        }
    }

    fn parse(input: &str) -> SecretUrl {
        SecretUrl::parse_for_test(input).unwrap()
    }

    fn semantic(
        topic_filter: &str,
        decoder_allowlist_version: u64,
        assignment_policy_version: u64,
        mcap_time_type: TimeType,
        requested_consistency_policy: RepresentationConsistencyPolicy,
        actual_consistency: RepresentationConsistency,
    ) -> RemoteMcapSemanticConfigV1 {
        RemoteMcapSemanticConfigV1::new_v1(
            topic_filter.as_bytes().to_vec().into_boxed_slice(),
            decoder_allowlist_version,
            assignment_policy_version,
            mcap_time_type,
            requested_consistency_policy,
            actual_consistency,
        )
    }

    #[test]
    fn exact_canonical_match_reuses_existing_source_when_semantics_match() {
        let key = [7; 32];
        let mut registry = SourceReuseRegistryV1::new_v1(
            InstanceKeyedHmacFingerprintPolicyV1::new_v1(key),
            8,
            8,
            16_384,
        );
        let semantic = semantic(
            "/topic",
            1,
            2,
            TimeType::Sequence,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistency::StrongValidator,
        );

        let first = registry
            .classify_or_insert_v1(parse("https://example.com/a?x=1"), semantic.clone())
            .unwrap();
        let second = registry
            .classify_or_insert_v1(parse("https://EXAMPLE.com:443/a?x=1"), semantic.clone())
            .unwrap();

        let SourceReuseDecisionV1::InsertedFresh { source_token } = first else {
            panic!("first insert should be fresh");
        };
        let SourceReuseDecisionV1::ReusedExisting {
            source_token: reused,
        } = second
        else {
            panic!("second insert should reuse");
        };
        assert_eq!(source_token, reused);
        assert_eq!(registry.bucket_count_v1(), 1);
    }

    #[test]
    fn semantic_difference_is_an_exact_conflict() {
        let key = [11; 32];
        let mut registry = SourceReuseRegistryV1::new_v1(
            InstanceKeyedHmacFingerprintPolicyV1::new_v1(key),
            8,
            8,
            16_384,
        );
        let baseline = semantic(
            "/topic",
            1,
            2,
            TimeType::Sequence,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistency::StrongValidator,
        );
        registry
            .classify_or_insert_v1(parse("https://example.com/a"), baseline.clone())
            .unwrap();

        for conflicting in [
            semantic(
                "/topic-2",
                1,
                2,
                TimeType::Sequence,
                RepresentationConsistencyPolicy::RequireStrongValidator,
                RepresentationConsistency::StrongValidator,
            ),
            semantic(
                "/topic",
                2,
                2,
                TimeType::Sequence,
                RepresentationConsistencyPolicy::RequireStrongValidator,
                RepresentationConsistency::StrongValidator,
            ),
            semantic(
                "/topic",
                1,
                3,
                TimeType::Sequence,
                RepresentationConsistencyPolicy::RequireStrongValidator,
                RepresentationConsistency::StrongValidator,
            ),
            semantic(
                "/topic",
                1,
                2,
                TimeType::DurationNs,
                RepresentationConsistencyPolicy::RequireStrongValidator,
                RepresentationConsistency::StrongValidator,
            ),
            semantic(
                "/topic",
                1,
                2,
                TimeType::Sequence,
                RepresentationConsistencyPolicy::AllowDeploymentAssumed,
                RepresentationConsistency::StrongValidator,
            ),
            semantic(
                "/topic",
                1,
                2,
                TimeType::Sequence,
                RepresentationConsistencyPolicy::RequireStrongValidator,
                RepresentationConsistency::DeploymentAssumed,
            ),
        ] {
            assert_eq!(
                registry
                    .classify_or_insert_v1(parse("https://example.com/a"), conflicting)
                    .unwrap_err(),
                SourceReuseRegistryErrorV1::ExistingSourceOptionsConflict
            );
        }
    }

    #[test]
    fn fingerprint_collisions_are_deterministic_and_bounded() {
        let mut registry = SourceReuseRegistryV1::new_v1(
            ConstantFingerprintPolicyV1(SecretUrlFingerprintV1([1; 32])),
            1,
            2,
            32_768,
        );
        let semantic = semantic(
            "/topic",
            1,
            2,
            TimeType::Sequence,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistency::StrongValidator,
        );

        let first = registry
            .classify_or_insert_v1(parse("https://example.com/a"), semantic.clone())
            .unwrap();
        let second = registry
            .classify_or_insert_v1(parse("https://example.com/b"), semantic.clone())
            .unwrap();
        let third = registry
            .classify_or_insert_v1(parse("https://example.com/a?query=1"), semantic)
            .unwrap_err();

        assert!(matches!(first, SourceReuseDecisionV1::InsertedFresh { .. }));
        assert!(matches!(
            second,
            SourceReuseDecisionV1::InsertedFresh { .. }
        ));
        assert_eq!(third, SourceReuseRegistryErrorV1::SourceLimitReached);
        assert_eq!(registry.bucket_count_v1(), 1);
    }

    #[test]
    fn removal_zeroizes_the_retained_secret_url() {
        let mut registry = SourceReuseRegistryV1::new_v1(
            InstanceKeyedHmacFingerprintPolicyV1::new_v1([3; 32]),
            4,
            4,
            16_384,
        );
        let semantic = semantic(
            "/topic",
            1,
            2,
            TimeType::Sequence,
            RepresentationConsistencyPolicy::RequireStrongValidator,
            RepresentationConsistency::StrongValidator,
        );

        crate::secret_url::reset_test_drop_counts();
        let inserted = registry
            .classify_or_insert_v1(parse("https://example.com/a?secret=1"), semantic)
            .unwrap();
        let SourceReuseDecisionV1::InsertedFresh { source_token } = inserted else {
            panic!("expected a fresh insert");
        };
        assert!(registry.remove_by_source_token_v1(source_token));
        assert_eq!(crate::secret_url::test_drop_counts(), (3, 3));
    }
}
