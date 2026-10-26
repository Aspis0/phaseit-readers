// SPDX-License-Identifier: Apache-2.0
//! Allocation guardrails.
//!
//! Microscopy files can declare pathological or corrupt dimensions. Every
//! reader must size a plane BEFORE allocating, so a bogus header is rejected
//! with an error instead of OOM-killing the process. Overflow is handled with
//! `checked_mul`, never wrapping.

use crate::error::Result;
use crate::types::{DType, Dims5};

/// Maximum bytes for a single decoded 2-D plane (4 GiB).
///
/// Comfortably above any real camera frame (a 64k×64k 16-bit plane is 8 GiB and
/// already unrealistic), while still catching corrupt/absurd dimensions early.
pub const MAX_DECODED_PLANE_BYTES: usize = 4 * 1024 * 1024 * 1024;

/// Maximum bytes a reader is allowed to memory-map for one file (64 GiB).
///
/// Bounds whole-file mmaps so a truncated/oversized declared size cannot map an
/// unreasonable region. Enforced by readers when they set up their mmap.
pub const MAX_MMAP_BYTES: usize = 64 * 1024 * 1024 * 1024;

/// Maximum pixel extent (per Y or X axis) a single scene may declare (1,000,000).
///
/// Unlike [`MAX_DECODED_PLANE_BYTES`], this bounds a scene's DIMENSIONS rather
/// than a materialized plane's byte size, so it also protects code that scales
/// with the tile grid (`⌈y/chunk⌉·⌈x/chunk⌉`, e.g. a consumer that builds
/// pyramids) rather than the plane bytes — a grid that never allocates a whole
/// plane but whose per-tile bookkeeping would still explode for a bogus
/// dimension.
///
/// A whole-slide scene is deliberately allowed to OPEN past
/// [`MAX_DECODED_PLANE_BYTES`] (its full plane is streamed region-by-region), so
/// the plane-bytes cap can't catch a corrupt/crafted extent here. No real
/// whole-slide scanner produces a single-axis extent anywhere near 1,000,000 px
/// (the largest pathology/Axioscan mosaics are ~200k px/edge); a larger value is
/// corrupt or adversarial (e.g. a crafted CZI subblock with `X.Start` ~2e9). At
/// this bound the level-0 tile grid stays ≤ ~1954² cells, so the writer's per-
/// tile work is bounded instead of requesting a multi-TB allocation and aborting
/// the process via `handle_alloc_error` (uncatchable, bypasses `Result`).
pub const MAX_SCENE_DIM: usize = 1_000_000;

/// Compute one plane's byte size (`x * y * dtype.size_bytes()`), refusing it if
/// it overflows `usize` or exceeds [`MAX_DECODED_PLANE_BYTES`].
///
/// Call this BEFORE reserving any memory. Returns the byte size on success.
///
/// Thin wrapper over [`Dims5::checked_plane_bytes`], which is the single source
/// of truth for the sizing math.
pub fn checked_plane_alloc(dims: &Dims5, dtype: DType) -> Result<usize> {
    dims.checked_plane_bytes(dtype)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn normal_plane_passes() {
        let dims = Dims5 { t: 1, c: 1, z: 1, y: 2048, x: 2048 };
        let bytes = checked_plane_alloc(&dims, DType::U16).unwrap();
        assert_eq!(bytes, 2048 * 2048 * 2);
        assert!(bytes <= MAX_DECODED_PLANE_BYTES);
    }

    #[test]
    fn oversized_plane_refused() {
        // 100k x 100k x 4 bytes = 40 GB, well past the 4 GiB cap.
        let dims = Dims5 { t: 1, c: 1, z: 1, y: 100_000, x: 100_000 };
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
        // x * y overflows usize; must be a clean error, not a panic.
        let dims = Dims5 { t: 1, c: 1, z: 1, y: usize::MAX, x: 2 };
        match checked_plane_alloc(&dims, DType::U8) {
            Err(Error::OversizedAllocation { requested, cap }) => {
                assert_eq!(requested, usize::MAX);
                assert_eq!(cap, MAX_DECODED_PLANE_BYTES);
            }
            other => panic!("expected OversizedAllocation on overflow, got {other:?}"),
        }
    }
}
