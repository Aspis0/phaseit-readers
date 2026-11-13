// SPDX-License-Identifier: Apache-2.0
//! The image files themselves: each one is opened when a plane of it is first read and kept for the next plane.
//! Every file is checked against the dataset's own geometry, so a file of another sample type or another size is
//! refused by name instead of decoded into a plane of the wrong shape.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::stk_reader::StkReader;
use crate::tiff_reader::TiffReader;
use crate::types::{DType, ImageReader};

/// How many image files are kept open at once; a dataset is read plane after plane, so a few files cover it.
const MAX_CACHED_SOURCES: usize = 8;

/// The geometry every image file of a dataset shares: its pixel type, its sample count and its x/y extent.
#[derive(Debug, Clone, Copy)]
pub(super) struct Geometry {
    pub dtype: DType,
    pub x: usize,
    pub y: usize,
    /// Samples per pixel: a grey file holds one, an RGB file three.
    pub samples: u32,
}

/// The dataset's image files, opened on demand.
pub(super) struct Sources {
    geometry: Geometry,
    open: Mutex<Vec<(PathBuf, Arc<dyn ImageReader>)>>,
}

impl Sources {
    pub(super) fn new(geometry: Geometry) -> Sources {
        Sources {
            geometry,
            open: Mutex::new(Vec::new()),
        }
    }

    /// The reader of the dataset's first file, kept open from the start.
    pub(super) fn insert(&self, path: &Path, reader: Arc<dyn ImageReader>) {
        let mut open = self.open.lock().unwrap_or_else(|error| error.into_inner());
        open.push((path.to_path_buf(), reader));
    }

    /// The reader of the image file at `path`, opened on first use.
    pub(super) fn reader(&self, path: &Path) -> Result<Arc<dyn ImageReader>> {
        let mut open = self.open.lock().unwrap_or_else(|error| error.into_inner());
        if let Some((_, reader)) = open.iter().find(|(at, _)| at == path) {
            return Ok(Arc::clone(reader));
        }
        let reader = open_file(path)?;
        self.check(reader.as_ref(), path)?;
        open.push((path.to_path_buf(), Arc::clone(&reader)));
        // The oldest file goes first: the bound keeps a dataset of many files from holding them all open.
        if open.len() > MAX_CACHED_SOURCES {
            open.remove(0);
        }
        Ok(reader)
    }

    /// The file's own geometry against the dataset's: the first mismatch is named.
    fn check(&self, reader: &dyn ImageReader, path: &Path) -> Result<()> {
        let name = file_name(path);
        let scene = reader.meta().scene(0)?;
        if scene.dtype != self.geometry.dtype {
            return Err(Error::Unsupported(format!(
                "MetaMorph .nd: the image file {name} holds {:?} pixels while the dataset's first file holds {:?}",
                scene.dtype, self.geometry.dtype
            )));
        }
        if scene.samples != self.geometry.samples {
            return Err(Error::Unsupported(format!(
                "MetaMorph .nd: the image file {name} holds {} sample(s) per pixel while the dataset's first file \
                 holds {}",
                scene.samples, self.geometry.samples
            )));
        }
        if scene.dims.x != self.geometry.x || scene.dims.y != self.geometry.y {
            return Err(Error::Unsupported(format!(
                "MetaMorph .nd: the image file {name} is {}x{} while the dataset's first file is {}x{}",
                scene.dims.x, scene.dims.y, self.geometry.x, self.geometry.y
            )));
        }
        Ok(())
    }
}

/// The reader of one image file: a z-series is a MetaMorph STK, a single plane a plain TIFF. The dataset names the
/// files it lists, and the listing matches the name whatever its case, so the file's own extension names the reader.
pub(super) fn open_file(path: &Path) -> Result<Arc<dyn ImageReader>> {
    let z_stack = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("stk"));
    let opened = if z_stack {
        StkReader::open(path).map(|reader| Arc::new(reader) as Arc<dyn ImageReader>)
    } else {
        TiffReader::open(path).map(|reader| Arc::new(reader) as Arc<dyn ImageReader>)
    };
    // The refusal names the file, never the path it was found under.
    opened.map_err(|error| {
        Error::Decode(format!(
            "MetaMorph .nd: the image file {} could not be read: {error}",
            file_name(path)
        ))
    })
}

/// The name of a file, without the directory it sits in.
fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("the image file")
        .to_string()
}
