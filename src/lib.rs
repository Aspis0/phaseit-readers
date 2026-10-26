// SPDX-License-Identifier: Apache-2.0
//! phasit-imread — native Rust reader for proprietary microscopy image formats
//! (TIFF/OME-TIFF, LSM, LIF, CZI, ND2, OIB/OIF).
//!
//! Offline, in-process, no external services. The public contract lives in
//! [`types`] (canonical axis order **TCZYX**); [`detect`] handles format
//! sniffing and the [`open`] dispatch; [`guard`] holds the allocation
//! guardrails every reader calls before touching memory.
//!
//! Concrete format backends are filled in per format:
//! TIFF/OME-TIFF, LSM, LIF, CZI and ND2 have readers; OIB and OIF are detected
//! but have none yet.
//!
//! ```no_run
//! # fn main() -> Result<(), phasit_imread::Error> {
//! let path = std::path::Path::new("scan.tif");
//! let reader = phasit_imread::open(path)?;  // dispatch by magic + extension
//! let meta = reader.meta();                 // the scenes of the whole file
//! println!("{} scenes", meta.scene_count());
//! let plane = reader.plane(0, 0, 0, 0)?;    // (scene, t, c, z)
//! println!("{}x{} plane", plane.shape[0], plane.shape[1]);
//! println!("{} refused", reader.refused_scenes().len());  // the scenes refused
//! println!("scene 0 is image {}", reader.scene_file_index(0));  // its file position
//! # Ok(())
//! # }
//! ```

pub mod detect;
pub mod error;
pub mod guard;
pub(crate) mod io;
pub mod types;

pub mod czi_reader;
pub mod lif_reader;
pub mod lsm_reader;
pub mod nd2_reader;
pub mod tiff_calibration;
pub mod tiff_reader;

mod tiff_imagej;
mod tiff_ome;
mod tiff_ome_geometry;
mod tiff_ome_planes;
mod tiff_ome_scene;
mod tiff_page;
mod tiff_palette;
mod tiff_plain;
mod tiff_tags;

/// CZI JPEG-XR (compression id 4) decode via vendored jxrlib. Opt-in: only
/// compiled with the `jpegxr` feature (needs a C toolchain to build jxrlib).
#[cfg(feature = "jpegxr")]
mod jpegxr_ffi;

pub use crate::detect::{detect_format, detect_from, open, Format};
pub use crate::error::{Error, Result};
pub use crate::guard::{checked_plane_alloc, MAX_DECODED_PLANE_BYTES, MAX_MMAP_BYTES, MAX_SCENE_DIM};
pub use crate::types::{
    ChannelMeta, DType, Dims5, Endian, ImageMeta, ImageReader, Plane, PlaneBuf, PxSizeUm,
    RefusedScene, SceneMeta,
};

#[cfg(feature = "ndarray")]
pub use crate::types::PlaneArray;
