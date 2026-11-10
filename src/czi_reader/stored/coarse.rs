//! The coarse tiles of a file: the ones that can be read, and why the others cannot.

use crate::czi_reader::codec::pixel_type_to_dtype;
use crate::czi_reader::directory::RawDirEntry;
use crate::czi_reader::read::undecodable_compression;
use crate::czi_reader::subblock::{subblock_pixel_span, SubBlock};

use super::PyramidBlock;

/// Why a coarse tile could not be read.
#[derive(Clone)]
pub(in crate::czi_reader) enum Skip {
    /// The pixel type the reader does not model.
    UnmodelledPixelType,
    /// The pixel span runs out of the file.
    BadSpan,
    /// The compression this build cannot decode.
    UndecodableCompression(String),
    /// The stored or the logical size is zero.
    StoredSizeZero,
    /// The stored size does not divide the tile's size.
    StoredSizeNotDivisor,
    /// The stored size subsamples the two axes differently.
    StoredSizeAnisotropic,
}

impl Skip {
    pub(super) fn reason(&self) -> String {
        match self {
            Skip::UnmodelledPixelType => "a pixel type the reader does not model".to_string(),
            Skip::BadSpan => "a pixel span out of bounds".to_string(),
            Skip::UndecodableCompression(reason) => reason.clone(),
            Skip::StoredSizeZero => "a size of zero".to_string(),
            Skip::StoredSizeNotDivisor => "a stored size that does not divide the tile's size".to_string(),
            Skip::StoredSizeAnisotropic => "a stored size that subsamples the axes differently".to_string(),
        }
    }
}

/// A coarse tile the reader could not read.
pub(in crate::czi_reader) struct Skipped {
    /// The minification of the tile, when its stored size names one.
    pub factor: Option<usize>,
    /// The tile's size in source pixels: the cell of the level it belongs to.
    pub size: [usize; 2],
    /// Why the tile was skipped.
    pub reason: Skip,
}

/// One coarse subblock: its minification, its entry and its pixels, or why it has none.
pub(super) fn read_coarse(buf: &[u8], entry: &RawDirEntry) -> Result<PyramidBlock, Skipped> {
    let size = [entry.size_x, entry.size_y];
    let factor =
        minification_of(entry).map_err(|reason| Skipped {
            factor: None,
            size,
            reason,
        })?;
    if let Some(reason) = undecodable_compression(entry.compression) {
        return Err(Skipped {
            factor: Some(factor),
            size,
            reason: Skip::UndecodableCompression(reason),
        });
    }
    let (pixel_off, data_size) = subblock_pixel_span(buf, entry.file_position).map_err(|_| {
        Skipped {
            factor: Some(factor),
            size,
            reason: Skip::BadSpan,
        }
    })?;
    let (dtype, samples) = pixel_type_to_dtype(entry.pixel_type).map_err(|_| Skipped {
        factor: Some(factor),
        size,
        reason: Skip::UnmodelledPixelType,
    })?;
    Ok(PyramidBlock {
        factor,
        entry: entry.clone(),
        block: SubBlock {
            pixel_off,
            data_size,
            compression: entry.compression,
            dtype,
            samples,
            m: entry.m,
            start_x: entry.start_x,
            start_y: entry.start_y,
            stored_x: entry.stored_x,
            stored_y: entry.stored_y,
        },
    })
}

/// The minification a coarse subblock's stored size names, or the reason it names none.
pub(super) fn minification_of(entry: &RawDirEntry) -> Result<usize, Skip> {
    if entry.stored_x == 0 || entry.stored_y == 0 || entry.size_x == 0 || entry.size_y == 0 {
        return Err(Skip::StoredSizeZero);
    }
    let factor_x = entry.size_x / entry.stored_x;
    let factor_y = entry.size_y / entry.stored_y;
    // The subblock is a clean sub-sample only when its stored size divides its logical size.
    if factor_x * entry.stored_x != entry.size_x || factor_y * entry.stored_y != entry.size_y {
        return Err(Skip::StoredSizeNotDivisor);
    }
    if factor_x != factor_y {
        return Err(Skip::StoredSizeAnisotropic);
    }
    // A factor of one is stored == size on both axes, which is not a coarse tile at all.
    Ok(factor_x)
}
