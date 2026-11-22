//! The canvas's errors.

use std::fmt;

use crate::LayerId;

/// Why a canvas operation did not happen. Nothing is changed when one is
/// returned.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// A width or height of zero, or over [`crate::MAX_SIZE`].
    Size(String),
    /// A rectangle or point outside the canvas, or a pixel buffer of the
    /// wrong length.
    Rect(String),
    /// A parameter that is not finite or not in range.
    Params(String),
    /// No such layer.
    NoLayer(LayerId),
    /// The layer is not editable; the text is why.
    ReadOnly(String),
    /// A stroke is in progress.
    Busy,
    /// No stroke is in progress.
    NoStroke,
    /// A PNG that cannot be read or written.
    Png(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Size(s) | Error::Rect(s) | Error::Params(s) | Error::Png(s) => f.write_str(s),
            Error::NoLayer(id) => write!(f, "no layer {id}"),
            Error::ReadOnly(why) => write!(f, "read-only: {why}"),
            Error::Busy => f.write_str("a stroke is in progress"),
            Error::NoStroke => f.write_str("no stroke is in progress"),
        }
    }
}

impl std::error::Error for Error {}
