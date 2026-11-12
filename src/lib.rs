// SPDX-License-Identifier: Apache-2.0
//! Rust reader for TIFF/OME-TIFF, LSM, LIF, CZI, ND2, MRC2014 and DeltaVision, offline and in-process. Contract:
//! [`types`], axis **TCZYX**.
//! [`detect`] sniffs the format, [`open`] dispatches, [`guard`] holds the guardrails; OIB and OIF have no reader yet.
//!
//! ```no_run
//! # fn main() -> Result<(), phasit_imread::Error> {
//! let path = std::path::Path::new("scan.tif");
//! let reader = phasit_imread::open(path)?;
//! let meta = reader.meta();
//! println!("{} scenes", meta.scene_count());
//! let plane = reader.plane(0, 0, 0, 0)?;
//! println!("{}x{} plane", plane.shape[0], plane.shape[1]);
//! println!("{} refused", reader.refused_scenes().len());
//! println!("scene 0 is image {}", reader.scene_file_index(0));
//! # Ok(())
//! # }
//! ```

pub mod calibration;
pub mod czi_reader;
pub mod detect;
pub mod dv_reader;
pub mod error;
pub mod guard;
pub(crate) mod io;
pub mod lif_reader;
pub mod lsm_reader;
pub mod mrc_reader;
pub mod nd2_reader;
pub mod tiff_calibration;
pub mod tiff_reader;
pub mod types;

mod lsm_bytes;
mod lsm_channel_colors;
mod lsm_cz_info;
mod lsm_positions;
mod lsm_region;
mod rows;
mod tiff_imagej;
mod tiff_layout;
mod tiff_ome;
mod tiff_ome_geometry;
mod tiff_ome_planes;
mod tiff_ome_scene;
mod tiff_page;
mod tiff_pages;
mod tiff_palette;
mod tiff_plain;
mod tiff_region;
mod tiff_tags;
mod xml_entities;

// The real-file gate is shared with the integration tests; `cfg(test)` keeps it out of the public API.
#[cfg(test)]
#[path = "../tests/real_data/mod.rs"]
mod real_data;

/// CZI JPEG-XR (compression id 4) via vendored jxrlib; needs a C toolchain, so it is opt-in.
#[cfg(feature = "jpegxr")]
mod jpegxr_ffi;

pub use crate::detect::{detect_format, detect_from, open, Format};
pub use crate::error::{Error, Result};
pub use crate::guard::{checked_plane_alloc, MAX_DECODED_PLANE_BYTES, MAX_MMAP_BYTES, MAX_SCENE_DIM};
pub use crate::types::{
    ChannelMeta, DType, Dims5, Endian, ImageMeta, ImageReader, Plane, PlaneBuf, PxSizeUm,
    RefusedLevel, RefusedScene, SceneMeta, StoredLevel, StoredPyramid,
};

#[cfg(feature = "ndarray")]
pub use crate::types::PlaneArray;
