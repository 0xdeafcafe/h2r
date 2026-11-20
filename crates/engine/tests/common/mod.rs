//! Helpers the XBE-backed tests share: the image's path and its hash, now
//! `lifted::image`'s (one fixture for every differential test).

#![allow(dead_code, unused_imports)]

pub use lifted::image::{IMAGE_SHA256, sha256, xbe_path};
