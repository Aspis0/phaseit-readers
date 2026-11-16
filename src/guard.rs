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

/// Largest number of wavelengths a MetaMorph `.nd` dataset may declare (1024): a wavelength costs one
/// `ChannelMeta` (24 bytes) in every scene, and the plane cap bounds the product of the axes.
pub const MAX_ND_WAVELENGTHS: usize = 1_024;

/// Largest number of stage positions, time points or z steps a MetaMorph `.nd` dataset may declare
/// (65 536): a stage costs one `SceneMeta` (160 bytes), one channel list and its own name.
pub const MAX_ND_AXIS_LEN: usize = 65_536;

/// Largest number of files a MetaMorph `.nd` dataset's directory may hold (100 000): the listing is read into
/// memory, and an acquisition folder holds thousands of files, not millions.
pub const MAX_ND_LISTING_ENTRIES: usize = 100_000;

/// Most MetaMorph `.nd` image files one dataset keeps open (256). The budget is per dataset, not per process: each open
/// file holds a handle the caller shares, so datasets open together add up. Measured on Windows: about 1 KB of heap and
/// one handle per open member.
pub(crate) const MAX_ND_OPEN_MEMBERS: usize = 256;

/// Byte size of one plane, refused on overflow or above [`MAX_DECODED_PLANE_BYTES`]; call it BEFORE allocating.
pub fn checked_plane_alloc(dims: &Dims5, dtype: DType) -> Result<usize> {
    dims.checked_plane_bytes(dtype)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::types::{ChannelMeta, SceneMeta};

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

    /// The `.nd` ceilings are the ones the reader can hold: a stage position and a wavelength cost
    /// the reader a scene and a channel each, and the plane cap bounds their product.
    #[test]
    fn the_nd_ceilings_are_where_the_reader_can_hold_them() {
        assert_eq!(MAX_ND_AXIS_LEN, 65_536, "the stage position ceiling");
        assert_eq!(MAX_ND_WAVELENGTHS, 1_024, "the wavelength ceiling");
    }

    /// The `.nd` ceilings are sized from what a stage position and a wavelength cost the reader, so
    /// the bytes the comments state are the bytes the types cost.
    #[test]
    fn the_nd_entry_costs_are_what_the_ceilings_assume() {
        // The costs are of a 64-bit build; a 32-bit one is smaller, and the ceilings still hold.
        if usize::BITS == 64 {
            assert_eq!(
                size_of::<SceneMeta>(),
                160,
                "the scene cost the .nd axis ceiling is sized from"
            );
            assert_eq!(
                size_of::<ChannelMeta>(),
                24,
                "the channel cost the .nd wavelength ceiling is sized from"
            );
        }
    }

    /// The scene table of a dataset at the ceilings: the scenes, their channel lists and their names.
    #[test]
    fn the_scene_table_at_the_ceilings_is_bounded() {
        // The plane cap bounds the product of the axes, so the channel lists of all the scenes cost
        // at most one channel per plane; the names are shared, so their text is paid once.
        let per_scene = size_of::<SceneMeta>() + size_of::<Vec<ChannelMeta>>() + size_of::<String>();
        let table = MAX_ND_AXIS_LEN * per_scene + MAX_ND_PLANES * size_of::<ChannelMeta>();
        assert!(
            table <= 40 * 1024 * 1024,
            "the scene table of a dataset at the ceilings holds {} MiB",
            table / (1024 * 1024)
        );
    }
}
