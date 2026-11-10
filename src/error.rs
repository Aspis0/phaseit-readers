// SPDX-License-Identifier: Apache-2.0

use thiserror::Error;

use crate::detect::Format;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Recognized, but no reader exists yet (OIB and OIF).
    #[error("reader not implemented for format {0:?}")]
    NotImplemented(Format),

    #[error("decode error: {0}")]
    Decode(String),

    #[error("parse error: {0}")]
    Parse(String),

    /// Checked BEFORE any memory is reserved, so a bogus size never OOMs the process.
    #[error("oversized allocation refused: requested {requested} bytes, cap {cap} bytes")]
    OversizedAllocation { requested: usize, cap: usize },

    #[error("scene index {scene} out of range (file has {count} scene(s))")]
    SceneOutOfRange { scene: usize, count: usize },

    #[error("plane coordinate out of range: scene {scene}, t {t}, c {c}, z {z}")]
    OutOfRange { scene: usize, t: usize, c: usize, z: usize },

    /// Work the caller abandoned on purpose (e.g. a cancelled pyramid build), not a failure.
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
