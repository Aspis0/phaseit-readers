// SPDX-License-Identifier: Apache-2.0
//! Read-only memory maps of microscopy files, shared by the lazy readers.
//! Each reader maps its file once at `open()` and decodes from the bytes on every
//! `plane()` call; the `unsafe` map and the size guard live only here.
//! TODO: no read fallback for UNC/network paths, where mmap is unsafe under concurrent writers.

use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::guard::MAX_MMAP_BYTES;

/// Memory-maps a file read-only; the caller keeps it immutable while the map lives.
/// The [`File`] is dropped: the mapping outlives it on every supported platform.
/// Files over [`MAX_MMAP_BYTES`] are refused before mapping.
pub(crate) fn map_file(path: &Path) -> Result<Mmap> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_MMAP_BYTES as u64 {
        return Err(Error::OversizedAllocation {
            requested: usize::try_from(len).unwrap_or(usize::MAX),
            cap: MAX_MMAP_BYTES,
        });
    }

    // SAFETY: `Mmap::map` is UB if another handle/process mutates the bytes while the map lives;
    // our callers open read-only and treat the file as immutable for the reader's lifetime.
    // See the module TODO for the UNC/network fallback.
    let mmap = unsafe { Mmap::map(&file)? };
    Ok(mmap)
}
