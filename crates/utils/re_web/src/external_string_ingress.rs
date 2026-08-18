//! Bounded strict remote-MCAP external string ingress.
//!
//! The browser adapter performs the JavaScript primitive/UTF-16 check before calling into Wasm.
//! This module is the second gate: it accepts only an opaque, already-copied UTF-8 value and
//! reserves the complete retained object graph before constructing semantic identifiers.

use std::{borrow::Cow, fmt, num::NonZeroU64};

pub const MAX_FIELD_UTF16: u64 = 65_536;
pub const MAX_FIELD_UTF8: u64 = 65_536;
pub const MAX_COMBINED_UTF8: u64 = 65_536;
pub const MAX_OBJECT_GRAPH_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_FIELDS: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalStringIngressError {
    InvalidUtf8,
    FieldLimit,
    CombinedLimit,
    ArithmeticOverflow,
}

/// Opaque proof that the JS adapter observed a primitive string and copied it exactly once.
pub struct OpaqueJsString<'a> {
    value: Cow<'a, str>,
}

impl fmt::Debug for OpaqueJsString<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueJsString(<redacted>)")
    }
}

impl<'a> OpaqueJsString<'a> {
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(
        value: &wasm_bindgen::JsValue,
    ) -> Result<OpaqueJsString<'static>, ExternalStringIngressError> {
        if !value.is_string() {
            return Err(ExternalStringIngressError::InvalidUtf8);
        }
        let js_string = js_sys::JsString::from(value.clone());
        let units = js_string.length() as u64;
        if units > MAX_FIELD_UTF16 {
            return Err(ExternalStringIngressError::FieldLimit);
        }
        let owned = js_string
            .as_string()
            .ok_or(ExternalStringIngressError::InvalidUtf8)?;
        let bytes = owned.len() as u64;
        if bytes > MAX_FIELD_UTF8 {
            return Err(ExternalStringIngressError::FieldLimit);
        }
        Ok(OpaqueJsString {
            value: Cow::Owned(owned),
        })
    }

    pub fn from_utf8(bytes: &'a [u8]) -> Result<Self, ExternalStringIngressError> {
        let value =
            std::str::from_utf8(bytes).map_err(|_| ExternalStringIngressError::InvalidUtf8)?;
        let units = value.encode_utf16().count() as u64;
        let bytes_len = bytes.len() as u64;
        if units > MAX_FIELD_UTF16 || bytes_len > MAX_FIELD_UTF8 {
            return Err(ExternalStringIngressError::FieldLimit);
        }
        Ok(Self {
            value: Cow::Borrowed(value),
        })
    }

    pub fn as_str(&self) -> &str {
        self.value.as_ref()
    }

    pub fn utf8_len(&self) -> u64 {
        self.value.len() as u64
    }

    pub fn utf16_len(&self) -> u64 {
        self.as_str().encode_utf16().count() as u64
    }
}

/// Request-local permit covering all copied strings and the retained Rust object graph.
#[derive(Debug, PartialEq, Eq)]
pub struct CombinedCopyPermit {
    fields: u32,
    utf16_code_units: u64,
    utf8_bytes: u64,
    object_graph_bytes: NonZeroU64,
}

#[allow(dead_code, reason = "accessors consumed by the future Wasm handoff adapter")]
impl CombinedCopyPermit {
    pub const fn fields(&self) -> u32 {
        self.fields
    }
    pub const fn utf16_code_units(&self) -> u64 {
        self.utf16_code_units
    }
    pub const fn utf8_bytes(&self) -> u64 {
        self.utf8_bytes
    }
    pub const fn object_graph_bytes(&self) -> NonZeroU64 {
        self.object_graph_bytes
    }
}

impl CombinedCopyPermit {
    pub fn prepare<'a, I>(
        strings: I,
        object_graph_bytes: u64,
    ) -> Result<Self, ExternalStringIngressError>
    where
        I: IntoIterator<Item = OpaqueJsString<'a>>,
    {
        let mut fields = 0u32;
        let mut utf16 = 0u64;
        let mut utf8 = 0u64;
        for value in strings {
            fields = fields
                .checked_add(1)
                .ok_or(ExternalStringIngressError::ArithmeticOverflow)?;
            if fields > MAX_FIELDS {
                return Err(ExternalStringIngressError::CombinedLimit);
            }
            utf16 = utf16
                .checked_add(value.utf16_len())
                .ok_or(ExternalStringIngressError::ArithmeticOverflow)?;
            utf8 = utf8
                .checked_add(value.utf8_len())
                .ok_or(ExternalStringIngressError::ArithmeticOverflow)?;
            if utf8 > MAX_COMBINED_UTF8 {
                return Err(ExternalStringIngressError::CombinedLimit);
            }
        }
        let object_graph_bytes =
            NonZeroU64::new(object_graph_bytes).ok_or(ExternalStringIngressError::FieldLimit)?;
        if object_graph_bytes.get() > MAX_OBJECT_GRAPH_BYTES {
            return Err(ExternalStringIngressError::FieldLimit);
        }
        object_graph_bytes
            .get()
            .checked_add(utf8)
            .ok_or(ExternalStringIngressError::ArithmeticOverflow)?;
        Ok(Self {
            fields,
            utf16_code_units: utf16,
            utf8_bytes: utf8,
            object_graph_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_utf8_and_combined_overflow() {
        assert_eq!(
            OpaqueJsString::from_utf8(&[0xff]).unwrap_err(),
            ExternalStringIngressError::InvalidUtf8
        );
        let a = OpaqueJsString::from_utf8(Box::leak(
            vec![b'x'; MAX_FIELD_UTF8 as usize].into_boxed_slice(),
        ))
        .unwrap();
        let b = OpaqueJsString::from_utf8(Box::leak(vec![b'y'; 1].into_boxed_slice())).unwrap();
        assert_eq!(
            CombinedCopyPermit::prepare([a, b], 1).unwrap_err(),
            ExternalStringIngressError::CombinedLimit
        );
    }

    #[test]
    fn reserves_nonempty_object_graph() {
        let value = OpaqueJsString::from_utf8(b"timeline").unwrap();
        assert!(format!("{value:?}").contains("<redacted>"));
        let permit = CombinedCopyPermit::prepare([value], 8).unwrap();
        assert_eq!(permit.utf16_code_units(), 8);
        assert_eq!(permit.utf8_bytes(), 8);
        let value = OpaqueJsString::from_utf8(b"timeline").unwrap();
        assert_eq!(
            CombinedCopyPermit::prepare([value], MAX_OBJECT_GRAPH_BYTES + 1).unwrap_err(),
            ExternalStringIngressError::FieldLimit
        );
    }
}
