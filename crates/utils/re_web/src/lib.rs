//! Utilities for interacting with Web APIs.
//!
//! Remote-MCAP resource accounting is intentionally absent from the native production API:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::remote_limits;
//! ```
//!
//! Secret-bearing Web URL normalization is also absent from the native production API:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::secret_url;
//! ```
//!
//! Typed remote-object validators are likewise Web-only:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::remote_validator;
//! ```
//!
//! Strict-open wire identities and envelopes are also absent from the native production API:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::strict_open_wire;
//! ```
//!
//! Open-source terminal ownership and one-shot capability issuance are likewise Web-only:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::open_source_terminal;
//! ```
//!
//! Generation-aware Store publication identities are likewise Web-only:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::store_publication;
//! ```
//!
//! Exact-length Chrome BYOB body pumping is likewise Web-only:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::chrome_byob;
//! ```
//!
//! Strict Chrome Range request/response validation is Web-only as well:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::chrome_range;
//! ```
//!
//! Metadata-opening Range retry ownership is likewise Web-only and production-disarmed:
//!
//! ```compile_fail,ignore-wasm32
//! use re_web::range_retry;
//! ```

pub mod browser;

#[cfg(target_arch = "wasm32")]
pub mod chrome_byob;
#[cfg(target_arch = "wasm32")]
pub mod chrome_range;
#[cfg(target_arch = "wasm32")]
mod format_sniffer;
#[cfg(target_arch = "wasm32")]
pub mod open_source_terminal;
#[cfg(target_arch = "wasm32")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the sealed metadata retry foundation is wired to page execution later"
    )
)]
pub(crate) mod range_retry;
#[cfg(target_arch = "wasm32")]
pub mod remote_limits;
#[cfg(target_arch = "wasm32")]
pub mod remote_validator;
#[cfg(target_arch = "wasm32")]
pub mod secret_url;
#[cfg(target_arch = "wasm32")]
pub mod store_publication;
#[cfg(target_arch = "wasm32")]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the sealed strict-open decoder is foundational until its raw-JS adapter lands"
    )
)]
pub mod strict_open_wire;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise the Web-only schema without publishing its API"
)]
mod remote_limits;

// Host builds compile the pure pump state machine only for its unit tests and do not publish it.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod chrome_byob;

// Host builds compile only the pure Range/header state machine for differential unit tests.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod chrome_range;

// Host builds compile only the pure metadata retry state machine for differential unit tests.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod range_retry;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise Web-only validator parsing without publishing its API"
)]
mod remote_validator;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise the Web-only strict-open codec without publishing its API"
)]
mod strict_open_wire;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise the Web-only open-source terminal schema without publishing its API"
)]
mod open_source_terminal;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise Web-only URL normalization without publishing its API"
)]
mod secret_url;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise Web-only Store publication identities without publishing them"
)]
mod store_publication;

#[cfg(target_arch = "wasm32")]
mod error;
#[cfg(target_arch = "wasm32")]
pub mod fs;

#[cfg(target_arch = "wasm32")]
pub use error::Error;
