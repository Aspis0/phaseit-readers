// SPDX-License-Identifier: Apache-2.0

use crate::error::Result;
use crate::types::{DType, Dims5};

/// Largest decoded 2-D plane (4 GiB): above any real camera frame, far below a corrupt header's claim.
pub const MAX_DECODED_PLANE_BYTES: usize = 4 * 1024 * 1024 * 1024;

/// Largest file a reader may memory-map (64 GiB); bounds the map a corrupt size can ask for.
pub const MAX_MMAP_BYTES: usize = 64 * 1024 * 1024 * 1024;

/// Largest pixel extent per axis a scene may declare. Whole-slide scenes stream past
/// [`MAX_DECODED_PLANE_BYTES`]; no scanner exceeds ~200k px per edge, so this bound catches crafted extents.
pub const MAX_SCENE_DIM: usize = 1_000_000;

/// Largest `attributes.json` an N5 reader reads (1 MiB): detection probes exactly this far, so a larger file
/// is refused with the same sentence by both, never mistaken for another format.
pub const MAX_ATTRIBUTES_BYTES: u64 = 1024 * 1024;

/// Largest MetaMorph `.nd` file a reader reads (1 MiB): the file lists an acquisition, not pixels, so a real one
/// is a few kilobytes; a larger file is refused before it is read.
pub const MAX_ND_FILE_BYTES: u64 = 1024 * 1024;

/// Largest number of planes a MetaMorph `.nd` dataset may declare (stage positions x wavelengths x time points x
/// z steps): no real acquisition holds a million of them, and the product is what a reader has to open.
pub const MAX_ND_PLANES: usize = 1_000_000;

/// Byte size of one plane, refused on overflow or above [`MAX_DECODED_PLANE_BYTES`]; call it BEFORE allocating.
pub fn checked_plane_alloc(dims: &Dims5, dtype: DType) -> Result<usize> {
    dims.checked_plane_bytes(dtype)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn normal_plane_passes() {
        let dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: 2048,
            x: 2048,
        };
        let bytes = checked_plane_alloc(&dims, DType::U16).unwrap();
        assert_eq!(bytes, 2048 * 2048 * 2);
        assert!(bytes <= MAX_DECODED_PLANE_BYTES);
    }

    #[test]
    fn oversized_plane_refused() {
        let dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: 100_000,
            x: 100_000,
        };
        match checked_plane_alloc(&dims, DType::F32) {
            Err(Error::OversizedAllocation { requested, cap }) => {
                assert_eq!(cap, MAX_DECODED_PLANE_BYTES);
                assert!(requested > cap);
            }
            other => panic!("expected OversizedAllocation, got {other:?}"),
        }
    }

    #[test]
    fn overflow_does_not_panic() {
        let dims = Dims5 {
            t: 1,
            c: 1,
            z: 1,
            y: usize::MAX,
            x: 2,
        };
        match checked_plane_alloc(&dims, DType::U8) {
            Err(Error::OversizedAllocation { requested, cap }) => {
                assert_eq!(requested, usize::MAX);
                assert_eq!(cap, MAX_DECODED_PLANE_BYTES);
            }
            other => panic!("expected OversizedAllocation on overflow, got {other:?}"),
        }
    }
}
