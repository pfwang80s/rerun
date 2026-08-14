//! Sealed typed output descriptors for the remote MCAP dispatch stage.

#![allow(dead_code)]
#![allow(clippy::map_err_ignore)]

#[derive(PartialEq, Eq)]
pub(crate) enum RemoteTypedOutputKindV1 {
    Ros2Reflection,
    Protobuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteTypedScalarKindV1 {
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTypedFieldContractV1 {
    tag: u32,
    kind: RemoteTypedScalarKindV1,
    name: Box<str>,
}

impl RemoteTypedFieldContractV1 {
    pub(crate) fn new(tag: u32, kind: RemoteTypedScalarKindV1, name: String) -> Self {
        Self {
            tag,
            kind,
            name: name.into_boxed_str(),
        }
    }
}

/// Move-only output contract issued exclusively by a live executable factory.
///
/// Construction is private to this module and invoked through the factory's sealed method.  The
/// descriptor is not `Clone`/`Copy`; 031 must consume it while it still owns the matching factory,
/// which revalidates source/policy/config generation before issuance.
pub(crate) struct RemoteTypedOutputDescriptorV1 {
    channel_id: u16,
    kind: RemoteTypedOutputKindV1,
    config_digest: [u8; 16],
    schema_handle: u16,
    binding: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
    fields: Box<[RemoteTypedFieldContractV1]>,
    entity_path: String,
    component: re_sdk_types::ComponentDescriptor,
    time_type: crate::remote_time::RemoteMcapTimeType,
}

impl RemoteTypedOutputDescriptorV1 {
    pub(crate) const fn channel_id_v1(&self) -> u16 {
        self.channel_id
    }

    pub(crate) fn matches_v1(&self, other: &Self) -> bool {
        self.channel_id == other.channel_id
            && self.kind == other.kind
            && self.config_digest == other.config_digest
            && self.schema_handle == other.schema_handle
            && self.binding.matches_v1(&other.binding)
            && self.fields == other.fields
            && self.entity_path == other.entity_path
            && self.component == other.component
            && self.time_type == other.time_type
    }

    pub(crate) fn ensure_current_v1(&self) -> Result<(), ()> {
        self.binding.ensure_current_v1().map_err(|_| ())
    }

    pub(crate) fn matches_binding_v1(
        &self,
        binding: &crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
    ) -> bool {
        self.binding.matches_v1(binding)
    }

    pub(crate) fn matches_batch_v1(
        &self,
        batch: &crate::remote_protobuf_descriptor::RemoteTypedDecodedBatchV1,
    ) -> bool {
        self.channel_id == batch.channel_id_v1()
            && self.config_digest == batch.config_digest_v1()
            && self.binding.matches_v1(batch.binding_v1())
    }

    pub(crate) fn accepts_field_v1(&self, tag: u32, kind: RemoteTypedScalarKindV1) -> bool {
        self.fields
            .iter()
            .any(|field| field.tag == tag && field.kind == kind)
    }

    pub(crate) fn field_kind_v1(&self, tag: u32) -> Option<RemoteTypedScalarKindV1> {
        self.fields
            .iter()
            .find(|field| field.tag == tag)
            .map(|field| field.kind)
    }

    pub(crate) fn single_field_v1(&self) -> Option<&RemoteTypedFieldContractV1> {
        let [field] = self.fields.as_ref() else {
            return None;
        };
        Some(field)
    }

    pub(crate) fn entity_path_v1(&self) -> &str {
        &self.entity_path
    }
    pub(crate) fn component_v1(&self) -> &re_sdk_types::ComponentDescriptor {
        &self.component
    }
    pub(crate) const fn time_type_v1(&self) -> crate::remote_time::RemoteMcapTimeType {
        self.time_type
    }

    pub(crate) fn retained_metadata_bytes_v1(&self) -> Option<u64> {
        let mut bytes = u64::try_from(self.entity_path.len()).ok()?;
        for field in &self.fields {
            bytes = bytes.checked_add(u64::try_from(field.name.len()).ok()?)?;
        }
        bytes = bytes.checked_add(u64::try_from(self.component.component.as_str().len()).ok()?)?;
        if let Some(archetype) = self.component.archetype {
            bytes = bytes.checked_add(u64::try_from(archetype.as_str().len()).ok()?)?;
        }
        if let Some(component_type) = self.component.component_type {
            bytes = bytes.checked_add(u64::try_from(component_type.as_str().len()).ok()?)?;
        }
        Some(bytes)
    }
}

impl RemoteTypedFieldContractV1 {
    pub(crate) fn name_v1(&self) -> &str {
        &self.name
    }
    pub(crate) const fn kind_v1(&self) -> RemoteTypedScalarKindV1 {
        self.kind
    }
}

pub(super) fn issue_from_live_factory_v1(
    channel_id: u16,
    kind: RemoteTypedOutputKindV1,
    config_digest: [u8; 16],
    schema_handle: u16,
    binding: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
    fields: Box<[RemoteTypedFieldContractV1]>,
    entity_path: String,
    component: re_sdk_types::ComponentDescriptor,
    time_type: crate::remote_time::RemoteMcapTimeType,
) -> RemoteTypedOutputDescriptorV1 {
    RemoteTypedOutputDescriptorV1 {
        channel_id,
        kind,
        config_digest,
        schema_handle,
        binding,
        fields,
        entity_path,
        component,
        time_type,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(
        binding: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1,
    ) -> RemoteTypedOutputDescriptorV1 {
        issue_from_live_factory_v1(
            7,
            RemoteTypedOutputKindV1::Ros2Reflection,
            [9; 16],
            3,
            binding,
            Box::new([RemoteTypedFieldContractV1::new(
                1,
                RemoteTypedScalarKindV1::Int64,
                "value".to_owned(),
            )]),
            "/test".to_owned(),
            re_sdk_types::ComponentDescriptor::partial("message"),
            crate::remote_time::RemoteMcapTimeType::TimestampNs,
        )
    }

    #[test]
    fn identical_scalar_config_from_distinct_sources_never_matches() {
        let first = descriptor(
            crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
        );
        let second = descriptor(
            crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
        );
        assert!(!first.matches_v1(&second));
        assert!(first.matches_v1(&first));
    }
}
