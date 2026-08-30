//! Bounded runtime-identifier admission for the disarmed remote-MCAP decode chain.
//!
//! Every Summary and Chunk unit enters the MCAP-012 side-map transaction before a manifest,
//! executable parser, or Store mutation can be constructed.

#![allow(dead_code)]

use re_chunk::{EntityPath, TimelineName};
use re_log_types::EntityPathPart;
use re_sdk_types::{ArchetypeName, ComponentIdentifier};
use re_string_interner::bounded_runtime_intern::{
    self as intern, BoundedRemoteIdentifierCensus, CommittedRemoteInternBatch,
    RemoteInternBatchTelemetry, RemoteMcapRawIdentifier, RemoteMcapRuntimeInternError,
    RemoteMcapRuntimeInternLimits,
};
#[cfg(test)]
use std::num::NonZeroU64;

const REMOTE_RUNTIME_INTERN_CENSUS_LIMIT_V1: usize = 16 * 1024;
const REMOTE_RUNTIME_INTERN_CENSUS_BYTES_V1: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRuntimeInternAdmissionPhaseV1 {
    Opening,
    ActiveChunk,
}

impl From<RemoteMcapRuntimeInternError> for RemoteRuntimeInternAdmissionErrorV1 {
    fn from(error: RemoteMcapRuntimeInternError) -> Self {
        Self::from_intern(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRuntimeInternAdmissionErrorV1 {
    CensusLimitExceeded,
    BudgetExhausted,
    CoordinationRevisionExhausted,
    AllocationFailed,
    ProtocolViolation,
    ConstructorCannotBeDelayed,
}

impl RemoteRuntimeInternAdmissionErrorV1 {
    pub(crate) const fn terminal_kind_v1(
        phase: RemoteRuntimeInternAdmissionPhaseV1,
    ) -> RemoteRuntimeInternTerminalV1 {
        match phase {
            RemoteRuntimeInternAdmissionPhaseV1::Opening => {
                RemoteRuntimeInternTerminalV1::OpeningExhausted
            }
            RemoteRuntimeInternAdmissionPhaseV1::ActiveChunk => {
                RemoteRuntimeInternTerminalV1::ActiveSessionFatal
            }
        }
    }

    fn from_intern(error: RemoteMcapRuntimeInternError) -> Self {
        match error {
            RemoteMcapRuntimeInternError::ModuleBudgetNotInitialized
            | RemoteMcapRuntimeInternError::ModuleBudgetAlreadyInitializedWithDifferentLimits => {
                Self::ProtocolViolation
            }
            RemoteMcapRuntimeInternError::CensusIdentifierLimitExceeded
            | RemoteMcapRuntimeInternError::CensusRawBytesExceeded
            | RemoteMcapRuntimeInternError::CensusRetainedBytesExceeded => {
                Self::CensusLimitExceeded
            }
            RemoteMcapRuntimeInternError::CandidatePeakExceeded
            | RemoteMcapRuntimeInternError::AllocationFailed => Self::AllocationFailed,
            RemoteMcapRuntimeInternError::InvalidLimitProfile
            | RemoteMcapRuntimeInternError::ArithmeticOverflow
            | RemoteMcapRuntimeInternError::CensusCanonicalizationFailed
            | RemoteMcapRuntimeInternError::IdentifierHashCollision
            | RemoteMcapRuntimeInternError::ProtocolViolation => Self::ProtocolViolation,
            RemoteMcapRuntimeInternError::StringBudgetExceeded
            | RemoteMcapRuntimeInternError::EntryAndCapacityBudgetExceeded
            | RemoteMcapRuntimeInternError::SideMapEntryLimitExceeded
            | RemoteMcapRuntimeInternError::BudgetRevisionExhausted => Self::BudgetExhausted,
            RemoteMcapRuntimeInternError::CoordinationRevisionExhausted => {
                Self::CoordinationRevisionExhausted
            }
        }
    }
}

struct RemoteEntityPathTokenIter<'a> {
    bytes: &'a [u8],
}

impl<'a> RemoteEntityPathTokenIter<'a> {
    const fn new(raw: &'a str) -> Self {
        Self {
            bytes: raw.as_bytes(),
        }
    }
}

impl<'a> Iterator for RemoteEntityPathTokenIter<'a> {
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

        let token = std::str::from_utf8(&self.bytes[..index]).ok()?;
        self.bytes = &self.bytes[index..];
        Some(token)
    }
}

fn parse_remote_entity_path_unicode_escape<'a>(input: &mut &'a str) -> Result<char, &'a str> {
    let consumed_start = *input;
    let mut consumed_bytes = 0_usize;
    while let Some(character) = input.chars().next() {
        *input = &input[character.len_utf8()..];
        consumed_bytes += character.len_utf8();
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

fn push_remote_entity_path_unescaped_character(input: &mut &str, first: char, output: &mut String) {
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
        'u' => match parse_remote_entity_path_unicode_escape(input) {
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

fn canonicalize_remote_entity_path_part_into(raw: &str, output: &mut String) {
    let mut input = raw;
    while let Some(first) = input.chars().next() {
        input = &input[first.len_utf8()..];
        push_remote_entity_path_unescaped_character(&mut input, first, output);
    }
}

fn remote_entity_path_matches_v1(value: &EntityPath, raw_lookup: &str) -> bool {
    let mut actual_parts = value.iter();
    let mut raw_parts = RemoteEntityPathTokenIter::new(raw_lookup).filter(|token| *token != "/");
    let mut canonical = String::new();

    loop {
        match (actual_parts.next(), raw_parts.next()) {
            (Some(actual), Some(raw)) => {
                canonical.clear();
                canonicalize_remote_entity_path_part_into(raw, &mut canonical);
                if actual.unescaped_str() != canonical {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteRuntimeInternTerminalV1 {
    OpeningExhausted,
    ActiveSessionFatal,
}

#[derive(Debug)]
struct RawRuntimeIdentifierCensusV1<'a> {
    identifiers: Vec<RemoteMcapRawIdentifier<'a>>,
}

impl<'a> RawRuntimeIdentifierCensusV1<'a> {
    fn try_new<I>(identifiers: I) -> Result<Self, RemoteRuntimeInternAdmissionErrorV1>
    where
        I: IntoIterator<Item = RemoteMcapRawIdentifier<'a>>,
        I::IntoIter: Clone,
    {
        let input = identifiers.into_iter();
        let count = input.clone().count();
        if count > REMOTE_RUNTIME_INTERN_CENSUS_LIMIT_V1 {
            return Err(RemoteRuntimeInternAdmissionErrorV1::CensusLimitExceeded);
        }
        let mut raw_bytes = 0_u64;
        for identifier in input.clone() {
            let bytes = u64::try_from(identifier.as_str().len())
                .map_err(|_overflow| RemoteRuntimeInternAdmissionErrorV1::CensusLimitExceeded)?;
            raw_bytes = raw_bytes
                .checked_add(bytes)
                .ok_or(RemoteRuntimeInternAdmissionErrorV1::CensusLimitExceeded)?;
        }
        if raw_bytes > REMOTE_RUNTIME_INTERN_CENSUS_BYTES_V1 {
            return Err(RemoteRuntimeInternAdmissionErrorV1::CensusLimitExceeded);
        }

        let mut owned = Vec::new();
        owned
            .try_reserve_exact(count)
            .map_err(|_allocation_error| RemoteRuntimeInternAdmissionErrorV1::AllocationFailed)?;
        owned.extend(input);
        Ok(Self { identifiers: owned })
    }
}

#[derive(Debug)]
enum RemoteRuntimeDomainIdentifierV1 {
    Timeline(TimelineName),
    EntityPath { value: EntityPath },
    Component(ComponentIdentifier),
    Archetype(ArchetypeName),
}

/// The complete result of one Summary runtime-identifier admission.
pub(crate) struct RemoteRuntimeIdentifiersV1 {
    domains: Vec<RemoteRuntimeDomainIdentifierV1>,
    committed: CommittedRemoteInternBatch,
}

impl RemoteRuntimeIdentifiersV1 {
    pub(crate) fn timeline(&self, name: &str) -> Option<TimelineName> {
        self.domains.iter().find_map(|identifier| match identifier {
            RemoteRuntimeDomainIdentifierV1::Timeline(value) => {
                (value.as_str() == name).then_some(*value)
            }
            _ => None,
        })
    }

    pub(crate) fn entity_path(&self, raw_lookup: &str) -> Option<EntityPath> {
        self.domains.iter().find_map(|identifier| match identifier {
            RemoteRuntimeDomainIdentifierV1::EntityPath { value } => {
                remote_entity_path_matches_v1(value, raw_lookup).then_some(value.clone())
            }
            _ => None,
        })
    }

    pub(crate) fn component(&self, raw: &str) -> Option<ComponentIdentifier> {
        self.domains.iter().find_map(|identifier| match identifier {
            RemoteRuntimeDomainIdentifierV1::Component(value) => {
                (value.as_str() == raw).then_some(*value)
            }
            _ => None,
        })
    }

    pub(crate) fn archetype(&self, raw: &str) -> Option<ArchetypeName> {
        self.domains.iter().find_map(|identifier| match identifier {
            RemoteRuntimeDomainIdentifierV1::Archetype(value) => {
                (value.as_str() == raw).then_some(*value)
            }
            _ => None,
        })
    }

    pub(crate) fn telemetry(&self) -> RemoteInternBatchTelemetry {
        self.committed.telemetry()
    }
}

/// Proof that one Chunk unit completed its descriptor census before parser construction.
pub(crate) struct RemoteChunkRuntimeIdentifiersV1 {
    committed: CommittedRemoteInternBatch,
}

impl RemoteChunkRuntimeIdentifiersV1 {
    pub(crate) fn telemetry(&self) -> RemoteInternBatchTelemetry {
        self.committed.telemetry()
    }
}

fn commit_census(
    census: BoundedRemoteIdentifierCensus,
) -> Result<CommittedRemoteInternBatch, RemoteRuntimeInternAdmissionErrorV1> {
    intern::prepare_remote_mcap_runtime_intern_from_domain_construction_token(
        census.into_domain_construction_token(),
    )
    .and_then(intern::PreparedRemoteInternBatch::commit)
    .map_err(RemoteRuntimeInternAdmissionErrorV1::from_intern)
}

fn commit_raw_census(
    raw: &RawRuntimeIdentifierCensusV1<'_>,
) -> Result<CommittedRemoteInternBatch, RemoteRuntimeInternAdmissionErrorV1> {
    BoundedRemoteIdentifierCensus::try_new_v1(raw.identifiers.iter().copied())
        .map_err(RemoteRuntimeInternAdmissionErrorV1::from_intern)
        .and_then(commit_census)
}

fn redeem_summary_domain_handles_v1(
    committed: &CommittedRemoteInternBatch,
) -> Result<Vec<RemoteRuntimeDomainIdentifierV1>, RemoteRuntimeInternAdmissionErrorV1> {
    let mut domains = Vec::new();
    let domain_capacity = committed
        .len()
        .checked_mul(2)
        .ok_or(RemoteRuntimeInternAdmissionErrorV1::CensusLimitExceeded)?;
    domains
        .try_reserve_exact(domain_capacity)
        .map_err(|_allocation_error| RemoteRuntimeInternAdmissionErrorV1::AllocationFailed)?;
    for index in 0..committed.len() {
        let handle = committed
            .domain_handle(index)
            .ok_or(RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation)?;
        let domain = match handle {
            intern::RemoteMcapDomainIdentifierHandle::Timeline(handle) => {
                RemoteRuntimeDomainIdentifierV1::Timeline(
                    TimelineName::try_from_interned(handle)
                        .map_err(|_error| RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation)?,
                )
            }
            intern::RemoteMcapDomainIdentifierHandle::EntityPathPart(_) => {
                return Err(RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation);
            }
            intern::RemoteMcapDomainIdentifierHandle::Component(handle) => {
                RemoteRuntimeDomainIdentifierV1::Component(
                    ComponentIdentifier::try_from_interned(handle)
                        .map_err(|_error| RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation)?,
                )
            }
            intern::RemoteMcapDomainIdentifierHandle::EntityPath(parts) => {
                let value = parts
                    .iter()
                    .map(|part| EntityPathPart::new(*part))
                    .collect::<EntityPath>();
                RemoteRuntimeDomainIdentifierV1::EntityPath { value }
            }
        };
        domains.push(domain);
        if let intern::RemoteMcapDomainIdentifierHandle::Component(handle) = handle {
            domains.push(RemoteRuntimeDomainIdentifierV1::Archetype(
                ArchetypeName::try_from_interned(handle)
                    .map_err(|_error| RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation)?,
            ));
        }
    }
    Ok(domains)
}

pub(crate) struct RemoteRuntimeDecoderIdentifierV1 {
    pub(crate) topic: String,
    pub(crate) archetype: String,
    pub(crate) component: String,
}

pub(crate) fn archetype_short_name_v1(archetype: &str) -> &str {
    archetype
        .strip_prefix("rerun.archetypes.")
        .or_else(|| archetype.strip_prefix("rerun.blueprint.archetypes."))
        .unwrap_or(archetype)
}

pub(crate) fn admit_summary_identifiers_with_decoders_v1(
    definitions: &crate::remote_summary::definitions::ValidatedSummaryDefinitions<'_>,
    decoder_identifiers: &[RemoteRuntimeDecoderIdentifierV1],
) -> Result<RemoteRuntimeIdentifiersV1, RemoteRuntimeInternAdmissionErrorV1> {
    let mut identifiers = summary_definition_identifiers_v1(definitions)?;
    for decoder in decoder_identifiers {
        identifiers.push(RemoteMcapRawIdentifier::entity_path(&decoder.topic));
        identifiers.push(RemoteMcapRawIdentifier::component(&decoder.archetype)?);
        identifiers.push(RemoteMcapRawIdentifier::component(&decoder.component)?);
    }

    let raw = RawRuntimeIdentifierCensusV1::try_new(identifiers)?;
    let committed = commit_raw_census(&raw)?;
    let domains = redeem_summary_domain_handles_v1(&committed)?;

    Ok(RemoteRuntimeIdentifiersV1 { domains, committed })
}

fn summary_definition_identifiers_v1<'a>(
    definitions: &'a crate::remote_summary::definitions::ValidatedSummaryDefinitions<'a>,
) -> Result<Vec<RemoteMcapRawIdentifier<'a>>, RemoteRuntimeInternAdmissionErrorV1> {
    let mut identifiers = Vec::new();
    identifiers.push(raw_identifier(RemoteMcapRawIdentifier::timeline(
        "message_log_time",
    ))?);
    identifiers.push(raw_identifier(RemoteMcapRawIdentifier::timeline(
        "message_publish_time",
    ))?);
    identifiers.push(raw_identifier(RemoteMcapRawIdentifier::component(
        "message",
    ))?);

    for projection in definitions.projection_records() {
        match projection {
            crate::remote_summary::definitions::SummaryDefinitionProjectionRecord::Channel {
                record_index,
                ..
            } => {
                let Some(channel) = definitions.channel_at_record(record_index) else {
                    return Err(RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation);
                };
                identifiers.push(RemoteMcapRawIdentifier::entity_path(&channel.topic));
            }
            crate::remote_summary::definitions::SummaryDefinitionProjectionRecord::Schema {
                record_index,
                ..
            } => {
                let Some(schema) = definitions.schema_at_record(record_index) else {
                    return Err(RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation);
                };
                identifiers.push(raw_identifier(RemoteMcapRawIdentifier::component(
                    &schema.header.name,
                ))?);
            }
        }
    }
    Ok(identifiers)
}

pub(crate) fn admit_summary_identifiers_v1(
    definitions: &crate::remote_summary::definitions::ValidatedSummaryDefinitions<'_>,
) -> Result<RemoteRuntimeIdentifiersV1, RemoteRuntimeInternAdmissionErrorV1> {
    admit_summary_identifiers_with_decoders_v1(definitions, &[])
}

fn raw_identifier(
    identifier: Result<RemoteMcapRawIdentifier<'_>, RemoteMcapRuntimeInternError>,
) -> Result<RemoteMcapRawIdentifier<'_>, RemoteRuntimeInternAdmissionErrorV1> {
    identifier.map_err(RemoteRuntimeInternAdmissionErrorV1::from_intern)
}

pub(crate) fn admit_chunk_identifiers_v1<'a, I>(
    descriptors: I,
) -> Result<RemoteChunkRuntimeIdentifiersV1, RemoteRuntimeInternAdmissionErrorV1>
where
    I: IntoIterator<Item = &'a crate::remote_typed_output::RemoteTypedOutputDescriptorV1>,
{
    let mut identifiers = Vec::new();
    for descriptor in descriptors {
        identifiers.push(RemoteMcapRawIdentifier::timeline("message_log_time")?);
        identifiers.push(RemoteMcapRawIdentifier::timeline("message_publish_time")?);
        identifiers.push(RemoteMcapRawIdentifier::entity_path(
            descriptor.entity_path_raw_v1(),
        ));
        identifiers.push(RemoteMcapRawIdentifier::component(
            descriptor.component_v1().component.as_str(),
        )?);
        if let Some(archetype) = descriptor.component_v1().archetype {
            identifiers.push(RemoteMcapRawIdentifier::component(archetype.as_str())?);
        }
    }
    let raw = RawRuntimeIdentifierCensusV1::try_new(identifiers)?;
    let committed = commit_raw_census(&raw)?;
    Ok(RemoteChunkRuntimeIdentifiersV1 { committed })
}

pub(crate) fn reject_undelayable_constructor_v1() -> RemoteRuntimeInternAdmissionErrorV1 {
    RemoteRuntimeInternAdmissionErrorV1::ConstructorCannotBeDelayed
}

pub(crate) fn initialize_disarmed_v1(
    limits: RemoteMcapRuntimeInternLimits,
) -> Result<
    re_string_interner::bounded_runtime_intern::RemoteMcapRuntimeInternSnapshot,
    RemoteRuntimeInternAdmissionErrorV1,
> {
    intern::initialize_remote_mcap_runtime_intern(limits)
        .map_err(RemoteRuntimeInternAdmissionErrorV1::from_intern)
}

#[cfg(test)]
pub(crate) fn ensure_disarmed_test_profile_v1() {
    let limits = RemoteMcapRuntimeInternLimits::from_profile_values(
        NonZeroU64::new(4 * 1024 * 1024).unwrap(),
        NonZeroU64::new(4 * 1024 * 1024).unwrap(),
        NonZeroU64::new(16 * 1024).unwrap(),
        NonZeroU64::new(512 * 1024).unwrap(),
        NonZeroU64::new(4 * 1024 * 1024).unwrap(),
    );
    initialize_disarmed_v1(limits).expect("the disarmed interner profile is valid");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        AdversarialMcapFixtureBuilder, FixtureChannel, FixtureChunk, FixtureMessage, FixtureSchema,
    };
    use re_string_interner::InternedString;
    use re_string_interner::bounded_runtime_intern::remote_mcap_runtime_intern_snapshot;

    fn definitions(topic: &str, schema_name: &str) -> crate::testing::AdversarialMcapFixture {
        AdversarialMcapFixtureBuilder::new()
            .with_schemas([FixtureSchema::new(1, schema_name, "protobuf").with_data([0x0a, 0x00])])
            .with_channels([FixtureChannel::schema_less(1, topic).with_schema(1, "protobuf")])
            .with_chunks([FixtureChunk::single(FixtureMessage::new(1, 0, 1))])
            .build()
            .expect("the runtime-intern fixture builds")
    }

    #[test]
    fn summary_census_redeems_domain_handles_before_typed_construction() {
        ensure_disarmed_test_profile_v1();
        let fixture = definitions("/mcap083/summary/topic", "mcap083.SummarySchema");
        let validated = crate::remote_summary::validated_summary_definitions_for_test(&fixture);
        let admitted = admit_summary_identifiers_with_decoders_v1(
            &validated,
            &[RemoteRuntimeDecoderIdentifierV1 {
                topic: "/mcap083/summary/topic".to_owned(),
                archetype: "mcap083.SummarySchema".to_owned(),
                component: "mcap083.SummarySchema:message".to_owned(),
            }],
        )
        .unwrap();

        assert_eq!(admitted.telemetry().candidate_identifiers, 8);
        assert_eq!(admitted.telemetry().unique_identifiers, 6);
        assert_eq!(admitted.telemetry().canonical_unique_identifiers, 8);
        assert_eq!(
            admitted.entity_path("/mcap083/summary/topic").unwrap(),
            EntityPath::from("/mcap083/summary/topic")
        );
        let domain = admitted
            .typed_domain_output_v1("/mcap083/summary/topic", Some("mcap083.SummarySchema"))
            .unwrap();
        let descriptor = crate::remote_typed_output::issue_from_live_factory_for_test_v1(
            1,
            crate::remote_typed_output::RemoteTypedOutputKindV1::Protobuf,
            [1; 16],
            1,
            crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
            Box::new([]),
            Box::new([]),
            Box::new([]),
            Box::new([]),
            Some(0),
            domain,
            crate::remote_time::RemoteMcapTimeType::TimestampNs,
        );
        assert_eq!(
            descriptor.entity_path_v1(),
            &EntityPath::from("/mcap083/summary/topic")
        );
        assert_eq!(
            descriptor.component_v1().component.as_str(),
            "mcap083.SummarySchema:message"
        );
        assert_eq!(
            descriptor.component_v1().archetype.unwrap().as_str(),
            "mcap083.SummarySchema"
        );
    }

    #[test]
    fn equivalent_entity_path_raw_topics_redeem_the_same_census_domain() {
        ensure_disarmed_test_profile_v1();

        let identifiers = [
            RemoteMcapRawIdentifier::timeline("message_log_time").unwrap(),
            RemoteMcapRawIdentifier::timeline("message_publish_time").unwrap(),
            RemoteMcapRawIdentifier::component("message").unwrap(),
            RemoteMcapRawIdentifier::entity_path("/foo///bar/"),
            RemoteMcapRawIdentifier::entity_path("foo/bar"),
        ];
        let raw = RawRuntimeIdentifierCensusV1::try_new(identifiers).unwrap();
        let committed = commit_raw_census(&raw).unwrap();
        let domains = redeem_summary_domain_handles_v1(&committed).unwrap();
        let admitted = RemoteRuntimeIdentifiersV1 { domains, committed };

        assert_eq!(
            admitted.entity_path("/foo///bar/").unwrap().to_string(),
            "/foo/bar"
        );
        assert_eq!(
            admitted.entity_path("foo/bar").unwrap().to_string(),
            "/foo/bar"
        );
        assert!(admitted.typed_domain_output_v1("/foo///bar/", None).is_ok());
        assert!(admitted.typed_domain_output_v1("foo/bar", None).is_ok());
    }

    #[test]
    fn duplicate_scalar_before_entity_path_is_not_redeemed_as_entity_path_raw() {
        ensure_disarmed_test_profile_v1();

        let identifiers = [
            RemoteMcapRawIdentifier::timeline("message_log_time").unwrap(),
            RemoteMcapRawIdentifier::timeline("message_publish_time").unwrap(),
            RemoteMcapRawIdentifier::component("message").unwrap(),
            RemoteMcapRawIdentifier::component("shared/topic").unwrap(),
            RemoteMcapRawIdentifier::component("shared/topic").unwrap(),
            RemoteMcapRawIdentifier::entity_path("/foo///bar/"),
            RemoteMcapRawIdentifier::entity_path("foo/bar"),
        ];
        let raw = RawRuntimeIdentifierCensusV1::try_new(identifiers).unwrap();
        let committed = commit_raw_census(&raw).unwrap();
        let domains = redeem_summary_domain_handles_v1(&committed).unwrap();
        let admitted = RemoteRuntimeIdentifiersV1 { domains, committed };

        assert_eq!(
            admitted.entity_path("/foo///bar/").unwrap().to_string(),
            "/foo/bar"
        );
        assert_eq!(
            admitted.entity_path("foo/bar").unwrap().to_string(),
            "/foo/bar"
        );
        assert!(admitted.entity_path("shared/topic").is_none());
        assert_eq!(
            admitted.component("shared/topic").unwrap().as_str(),
            "shared/topic"
        );
    }

    #[test]
    fn intern_errors_are_classified_without_folding_internal_failures_into_census_limits() {
        assert_eq!(
            RemoteRuntimeInternAdmissionErrorV1::from_intern(
                RemoteMcapRuntimeInternError::InvalidLimitProfile,
            ),
            RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation
        );
        assert_eq!(
            RemoteRuntimeInternAdmissionErrorV1::from_intern(
                RemoteMcapRuntimeInternError::CandidatePeakExceeded,
            ),
            RemoteRuntimeInternAdmissionErrorV1::AllocationFailed
        );
        assert_eq!(
            RemoteRuntimeInternAdmissionErrorV1::from_intern(
                RemoteMcapRuntimeInternError::ArithmeticOverflow,
            ),
            RemoteRuntimeInternAdmissionErrorV1::ProtocolViolation
        );
    }

    #[test]
    fn chunk_descriptor_census_reuses_side_map_entries() {
        ensure_disarmed_test_profile_v1();
        let before = remote_mcap_runtime_intern_snapshot().unwrap();
        let descriptor =
            crate::remote_typed_output::RemoteTypedOutputDescriptorV1::new_protobuf_for_dispatch_test_v1(
                Box::new([]),
                0,
            );
        let proof = admit_chunk_identifiers_v1(std::iter::once(&descriptor)).unwrap();
        let after = remote_mcap_runtime_intern_snapshot().unwrap();

        assert!(proof.telemetry().candidate_identifiers >= 4);
        assert_eq!(
            proof
                .telemetry()
                .missing
                .saturating_sub(after.remote_entries.saturating_sub(before.remote_entries)),
            0
        );
        assert!(proof.telemetry().existing_legacy + proof.telemetry().existing_remote > 0);
    }

    #[test]
    fn legacy_existing_summary_census_has_zero_missing_and_zero_burn() {
        ensure_disarmed_test_profile_v1();
        let topic = "/mcap083/legacy/existing";
        let schema = "mcap083.LegacySchema";
        InternedString::new("message_log_time");
        InternedString::new("message_publish_time");
        InternedString::new("message");
        InternedString::new("mcap083");
        InternedString::new("legacy");
        InternedString::new("existing");
        InternedString::new(schema);
        let before = remote_mcap_runtime_intern_snapshot().unwrap();
        let fixture = definitions(topic, schema);
        let validated = crate::remote_summary::validated_summary_definitions_for_test(&fixture);
        let admitted = admit_summary_identifiers_v1(&validated).unwrap();
        let after = remote_mcap_runtime_intern_snapshot().unwrap();

        assert_eq!(admitted.telemetry().missing, 0);
        assert_eq!(after.burned_string_bytes, before.burned_string_bytes);
        assert_eq!(after.burned_entry_bytes, before.burned_entry_bytes);
        assert_eq!(after.budget_revision, before.budget_revision);
    }

    #[test]
    fn census_failure_is_zero_partial_and_classified_by_phase() {
        ensure_disarmed_test_profile_v1();
        let before = remote_mcap_runtime_intern_snapshot().unwrap();
        let raw = (0..=REMOTE_RUNTIME_INTERN_CENSUS_LIMIT_V1)
            .map(|index| {
                RemoteMcapRawIdentifier::timeline(Box::leak(
                    format!("mcap083/too-many/{index}").into_boxed_str(),
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let error = RawRuntimeIdentifierCensusV1::try_new(raw).unwrap_err();
        let after = remote_mcap_runtime_intern_snapshot().unwrap();

        assert_eq!(
            error,
            RemoteRuntimeInternAdmissionErrorV1::CensusLimitExceeded
        );
        assert_eq!(
            RemoteRuntimeInternAdmissionErrorV1::terminal_kind_v1(
                RemoteRuntimeInternAdmissionPhaseV1::Opening,
            ),
            RemoteRuntimeInternTerminalV1::OpeningExhausted
        );
        assert_eq!(
            RemoteRuntimeInternAdmissionErrorV1::terminal_kind_v1(
                RemoteRuntimeInternAdmissionPhaseV1::ActiveChunk,
            ),
            RemoteRuntimeInternTerminalV1::ActiveSessionFatal
        );
        assert_eq!(after, before);
    }

    #[test]
    fn viewer_restart_does_not_refund_the_module_budget() {
        ensure_disarmed_test_profile_v1();
        let fixture = definitions("/mcap083/restart/topic", "mcap083.RestartSchema");
        let validated = crate::remote_summary::validated_summary_definitions_for_test(&fixture);
        let admitted = admit_summary_identifiers_v1(&validated).unwrap();
        let burned = remote_mcap_runtime_intern_snapshot().unwrap();
        drop(admitted);

        ensure_disarmed_test_profile_v1();
        let after_restart = remote_mcap_runtime_intern_snapshot().unwrap();
        assert_eq!(after_restart, burned);
        let readmitted = admit_summary_identifiers_v1(&validated).unwrap();
        assert_eq!(readmitted.telemetry().missing, 0);
    }

    #[test]
    fn legacy_insertion_between_prepare_and_commit_recomputes_in_lock() {
        ensure_disarmed_test_profile_v1();
        let topic = "/mcap083/revision/race";
        let schema = "mcap083.RevisionRaceSchema";
        InternedString::new("message_log_time");
        InternedString::new("message_publish_time");
        InternedString::new("message");
        InternedString::new("mcap083");
        InternedString::new("revision");
        InternedString::new("race");

        let fixture = definitions(topic, schema);
        let validated = crate::remote_summary::validated_summary_definitions_for_test(&fixture);
        let identifiers = summary_definition_identifiers_v1(&validated).unwrap();
        let raw = RawRuntimeIdentifierCensusV1::try_new(identifiers).unwrap();
        let census =
            BoundedRemoteIdentifierCensus::try_new_v1(raw.identifiers.iter().copied()).unwrap();
        let prepared = intern::prepare_remote_mcap_runtime_intern_from_domain_construction_token(
            census.into_domain_construction_token(),
        )
        .unwrap();
        let before_race = remote_mcap_runtime_intern_snapshot().unwrap();
        let legacy_schema = InternedString::new(schema);
        let committed = prepared.commit().unwrap();

        assert!(committed.telemetry().existing_legacy > 0);
        assert_eq!(committed.telemetry().missing, 0);
        assert!(committed.telemetry().recomputed_after_revision_change);
        assert_eq!(legacy_schema.as_str(), schema);
        let after_race = remote_mcap_runtime_intern_snapshot().unwrap();
        assert_eq!(
            after_race.burned_string_bytes,
            before_race.burned_string_bytes
        );
        assert_eq!(
            after_race.burned_entry_bytes,
            before_race.burned_entry_bytes
        );
        assert_eq!(after_race.remote_entries, before_race.remote_entries);

        let domains = redeem_summary_domain_handles_v1(&committed).unwrap();
        let admitted = RemoteRuntimeIdentifiersV1 { domains, committed };
        assert_eq!(
            admitted.component(schema).map(|value| value.as_str()),
            Some(schema)
        );
    }

    #[test]
    fn manifest_parser_and_store_boundaries_require_runtime_ownership() {
        let manifest = include_str!("remote_manifest.rs");
        let parser = include_str!("remote_protobuf_descriptor.rs");
        let dispatch = include_str!("remote_chunk_dispatch.rs");
        let assignment = include_str!("remote_decoder_assignment.rs");

        let manifest_build = manifest.find("fn build_v1").unwrap();
        let manifest_runtime = manifest[manifest_build..]
            .find("runtime_identifiers:")
            .map(|offset| manifest_build + offset)
            .unwrap();
        let manifest_limits = manifest[manifest_build..]
            .find("limits:")
            .map(|offset| manifest_build + offset)
            .unwrap();
        assert!(manifest_runtime < manifest_limits);

        assert!(parser.contains(
            "runtime_identifiers: &crate::remote_runtime_intern::RemoteRuntimeIdentifiersV1"
        ));
        assert!(parser.contains(
            "_runtime_identifiers: &crate::remote_runtime_intern::RemoteChunkRuntimeIdentifiersV1"
        ));
        assert!(dispatch.contains(
            "_runtime_identifiers: crate::remote_runtime_intern::RemoteChunkRuntimeIdentifiersV1"
        ));
        assert!(assignment.contains("SummaryRuntimeIntern"));
        assert!(assignment.contains("RemoteChunkDispatchFailureV1::RuntimeIdentifier"));
    }

    #[test]
    fn undelayable_constructor_has_a_typed_rejection_before_domain_construction() {
        assert_eq!(
            reject_undelayable_constructor_v1(),
            RemoteRuntimeInternAdmissionErrorV1::ConstructorCannotBeDelayed
        );
    }

    fn run_ignored_proof_in_subprocess(test_name: &str) {
        let executable = std::env::current_exe().unwrap();
        let status = std::process::Command::new(executable)
            .args(["--ignored", "--exact", test_name, "--test-threads=1"])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "isolated proof `{test_name}` failed with status {status}"
        );
    }

    #[test]
    fn actual_budget_failure_proof_runs_in_an_isolated_process() {
        run_ignored_proof_in_subprocess(
            "remote_runtime_intern::tests::actual_budget_failure_is_atomic_and_nonremote_construction_survives",
        );
    }

    #[test]
    #[ignore = "exhausts the process-global module budget; run this proof in isolation"]
    fn actual_budget_failure_is_atomic_and_nonremote_construction_survives() {
        ensure_disarmed_test_profile_v1();
        let existing = "mcap083/existing-after-exhaustion".to_owned();
        let existing_only = intern::prepare_remote_mcap_runtime_intern(&[existing.as_str()])
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(existing_only.telemetry().missing, 1);
        let mut prior_growth = remote_mcap_runtime_intern_snapshot().unwrap();

        let mut exhaustion_snapshot = None;
        for index in 0..32 {
            let raw = format!("mcap083/exhaustion/{index}").repeat(8192);
            let before_failure = remote_mcap_runtime_intern_snapshot().unwrap();
            let result = intern::prepare_remote_mcap_runtime_intern(&[raw.as_str()])
                .and_then(intern::PreparedRemoteInternBatch::commit);
            match result {
                Ok(_) => {
                    let after_growth = remote_mcap_runtime_intern_snapshot().unwrap();
                    assert!(after_growth.remote_entries > prior_growth.remote_entries);
                    assert_eq!(after_growth.legacy_entries, prior_growth.legacy_entries);
                    assert_eq!(after_growth.legacy_capacity, prior_growth.legacy_capacity);
                    prior_growth = after_growth;
                }
                Err(
                    RemoteMcapRuntimeInternError::StringBudgetExceeded
                    | RemoteMcapRuntimeInternError::EntryAndCapacityBudgetExceeded
                    | RemoteMcapRuntimeInternError::SideMapEntryLimitExceeded,
                ) => {
                    assert_eq!(
                        remote_mcap_runtime_intern_snapshot().unwrap(),
                        before_failure
                    );
                    exhaustion_snapshot = Some(before_failure);
                    break;
                }
                Err(error) => panic!("unexpected remote exhaustion error: {error}"),
            }
        }
        let exhausted =
            exhaustion_snapshot.expect("the disarmed profile must exhaust in 32 batches");

        let nonremote = InternedString::new("mcap083/nonremote-survives");
        assert_eq!(nonremote.as_str(), "mcap083/nonremote-survives");
        let after_nonremote = remote_mcap_runtime_intern_snapshot().unwrap();
        let existing_after_exhaustion =
            intern::prepare_remote_mcap_runtime_intern(&[existing.as_str()])
                .unwrap()
                .commit()
                .unwrap();
        assert_eq!(existing_after_exhaustion.telemetry().missing, 0);
        assert_eq!(
            remote_mcap_runtime_intern_snapshot().unwrap(),
            after_nonremote
        );
        assert_eq!(
            after_nonremote.burned_string_bytes,
            exhausted.burned_string_bytes
        );
    }
}
