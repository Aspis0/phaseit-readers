use crate::error::Result;

use super::meta::{ImageMeta, RefusedScene, SceneMeta};
use super::plane::Plane;
use super::region::{checked_region, PixelRect};
use super::stored::StoredPyramid;

/// The reader contract; `plane` takes `&self`, so a backend with a `&mut self` decoder rebuilds it per call.
pub trait ImageReader: Send + Sync {
    // TODO(perf): defaulted bulk `planes(&self, scene, t, z, channels)`: no per-channel RGB re-decode; non-breaking.
    fn open(path: &std::path::Path) -> Result<Self>
    where
        Self: Sized;

    fn meta(&self) -> &ImageMeta;

    fn scene_meta(&self, scene: usize) -> Result<&SceneMeta> {
        self.meta().scene(scene)
    }

    /// Refused `<Image>`s in file order, with reasons; the empty default means never refusing, not a clean file.
    fn refused_scenes(&self) -> &[RefusedScene] {
        &[]
    }

    fn scene_file_index(&self, scene: usize) -> usize {
        scene
    }

    /// Decodes a native-endian [`Plane`]; implementors MUST validate the coordinate (`OutOfRange`, never a panic).
    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane>;

    fn supports_region(&self) -> bool {
        false
    }

    /// Decodes sub-region `rect`. The default decodes the whole plane and crops: byte-exact, but not out-of-core.
    fn region(&self, scene: usize, t: usize, c: usize, z: usize, rect: PixelRect) -> Result<Plane> {
        let dims = self.scene_meta(scene)?.dims;
        checked_region("region", &dims, scene, t, c, z, rect)?;
        self.plane(scene, t, c, z)?.crop(rect)
    }

    /// The source's own stored pyramid for `scene`: the coarse levels the file carries itself.
    ///
    /// `None` for a source that stores none: the default keeps every reader on today's path.
    fn stored_pyramid(&self, _scene: usize) -> Option<&dyn StoredPyramid> {
        None
    }
}
