//! Admitted second-pass dispatch and terminal publication for remote MCAP chunks.
//!
//! This module deliberately owns no Viewer or Store mutation.  It consumes the sealed
//! validation plan and one-shot executable adapter and returns an all-or-nothing terminal
//! payload.  Callers must publish the returned payload only after the terminal state is complete.

#![allow(dead_code)]
#![allow(
    clippy::ignored_unit_patterns,
    clippy::map_err_ignore,
    clippy::needless_pass_by_value
)]

use crate::remote_channel_group::CanonicalSourceOrderKeyV1;
use crate::remote_chunk_scan::{PhysicalChunkMessageEvidenceV1, RemoteMessageEnvelopeV1};
use crate::remote_chunk_validation_count::{
    RemoteValidationCountErrorV1, ValidatedChunkDispatchPlanV1,
};
use crate::remote_protobuf_descriptor::{
    RemoteExecutableAdapterErrorV1, RemoteExecutableDecoderAdapterV1, RemoteNormalizedEnvelopeV1,
    RemoteNormalizedFieldV1, RemoteNormalizedValueV1, RemoteTypedDecodedBatchV1,
};
use crate::remote_time::{RawMcapTime, canonicalize_raw_mcap_time};
use crate::remote_typed_output::{
    RemoteTypedOutputDescriptorV1, RemoteTypedProtobufFieldV1, RemoteTypedProtobufKindV1,
    RemoteTypedProtobufOneofV1, RemoteTypedScalarKindV1,
};
use arrow::array::{
    ArrayBuilder, ArrayRef, BinaryBuilder, BooleanArray, BooleanBuilder, Float32Array,
    Float32Builder, Float64Array, Float64Builder, Int8Array, Int16Array, Int32Array, Int32Builder,
    Int64Array, Int64Builder, ListBuilder, MapBuilder, MapFieldNames, StringBuilder, StructArray,
    StructBuilder, UInt8Array, UInt16Array, UInt32Array, UInt32Builder, UInt64Array, UInt64Builder,
};
use arrow::datatypes::{DataType, Field, Fields};
use re_byte_size::SizeBytes as _;
use re_chunk::{Chunk, TimePoint};
use re_log_types::TimeCell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteChunkDispatchFailureV1 {
    RuntimeIdentifier(crate::remote_runtime_intern::RemoteRuntimeInternAdmissionErrorV1),
    StaleSource,
    PlanMismatch,
    Decode(RemoteExecutableAdapterErrorV1),
    Validation(RemoteValidationCountErrorV1),
    DerivedInsertion(crate::remote_deterministic_insertion::RemoteDerivedChunkResourceLimitV1),
}

// The complete terminal owns the batch's derived chunks and reservation proofs by design.
#[expect(clippy::large_enum_variant)]
pub(crate) enum RemoteChunkTerminalV1 {
    Complete(RemoteTypedPartitionHandoffV1),
    CompleteEmpty,
    Failed(RemoteChunkDispatchFailureV1),
}

pub(crate) struct RemoteTypedChunkHandoffV1 {
    chunk: Chunk,
    root: crate::remote_manifest::ManifestRootDescriptorV1,
}

pub(crate) struct RemoteTypedPartitionHandoffV1 {
    partition: crate::remote_manifest::ManifestPartitionDescriptorV1,
    chunks: Box<[RemoteTypedChunkHandoffV1]>,
    _reservations: Box<[crate::remote_chunk_validation_count::RemoteTypedOutputReservationV1]>,
    _runtime_identifiers: crate::remote_runtime_intern::RemoteChunkRuntimeIdentifiersV1,
}

struct RemoteBuiltChannelChunkV1 {
    chunk: Chunk,
    reservation: crate::remote_chunk_validation_count::RemoteTypedOutputReservationV1,
}

pub(crate) struct RemoteAdmittedChannelDispatchV1<'adapter> {
    descriptor: RemoteTypedOutputDescriptorV1,
    adapter: RemoteExecutableDecoderAdapterV1<'adapter>,
}

impl<'adapter> RemoteAdmittedChannelDispatchV1<'adapter> {
    pub(crate) fn new_v1(
        descriptor: RemoteTypedOutputDescriptorV1,
        adapter: RemoteExecutableDecoderAdapterV1<'adapter>,
    ) -> Self {
        Self {
            descriptor,
            adapter,
        }
    }
}

fn row_id_from_envelope_v1(
    envelope: &RemoteMessageEnvelopeV1<'_>,
) -> Result<re_chunk::RowId, RemoteChunkDispatchFailureV1> {
    CanonicalSourceOrderKeyV1::new(
        envelope.top_level_record_absolute_offset,
        envelope.record_local_offset,
        0,
    )
    .map(CanonicalSourceOrderKeyV1::stable_row_id)
    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)
}

impl RemoteTypedChunkHandoffV1 {
    /// The publication arbiter may inspect the sealed chunk while the matching plan-root
    /// reservation is necessarily still held.
    pub(crate) const fn chunk_v1(&self) -> &Chunk {
        &self.chunk
    }

    pub(crate) const fn root_v1(&self) -> crate::remote_manifest::ManifestRootDescriptorV1 {
        self.root
    }
}

impl RemoteTypedPartitionHandoffV1 {
    pub(crate) const fn partition_v1(
        &self,
    ) -> crate::remote_manifest::ManifestPartitionDescriptorV1 {
        self.partition
    }

    pub(crate) fn chunks_v1(&self) -> impl ExactSizeIterator<Item = &Chunk> {
        self.chunks.iter().map(RemoteTypedChunkHandoffV1::chunk_v1)
    }

    pub(crate) fn root_handoffs_v1(
        &self,
    ) -> impl ExactSizeIterator<Item = &RemoteTypedChunkHandoffV1> {
        self.chunks.iter()
    }
}

type RemoteProtobufListBuilderV1 = ListBuilder<Box<dyn ArrayBuilder>>;
type RemoteProtobufMapBuilderV1 = MapBuilder<Box<dyn ArrayBuilder>, Box<dyn ArrayBuilder>>;

enum RemoteProtobufGroupedFieldV1<'a> {
    Regular(&'a RemoteTypedProtobufFieldV1),
    Oneof {
        descriptor: &'a RemoteTypedProtobufOneofV1,
        variants: Vec<&'a RemoteTypedProtobufFieldV1>,
    },
}

fn protobuf_fields_for_message_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    message: u32,
) -> impl Iterator<Item = &RemoteTypedProtobufFieldV1> {
    descriptor
        .protobuf_fields_v1()
        .iter()
        .filter(move |field| field.owner_message == message)
}

fn protobuf_grouped_fields_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    message: u32,
) -> Vec<RemoteProtobufGroupedFieldV1<'_>> {
    let fields = protobuf_fields_for_message_v1(descriptor, message).collect::<Vec<_>>();
    let mut grouped = Vec::new();
    let mut emitted_oneofs = Vec::new();
    for field in fields {
        let Some(oneof_index) = field.oneof_index else {
            grouped.push(RemoteProtobufGroupedFieldV1::Regular(field));
            continue;
        };
        let Some(oneof) = descriptor
            .protobuf_oneofs_v1()
            .iter()
            .find(|oneof| oneof.owner_message == message && oneof.index == oneof_index)
        else {
            grouped.push(RemoteProtobufGroupedFieldV1::Regular(field));
            continue;
        };
        let variants = protobuf_fields_for_message_v1(descriptor, message)
            .filter(|candidate| candidate.oneof_index == Some(oneof_index))
            .collect::<Vec<_>>();
        if oneof.synthetic || variants.len() <= 1 {
            grouped.push(RemoteProtobufGroupedFieldV1::Regular(field));
        } else if !emitted_oneofs.contains(&oneof_index) {
            emitted_oneofs.push(oneof_index);
            grouped.push(RemoteProtobufGroupedFieldV1::Oneof {
                descriptor: oneof,
                variants,
            });
        }
    }
    grouped
}

fn protobuf_arrow_field_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    protobuf_field: &RemoteTypedProtobufFieldV1,
) -> Result<Field, RemoteChunkDispatchFailureV1> {
    let field = Field::new(
        protobuf_field.name.as_ref(),
        protobuf_datatype_v1(descriptor, protobuf_field)?,
        true,
    );
    Ok(
        if matches!(protobuf_field.kind, RemoteTypedProtobufKindV1::Enum(_)) {
            field.with_metadata(
                std::iter::once((
                    "ARROW:extension:name".to_owned(),
                    "rerun.datatypes.ProtobufEnum".to_owned(),
                ))
                .collect(),
            )
        } else {
            field
        },
    )
}

fn protobuf_datatype_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    field: &RemoteTypedProtobufFieldV1,
) -> Result<DataType, RemoteChunkDispatchFailureV1> {
    let inner = match field.kind {
        RemoteTypedProtobufKindV1::Double => DataType::Float64,
        RemoteTypedProtobufKindV1::Float => DataType::Float32,
        RemoteTypedProtobufKindV1::Int64
        | RemoteTypedProtobufKindV1::SFixed64
        | RemoteTypedProtobufKindV1::SInt64 => DataType::Int64,
        RemoteTypedProtobufKindV1::UInt64 | RemoteTypedProtobufKindV1::Fixed64 => DataType::UInt64,
        RemoteTypedProtobufKindV1::Int32
        | RemoteTypedProtobufKindV1::SFixed32
        | RemoteTypedProtobufKindV1::SInt32 => DataType::Int32,
        RemoteTypedProtobufKindV1::UInt32 | RemoteTypedProtobufKindV1::Fixed32 => DataType::UInt32,
        RemoteTypedProtobufKindV1::Bool => DataType::Boolean,
        RemoteTypedProtobufKindV1::String => DataType::Utf8,
        RemoteTypedProtobufKindV1::Bytes => DataType::Binary,
        RemoteTypedProtobufKindV1::Message(message) => DataType::Struct(
            protobuf_grouped_fields_v1(descriptor, message)
                .into_iter()
                .map(|grouped| match grouped {
                    RemoteProtobufGroupedFieldV1::Regular(field) => {
                        protobuf_arrow_field_v1(descriptor, field)
                    }
                    RemoteProtobufGroupedFieldV1::Oneof {
                        descriptor: oneof,
                        variants,
                    } => {
                        let inner = variants
                            .into_iter()
                            .map(|field| protobuf_arrow_field_v1(descriptor, field))
                            .collect::<Result<Fields, _>>()?;
                        Ok(
                            Field::new(oneof.name.as_ref(), DataType::Struct(inner), true)
                                .with_metadata(
                                    std::iter::once((
                                        "ARROW:extension:name".to_owned(),
                                        "rerun.datatypes.ProtobufOneOf".to_owned(),
                                    ))
                                    .collect(),
                                ),
                        )
                    }
                })
                .collect::<Result<Fields, _>>()?,
        ),
        RemoteTypedProtobufKindV1::Map(message) => {
            let mut fields = protobuf_fields_for_message_v1(descriptor, message);
            let key = fields
                .next()
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            let value = fields
                .next()
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            if fields.next().is_some() {
                return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
            }
            DataType::Map(
                std::sync::Arc::new(Field::new(
                    "entries",
                    DataType::Struct(Fields::from(vec![
                        Field::new(
                            key.name.as_ref(),
                            protobuf_datatype_v1(descriptor, key)?,
                            false,
                        ),
                        Field::new(
                            value.name.as_ref(),
                            protobuf_datatype_v1(descriptor, value)?,
                            true,
                        ),
                    ])),
                    false,
                )),
                false,
            )
        }
        RemoteTypedProtobufKindV1::Enum(_) => DataType::Struct(Fields::from(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("value", DataType::Int32, true),
        ])),
    };
    Ok(
        if field.repeated && !matches!(field.kind, RemoteTypedProtobufKindV1::Map(_)) {
            DataType::new_list(inner, true)
        } else {
            inner
        },
    )
}

fn protobuf_inner_builder_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    field: &RemoteTypedProtobufFieldV1,
) -> Result<Box<dyn ArrayBuilder>, RemoteChunkDispatchFailureV1> {
    Ok(match field.kind {
        RemoteTypedProtobufKindV1::Double => Box::new(Float64Builder::new()),
        RemoteTypedProtobufKindV1::Float => Box::new(Float32Builder::new()),
        RemoteTypedProtobufKindV1::Int64
        | RemoteTypedProtobufKindV1::SFixed64
        | RemoteTypedProtobufKindV1::SInt64 => Box::new(Int64Builder::new()),
        RemoteTypedProtobufKindV1::UInt64 | RemoteTypedProtobufKindV1::Fixed64 => {
            Box::new(UInt64Builder::new())
        }
        RemoteTypedProtobufKindV1::Int32
        | RemoteTypedProtobufKindV1::SFixed32
        | RemoteTypedProtobufKindV1::SInt32 => Box::new(Int32Builder::new()),
        RemoteTypedProtobufKindV1::UInt32 | RemoteTypedProtobufKindV1::Fixed32 => {
            Box::new(UInt32Builder::new())
        }
        RemoteTypedProtobufKindV1::Bool => Box::new(BooleanBuilder::new()),
        RemoteTypedProtobufKindV1::String => Box::new(StringBuilder::new()),
        RemoteTypedProtobufKindV1::Bytes => Box::new(BinaryBuilder::new()),
        RemoteTypedProtobufKindV1::Message(message) => {
            Box::new(protobuf_struct_builder_v1(descriptor, message)?)
        }
        RemoteTypedProtobufKindV1::Map(message) => {
            let mut fields = protobuf_fields_for_message_v1(descriptor, message);
            let key = fields
                .next()
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            let value = fields
                .next()
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            if fields.next().is_some() {
                return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
            }
            return Ok(Box::new(MapBuilder::new(
                Some(MapFieldNames {
                    entry: "entries".to_owned(),
                    key: key.name.to_string(),
                    value: value.name.to_string(),
                }),
                protobuf_inner_builder_v1(descriptor, key)?,
                protobuf_inner_builder_v1(descriptor, value)?,
            )));
        }
        RemoteTypedProtobufKindV1::Enum(_) => Box::new(StructBuilder::new(
            Fields::from(vec![
                Field::new("name", DataType::Utf8, true),
                Field::new("value", DataType::Int32, true),
            ]),
            vec![
                Box::new(StringBuilder::new()),
                Box::new(Int32Builder::new()),
            ],
        )),
    })
}

fn protobuf_field_builder_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    field: &RemoteTypedProtobufFieldV1,
) -> Result<Box<dyn ArrayBuilder>, RemoteChunkDispatchFailureV1> {
    let inner = protobuf_inner_builder_v1(descriptor, field)?;
    Ok(
        if field.repeated && !matches!(field.kind, RemoteTypedProtobufKindV1::Map(_)) {
            Box::new(ListBuilder::new(inner))
        } else {
            inner
        },
    )
}

fn protobuf_struct_builder_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    message: u32,
) -> Result<StructBuilder, RemoteChunkDispatchFailureV1> {
    let grouped = protobuf_grouped_fields_v1(descriptor, message);
    let fields = grouped
        .iter()
        .map(|grouped| match grouped {
            RemoteProtobufGroupedFieldV1::Regular(field) => {
                protobuf_arrow_field_v1(descriptor, field)
            }
            RemoteProtobufGroupedFieldV1::Oneof {
                descriptor: oneof,
                variants,
            } => {
                let inner = variants
                    .iter()
                    .map(|field| protobuf_arrow_field_v1(descriptor, field))
                    .collect::<Result<Fields, _>>()?;
                Ok(
                    Field::new(oneof.name.as_ref(), DataType::Struct(inner), true).with_metadata(
                        std::iter::once((
                            "ARROW:extension:name".to_owned(),
                            "rerun.datatypes.ProtobufOneOf".to_owned(),
                        ))
                        .collect(),
                    ),
                )
            }
        })
        .collect::<Result<Fields, _>>()?;
    let builders = grouped
        .into_iter()
        .map(|grouped| match grouped {
            RemoteProtobufGroupedFieldV1::Regular(field) => {
                protobuf_field_builder_v1(descriptor, field)
            }
            RemoteProtobufGroupedFieldV1::Oneof { variants, .. } => {
                let fields = variants
                    .iter()
                    .map(|field| protobuf_arrow_field_v1(descriptor, field))
                    .collect::<Result<Fields, _>>()?;
                let builders = variants
                    .into_iter()
                    .map(|field| protobuf_field_builder_v1(descriptor, field))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Box::new(StructBuilder::new(fields, builders)) as Box<dyn ArrayBuilder>)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StructBuilder::new(fields, builders))
}

fn append_protobuf_null_v1(
    builder: &mut dyn ArrayBuilder,
) -> Result<(), RemoteChunkDispatchFailureV1> {
    macro_rules! primitive {
        ($ty:ty) => {
            if let Some(builder) = builder.as_any_mut().downcast_mut::<$ty>() {
                builder.append_null();
                return Ok(());
            }
        };
    }
    primitive!(BooleanBuilder);
    primitive!(Int32Builder);
    primitive!(Int64Builder);
    primitive!(UInt32Builder);
    primitive!(UInt64Builder);
    primitive!(Float32Builder);
    primitive!(Float64Builder);
    primitive!(StringBuilder);
    primitive!(BinaryBuilder);
    if let Some(builder) = builder.as_any_mut().downcast_mut::<StructBuilder>() {
        for child in builder.field_builders_mut() {
            append_protobuf_null_v1(child.as_mut())?;
        }
        builder.append_null();
        return Ok(());
    }
    if let Some(builder) = builder
        .as_any_mut()
        .downcast_mut::<RemoteProtobufListBuilderV1>()
    {
        builder.append_null();
        return Ok(());
    }
    if let Some(builder) = builder
        .as_any_mut()
        .downcast_mut::<RemoteProtobufMapBuilderV1>()
    {
        builder
            .append(false)
            .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
        return Ok(());
    }
    Err(RemoteChunkDispatchFailureV1::PlanMismatch)
}

fn protobuf_default_text_v1(field: &RemoteTypedProtobufFieldV1) -> Option<&str> {
    let value = field.default.as_deref()?;
    std::str::from_utf8(value).ok()
}

fn append_protobuf_default_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    builder: &mut dyn ArrayBuilder,
    field: &RemoteTypedProtobufFieldV1,
) -> Result<(), RemoteChunkDispatchFailureV1> {
    let text = protobuf_default_text_v1(field);
    match field.kind {
        RemoteTypedProtobufKindV1::Bool => builder
            .as_any_mut()
            .downcast_mut::<BooleanBuilder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(text == Some("true")),
        RemoteTypedProtobufKindV1::Int32
        | RemoteTypedProtobufKindV1::SFixed32
        | RemoteTypedProtobufKindV1::SInt32 => builder
            .as_any_mut()
            .downcast_mut::<Int32Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(
                text.map_or(Ok(0), str::parse)
                    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            ),
        RemoteTypedProtobufKindV1::Int64
        | RemoteTypedProtobufKindV1::SFixed64
        | RemoteTypedProtobufKindV1::SInt64 => builder
            .as_any_mut()
            .downcast_mut::<Int64Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(
                text.map_or(Ok(0), str::parse)
                    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            ),
        RemoteTypedProtobufKindV1::UInt32 | RemoteTypedProtobufKindV1::Fixed32 => builder
            .as_any_mut()
            .downcast_mut::<UInt32Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(
                text.map_or(Ok(0), str::parse)
                    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            ),
        RemoteTypedProtobufKindV1::UInt64 | RemoteTypedProtobufKindV1::Fixed64 => builder
            .as_any_mut()
            .downcast_mut::<UInt64Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(
                text.map_or(Ok(0), str::parse)
                    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            ),
        RemoteTypedProtobufKindV1::Float => builder
            .as_any_mut()
            .downcast_mut::<Float32Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(match text {
                None => 0.0,
                Some("inf") => f32::INFINITY,
                Some("-inf") => f32::NEG_INFINITY,
                Some("nan") => f32::NAN,
                Some(value) => value
                    .parse()
                    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            }),
        RemoteTypedProtobufKindV1::Double => builder
            .as_any_mut()
            .downcast_mut::<Float64Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(match text {
                None => 0.0,
                Some("inf") => f64::INFINITY,
                Some("-inf") => f64::NEG_INFINITY,
                Some("nan") => f64::NAN,
                Some(value) => value
                    .parse()
                    .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            }),
        RemoteTypedProtobufKindV1::String => builder
            .as_any_mut()
            .downcast_mut::<StringBuilder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(text.unwrap_or_default()),
        RemoteTypedProtobufKindV1::Bytes => builder
            .as_any_mut()
            .downcast_mut::<BinaryBuilder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(field.default.as_deref().unwrap_or_default()),
        RemoteTypedProtobufKindV1::Enum(enum_index) => {
            let enumeration = descriptor
                .protobuf_enums_v1()
                .iter()
                .find(|enumeration| enumeration.index == enum_index)
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            let value = if let Some(name) = text {
                enumeration
                    .values
                    .iter()
                    .find(|value| value.name.as_ref() == name)
            } else {
                enumeration.values.first()
            }
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            append_protobuf_enum_v1(builder, value.name.as_ref(), value.number)?;
        }
        RemoteTypedProtobufKindV1::Message(_) | RemoteTypedProtobufKindV1::Map(_) => {
            return append_protobuf_null_v1(builder);
        }
    }
    Ok(())
}

fn append_protobuf_enum_v1(
    builder: &mut dyn ArrayBuilder,
    name: &str,
    number: i32,
) -> Result<(), RemoteChunkDispatchFailureV1> {
    let builder = builder
        .as_any_mut()
        .downcast_mut::<StructBuilder>()
        .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
    let children = builder.field_builders_mut();
    children
        .get_mut(0)
        .and_then(|child| child.as_any_mut().downcast_mut::<StringBuilder>())
        .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
        .append_value(name);
    children
        .get_mut(1)
        .and_then(|child| child.as_any_mut().downcast_mut::<Int32Builder>())
        .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
        .append_value(number);
    builder.append(true);
    Ok(())
}

fn append_protobuf_value_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    envelope: &RemoteNormalizedEnvelopeV1,
    builder: &mut dyn ArrayBuilder,
    field: &RemoteTypedProtobufFieldV1,
    value: RemoteNormalizedValueV1,
) -> Result<(), RemoteChunkDispatchFailureV1> {
    match (field.kind.clone(), value) {
        (RemoteTypedProtobufKindV1::Bool, RemoteNormalizedValueV1::Bool(value)) => builder
            .as_any_mut()
            .downcast_mut::<BooleanBuilder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(value),
        (
            RemoteTypedProtobufKindV1::Int32
            | RemoteTypedProtobufKindV1::SFixed32
            | RemoteTypedProtobufKindV1::SInt32,
            RemoteNormalizedValueV1::Signed(value),
        ) => builder
            .as_any_mut()
            .downcast_mut::<Int32Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(
                i32::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            ),
        (
            RemoteTypedProtobufKindV1::Int64
            | RemoteTypedProtobufKindV1::SFixed64
            | RemoteTypedProtobufKindV1::SInt64,
            RemoteNormalizedValueV1::Signed(value),
        ) => builder
            .as_any_mut()
            .downcast_mut::<Int64Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(value),
        (RemoteTypedProtobufKindV1::UInt32, RemoteNormalizedValueV1::Unsigned(value)) => builder
            .as_any_mut()
            .downcast_mut::<UInt32Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(
                u32::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
            ),
        (RemoteTypedProtobufKindV1::Fixed32, RemoteNormalizedValueV1::Fixed32(value)) => builder
            .as_any_mut()
            .downcast_mut::<UInt32Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(value),
        (RemoteTypedProtobufKindV1::UInt64, RemoteNormalizedValueV1::Unsigned(value))
        | (RemoteTypedProtobufKindV1::Fixed64, RemoteNormalizedValueV1::Fixed64(value)) => {
            builder
                .as_any_mut()
                .downcast_mut::<UInt64Builder>()
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
                .append_value(value);
        }
        (RemoteTypedProtobufKindV1::Float, RemoteNormalizedValueV1::Float32(value)) => builder
            .as_any_mut()
            .downcast_mut::<Float32Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(f32::from_bits(value)),
        (RemoteTypedProtobufKindV1::Double, RemoteNormalizedValueV1::Float64(value)) => builder
            .as_any_mut()
            .downcast_mut::<Float64Builder>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
            .append_value(f64::from_bits(value)),
        (
            RemoteTypedProtobufKindV1::String | RemoteTypedProtobufKindV1::Bytes,
            RemoteNormalizedValueV1::Bytes { start, len },
        ) => {
            let bytes = envelope
                .bytes_v1(start, len)
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            if field.kind == RemoteTypedProtobufKindV1::String {
                builder
                    .as_any_mut()
                    .downcast_mut::<StringBuilder>()
                    .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
                    .append_value(
                        std::str::from_utf8(bytes)
                            .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                    );
            } else {
                builder
                    .as_any_mut()
                    .downcast_mut::<BinaryBuilder>()
                    .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?
                    .append_value(bytes);
            }
        }
        (
            RemoteTypedProtobufKindV1::Message(message),
            RemoteNormalizedValueV1::Message {
                first_value,
                value_count,
            },
        ) => {
            let fields = envelope
                .field_span_v1(first_value, value_count)
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            append_protobuf_message_v1(
                descriptor,
                envelope,
                builder
                    .as_any_mut()
                    .downcast_mut::<StructBuilder>()
                    .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?,
                message,
                fields,
            )?;
        }
        (RemoteTypedProtobufKindV1::Enum(enum_index), RemoteNormalizedValueV1::Signed(number)) => {
            let number =
                i32::try_from(number).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
            let value = descriptor
                .protobuf_enums_v1()
                .iter()
                .find(|enumeration| enumeration.index == enum_index)
                .and_then(|enumeration| {
                    enumeration
                        .values
                        .iter()
                        .find(|value| value.number == number)
                })
                .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
            append_protobuf_enum_v1(builder, value.name.as_ref(), number)?;
        }
        (RemoteTypedProtobufKindV1::Map(_), _) => {
            return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
        }
        _ => return Err(RemoteChunkDispatchFailureV1::PlanMismatch),
    }
    Ok(())
}

fn append_protobuf_message_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    envelope: &RemoteNormalizedEnvelopeV1,
    builder: &mut StructBuilder,
    message: u32,
    normalized: &[RemoteNormalizedFieldV1],
) -> Result<(), RemoteChunkDispatchFailureV1> {
    let grouped = protobuf_grouped_fields_v1(descriptor, message);
    if grouped.len() != builder.num_fields() {
        return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
    }
    for (index, grouped_field) in grouped.into_iter().enumerate() {
        let child = builder
            .field_builders_mut()
            .get_mut(index)
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        match grouped_field {
            RemoteProtobufGroupedFieldV1::Regular(field) => {
                append_protobuf_field_v1(descriptor, envelope, child.as_mut(), field, normalized)?;
            }
            RemoteProtobufGroupedFieldV1::Oneof { variants, .. } => {
                let oneof = child
                    .as_any_mut()
                    .downcast_mut::<StructBuilder>()
                    .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
                if variants.len() != oneof.num_fields() {
                    return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
                }
                let mut any_set = false;
                for (variant_builder, variant) in
                    std::iter::zip(oneof.field_builders_mut(), variants)
                {
                    if let Some(value) = normalized.iter().find(|value| value.tag == variant.tag) {
                        append_protobuf_value_v1(
                            descriptor,
                            envelope,
                            variant_builder.as_mut(),
                            variant,
                            value.value,
                        )?;
                        any_set = true;
                    } else {
                        append_protobuf_null_v1(variant_builder.as_mut())?;
                    }
                }
                oneof.append(any_set);
            }
        }
    }
    builder.append(true);
    Ok(())
}

fn append_protobuf_field_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    envelope: &RemoteNormalizedEnvelopeV1,
    child: &mut dyn ArrayBuilder,
    field: &RemoteTypedProtobufFieldV1,
    normalized: &[RemoteNormalizedFieldV1],
) -> Result<(), RemoteChunkDispatchFailureV1> {
    let observed = normalized.iter().find(|value| value.tag == field.tag);
    if let RemoteTypedProtobufKindV1::Map(entry_message) = field.kind {
        let map = child
            .as_any_mut()
            .downcast_mut::<RemoteProtobufMapBuilderV1>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let Some(RemoteNormalizedFieldV1 {
            value:
                RemoteNormalizedValueV1::Array {
                    first_value,
                    value_count,
                    ..
                },
            ..
        }) = observed
        else {
            map.append(false)
                .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
            return Ok(());
        };
        let entries = envelope
            .field_span_v1(*first_value, *value_count)
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let mut entry_fields = protobuf_fields_for_message_v1(descriptor, entry_message);
        let key_field = entry_fields
            .next()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let value_field = entry_fields
            .next()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        if entry_fields.next().is_some() {
            return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
        }
        let mut decoded_entries = entries
            .iter()
            .map(|entry| match entry.value {
                RemoteNormalizedValueV1::Message {
                    first_value,
                    value_count,
                } => envelope
                    .field_span_v1(first_value, value_count)
                    .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch),
                _ => Err(RemoteChunkDispatchFailureV1::PlanMismatch),
            })
            .collect::<Result<Vec<_>, _>>()?;
        decoded_entries.sort_by(|left, right| {
            protobuf_map_key_v1(envelope, key_field, left)
                .partial_cmp(&protobuf_map_key_v1(envelope, key_field, right))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut unique_entries: Vec<&[RemoteNormalizedFieldV1]> =
            Vec::with_capacity(decoded_entries.len());
        for entry in decoded_entries {
            if unique_entries.last().is_some_and(|previous| {
                protobuf_map_key_v1(envelope, key_field, previous)
                    == protobuf_map_key_v1(envelope, key_field, entry)
            }) {
                *unique_entries
                    .last_mut()
                    .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)? = entry;
            } else {
                unique_entries.push(entry);
            }
        }
        for entry in unique_entries {
            append_protobuf_field_v1(descriptor, envelope, map.keys().as_mut(), key_field, entry)?;
            if !entry.iter().any(|value| value.tag == value_field.tag)
                && let RemoteTypedProtobufKindV1::Message(message) = value_field.kind
            {
                append_protobuf_message_v1(
                    descriptor,
                    envelope,
                    map.values()
                        .as_any_mut()
                        .downcast_mut::<StructBuilder>()
                        .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?,
                    message,
                    &[],
                )?;
            } else {
                append_protobuf_field_v1(
                    descriptor,
                    envelope,
                    map.values().as_mut(),
                    value_field,
                    entry,
                )?;
            }
        }
        map.append(true)
            .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
        return Ok(());
    }
    if field.repeated {
        let list = child
            .as_any_mut()
            .downcast_mut::<RemoteProtobufListBuilderV1>()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let Some(RemoteNormalizedFieldV1 {
            value:
                RemoteNormalizedValueV1::Array {
                    first_value,
                    value_count,
                    ..
                },
            ..
        }) = observed
        else {
            list.append_null();
            return Ok(());
        };
        let values = envelope
            .field_span_v1(*first_value, *value_count)
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let mut element = field.clone();
        element.repeated = false;
        for value in values {
            if value.tag != field.tag {
                return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
            }
            append_protobuf_value_v1(
                descriptor,
                envelope,
                list.values().as_mut(),
                &element,
                value.value,
            )?;
        }
        list.append(true);
    } else if let Some(value) = observed {
        append_protobuf_value_v1(descriptor, envelope, child, field, value.value)?;
    } else if field.supports_presence {
        append_protobuf_null_v1(child)?;
    } else {
        append_protobuf_default_v1(descriptor, child, field)?;
    }
    Ok(())
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum RemoteProtobufMapKeyV1<'a> {
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    String(&'a [u8]),
}

fn protobuf_map_key_v1<'a>(
    envelope: &'a RemoteNormalizedEnvelopeV1,
    key_field: &RemoteTypedProtobufFieldV1,
    entry: &[RemoteNormalizedFieldV1],
) -> RemoteProtobufMapKeyV1<'a> {
    let value = entry
        .iter()
        .find(|value| value.tag == key_field.tag)
        .map(|value| value.value);
    match (key_field.kind.clone(), value) {
        (RemoteTypedProtobufKindV1::Bool, Some(RemoteNormalizedValueV1::Bool(value))) => {
            RemoteProtobufMapKeyV1::Bool(value)
        }
        (
            RemoteTypedProtobufKindV1::Int32
            | RemoteTypedProtobufKindV1::SFixed32
            | RemoteTypedProtobufKindV1::SInt32
            | RemoteTypedProtobufKindV1::Int64
            | RemoteTypedProtobufKindV1::SFixed64
            | RemoteTypedProtobufKindV1::SInt64,
            Some(RemoteNormalizedValueV1::Signed(value)),
        ) => RemoteProtobufMapKeyV1::Signed(value),
        (
            RemoteTypedProtobufKindV1::UInt32
            | RemoteTypedProtobufKindV1::Fixed32
            | RemoteTypedProtobufKindV1::UInt64
            | RemoteTypedProtobufKindV1::Fixed64,
            Some(RemoteNormalizedValueV1::Unsigned(value)),
        ) => RemoteProtobufMapKeyV1::Unsigned(value),
        (RemoteTypedProtobufKindV1::Fixed32, Some(RemoteNormalizedValueV1::Fixed32(value))) => {
            RemoteProtobufMapKeyV1::Unsigned(u64::from(value))
        }
        (RemoteTypedProtobufKindV1::Fixed64, Some(RemoteNormalizedValueV1::Fixed64(value))) => {
            RemoteProtobufMapKeyV1::Unsigned(value)
        }
        (
            RemoteTypedProtobufKindV1::String,
            Some(RemoteNormalizedValueV1::Bytes { start, len }),
        ) => RemoteProtobufMapKeyV1::String(envelope.bytes_v1(start, len).unwrap_or_default()),
        (RemoteTypedProtobufKindV1::Bool, _) => RemoteProtobufMapKeyV1::Bool(false),
        (RemoteTypedProtobufKindV1::String, _) => RemoteProtobufMapKeyV1::String(&[]),
        (
            RemoteTypedProtobufKindV1::Int32
            | RemoteTypedProtobufKindV1::SFixed32
            | RemoteTypedProtobufKindV1::SInt32
            | RemoteTypedProtobufKindV1::Int64
            | RemoteTypedProtobufKindV1::SFixed64
            | RemoteTypedProtobufKindV1::SInt64,
            _,
        ) => RemoteProtobufMapKeyV1::Signed(0),
        _ => RemoteProtobufMapKeyV1::Unsigned(0),
    }
}

fn validate_descriptor_batch_plan_v1(
    descriptor: &RemoteTypedOutputDescriptorV1,
    plan: &ValidatedChunkDispatchPlanV1<'_, '_, '_, '_, '_, '_>,
    batch: &RemoteTypedDecodedBatchV1,
) -> Result<(), RemoteChunkDispatchFailureV1> {
    descriptor
        .ensure_current_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    if !descriptor.matches_batch_v1(batch) {
        return Err(RemoteChunkDispatchFailureV1::StaleSource);
    }
    let authority = plan.authority_v1();
    authority
        .ensure_current_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    plan.ensure_current_v1()
        .map_err(RemoteChunkDispatchFailureV1::Validation)?;
    if !descriptor.matches_binding_v1(plan.evidence_v1().source_binding_v1()) {
        return Err(RemoteChunkDispatchFailureV1::StaleSource);
    }
    let expected_channel = plan
        .channels_v1()
        .iter()
        .find(|channel| channel.channel_id_v1() == descriptor.channel_id_v1())
        .copied()
        .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
    if batch.rows_v1() != expected_channel.message_count_v1()
        || batch.input_payload_bytes_v1() != expected_channel.payload_bytes_v1()
    {
        return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
    }
    Ok(())
}

fn build_ros_scalar_chunk_v1(
    descriptor: RemoteTypedOutputDescriptorV1,
    plan: &ValidatedChunkDispatchPlanV1<'_, '_, '_, '_, '_, '_>,
    batch: RemoteTypedDecodedBatchV1,
    output_ordinal: u32,
) -> Result<RemoteBuiltChannelChunkV1, RemoteChunkDispatchFailureV1> {
    validate_descriptor_batch_plan_v1(&descriptor, plan, &batch)?;
    let authority = plan.authority_v1();
    let output_reservation = plan
        .reserve_typed_output_v1(batch.rows_v1(), batch.input_payload_bytes_v1(), &descriptor)
        .map_err(RemoteChunkDispatchFailureV1::Validation)?;
    let root = authority
        .issue_root_v1(output_ordinal)
        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
    let mut builder =
        Chunk::builder_with_id(root.root_chunk_id_v1(), descriptor.entity_path_v1().clone());
    let mut messages = plan
        .evidence_v1()
        .message_envelopes_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    for envelope in batch.envelopes_v1() {
        let message = loop {
            let Some(message) = messages
                .next_v1()
                .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?
            else {
                return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
            };
            if message.channel_id == descriptor.channel_id_v1() {
                break message;
            }
        };
        let [field] = envelope.fields_v1() else {
            return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
        };
        let expected = descriptor
            .field_kind_v1(field.tag)
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let scalar: ArrayRef = match (expected, field.value) {
            (RemoteTypedScalarKindV1::Int8, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int8Array::from(vec![
                    i8::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::Int16, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int16Array::from(vec![
                    i16::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::Int32, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int32Array::from(vec![
                    i32::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::Int64, RemoteNormalizedValueV1::Signed(value)) => {
                std::sync::Arc::new(Int64Array::from(vec![value]))
            }
            (RemoteTypedScalarKindV1::UInt8, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt8Array::from(vec![
                    u8::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::UInt16, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt16Array::from(vec![
                    u16::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::UInt32, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt32Array::from(vec![
                    u32::try_from(value).map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ]))
            }
            (RemoteTypedScalarKindV1::UInt64, RemoteNormalizedValueV1::Unsigned(value)) => {
                std::sync::Arc::new(UInt64Array::from(vec![value]))
            }
            (RemoteTypedScalarKindV1::Float32, RemoteNormalizedValueV1::Float32(value)) => {
                std::sync::Arc::new(Float32Array::from(vec![f32::from_bits(value)]))
            }
            (RemoteTypedScalarKindV1::Float64, RemoteNormalizedValueV1::Float64(value)) => {
                std::sync::Arc::new(Float64Array::from(vec![f64::from_bits(value)]))
            }
            (RemoteTypedScalarKindV1::Bool, RemoteNormalizedValueV1::Bool(value)) => {
                std::sync::Arc::new(BooleanArray::from(vec![value]))
            }
            _ => return Err(RemoteChunkDispatchFailureV1::PlanMismatch),
        };
        let field_contract = descriptor
            .single_field_v1()
            .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
        let datatype = match expected {
            RemoteTypedScalarKindV1::Int8 => DataType::Int8,
            RemoteTypedScalarKindV1::Int16 => DataType::Int16,
            RemoteTypedScalarKindV1::Int32 => DataType::Int32,
            RemoteTypedScalarKindV1::Int64 => DataType::Int64,
            RemoteTypedScalarKindV1::UInt8 => DataType::UInt8,
            RemoteTypedScalarKindV1::UInt16 => DataType::UInt16,
            RemoteTypedScalarKindV1::UInt32 => DataType::UInt32,
            RemoteTypedScalarKindV1::UInt64 => DataType::UInt64,
            RemoteTypedScalarKindV1::Float32 => DataType::Float32,
            RemoteTypedScalarKindV1::Float64 => DataType::Float64,
            RemoteTypedScalarKindV1::Bool => DataType::Boolean,
        };
        let value: ArrayRef = std::sync::Arc::new(StructArray::new(
            Fields::from(vec![Field::new(field_contract.name_v1(), datatype, true)]),
            vec![scalar],
            None,
        ));
        let timepoint = TimePoint::from([
            (
                descriptor.timeline_log_time_v1(),
                TimeCell::new(
                    descriptor.time_type_v1().into(),
                    canonicalize_raw_mcap_time(RawMcapTime::new(message.log_time))
                        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ),
            ),
            (
                descriptor.timeline_publish_time_v1(),
                TimeCell::new(
                    descriptor.time_type_v1().into(),
                    canonicalize_raw_mcap_time(RawMcapTime::new(message.publish_time))
                        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ),
            ),
        ]);
        let row_id = row_id_from_envelope_v1(&message)?;
        builder = builder.with_row(
            row_id,
            timepoint,
            [(descriptor.component_v1().clone(), value)],
        );
    }
    while let Some(message) = messages
        .next_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?
    {
        if message.channel_id == descriptor.channel_id_v1() {
            return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
        }
    }
    descriptor
        .ensure_current_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    let chunk = builder
        .build()
        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
    Ok(RemoteBuiltChannelChunkV1 {
        chunk,
        reservation: output_reservation,
    })
}

fn build_protobuf_chunk_v1(
    descriptor: RemoteTypedOutputDescriptorV1,
    plan: &ValidatedChunkDispatchPlanV1<'_, '_, '_, '_, '_, '_>,
    batch: RemoteTypedDecodedBatchV1,
    output_ordinal: u32,
) -> Result<RemoteBuiltChannelChunkV1, RemoteChunkDispatchFailureV1> {
    validate_descriptor_batch_plan_v1(&descriptor, plan, &batch)?;
    if !descriptor.is_protobuf_v1() {
        return Err(RemoteChunkDispatchFailureV1::StaleSource);
    }
    let root_message = descriptor
        .protobuf_root_message_v1()
        .ok_or(RemoteChunkDispatchFailureV1::PlanMismatch)?;
    let authority = plan.authority_v1();
    let output_reservation = plan
        .reserve_typed_output_v1(batch.rows_v1(), batch.input_payload_bytes_v1(), &descriptor)
        .map_err(RemoteChunkDispatchFailureV1::Validation)?;
    #[cfg(test)]
    let typed_output_peak =
        crate::remote_chunk_validation_count::typed_output_peak_for_descriptor_test_v1(
            batch.rows_v1(),
            batch.input_payload_bytes_v1(),
            &descriptor,
        )
        .map_err(RemoteChunkDispatchFailureV1::Validation)?;
    #[cfg(test)]
    let allocation_guard = crate::remote_summary::tests::AllocationGuard::start();
    let root = authority
        .issue_root_v1(output_ordinal)
        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
    let mut builder =
        Chunk::builder_with_id(root.root_chunk_id_v1(), descriptor.entity_path_v1().clone());
    let mut messages = plan
        .evidence_v1()
        .message_envelopes_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    for envelope in batch.envelopes_v1() {
        let message = loop {
            let Some(message) = messages
                .next_v1()
                .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?
            else {
                return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
            };
            if message.channel_id == descriptor.channel_id_v1() {
                break message;
            }
        };
        let mut struct_builder = protobuf_struct_builder_v1(&descriptor, root_message)?;
        append_protobuf_message_v1(
            &descriptor,
            envelope,
            &mut struct_builder,
            root_message,
            envelope.fields_v1(),
        )?;
        let value: ArrayRef = std::sync::Arc::new(struct_builder.finish());
        let timepoint = TimePoint::from([
            (
                descriptor.timeline_log_time_v1(),
                TimeCell::new(
                    descriptor.time_type_v1().into(),
                    canonicalize_raw_mcap_time(RawMcapTime::new(message.log_time))
                        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ),
            ),
            (
                descriptor.timeline_publish_time_v1(),
                TimeCell::new(
                    descriptor.time_type_v1().into(),
                    canonicalize_raw_mcap_time(RawMcapTime::new(message.publish_time))
                        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?,
                ),
            ),
        ]);
        builder = builder.with_row(
            row_id_from_envelope_v1(&message)?,
            timepoint,
            [(descriptor.component_v1().clone(), value)],
        );
    }
    while let Some(message) = messages
        .next_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?
    {
        if message.channel_id == descriptor.channel_id_v1() {
            return Err(RemoteChunkDispatchFailureV1::PlanMismatch);
        }
    }
    descriptor
        .ensure_current_v1()
        .map_err(|_| RemoteChunkDispatchFailureV1::StaleSource)?;
    let chunk = builder
        .build()
        .map_err(|_| RemoteChunkDispatchFailureV1::PlanMismatch)?;
    #[cfg(test)]
    {
        let actual_locked_peak =
            crate::remote_summary::tests::AllocationGuard::high_water_locked_bytes();
        assert!(
            actual_locked_peak <= typed_output_peak,
            "protobuf typed output locked peak {actual_locked_peak} exceeds census {typed_output_peak}"
        );
        drop(allocation_guard);
    }
    Ok(RemoteBuiltChannelChunkV1 {
        chunk,
        reservation: output_reservation,
    })
}

impl RemoteChunkTerminalV1 {
    pub(crate) const fn is_complete_v1(&self) -> bool {
        matches!(self, Self::Complete(_) | Self::CompleteEmpty)
    }
}

/// Executes the admitted second pass.  No output is returned on any mismatch or decode error;
/// the adapter is consumed and its reservation is dropped, preventing partial publication.
pub(crate) fn dispatch_admitted_v1(
    dispatches: Box<[RemoteAdmittedChannelDispatchV1<'_>]>,
    plan: &ValidatedChunkDispatchPlanV1<'_, '_, '_, '_, '_, '_>,
    runtime_identifiers: crate::remote_runtime_intern::RemoteChunkRuntimeIdentifiersV1,
) -> RemoteChunkTerminalV1 {
    if let Err(error) = plan.ensure_current_v1() {
        return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::Validation(error));
    }
    let evidence: &PhysicalChunkMessageEvidenceV1<'_> = plan.evidence_v1();
    if evidence.ensure_current_v1().is_err() {
        return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::StaleSource);
    }
    if dispatches.len() != plan.channels_v1().len()
        || dispatches
            .iter()
            .zip(plan.channels_v1())
            .any(|(dispatch, expected)| {
                dispatch.descriptor.channel_id_v1() != expected.channel_id_v1()
            })
    {
        return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
    }
    let authority = plan.authority_v1();
    let insertion_limits = plan.derived_chunk_limits_v1();
    let root_capacity = match usize::try_from(insertion_limits.max_roots_per_partition_v1()) {
        Ok(capacity) => capacity,
        Err(_overflow) => {
            return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
        }
    };
    let mut chunks = Vec::new();
    if chunks.try_reserve_exact(root_capacity).is_err() {
        return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
    }
    let mut reservations = Vec::new();
    if reservations.try_reserve_exact(dispatches.len()).is_err() {
        return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
    }
    let mut output_physical_bytes = 0_u64;
    for (ordinal, dispatch) in dispatches.into_vec().into_iter().enumerate() {
        let expected = plan.channels_v1()[ordinal];
        let batch = match dispatch.adapter.execute_batch_v1(evidence) {
            Ok(batch)
                if batch.rows_v1() == expected.message_count_v1()
                    && batch.input_payload_bytes_v1() == expected.payload_bytes_v1() =>
            {
                batch
            }
            Ok(_batch) => {
                return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
            }
            Err(error) => {
                return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::Decode(error));
            }
        };
        if let Err(error) = validate_descriptor_batch_plan_v1(&dispatch.descriptor, plan, &batch) {
            return RemoteChunkTerminalV1::Failed(error);
        }
        let ordinal = match u32::try_from(ordinal) {
            Ok(ordinal) => ordinal,
            Err(_overflow) => {
                return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
            }
        };
        if expected.message_count_v1() == 0 {
            continue;
        }
        let built = if dispatch.descriptor.is_protobuf_v1() {
            build_protobuf_chunk_v1(dispatch.descriptor, plan, batch, ordinal)
        } else {
            build_ros_scalar_chunk_v1(dispatch.descriptor, plan, batch, ordinal)
        };
        let RemoteBuiltChannelChunkV1 { chunk, reservation } = match built {
            Ok(chunk) => chunk,
            Err(error) => return RemoteChunkTerminalV1::Failed(error),
        };
        let remaining_roots = match root_capacity.checked_sub(chunks.len()) {
            Some(remaining) => match u32::try_from(remaining) {
                Ok(remaining) => remaining,
                Err(_overflow) => {
                    return RemoteChunkTerminalV1::Failed(
                        RemoteChunkDispatchFailureV1::PlanMismatch,
                    );
                }
            },
            None => {
                return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
            }
        };
        let Some(remaining_output_bytes) = insertion_limits
            .max_output_physical_bytes_per_partition_v1()
            .checked_sub(output_physical_bytes)
        else {
            return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::PlanMismatch);
        };
        let channel_limits =
            insertion_limits.with_partition_remainder_v1(remaining_roots, remaining_output_bytes);
        if remaining_roots == 0 {
            return RemoteChunkTerminalV1::Failed(
                RemoteChunkDispatchFailureV1::DerivedInsertion(
                    crate::remote_deterministic_insertion::RemoteDerivedChunkResourceLimitV1::RootsPerPartition,
                ),
            );
        }
        // `Chunk::row_sliced_deep` and Arrow's builders allocate infallibly. A probe allocation
        // cannot transfer ownership to those later allocations, so the production path validates
        // the already-built root and rejects before any candidate copy when deep splitting would
        // be required. A later implementation needs a real fallible arena-backed builder.
        if let Err(error) = channel_limits.validate_prebuilt_root_v1(&chunk) {
            return RemoteChunkTerminalV1::Failed(RemoteChunkDispatchFailureV1::DerivedInsertion(
                error,
            ));
        }
        if chunk.total_size_bytes() > remaining_output_bytes {
            return RemoteChunkTerminalV1::Failed(
                RemoteChunkDispatchFailureV1::DerivedInsertion(
                    crate::remote_deterministic_insertion::RemoteDerivedChunkResourceLimitV1::OutputPhysicalBytes,
                ),
            );
        }
        let derived_chunks = [chunk];
        for chunk in derived_chunks {
            output_physical_bytes =
                match output_physical_bytes.checked_add(chunk.total_size_bytes()) {
                    Some(bytes) => bytes,
                    None => {
                        return RemoteChunkTerminalV1::Failed(
                            RemoteChunkDispatchFailureV1::PlanMismatch,
                        );
                    }
                };
            let output_ordinal = match u32::try_from(chunks.len()) {
                Ok(ordinal) => ordinal,
                Err(_overflow) => {
                    return RemoteChunkTerminalV1::Failed(
                        RemoteChunkDispatchFailureV1::PlanMismatch,
                    );
                }
            };
            let root = match authority.issue_root_v1(output_ordinal) {
                Ok(root) => root,
                Err(_error) => {
                    return RemoteChunkTerminalV1::Failed(
                        RemoteChunkDispatchFailureV1::PlanMismatch,
                    );
                }
            };
            chunks.push(RemoteTypedChunkHandoffV1 {
                chunk: chunk.with_id(root.root_chunk_id_v1()),
                root,
            });
        }
        reservations.push(reservation);
    }
    if chunks.is_empty() {
        RemoteChunkTerminalV1::CompleteEmpty
    } else {
        RemoteChunkTerminalV1::Complete(RemoteTypedPartitionHandoffV1 {
            partition: authority.partition_v1(),
            chunks: chunks.into_boxed_slice(),
            _reservations: reservations.into_boxed_slice(),
            _runtime_identifiers: runtime_identifiers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteChunkDispatchFailureV1, RemoteChunkTerminalV1, append_protobuf_message_v1,
        protobuf_struct_builder_v1, row_id_from_envelope_v1,
    };
    use arrow::array::{Array as _, Int32Array, ListArray, StringArray, StructArray};

    #[test]
    fn protobuf_builder_preserves_default_presence_nested_and_repeated_semantics() {
        use crate::remote_protobuf_descriptor::{
            RemoteNormalizedEnvelopeV1, RemoteNormalizedFieldV1, RemoteNormalizedValueV1,
        };
        use crate::remote_typed_output::{
            RemoteTypedOutputDescriptorV1, RemoteTypedProtobufFieldV1, RemoteTypedProtobufKindV1,
        };

        let field = |owner_message, tag, name: &str, kind, supports_presence, repeated| {
            RemoteTypedProtobufFieldV1 {
                owner_message,
                tag,
                name: name.to_owned().into_boxed_str(),
                kind,
                nullable: true,
                supports_presence,
                repeated,
                oneof_index: None,
                proto3_optional: false,
                packed: None,
                default: None,
            }
        };
        let descriptor = RemoteTypedOutputDescriptorV1::new_protobuf_for_dispatch_test_v1(
            vec![
                field(
                    0,
                    1,
                    "implicit",
                    RemoteTypedProtobufKindV1::Int32,
                    false,
                    false,
                ),
                field(
                    0,
                    2,
                    "optional",
                    RemoteTypedProtobufKindV1::String,
                    true,
                    false,
                ),
                field(
                    0,
                    3,
                    "nested",
                    RemoteTypedProtobufKindV1::Message(1),
                    true,
                    false,
                ),
                field(
                    0,
                    4,
                    "values",
                    RemoteTypedProtobufKindV1::Int32,
                    false,
                    true,
                ),
                field(
                    1,
                    1,
                    "child",
                    RemoteTypedProtobufKindV1::Int32,
                    false,
                    false,
                ),
            ]
            .into_boxed_slice(),
            0,
        );
        let envelope = RemoteNormalizedEnvelopeV1::new_for_dispatch_test_v1(
            vec![
                RemoteNormalizedFieldV1 {
                    tag: 1,
                    value: RemoteNormalizedValueV1::Signed(9),
                },
                RemoteNormalizedFieldV1 {
                    tag: 4,
                    value: RemoteNormalizedValueV1::Signed(1),
                },
                RemoteNormalizedFieldV1 {
                    tag: 4,
                    value: RemoteNormalizedValueV1::Signed(2),
                },
                RemoteNormalizedFieldV1 {
                    tag: 3,
                    value: RemoteNormalizedValueV1::Message {
                        first_value: 0,
                        value_count: 1,
                    },
                },
                RemoteNormalizedFieldV1 {
                    tag: 4,
                    value: RemoteNormalizedValueV1::Array {
                        first_value: 1,
                        value_count: 2,
                        fixed: false,
                    },
                },
            ],
            Vec::new(),
            3,
            2,
        );
        let mut builder = protobuf_struct_builder_v1(&descriptor, 0).unwrap();
        append_protobuf_message_v1(
            &descriptor,
            &envelope,
            &mut builder,
            0,
            envelope.fields_v1(),
        )
        .unwrap();
        let output = builder.finish();
        assert_eq!(output.len(), 1);
        let implicit = output
            .column(0)
            .as_any()
            .downcast_ref::<Int32Array>()
            .unwrap();
        assert_eq!(implicit.value(0), 0);
        let optional = output
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert!(optional.is_null(0));
        let nested = output
            .column(2)
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        assert!(nested.is_valid(0));
        assert_eq!(
            nested
                .column(0)
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .value(0),
            9
        );
        let values = output
            .column(3)
            .as_any()
            .downcast_ref::<ListArray>()
            .unwrap();
        assert!(values.is_valid(0));
        assert_eq!(
            values
                .value(0)
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .values(),
            &[1, 2]
        );
    }

    #[test]
    fn row_ids_follow_canonical_source_order_and_reject_unrepresentable_offsets() {
        let binding =
            crate::remote_chunk_scan::PhysicalChunkSourceBindingV1::new_unscanned_for_assignment_test_v1();
        let first = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding.clone(),
            9,
            12,
        );
        let second = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding.clone(),
            9,
            13,
        );
        let later_record = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding.clone(),
            10,
            0,
        );
        assert_eq!(
            row_id_from_envelope_v1(&first).unwrap().as_tuid().as_u128(),
            (9_u128 << 64) | (12_u128 << 16)
        );
        assert!(
            row_id_from_envelope_v1(&first).unwrap() < row_id_from_envelope_v1(&second).unwrap()
        );
        assert!(
            row_id_from_envelope_v1(&second).unwrap()
                < row_id_from_envelope_v1(&later_record).unwrap()
        );
        let too_large = crate::remote_chunk_scan::RemoteMessageEnvelopeV1::new_with_source_offsets_for_dispatch_test_v1(
            binding,
            10,
            1_u64 << 48,
        );
        assert_eq!(
            row_id_from_envelope_v1(&too_large),
            Err(RemoteChunkDispatchFailureV1::PlanMismatch)
        );
    }

    #[test]
    fn terminal_states_are_total_and_non_partial() {
        assert!(
            !RemoteChunkTerminalV1::Failed(super::RemoteChunkDispatchFailureV1::PlanMismatch)
                .is_complete_v1()
        );
        assert!(RemoteChunkTerminalV1::CompleteEmpty.is_complete_v1());
    }
}
