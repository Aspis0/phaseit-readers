// SPDX-License-Identifier: Apache-2.0
//! Writes the small N5 datasets the reader tests read back: `attributes.json` under a dataset directory, and one
//! block file per grid position, at `g0/g1/…` (one directory per axis but the last, which is the file name).
//! A block header is big-endian: mode, ndim, then this block's extent along each axis. Its payload is the block's
//! own samples, plane after plane of its block row, big-endian, dimension 0 fastest.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use flate2::write::GzEncoder;
use phasit_imread::DType;

/// One scratch folder per dataset: tests run in parallel in one process, so a name alone is not unique.
static NEXT_FOLDER: AtomicUsize = AtomicUsize::new(0);

fn folder(name: &str) -> PathBuf {
    let unique = NEXT_FOLDER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("phasit_n5_{name}_{}_{unique}", std::process::id()))
}

/// The sample at `(y, x)` of plane `z`, the same in the test and in the writer. The weights leave every sample
/// below 256, so a uint8 dataset holds it too.
pub fn sample_i32(z: usize, y: usize, x: usize) -> i32 {
    z as i32 * 40 + y as i32 * 8 + x as i32
}

/// One dataset under its own directory, removed by `Drop` even when an assertion fails.
pub struct N5Dataset {
    dir: PathBuf,
    dimensions: Vec<usize>,
    block_size: Vec<usize>,
    dtype: DType,
    /// The compression as the attributes state it: `None` raw, and `Some("gzip")` / `Some("zstd")`.
    compression: Option<&'static str>,
    /// The attributes as last written.
    attributes: String,
}

impl N5Dataset {
    /// A raw dataset of `dimensions` and block `block_size` (dimension 0 fastest), its attributes written.
    pub fn raw(name: &str, dimensions: &[usize], block_size: &[usize], dtype: DType) -> N5Dataset {
        N5Dataset::build(name, dimensions, block_size, dtype, None)
    }

    /// A dataset of `dimensions` and block `block_size`, compressed as `compression` (`"gzip"` or `"zstd"`).
    pub fn new(
        name: &str,
        dimensions: &[usize],
        block_size: &[usize],
        dtype: DType,
        compression: Option<&'static str>,
    ) -> N5Dataset {
        N5Dataset::build(name, dimensions, block_size, dtype, compression)
    }

    fn build(
        name: &str,
        dimensions: &[usize],
        block_size: &[usize],
        dtype: DType,
        compression: Option<&'static str>,
    ) -> N5Dataset {
        let dir = folder(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch dataset");
        let mut dataset = N5Dataset {
            dir,
            dimensions: dimensions.to_vec(),
            block_size: block_size.to_vec(),
            dtype,
            compression,
            attributes: String::new(),
        };
        dataset.write("");
        dataset
    }

    /// A dataset whose `attributes.json` is the JSON the caller has written (a refusal test varies one key of it).
    pub fn at(name: &str, attributes: &str) -> N5Dataset {
        let dir = folder(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch dataset");
        std::fs::write(dir.join("attributes.json"), attributes).expect("write the attributes");
        N5Dataset {
            dir,
            dimensions: Vec::new(),
            block_size: Vec::new(),
            dtype: DType::U8,
            compression: None,
            attributes: attributes.to_string(),
        }
    }

    pub fn dimensions(&self) -> &[usize] {
        &self.dimensions
    }

    pub fn block_size(&self) -> &[usize] {
        &self.block_size
    }

    pub fn dtype(&self) -> DType {
        self.dtype
    }

    pub fn attributes(&self) -> &str {
        &self.attributes
    }

    pub fn attributes_path(&self) -> PathBuf {
        self.dir.join("attributes.json")
    }

    /// The compression as the legacy `compressionType` string.
    pub fn compress_legacy(&mut self, compression: &'static str) {
        self.compression = Some(compression);
        self.attributes = format!(
            r#"{{"dimensions":{:?},"blockSize":{:?},"dataType":"{}","compressionType":"{compression}"}}"#,
            self.dimensions,
            self.block_size,
            dtype_name(self.dtype)
        );
        self.flush();
    }

    /// The `pixelResolution` a calibration test states, the dataset's own compression kept.
    pub fn calibrate(&mut self, resolution: &str) {
        self.attributes = format!(
            r#"{{"dimensions":{:?},"blockSize":{:?},"dataType":"{}","compression":{{"type":"{}"}},"pixelResolution":{resolution}}}"#,
            self.dimensions,
            self.block_size,
            dtype_name(self.dtype),
            self.compression.unwrap_or("raw")
        );
        self.flush();
    }

    /// The attributes with further keys appended to them.
    pub fn written(mut self, extra: &str) -> N5Dataset {
        self.write(extra);
        self
    }

    fn write(&mut self, extra: &str) {
        self.attributes = format!(
            r#"{{"dimensions":{:?},"blockSize":{:?},"dataType":"{}","compression":{{"type":"{}"}}{}}}"#,
            self.dimensions,
            self.block_size,
            dtype_name(self.dtype),
            self.compression.unwrap_or("raw"),
            extra
        );
        self.flush();
    }

    fn flush(&self) {
        std::fs::write(self.attributes_path(), &self.attributes).expect("write attributes.json");
    }

    /// The path of the block at `grid`: `g0/g1/…`, one directory per axis but the last.
    pub fn block_path(&self, grid: &[usize]) -> PathBuf {
        let (last, directories) = grid.split_last().expect("a grid position");
        let mut path = self.dir.clone();
        for axis in directories {
            path.push(axis.to_string());
        }
        path.push(last.to_string());
        path
    }

    /// The block at `grid` and the payload file at `path` are written exactly as given, header and all.
    pub fn write_bytes(&self, grid: &[usize], bytes: &[u8]) {
        let path = self.block_path(grid);
        let folder = path.parent().expect("a block path inside the dataset");
        std::fs::create_dir_all(folder).expect("create the block folder");
        std::fs::write(&path, bytes).expect("write the block");
    }

    /// The block at `grid` whose header states `size` and whose payload is `payload`, both exactly as given.
    pub fn write_raw(&self, grid: &[usize], size: &[usize], payload: &[u8]) {
        let mut bytes: Vec<u8> = [0u16, size.len() as u16]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        bytes.extend(size.iter().flat_map(|v| (*v as u32).to_be_bytes()));
        bytes.extend_from_slice(payload);
        self.write_bytes(grid, &bytes);
    }

    /// The block at `grid` covering `size`, every plane of it stored as the dataset's compression states.
    pub fn write_block(&self, grid: &[usize], size: &[usize], dtype: DType) {
        let payload = self.samples(grid, size, dtype);
        self.write_raw(grid, size, &payload);
    }

    /// The block at `grid` whose header states `size`, holding the dataset's compression of exactly the plain
    /// bytes given: a stream the decoder inflates to `plain.len()` bytes, whatever its stored length is.
    pub fn write_stream(&self, grid: &[usize], size: &[usize], plain: &[u8]) {
        let stored = match self.compression {
            Some("gzip") => {
                let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(plain).expect("deflate the stream");
                encoder.finish().expect("finish the gzip stream")
            }
            Some("zstd") => {
                ruzstd::encoding::compress_to_vec(plain, ruzstd::encoding::CompressionLevel::Fastest)
            }
            _ => plain.to_vec(),
        };
        self.write_raw(grid, size, &stored);
    }

    /// Every block of the dataset, each sized to the extent it covers so every edge block is partial.
    pub fn write_all_blocks(&self) {
        for g_x in 0..self.count(0) {
            for g_y in 0..self.count(1) {
                for g_z in 0..self.count(2) {
                    let mut at = vec![g_x, g_y, g_z];
                    at.truncate(self.dimensions.len());
                    let size: Vec<usize> = (0..at.len())
                        .map(|axis| {
                            self.block_size[axis]
                                .min(self.dimensions[axis] - at[axis] * self.block_size[axis])
                        })
                        .collect();
                    self.write_block(&at, &size, self.dtype);
                }
            }
        }
    }

    /// The blocks along one axis: a 2-D dataset has one plane, stated as extent 1 and block side 1.
    fn count(&self, axis: usize) -> usize {
        let extent = self.dimensions.get(axis).copied().unwrap_or(1);
        let side = self.block_size.get(axis).copied().unwrap_or(1);
        extent.div_ceil(side)
    }

    fn samples(&self, grid: &[usize], size: &[usize], dtype: DType) -> Vec<u8> {
        let raw = self.raw_samples(grid, size, dtype);
        if self.compression == Some("gzip") {
            let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder
                .write_all(&raw)
                .expect("deflate the synthetic block");
            return encoder.finish().expect("finish the gzip stream");
        }
        if self.compression == Some("zstd") {
            return ruzstd::encoding::compress_to_vec(
                &raw[..],
                ruzstd::encoding::CompressionLevel::Fastest,
            );
        }
        raw
    }

    /// Every sample of the block, plane after plane, then y then x, big-endian, float samples carrying +0.5.
    fn raw_samples(&self, grid: &[usize], size: &[usize], dtype: DType) -> Vec<u8> {
        let planes = size.get(2).copied().unwrap_or(1);
        let mut out = Vec::with_capacity(size.iter().product::<usize>() * dtype.size_bytes());
        for z in 0..planes {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let at = [
                        grid[0] * self.block_size[0] + x,
                        grid[1] * self.block_size[1] + y,
                        grid.get(2).map_or(z, |g| g * self.block_size[2] + z),
                    ];
                    let sample = sample_bytes(sample_i32(at[2], at[1], at[0]), dtype);
                    out.extend_from_slice(&sample[..dtype.size_bytes()]);
                }
            }
        }
        out
    }
}

/// The sample as the writer stores it: big-endian, dimension 0 fastest, floats carrying the writer's +0.5.
fn sample_bytes(value: i32, dtype: DType) -> [u8; 4] {
    match dtype {
        DType::U8 => [value as u8, 0, 0, 0],
        DType::U16 => {
            let b = (value as u16).to_be_bytes();
            [b[0], b[1], 0, 0]
        }
        DType::I16 => {
            let b = (value as i16).to_be_bytes();
            [b[0], b[1], 0, 0]
        }
        DType::F32 => ((value as f32) + 0.5).to_be_bytes(),
        _ => panic!("the synthetic writer states no sample of {dtype:?}"),
    }
}

impl Drop for N5Dataset {
    fn drop(&mut self) {
        // Drop ignores every error on purpose: a panic in Drop during an unwinding test would abort the run.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl std::ops::Deref for N5Dataset {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.dir
    }
}

fn dtype_name(dtype: DType) -> &'static str {
    match dtype {
        DType::U8 => "uint8",
        DType::U16 => "uint16",
        DType::I16 => "int16",
        DType::F32 => "float32",
        _ => panic!("the synthetic writer names no sample type {dtype:?}"),
    }
}
