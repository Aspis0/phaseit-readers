// SPDX-License-Identifier: Apache-2.0
//! Shared data types: the FROZEN public contract of phasit-imread (a breaking change here is a semver break).
//! Axis order TCZYX (as in OME and tifffile); buffers are owned, native-endian, byteswapped by `PlaneBuf::from_bytes`.

mod box3;
mod buffer;
mod dims;
mod meta;
mod pixel;
mod plane;
mod reader;
mod region;
mod stored;
#[cfg(test)]
mod tests;
mod volume;

pub use self::box3::{bricks, Box3};
pub use self::buffer::PlaneBuf;
pub use self::dims::Dims5;
pub use self::meta::{ChannelMeta, ImageMeta, PxSizeUm, RefusedScene, SceneMeta};
pub use self::pixel::{DType, Endian};
pub use self::plane::Plane;
#[cfg(feature = "ndarray")]
pub use self::plane::PlaneArray;
pub use self::reader::ImageReader;
pub use self::region::PixelRect;
pub(crate) use self::region::{checked_region, empty_region_plane};
pub use self::stored::{RefusedLevel, StoredLevel, StoredPyramid, StoredRegion};
pub use self::volume::{Volume, MAX_VOLUME_BYTES};
