use std::collections::BTreeMap;

use super::MessageDecoder;
use crate::parsers::MessageParser;
use crate::parsers::ros2msg::Ros2MessageParser;
use crate::parsers::ros2msg::sensor_msgs::{
    BatteryStateMessageParser, CompressedImageMessageParser, FluidPressureMessageParser,
    IlluminanceMessageParser, ImageMessageParser, ImuMessageParser, JointStateMessageParser,
    JoyMessageParser, NavSatFixMessageParser, PointCloud2MessageParser, RangeMessageParser,
    RelativeHumidityMessageParser, TemperatureMessageParser,
};
use crate::parsers::ros2msg::std_msgs::{
    Float64ArrayMessageParser, Float64MultiArrayMessageParser,
};
use crate::parsers::ros2msg::tf2_msgs::tf_message::TfMessageParser;

type ParserFactory = fn(usize) -> Box<dyn MessageParser>;

macro_rules! builtin_ros2_parsers {
    ($consumer:ident) => {
        $consumer! {
            BatteryStateMessageParser => "sensor_msgs/msg/BatteryState",
            CompressedImageMessageParser => "sensor_msgs/msg/CompressedImage",
            FluidPressureMessageParser => "sensor_msgs/msg/FluidPressure",
            IlluminanceMessageParser => "sensor_msgs/msg/Illuminance",
            ImageMessageParser => "sensor_msgs/msg/Image",
            ImuMessageParser => "sensor_msgs/msg/Imu",
            JoyMessageParser => "sensor_msgs/msg/Joy",
            JointStateMessageParser => "sensor_msgs/msg/JointState",
            NavSatFixMessageParser => "sensor_msgs/msg/NavSatFix",
            PointCloud2MessageParser => "sensor_msgs/msg/PointCloud2",
            RangeMessageParser => "sensor_msgs/msg/Range",
            RelativeHumidityMessageParser => "sensor_msgs/msg/RelativeHumidity",
            TemperatureMessageParser => "sensor_msgs/msg/Temperature",
            Float64ArrayMessageParser => "std_msgs/msg/Float64Array",
            Float64MultiArrayMessageParser => "std_msgs/msg/Float64MultiArray",
            TfMessageParser => "tf2_msgs/msg/TFMessage",
        }
    };
}

/// Allocation-free recognition shape of the built-in semantic ROS 2 decoder.
///
/// Remote assignment uses this only to fail closed while its exact-safe table is empty.
/// It does not construct a parser or make the native registry available to the remote route.
#[cfg(any(test, re_mcap_locked_remote_wasm_allocator_v1))]
pub(crate) fn supports_builtin_semantic_schema(schema_name: &str) -> bool {
    macro_rules! recognizes_schema {
        ($($parser:ty => $registered:literal),+ $(,)?) => {
            matches!(schema_name, $($registered)|+)
        };
    }
    builtin_ros2_parsers!(recognizes_schema)
}

#[derive(Debug)]
pub struct McapRos2Decoder {
    registry: BTreeMap<String, ParserFactory>,
}

impl McapRos2Decoder {
    const ENCODING: &str = "ros2msg";

    fn empty() -> Self {
        Self {
            registry: BTreeMap::new(),
        }
    }

    /// Creates a new [`McapRos2Decoder`] with all supported message types pre-registered
    pub fn new() -> Self {
        macro_rules! register_parsers {
            ($($parser:ty => $schema:literal),+ $(,)?) => {
                Self::empty()$(.register_parser::<$parser>($schema))+
            };
        }
        builtin_ros2_parsers!(register_parsers)
    }

    /// Registers a new message parser for the given schema name
    pub fn register_parser<T: Ros2MessageParser + 'static>(mut self, schema_name: &str) -> Self {
        self.registry
            .insert(schema_name.to_owned(), |n| Box::new(T::new(n)));
        self
    }

    /// Registers a message parser with a custom factory function
    pub fn register_parser_with_factory(
        mut self,
        schema_name: &str,
        factory: ParserFactory,
    ) -> Self {
        self.registry.insert(schema_name.to_owned(), factory);
        self
    }

    /// Returns true if the given schema is supported by this decoder
    pub fn supports_schema(&self, schema_name: &str) -> bool {
        self.registry.contains_key(schema_name)
    }
}

impl Default for McapRos2Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageDecoder for McapRos2Decoder {
    fn identifier() -> super::DecoderIdentifier {
        "ros2msg".into()
    }

    fn supports_channel(&self, channel: &mcap::Channel<'_>) -> bool {
        let Some(schema) = channel.schema.as_ref() else {
            return false;
        };

        if !self.registry.contains_key(&schema.name) {
            return false;
        }

        supports_ros2_cdr_channel(channel)
    }

    fn message_parser(
        &self,
        channel: &mcap::Channel<'_>,
        num_rows: usize,
    ) -> Option<Box<dyn MessageParser>> {
        let schema = channel.schema.as_ref()?;
        if schema.encoding.as_str() != Self::ENCODING {
            return None;
        }

        if let Some(make) = self.registry.get(&schema.name) {
            Some(make(num_rows))
        } else {
            re_log::warn_once!(
                "Message schema {:?} is currently not supported",
                schema.name
            );

            None
        }
    }
}

fn is_cdr_message_encoding(message_encoding: &str) -> bool {
    message_encoding.eq_ignore_ascii_case("cdr")
}

/// Returns true for channels that explicitly advertise CDR payloads.
pub(super) fn is_cdr_encoded_channel(channel: &mcap::Channel<'_>) -> bool {
    is_cdr_message_encoding(&channel.message_encoding)
}

/// Warns once if a ROS2 schema is not encoded as CDR.
pub(super) fn warn_if_ros2msg_non_cdr_channel(channel: &mcap::Channel<'_>) {
    // Note: empty encodings have a separate, ROS-independent warning.
    if channel.message_encoding.trim().is_empty()
        || is_cdr_message_encoding(&channel.message_encoding)
    {
        return;
    }

    re_log::warn_once!(
        concat!(
            "MCAP channel '{}' has a ROS2 message schema, but unknown encoding '{}'. ",
            "ROS 2 deserialization is only supported for CDR-encoded messages."
        ),
        channel.topic,
        channel.message_encoding,
    );
}

/// Returns true if the channel carries a ROS2 message schema with CDR payloads.
pub(super) fn supports_ros2_cdr_channel(channel: &mcap::Channel<'_>) -> bool {
    let Some(schema) = channel.schema.as_ref() else {
        return false;
    };

    if schema.encoding.as_str() != McapRos2Decoder::ENCODING {
        return false;
    }

    if !is_cdr_encoded_channel(channel) {
        warn_if_ros2msg_non_cdr_channel(channel);
        return false;
    }

    true
}
