// SPDX-License-Identifier: Apache-2.0
//! The image files themselves: each one is opened when a plane of it is first read and kept for the next plane.
//! Every file is checked against the dataset's own geometry, so a file of another sample type or another size is
//! refused by name instead of decoded into a plane of the wrong shape.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::guard::MAX_ND_OPEN_MEMBERS;
use crate::stk_reader::StkReader;
use crate::tiff_reader::TiffReader;
use crate::types::{DType, ImageReader};

/// Fewest open slots a dataset is sized for, whatever its file count: a three-file dataset gets eight, not three.
const MIN_OPEN_MEMBERS: usize = 8;

/// The geometry every image file of a dataset shares: its pixel type, its sample count and its x/y extent.
#[derive(Debug, Clone, Copy)]
pub(super) struct Geometry {
    pub dtype: DType,
    pub x: usize,
    pub y: usize,
    /// Samples per pixel: a grey file holds one, an RGB file three.
    pub samples: u32,
}

/// The dataset's image files, opened on demand; at most `bound` of them are open at once.
pub(super) struct Sources {
    geometry: Geometry,
    bound: usize,
    open: Mutex<Vec<(PathBuf, Arc<dyn ImageReader>)>>,
}

impl Sources {
    pub(super) fn new(geometry: Geometry, bound: usize) -> Sources {
        Sources {
            geometry,
            bound,
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
        self.reader_with(path, open_reader)
    }

    /// `reader` with the opener as a parameter, so a test can make an open fail the way the OS does.
    fn reader_with(
        &self,
        path: &Path,
        opener: impl Fn(&Path) -> Result<Arc<dyn ImageReader>>,
    ) -> Result<Arc<dyn ImageReader>> {
        let mut open = self.open.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(at) = open.iter().position(|(known, _)| known == path) {
            // A file read again moves to the back: the eviction below takes the least recently used one.
            let entry = open.remove(at);
            let reader = Arc::clone(&entry.1);
            open.push(entry);
            return Ok(reader);
        }
        let reader = open_evicting(&mut open, path, opener)?;
        self.check(reader.as_ref(), path)?;
        open.push((path.to_path_buf(), Arc::clone(&reader)));
        // The bound keeps a dataset of many files from holding them all open.
        if open.len() > self.bound {
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

/// How many image files a dataset of `files` keeps open: one per file it holds, never fewer than the floor and never
/// more than the ceiling.
pub(super) fn open_bound(files: usize) -> usize {
    files.clamp(MIN_OPEN_MEMBERS, MAX_ND_OPEN_MEMBERS)
}

/// The note a scene carries when the dataset holds more image files than stay open: reading across them reopens the
/// ones that were closed, so the cost is said by number.
pub(super) fn cache_note(files: usize, bound: usize) -> Option<String> {
    (files > bound).then(|| {
        format!("MetaMorph .nd: {files} image files, at most {bound} stay open: reading across files reopens them")
    })
}

/// The reader of one image file: a z-series is a MetaMorph STK, a single plane a plain TIFF. The dataset names the
/// files it lists, and the listing matches the name whatever its case, so the file's own extension names the reader.
fn open_reader(path: &Path) -> Result<Arc<dyn ImageReader>> {
    let z_stack = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("stk"));
    if z_stack {
        StkReader::open(path).map(|reader| Arc::new(reader) as Arc<dyn ImageReader>)
    } else {
        TiffReader::open(path).map(|reader| Arc::new(reader) as Arc<dyn ImageReader>)
    }
}

/// The reader of one image file, or the refusal that names it.
pub(super) fn open_file(path: &Path) -> Result<Arc<dyn ImageReader>> {
    open_reader(path).map_err(|error| member_error(path, error))
}

/// Opens a member; when the process is out of handles, the least recently used member gives its handle back and the
/// open is tried once more.
fn open_evicting(
    open: &mut Vec<(PathBuf, Arc<dyn ImageReader>)>,
    path: &Path,
    opener: impl Fn(&Path) -> Result<Arc<dyn ImageReader>>,
) -> Result<Arc<dyn ImageReader>> {
    match opener(path) {
        Err(error) if out_of_handles(&error) && !open.is_empty() => {
            open.remove(0);
            opener(path).map_err(|error| member_error(path, error))
        }
        other => other.map_err(|error| member_error(path, error)),
    }
}

/// The refusal for a member that will not open. It names the file, never the path it was found under, and a shortage
/// of handles is the process's, so it is not reported as a file that cannot be read.
fn member_error(path: &Path, error: Error) -> Error {
    let name = file_name(path);
    if out_of_handles(&error) {
        Error::Decode(format!(
            "MetaMorph .nd: the process ran out of file handles opening the image file {name}: {error}"
        ))
    } else {
        Error::Decode(format!(
            "MetaMorph .nd: the image file {name} could not be read: {error}"
        ))
    }
}

/// Whether an open failed for want of file handles in this process.
fn out_of_handles(error: &Error) -> bool {
    matches!(error, Error::Io(io) if io.raw_os_error().is_some_and(|code| OUT_OF_HANDLES.contains(&code)))
}

/// OS codes for a process out of file handles: EMFILE and ENFILE on Unix (24 and 23 on Linux and macOS).
#[cfg(not(windows))]
const OUT_OF_HANDLES: &[i32] = &[24, 23];

/// ERROR_TOO_MANY_OPEN_FILES on Windows; the kernel's no-resources code is not claimed here.
#[cfg(windows)]
const OUT_OF_HANDLES: &[i32] = &[4];

/// The name of a file, without the directory it sits in.
fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("the image file")
        .to_string()
}
