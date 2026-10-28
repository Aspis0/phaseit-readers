// SPDX-License-Identifier: Apache-2.0
//! The ND2 reader opens a container and decodes its planes.
//!
//! The container is written by the shared writer in `tests/support/nd2.rs` —
//! the ONE ND2 writer of this crate: the reader's own unit tests used to
//! assemble a second copy of it.

mod support;

use phasit_imread::nd2_reader::Nd2Reader;
use phasit_imread::types::Dims5;
use phasit_imread::{DType, Error, ImageReader, PlaneBuf};

use support::nd2::{write_nd2, Nd2File, Nd2Frame};
use support::scratch::Scratch;

/// A scratch ND2 file of this test run.
fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!(
        "phasit_nd2_open_{name}_{}",
        std::process::id()
    )))
}

/// Write an ND2 and open it.
fn open(file: &Nd2File, name: &str) -> (Scratch, Nd2Reader) {
    let path = scratch(name);
    write_nd2(&path, file);
    let reader = Nd2Reader::open(&path).expect("open the synthetic ND2");
    (path, reader)
}

/// The pixel bytes of one U16 frame.
fn u16_frame(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[test]
fn a_synthetic_file_opens_and_decodes_its_planes() {
    // A 2x2, single-component, U16 file with two Z planes.
    let file = Nd2File {
        width: 2,
        height: 2,
        comp: 1,
        width_bytes: 4,
        itemsize: 2,
        z: 2,
        e_compression: Some(2),
        frames: vec![
            Nd2Frame {
                pixels: u16_frame(&[1, 2, 3, 4]),
            },
            Nd2Frame {
                pixels: u16_frame(&[5, 6, 7, 8]),
            },
        ],
    };
    let (_path, reader) = open(&file, "open");

    let meta = reader.meta();
    assert_eq!(meta.scene_count(), 1);
    let scene = meta.scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 1, z: 2, y: 2, x: 2 });
    assert_eq!(scene.dtype, DType::U16);
    assert_eq!(scene.samples, 1);

    let p0 = reader.plane(0, 0, 0, 0).unwrap();
    assert_eq!(p0.shape, [2, 2]);
    assert_eq!(p0.data, PlaneBuf::U16(vec![1, 2, 3, 4]));
    let p1 = reader.plane(0, 0, 0, 1).unwrap();
    assert_eq!(p1.data, PlaneBuf::U16(vec![5, 6, 7, 8]));

    // Out-of-range coordinates are clean errors, never panics.
    assert!(matches!(
        reader.plane(0, 0, 0, 2),
        Err(Error::OutOfRange { z: 2, .. })
    ));
    assert!(matches!(
        reader.plane(0, 0, 1, 0),
        Err(Error::OutOfRange { c: 1, .. })
    ));
    assert!(matches!(
        reader.plane(1, 0, 0, 0),
        Err(Error::SceneOutOfRange { scene: 1, .. })
    ));
}

#[test]
fn a_multicomponent_padded_frame_gathers_each_component() {
    // width=3, height=2, comp=2, U16 (bps=2): 12 content bytes/row + 4 pad
    // bytes = widthBytes 16. This forces the general gather path (comp != 1
    // and widthBytes != width*bps), honouring the per-row byte stride.
    let (width, height, comp, width_bytes) = (3usize, 2usize, 2usize, 16usize);
    // Per-row interleaved (X,C) samples: [x0c0, x0c1, x1c0, x1c1, x2c0, x2c1].
    let rows: [[u16; 6]; 2] = [[10, 11, 12, 13, 14, 15], [20, 21, 22, 23, 24, 25]];
    let mut block = Vec::new();
    for row in rows {
        for v in row {
            block.extend_from_slice(&v.to_le_bytes());
        }
        block.extend_from_slice(&[0xFF; 4]); // row padding, must be skipped
    }
    assert_eq!(block.len(), height * width_bytes);

    let file = Nd2File {
        width,
        height,
        comp,
        width_bytes,
        itemsize: 2,
        z: 1,
        e_compression: Some(2),
        frames: vec![Nd2Frame { pixels: block }],
    };
    let (_path, reader) = open(&file, "padded_gather");
    let scene = reader.meta().scene(0).unwrap();
    assert_eq!(scene.dims, Dims5 { t: 1, c: 2, z: 1, y: 2, x: 3 });

    // Component 0 gathers samples at (x*comp + 0); component 1 at (x*comp + 1).
    // Padding (0xFF) must never leak into either plane.
    let p0 = reader.plane(0, 0, 0, 0).unwrap();
    assert_eq!(p0.shape, [2, 3]);
    assert_eq!(p0.data, PlaneBuf::U16(vec![10, 12, 14, 20, 22, 24]));
    let p1 = reader.plane(0, 0, 1, 0).unwrap();
    assert_eq!(p1.shape, [2, 3]);
    assert_eq!(p1.data, PlaneBuf::U16(vec![11, 13, 15, 21, 23, 25]));
}
