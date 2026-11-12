// SPDX-License-Identifier: Apache-2.0
//! One buffer, one plane: a box the cap admits must cost the volume plus the single plane being copied, not a plane
//! per depth plus a second buffer of the volume's size. A counting allocator measures the peak, so the guarantee is
//! observed rather than asserted about the source.
//! This is its own test binary on purpose: another test allocating in parallel would count against the peak.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use phasit_imread::types::{Box3, PixelRect};
use phasit_imread::{
    DType, Dims5, Error, ImageMeta, ImageReader, Plane, PlaneBuf, Result, SceneMeta,
};

/// Allocations below this are the harness's and the metadata's; only the buffers of the box are measured.
const COUNTED: usize = 1024;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() && layout.size() >= COUNTED {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() >= COUNTED {
            LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        }
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// A reader whose planes are worth `WIDTH * HEIGHT` bytes each, tagged with the z they were read at.
struct Fake {
    meta: ImageMeta,
    reads: AtomicUsize,
}

impl Fake {
    fn new(z: usize) -> Fake {
        let scene = SceneMeta {
            dims: Dims5 {
                t: 1,
                c: 1,
                z,
                y: HEIGHT,
                x: WIDTH,
            },
            dtype: DType::U8,
            samples: 1,
            px_size_um: None,
            notes: Vec::new(),
            channels: Vec::new(),
            palette: None,
            name: "fake".to_string(),
        };
        Fake {
            meta: ImageMeta {
                name: "fake".to_string(),
                scenes: vec![scene],
            },
            reads: AtomicUsize::new(0),
        }
    }
}

const WIDTH: usize = 128;
const HEIGHT: usize = 64;
const DEPTH: usize = 4;

impl ImageReader for Fake {
    fn open(_path: &std::path::Path) -> Result<Self> {
        Err(Error::Unsupported(
            "the fake reader has no file".to_string(),
        ))
    }

    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn plane(&self, scene: usize, t: usize, c: usize, z: usize) -> Result<Plane> {
        self.region(scene, t, c, z, PixelRect::new(0, 0, WIDTH, HEIGHT))
    }

    fn region(
        &self,
        _scene: usize,
        _t: usize,
        _c: usize,
        z: usize,
        rect: PixelRect,
    ) -> Result<Plane> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Plane::new(
            PlaneBuf::U8(vec![z as u8; rect.w * rect.h]),
            [rect.h, rect.w],
        )
    }
}

#[test]
fn a_box_costs_the_volume_plus_one_plane() {
    let reader = Fake::new(DEPTH);
    let bx = Box3::new(0, 0, 0, WIDTH, HEIGHT, DEPTH);
    let volume_bytes = WIDTH * HEIGHT * DEPTH;

    LIVE.store(0, Ordering::SeqCst);
    PEAK.store(0, Ordering::SeqCst);
    let volume = reader.volume(0, 0, 0, bx).expect("read the box");
    let peak = PEAK.load(Ordering::SeqCst);
    drop(volume);

    assert_eq!(
        reader.reads.load(Ordering::SeqCst),
        DEPTH,
        "one read per plane"
    );
    // Stacking kept every plane alive beside the output buffer: four planes plus the volume, about twice the volume.
    // One plane beside the volume is what a plane-at-a-time copy may cost; the slack covers the allocator's rounding.
    let bound = volume_bytes + 2 * WIDTH * HEIGHT;
    assert!(
        peak < bound,
        "a box peaked at {peak} bytes, the plane-at-a-time copy must stay below {bound}"
    );
}
