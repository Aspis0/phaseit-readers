// SPDX-License-Identifier: Apache-2.0
//! Error type for phasit-imread.

use thiserror::Error;

use crate::detect::Format;

/// Errors returned by phasit-imread.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The requested file/format is not supported (unrecognized or out of scope).
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// The format was recognized, but no reader is implemented for it yet
    /// (OIB and OIF).
    #[error("reader not implemented for format {0:?}")]
    NotImplemented(Format),

    /// A byte buffer could not be decoded into pixels (bad length, corrupt run, etc.).
    #[error("decode error: {0}")]
    Decode(String),

    /// Container/metadata parsing failed (bad XML, unexpected structure, etc.).
    #[error("parse error: {0}")]
    Parse(String),

    /// A guarded allocation was refused because it exceeded a safety cap.
    /// Computed BEFORE any memory is reserved, so it never OOMs the process.
    #[error("oversized allocation refused: requested {requested} bytes, cap {cap} bytes")]
    OversizedAllocation { requested: usize, cap: usize },

    /// A scene index was out of range for the file (`scene >= count`).
    #[error("scene index {scene} out of range (file has {count} scene(s))")]
    SceneOutOfRange { scene: usize, count: usize },

    /// A plane coordinate was out of range for its scene. Returned by
    /// `plane()` when `(scene, t, c, z)` does not address a real plane.
    #[error("plane coordinate out of range: scene {scene}, t {t}, c {c}, z {z}")]
    OutOfRange { scene: usize, t: usize, c: usize, z: usize },

    /// The caller asked for the work to stop. NOT a failure: nothing went
    /// wrong, the operation was abandoned on purpose (e.g. a cancelled pyramid
    /// build), so a caller can tell it apart from a real error.
    #[error("cancelled")]
    Cancelled,
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
