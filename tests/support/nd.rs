// SPDX-License-Identifier: Apache-2.0
//! Writes the small MetaMorph `.nd` datasets the reader tests read back: the `.nd` text file, and one image file
//! per (wavelength, stage position, time point), named the way the dataset names it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tiff::encoder::Compression;

use super::stk::{sample_at, StkFile};
use super::tiff_page::SampleType;
use super::tiff_striped::{write_striped, StripedPage};
use super::{write_grey, GreyPage};

/// One scratch folder per dataset: tests run in parallel in one process, so a name alone is not unique.
static NEXT_FOLDER: AtomicUsize = AtomicUsize::new(0);

fn folder(name: &str) -> PathBuf {
    let unique = NEXT_FOLDER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("phasit_nd_{name}_{}_{unique}", std::process::id()))
}

/// A dataset under its own directory, removed by `Drop` even when an assertion fails.
pub struct NdDataset {
    dir: PathBuf,
}

impl NdDataset {
    /// A dataset whose `.nd` file holds exactly the text given.
    pub fn with_text(name: &str, text: &str) -> NdDataset {
        let dir = folder(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch dataset");
        let dataset = NdDataset { dir };
        dataset.write_nd("dataset", text);
        dataset
    }

    /// The `.nd` file of the dataset: the text given, under the stem `base`.
    pub fn write_nd(&self, base: &str, text: &str) -> PathBuf {
        let path = self.dir.join(format!("{base}.nd"));
        std::fs::write(&path, text).expect("write the .nd file");
        path
    }

    /// The image file named `name`: a one-plane TIFF whose every pixel is the value given.
    pub fn write_tiff(&self, name: &str, width: u32, height: u32, value: u8) -> PathBuf {
        let path = self.dir.join(name);
        write_grey(
            &path,
            None,
            &[GreyPage {
                width,
                height,
                value,
            }],
        );
        path
    }

    /// The image file named `name`: a one-plane TIFF of 16-bit samples.
    pub fn write_tiff_u16(&self, name: &str, width: u32, height: u32) -> PathBuf {
        let path = self.dir.join(name);
        write_striped(
            &path,
            None,
            &[StripedPage {
                width,
                height,
                samples: 1,
                sample_type: SampleType::U16,
                rows_per_strip: height,
                planar: false,
            }],
            Compression::Uncompressed,
        );
        path
    }

    /// The image file named `name`: a one-plane TIFF of three samples per pixel.
    pub fn write_tiff_rgb(&self, name: &str, width: u32, height: u32) -> PathBuf {
        let path = self.dir.join(name);
        write_striped(
            &path,
            None,
            &[StripedPage {
                width,
                height,
                samples: 3,
                sample_type: SampleType::U8,
                rows_per_strip: height,
                planar: false,
            }],
            Compression::Uncompressed,
        );
        path
    }

    /// The image file named `name`: an STK of `planes` planes, every sample carrying `offset`.
    pub fn write_stk(
        &self,
        name: &str,
        width: u32,
        height: u32,
        planes: usize,
        offset: u16,
    ) -> PathBuf {
        let path = self.dir.join(name);
        let mut file = StkFile::u16(width, height, planes);
        file.value_offset = offset;
        file.write(&path);
        path
    }
}

/// The samples of one plane of an STK the writer wrote: its own pattern plus the file's offset.
pub fn plane_samples(plane: usize, width: u32, height: u32, offset: u16) -> Vec<u16> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| sample_at(plane, y, x).wrapping_add(offset)))
        .collect()
}

/// The `.nd` text of a dataset: the keys given, in the order MetaMorph writes them.
pub struct NdText {
    lines: Vec<String>,
}

impl NdText {
    /// A text that opens with the `"NDInfoFile"` key the reader looks for.
    pub fn new() -> NdText {
        NdText {
            lines: vec!["\"NDInfoFile\", Version 1.0".to_string()],
        }
    }

    /// One more key: a quoted name, or a value as the file states it (`TRUE`, a number).
    pub fn key(mut self, key: &str, value: &str) -> NdText {
        self.lines.push(format!("\"{key}\", {value}"));
        self
    }

    /// The text of the file, ending with the `"EndFile"` marker.
    pub fn text(self) -> String {
        let mut lines = self.lines;
        lines.push("\"EndFile\"".to_string());
        // A Windows acquisition writes its lines with CRLF.
        lines.join("\r\n") + "\r\n"
    }
}

impl Drop for NdDataset {
    fn drop(&mut self) {
        // Drop ignores every error on purpose: a panic in Drop during an unwinding test would abort the run.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl std::ops::Deref for NdDataset {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.dir
    }
}
