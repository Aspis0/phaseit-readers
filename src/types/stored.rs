//! The coarse levels a source carries itself: the geometry of each, and the reads over them.

use crate::error::Result;

use super::plane::Plane;
use super::region::PixelRect;

/// One coarse level the source file stores itself.
#[derive(Debug, Clone)]
pub struct StoredLevel {
    /// The minification: one level pixel covers this many source pixels.
    pub factor: usize,
    /// The level's origin, in source pixels, relative to the source scene's origin.
    pub origin: [i64; 2],
    /// The grid's cell size, in source pixels.
    pub cell: [usize; 2],
    /// The level's size, in source pixels.
    pub extent: [usize; 2],
    /// The share of the scene's area the level's own tiles cover, for the plane they cover
    /// worst, in [0, 1].
    pub coverage_share: f32,
}

/// A stored level read: the plane, and where the read's first pixel sits.
#[derive(Debug)]
pub struct StoredRegion {
    /// The level's pixels over the rect, in whole level pixels of the level's lattice.
    pub plane: Plane,
    /// The plane's first pixel's corner, in the scene's source pixels: the read runs on the
    /// level's lattice, so it may start up to `factor - 1` source pixels before the rect.
    pub origin: [i64; 2],
}

/// A coarse level the reader refused, with the count and the reason behind it.
#[derive(Debug, Clone)]
pub struct RefusedLevel {
    /// The minification the level would have had, or 0 when its tiles' stored size names none.
    pub factor: usize,
    /// The tiles behind the refusal.
    pub skipped: usize,
    /// Why the level is not offered.
    pub reason: String,
}

/// The coarse levels a source carries itself, and the reads over them.
///
/// WHY: a file that stores its own pyramid can show its overview without its full-resolution pixels.
pub trait StoredPyramid: Send + Sync {
    /// The levels that are offered, finest first.
    fn levels(&self) -> &[StoredLevel];

    /// The levels that are not offered, with the reason each was refused.
    fn refused_levels(&self) -> &[RefusedLevel];

    /// Reads the level over the rect's intersection with the scene, in whole level pixels of the
    /// level's lattice: the plane may start up to `factor - 1` source pixels before the rect,
    /// which `origin` names. A pixel no tile covers holds the full resolution's box average.
    fn region(
        &self,
        factor: usize,
        t: usize,
        c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<StoredRegion>;
}
