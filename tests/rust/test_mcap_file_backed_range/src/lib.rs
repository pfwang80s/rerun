//! Chrome-only browser probes against the file-backed Range fixture.
//!
//! The production remote-MCAP capability remains disarmed; these probes exercise only the
//! file-backed transport boundary (exact ranges, byte validation, ETag/Content-Range,
//! EOF/BYOB, cancel, query rejection, out-of-range, and error redaction).
