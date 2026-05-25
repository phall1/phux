//! Wire decode errors.
//!
//! Owned by phux-6yl.4.

use thiserror::Error;

/// Errors that can occur while decoding a wire frame.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// Placeholder until phux-6yl.4 fleshes this out.
    #[error("decode error: not yet implemented")]
    NotImplemented,
}
