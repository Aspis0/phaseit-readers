// SPDX-License-Identifier: Apache-2.0
//! A plain TIFF whose ImageJ description states two frames: the plane at t=1
//! is the second page. No file of the parity corpus has T > 1, so this is the
//! only check of the time term of the plain-TIFF page map.

mod support;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::{ImageReader, PlaneBuf};
use support::{scratch_dir, write_grey, GreyPage};

#[test]
fn an_imagej_stack_with_two_frames_reads_each_frame_from_its_own_page() {
    let dir = scratch_dir("imagej_frames");
    let path = dir.join("frames.tif");
    let description = "ImageJ=1.11a\nimages=2\nslices=1\nframes=2\n";
    let pages = [
        GreyPage {
            width: 2,
            height: 2,
            value: 40,
        },
        GreyPage {
            width: 2,
            height: 2,
            value: 41,
        },
    ];
    write_grey(&path, Some(description), &pages);

    let reader = TiffReader::open(&path).expect("open the ImageJ stack");
    assert_eq!(reader.meta().scene(0).expect("one scene").dims.t, 2);
    let first_value = |t: usize| match reader.plane(0, t, 0, 0).expect("read the plane").data {
        PlaneBuf::U8(values) => values[0],
        _ => panic!("the fixture is 8-bit"),
    };
    assert_eq!(first_value(0), 40);
    assert_eq!(first_value(1), 41);
}
