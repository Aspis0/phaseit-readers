// SPDX-License-Identifier: Apache-2.0
//! Shared, crate-internal I/O helpers.
//!
//! Every reader that decodes pixels lazily (TIFF/LSM/LIF/…) maps the file
//! read-only once at `open()` time and then builds throw-away decoders over the
//! mapped bytes on each `plane()` call. Centralising the mmap here keeps the
//! `unsafe` and the size guard in one audited place.

use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::guard::MAX_MMAP_BYTES;

/// Memory-map a file read-only.
///
/// UNSAFE upstream: the caller must treat the file as immutable for the returned
/// map's lifetime. We hold the [`Mmap`] (which keeps the mapping alive) but do
/// NOT keep the [`File`] handle open — on every supported platform the mapping
/// remains valid after the descriptor is closed.
///
/// Rejects files larger than [`MAX_MMAP_BYTES`] before mapping, so a truncated
/// or absurd declared size cannot map an unreasonable region.
///
/// TODO: add a buffered positioned-read fallback for network / UNC paths
/// where mmap is unavailable or unsafe (concurrent external writers).
pub(crate) fn map_file(path: &Path) -> Result<Mmap> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_MMAP_BYTES as u64 {
        return Err(Error::OversizedAllocation {
            requested: usize::try_from(len).unwrap_or(usize::MAX),
            cap: MAX_MMAP_BYTES,
        });
    }

    // SAFETY: `Mmap::map` is unsafe because the borrowed file's contents must not
    // be mutated by another handle/process while the map is live (doing so is UB
    // for the resulting `&[u8]`). Our callers open microscopy files read-only and
    // treat them as immutable inputs for the lifetime of the reader, upholding
    // that invariant. See the module-level TODO for the network/UNC fallback.
    let mmap = unsafe { Mmap::map(&file)? };
    Ok(mmap)
}
