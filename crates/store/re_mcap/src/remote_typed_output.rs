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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteTypedProtobufKindV1 {
    Double,
    Float,
    Int64,
    UInt64,
    Int32,
    Fixed64,
    Fixed32,
    Bool,
    String,
    Bytes,
    UInt32,
    Enum(u32),
    Message(u32),
    Map(u32),
    SFixed32,
    SFixed64,
    SInt32,
    SInt64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTypedProtobufOneofV1 {
    pub(crate) owner_message: u32,
    pub(crate) index: u32,
    pub(crate) name: Box<str>,
    pub(crate) synthetic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTypedProtobufEnumValueV1 {
    pub(crate) number: i32,
    pub(crate) name: Box<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTypedProtobufEnumV1 {
    pub(crate) index: u32,
    pub(crate) values: Box<[RemoteTypedProtobufEnumValueV1]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RemoteTypedProtobufCensusV1 {
    pub(crate) fields: u64,
    pub(crate) repeated_fields: u64,
    pub(crate) message_fields: u64,
    pub(crate) map_fields: u64,
    pub(crate) real_oneofs: u64,
    pub(crate) enum_fields: u64,
    pub(crate) enum_values: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteTypedProtobufFieldV1 {
    pub(crate) owner_message: u32,
    pub(crate) tag: u32,
    pub(crate) name: Box<str>,
    pub(crate) kind: RemoteTypedProtobufKindV1,
    pub(crate) nullable: bool,
    pub(crate) supports_presence: bool,
    pub(crate) repeated: bool,
    pub(crate) oneof_index: Option<u32>,
    pub(crate) proto3_optional: bool,
    pub(crate) packed: Option<bool>,
    pub(crate) default: Option<Box<[u8]>>,
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
    protobuf_fields: Box<[RemoteTypedProtobufFieldV1]>,
    protobuf_oneofs: Box<[RemoteTypedProtobufOneofV1]>,
    protobuf_enums: Box<[RemoteTypedProtobufEnumV1]>,
    protobuf_root_message: Option<u32>,
    entity_path: String,
    component: re_sdk_types::ComponentDescriptor,
    time_type: crate::remote_time::RemoteMcapTimeType,
}

impl RemoteTypedOutputDescriptorV1 {
    pub(crate) fn is_protobuf_v1(&self) -> bool {
        self.kind == RemoteTypedOutputKindV1::Protobuf
    }
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
            && self.protobuf_fields == other.protobuf_fields
            && self.protobuf_oneofs == other.protobuf_oneofs
            && self.protobuf_enums == other.protobuf_enums
            && self.protobuf_root_message == other.protobuf_root_message
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

    pub(crate) const fn protobuf_root_message_v1(&self) -> Option<u32> {
        self.protobuf_root_message
    }

    pub(crate) fn protobuf_fields_v1(&self) -> &[RemoteTypedProtobufFieldV1] {
        &self.protobuf_fields
    }

    pub(crate) fn protobuf_oneofs_v1(&self) -> &[RemoteTypedProtobufOneofV1] {
        &self.protobuf_oneofs
    }

    pub(crate) fn protobuf_enums_v1(&self) -> &[RemoteTypedProtobufEnumV1] {
        &self.protobuf_enums
    }

    pub(crate) fn protobuf_census_v1(&self) -> Option<RemoteTypedProtobufCensusV1> {
        if !self.is_protobuf_v1() {
            return None;
        }
        Some(RemoteTypedProtobufCensusV1 {
            fields: u64::try_from(self.protobuf_fields.len()).ok()?,
            repeated_fields: u64::try_from(
                self.protobuf_fields
                    .iter()
                    .filter(|field| field.repeated)
                    .count(),
            )
            .ok()?,
            message_fields: u64::try_from(
                self.protobuf_fields
                    .iter()
                    .filter(|field| matches!(field.kind, RemoteTypedProtobufKindV1::Message(_)))
                    .count(),
            )
            .ok()?,
            map_fields: u64::try_from(
                self.protobuf_fields
                    .iter()
                    .filter(|field| matches!(field.kind, RemoteTypedProtobufKindV1::Map(_)))
                    .count(),
            )
            .ok()?,
            real_oneofs: u64::try_from(
                self.protobuf_oneofs
                    .iter()
                    .filter(|oneof| !oneof.synthetic)
                    .count(),
            )
            .ok()?,
            enum_fields: u64::try_from(
                self.protobuf_fields
                    .iter()
                    .filter(|field| matches!(field.kind, RemoteTypedProtobufKindV1::Enum(_)))
                    .count(),
            )
            .ok()?,
            enum_values: self
                .protobuf_enums
                .iter()
                .try_fold(0_u64, |total, enumeration| {
                    total.checked_add(u64::try_from(enumeration.values.len()).ok()?)
                })?,
        })
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
        for field in &self.protobuf_fields {
            bytes = bytes.checked_add(u64::try_from(field.name.len()).ok()?)?;
            bytes = bytes.checked_add(
                u64::try_from(field.default.as_deref().map_or(0, <[u8]>::len)).ok()?,
            )?;
        }
        for oneof in &self.protobuf_oneofs {
            bytes = bytes.checked_add(u64::try_from(oneof.name.len()).ok()?)?;
        }
        for enumeration in &self.protobuf_enums {
            for value in &enumeration.values {
                bytes = bytes.checked_add(u64::try_from(value.name.len()).ok()?)?;
            }
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

#[cfg(test)]
impl RemoteTypedOutputDescriptorV1 {
    pub(crate) fn new_protobuf_for_dispatch_test_v1(
        fields: Box<[RemoteTypedProtobufFieldV1]>,
        root_message: u32,
    ) -> Self {
        Self {
            channel_id: 1,
            kind: RemoteTypedOutputKindV1::Protobuf,
            config_digest: [7; 16],
            schema_handle: 7,
            binding: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
            fields: Box::new([]),
            protobuf_fields: fields,
            protobuf_oneofs: Box::new([]),
            protobuf_enums: Box::new([]),
            protobuf_root_message: Some(root_message),
            entity_path: "/protobuf".to_owned(),
            component: re_sdk_types::ComponentDescriptor::partial("message"),
            time_type: crate::remote_time::RemoteMcapTimeType::TimestampNs,
        }
    }

    pub(crate) fn cross_wired_source_and_config_for_dispatch_test_v1(&self) -> Self {
        let mut config_digest = self.config_digest;
        config_digest[0] ^= 0xff;
        Self {
            channel_id: self.channel_id,
            kind: match self.kind {
                RemoteTypedOutputKindV1::Ros2Reflection => {
                    RemoteTypedOutputKindV1::Ros2Reflection
                }
                RemoteTypedOutputKindV1::Protobuf => RemoteTypedOutputKindV1::Protobuf,
            },
            config_digest,
            schema_handle: self.schema_handle,
            binding: crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1(),
            fields: self.fields.clone(),
            protobuf_fields: self.protobuf_fields.clone(),
            protobuf_oneofs: self.protobuf_oneofs.clone(),
            protobuf_enums: self.protobuf_enums.clone(),
            protobuf_root_message: self.protobuf_root_message,
            entity_path: self.entity_path.clone(),
            component: self.component.clone(),
            time_type: self.time_type,
        }
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
    protobuf_fields: Box<[RemoteTypedProtobufFieldV1]>,
    protobuf_oneofs: Box<[RemoteTypedProtobufOneofV1]>,
    protobuf_enums: Box<[RemoteTypedProtobufEnumV1]>,
    protobuf_root_message: Option<u32>,
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
        protobuf_fields,
        protobuf_oneofs,
        protobuf_enums,
        protobuf_root_message,
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
            Box::new([]),
            Box::new([]),
            Box::new([]),
            None,
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
