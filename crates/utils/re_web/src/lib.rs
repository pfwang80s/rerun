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

pub mod browser;

#[cfg(target_arch = "wasm32")]
pub mod remote_limits;
#[cfg(target_arch = "wasm32")]
pub mod secret_url;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise the Web-only schema without publishing its API"
)]
mod remote_limits;

// Host builds compile the module only for its unit tests and do not publish it to consumers.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[expect(
    dead_code,
    reason = "native tests exercise Web-only URL normalization without publishing its API"
)]
mod secret_url;

#[cfg(target_arch = "wasm32")]
mod error;
#[cfg(target_arch = "wasm32")]
pub mod fs;

#[cfg(target_arch = "wasm32")]
pub use error::Error;
