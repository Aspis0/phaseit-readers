// SPDX-License-Identifier: Apache-2.0
//! One block file: a big-endian `mode`, `ndim` and `size`, then the samples compressed as the dataset states.
//! The payload is inflated to exactly the count its header states and kept as big-endian bytes; the plane is
//! converted from them once, after its last block is in place.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use ruzstd::decoding::StreamingDecoder;

use crate::error::{Error, Result};
use crate::guard::MAX_DECODED_PLANE_BYTES;
use crate::types::DType;

use super::attributes::Compression;

#[derive(Debug)]
pub(super) struct BlockHeader {
    /// This block's extent per axis, dimension 0 fastest; never above the dataset's block size nor past its edge.
    pub size: Vec<usize>,
    /// The big-endian length of the header that states these extents.
    pub len: usize,
}

/// A decoded block: its own `[x, y, z]` extent and its samples as big-endian bytes, dimension 0 fastest.
pub(super) struct Block {
    shape: [usize; 3],
    bytes: Vec<u8>,
    sample_bytes: usize,
}

impl Block {
    /// The block's own extent per axis, dimension 0 fastest; a 2-D dataset's blocks hold one plane.
    pub fn shape(&self) -> [usize; 3] {
        self.shape
    }

    /// The block's row `y` of its plane `z`; `None` when the row is past the block's own bytes.
    pub fn row(&self, z: usize, y: usize) -> Result<&[u8]> {
        let at = z * self.plane_bytes() + y * self.row_bytes();
        self.bytes.get(at..at + self.row_bytes()).ok_or_else(|| {
            Error::Decode(format!(
                "N5 block row {y} is past its own {} byte(s)",
                self.bytes.len()
            ))
        })
    }

    /// Bytes of one row of one plane of the block.
    pub fn row_bytes(&self) -> usize {
        self.shape[0] * self.sample_bytes
    }

    /// Bytes of one plane of the block.
    fn plane_bytes(&self) -> usize {
        self.shape[1] * self.row_bytes()
    }

    /// Bytes of one sample of the block.
    pub fn sample_bytes(&self) -> usize {
        self.sample_bytes
    }
}

/// The block file at `path` read whole, its header checked and its payload inflated.
pub(super) fn read(
    path: &Path,
    block_size: &[usize],
    edge: &[usize],
    dtype: DType,
    compression: Compression,
) -> Result<Block> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let header = header(&bytes, block_size, edge)?;
    decode(&header, &bytes[header.len..], compression, dtype)
}

fn header(bytes: &[u8], block_size: &[usize], edge: &[usize]) -> Result<BlockHeader> {
    let mode = word(bytes, 0, 2)?;
    // The two var-length modes encode the extents elsewhere; mode 2 stores objects, not samples.
    let unsupported = match mode {
        0 => None,
        1 => Some("1, variable length".to_string()),
        2 => Some("2, an object".to_string()),
        other => Some(format!("{other}, which is not a mode this reader knows")),
    };
    if let Some(what) = unsupported {
        return Err(Error::Unsupported(format!(
            "N5 block mode is {what}: not the default (0) this reader reads"
        )));
    }
    let ndim = word(bytes, 2, 2)?;
    if ndim as usize != block_size.len() {
        return Err(Error::Parse(format!(
            "N5 block states {ndim} dimension(s), the dataset states {}",
            block_size.len()
        )));
    }
    let mut size = Vec::with_capacity(ndim as usize);
    for axis in 0..ndim as usize {
        size.push(extent(
            axis,
            word(bytes, 4 + 4 * axis, 4)?,
            block_size,
            edge,
        )?);
    }
    Ok(BlockHeader {
        size,
        len: 4 + 4 * ndim as usize,
    })
}

/// A block's extent along one axis: positive, no larger than the dataset's block size and inside the dataset edge.
fn extent(axis: usize, stated: u64, block_size: &[usize], edge: &[usize]) -> Result<usize> {
    let block = block_size.get(axis).copied().unwrap_or(0);
    let dataset = edge.get(axis).copied().unwrap_or(0);
    match usize::try_from(stated) {
        Ok(stated) if stated > 0 && stated <= block && stated <= dataset => Ok(stated),
        _ => Err(Error::Parse(format!(
            "N5 block size {stated} is not between 1 and the block size {block} and dataset edge {dataset} along axis {axis}"
        ))),
    }
}

/// The block's bytes, inflated to the count its own header states: a payload that holds another block's samples
/// (or none) is refused rather than read as this block's.
pub(super) fn decode(
    header: &BlockHeader,
    stored: &[u8],
    compression: Compression,
    dtype: DType,
) -> Result<Block> {
    let sample_bytes = dtype.size_bytes();
    let bytes = priced(&header.size, sample_bytes)?;
    Ok(Block {
        shape: [
            header.size[0],
            header.size[1],
            header.size.get(2).copied().unwrap_or(1),
        ],
        bytes: inflate(stored, compression, bytes)?,
        sample_bytes,
    })
}

/// The bytes a block's header promises: every extent priced in and capped through the guard before anything is
/// reserved. A 3-D block is the smallest unit the reader decodes, so its z extent counts against the ceiling too.
fn priced(size: &[usize], sample_bytes: usize) -> Result<usize> {
    let bytes = size
        .iter()
        .try_fold(1usize, |total, extent| total.checked_mul(*extent))
        .and_then(|elements| elements.checked_mul(sample_bytes))
        .ok_or_else(|| Error::Parse("N5 block size overflows usize".to_string()))?;
    if bytes > MAX_DECODED_PLANE_BYTES {
        return Err(Error::OversizedAllocation {
            requested: bytes,
            cap: MAX_DECODED_PLANE_BYTES,
        });
    }
    Ok(bytes)
}

/// The payload inflated to exactly `expected` bytes: a stream of any other length is refused, so a decompression
/// bomb never fills memory (only `expected + 1` bytes are read).
fn inflate(payload: &[u8], compression: Compression, expected: usize) -> Result<Vec<u8>> {
    match compression {
        Compression::None if payload.len() == expected => Ok(payload.to_vec()),
        Compression::None => Err(length_error(compression, payload.len(), expected)),
        Compression::Gzip => bounded(GzDecoder::new(payload), compression, expected),
        Compression::Zstd => {
            let decoder = StreamingDecoder::new(payload)
                .map_err(|e| Error::Decode(format!("N5 zstd block: {e}")))?;
            bounded(decoder, compression, expected)
        }
    }
}

fn bounded(mut source: impl Read, compression: Compression, expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    // `take` wraps the decoder, so the bound is on the inflated bytes, not on the stored ones.
    let read = source
        .by_ref()
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| Error::Decode(format!("N5 {} block is broken: {e}", compression.name())))?;
    if read == expected {
        return Ok(out);
    }
    Err(length_error(compression, read, expected))
}

fn length_error(compression: Compression, read: usize, expected: usize) -> Error {
    Error::Decode(format!(
        "N5 {} block inflates to {read} byte(s), not the {expected} its header says",
        compression.name()
    ))
}

/// The big-endian `width`-byte integer at `at`, or a refusal when the header stops before it.
fn word(bytes: &[u8], at: usize, width: usize) -> Result<u64> {
    let raw = bytes
        .get(at..at + width)
        .ok_or_else(|| Error::Parse(format!("N5 block header stops before byte {}", at + width)))?;
    Ok(raw
        .iter()
        .fold(0u64, |value, byte| (value << 8) | *byte as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header: mode 0, `ndim` 2, then the two big-endian extents.
    fn header_bytes(size: &[u32]) -> Vec<u8> {
        let mut out: Vec<u8> = [0u16, 2].iter().flat_map(|v| v.to_be_bytes()).collect();
        out.extend(size.iter().flat_map(|v| v.to_be_bytes()));
        out
    }

    fn block_header(size: &[u32]) -> BlockHeader {
        header(&header_bytes(size), &[3, 2], &[7, 5]).unwrap()
    }

    #[test]
    fn a_header_reads_its_extents_big_endian() {
        // Byte order: read little-endian, the same bytes would be 50331648 and 33554432.
        assert_eq!(block_header(&[3, 2]).size, vec![3, 2]);
        assert_eq!(block_header(&[3, 2]).len, 12);
        assert_eq!(
            block_header(&[3, 1]).size,
            vec![3, 1],
            "an edge block may be smaller"
        );
    }

    #[test]
    fn a_block_is_refused_for_its_mode_its_rank_its_extents_and_a_short_header() {
        for (bytes, what) in [
            (vec![0, 1, 0, 2, 0, 0, 0, 3, 0, 0, 0, 2], "variable length"),
            (vec![0, 2, 0, 2, 0, 0, 0, 3, 0, 0, 0, 2], "an object"),
            (vec![0, 0, 0, 7, 0, 0, 0, 3, 0, 0, 0, 2], "dimension(s)"),
            (vec![0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2], "axis 0"),
            (vec![0, 0, 0, 2, 0, 0, 0, 4, 0, 0, 0, 2], "axis 0"),
            (vec![0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 6], "axis 1"),
            (vec![0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0], "axis 1"),
            (vec![0, 0, 0, 2, 0, 0, 0, 3], "stops before"),
            (vec![0, 0, 0], "stops before"),
            (vec![], "stops before"),
        ] {
            let error = header(&bytes, &[3, 2], &[7, 5]).expect_err(what);
            assert!(error.to_string().contains(what), "{what}: {error}");
        }
        let past = header(&header_bytes(&[3, 2]), &[3, 2], &[7, 1]).expect_err("past the edge");
        assert!(past.to_string().contains("axis 1"), "{past}");
        let huge = header(&header_bytes(&[u32::MAX, 2]), &[3, 2], &[7, 5]).expect_err("u32::MAX");
        assert!(huge.to_string().contains("4294967295"), "{huge}");
        let rank = header(&header_bytes(&[3, 2]), &[3, 2, 2], &[7, 5, 3])
            .expect_err("2-D header of a 3-D dataset");
        assert!(rank.to_string().contains("dimension(s)"), "{rank}");
    }

    /// The samples 1..=6 as big-endian u16, stored the three ways.
    fn stored(compression: Compression) -> Vec<u8> {
        let raw: Vec<u8> = [1u16, 2, 3, 4, 5, 6]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        match compression {
            Compression::None => raw,
            Compression::Gzip => {
                use flate2::write::GzEncoder;
                use std::io::Write;
                let mut out = Vec::new();
                GzEncoder::new(&mut out, flate2::Compression::default())
                    .write_all(&raw)
                    .unwrap();
                out
            }
            Compression::Zstd => ruzstd::encoding::compress_to_vec(
                &raw[..],
                ruzstd::encoding::CompressionLevel::Fastest,
            ),
        }
    }

    #[test]
    fn raw_gzip_and_zstd_blocks_decode_the_same_samples() {
        for compression in [Compression::None, Compression::Gzip, Compression::Zstd] {
            let decoded = decode(
                &block_header(&[3, 2]),
                &stored(compression),
                compression,
                DType::U16,
            )
            .unwrap();
            assert_eq!(decoded.shape(), [3, 2, 1]);
            assert_eq!(decoded.sample_bytes(), 2);
            assert_eq!(decoded.row_bytes(), 6);
            assert_eq!(decoded.row(0, 0).unwrap(), &[0, 1, 0, 2, 0, 3]);
            assert_eq!(decoded.row(0, 1).unwrap(), &[0, 4, 0, 5, 0, 6]);
            assert!(
                decoded.row(0, 2).is_err(),
                "a row past the block is not a row of it"
            );
            assert!(
                decoded.row(1, 0).is_err(),
                "a plane past the block is not a plane of it"
            );
        }
    }

    #[test]
    fn a_block_of_two_planes_holds_the_second_after_the_first() {
        // A 3-D block's samples run plane after plane: row (1, 0) is the first row of the second plane.
        let header = BlockHeader {
            size: vec![3, 2, 2],
            len: 16,
        };
        let raw: Vec<u8> = (1u16..=12).flat_map(|v| v.to_be_bytes()).collect();
        let decoded = decode(&header, &raw, Compression::None, DType::U16).unwrap();
        assert_eq!(decoded.shape(), [3, 2, 2]);
        assert_eq!(decoded.row(0, 0).unwrap(), &[0, 1, 0, 2, 0, 3]);
        assert_eq!(decoded.row(1, 0).unwrap(), &[0, 7, 0, 8, 0, 9]);
        assert!(
            decoded.row(2, 0).is_err(),
            "a plane past the block is not a plane of it"
        );
    }

    #[test]
    fn a_payload_of_another_length_is_refused() {
        // One byte too few and one too many (a bomb): neither is the block's own size.
        let header = block_header(&[3, 1]);
        let raw = [1, 0, 2, 0, 3, 0];
        let long = [1, 0, 2, 0, 3, 0, 4, 0];
        for (payload, name) in [(&raw[..5], "short"), (&long[..], "long")] {
            let error = decode(&header, payload, Compression::None, DType::U16)
                .err()
                .expect(name);
            assert!(
                matches!(error, Error::Decode(ref message) if message.contains("not the 6")),
                "{name}: {error}"
            );
        }
        for compression in [Compression::Gzip, Compression::Zstd] {
            let payload = stored(compression);
            let error = decode(
                &header,
                &payload[..payload.len() - 1],
                compression,
                DType::U16,
            )
            .err()
            .expect("cut");
            assert!(
                matches!(error, Error::Decode(_)),
                "{}: {error}",
                compression.name()
            );
        }
    }

    #[test]
    fn a_block_priced_past_the_ceiling_is_refused_before_anything_is_reserved() {
        // Every extent is legal against its blockSize; the 4 TB they promise is refused in process, before
        // the 32-byte payload is read.
        let header = BlockHeader {
            size: vec![2000, 2000, 1_000_000],
            len: 16,
        };
        let error = decode(&header, &[0u8; 32], Compression::Gzip, DType::U8)
            .err()
            .expect("over the ceiling");
        assert!(
            matches!(error, Error::OversizedAllocation { requested: 4_000_000_000_000, .. }),
            "{error}"
        );
    }
}
