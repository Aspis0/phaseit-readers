// SPDX-License-Identifier: Apache-2.0
//! A count a file states as zero: the reader reads one plane, and the scene says so instead of reading it silently.

mod support;

use phasit_imread::tiff_reader::TiffReader;
use phasit_imread::ImageReader;
use support::{scratch_dir, write_grey, GreyPage};

/// The OME-XML of one scene whose axis counts are these.
fn ome_xml(size_z: &str, size_c: &str, size_t: &str) -> String {
    format!(
        "<OME xmlns=\"http://www.openmicroscopy.org/Schemas/OME/2016-06\">\
         <Image Name=\"scene\">\
         <Pixels DimensionOrder=\"XYCZT\" Type=\"uint8\" SizeX=\"2\" SizeY=\"2\" SizeZ=\"{size_z}\" \
         SizeC=\"{size_c}\" SizeT=\"{size_t}\">\
         <TiffData IFD=\"0\" PlaneCount=\"1\"/>\
         </Pixels>\
         </Image>\
         </OME>"
    )
}

/// A one-page grey TIFF whose first page carries this description.
///
/// The scratch folder is returned with it: it holds the file, and it is removed when it is dropped.
fn one_page_tiff(
    name: &str,
    description: &str,
) -> (support::scratch::ScratchDir, std::path::PathBuf) {
    let dir = scratch_dir(name);
    let path = dir.join("zero.tif");
    let pages = [GreyPage {
        width: 2,
        height: 2,
        value: 7,
    }];
    write_grey(&path, Some(description), &pages);
    (dir, path)
}

/// The notes the scene carries.
fn notes(path: &std::path::Path) -> Vec<String> {
    let reader = TiffReader::open(path).expect("open the file");
    reader
        .meta()
        .scene(0)
        .expect("one scene")
        .notes
        .clone()
}

/// A `SizeZ` of zero is read as one z plane, and the note names the field and the value the file stated.
#[test]
fn an_ome_size_z_of_zero_is_read_as_one_plane_and_says_so() {
    let (_dir, path) = one_page_tiff("ome_size_z_zero", &ome_xml("0", "1", "1"));
    let reader = TiffReader::open(&path).expect("open the OME-TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.dims.z, 1, "a count of zero is read as one plane");
    assert_eq!(scene.notes.len(), 1, "the count the file states is named");
    assert!(scene.notes[0].contains("SizeZ"), "{}", scene.notes[0]);
    assert!(scene.notes[0].contains("as 0"), "{}", scene.notes[0]);
}

/// A `SizeC` of zero is read as one channel, and says so.
#[test]
fn an_ome_size_c_of_zero_is_read_as_one_channel_and_says_so() {
    let (_dir, path) = one_page_tiff("ome_size_c_zero", &ome_xml("1", "0", "1"));
    let reader = TiffReader::open(&path).expect("open the OME-TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.dims.c, 1, "a count of zero is read as one channel");
    assert_eq!(scene.notes.len(), 1, "the count the file states is named");
    assert!(scene.notes[0].contains("SizeC"), "{}", scene.notes[0]);
}

/// A `SizeT` of zero is read as one time point, and says so.
#[test]
fn an_ome_size_t_of_zero_is_read_as_one_time_point_and_says_so() {
    let (_dir, path) = one_page_tiff("ome_size_t_zero", &ome_xml("1", "1", "0"));
    let reader = TiffReader::open(&path).expect("open the OME-TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.dims.t, 1, "a count of zero is read as one time point");
    assert_eq!(scene.notes.len(), 1, "the count the file states is named");
    assert!(scene.notes[0].contains("SizeT"), "{}", scene.notes[0]);
}

/// The counts a file states are read as they are, and no count of theirs is named.
#[test]
fn an_ome_with_real_counts_earns_no_note() {
    let (_dir, path) = one_page_tiff("ome_counts", &ome_xml("1", "1", "1"));
    assert!(
        notes(&path).is_empty(),
        "a count the reader uses earns no note"
    );
}

/// An ImageJ count of zero is read as one, and the note names the field and the value the file stated.
#[test]
fn an_imagej_count_of_zero_is_read_as_one_and_says_so() {
    let description = "ImageJ=1.54p\nimages=1\nchannels=0\nslices=1\n";
    let (_dir, path) = one_page_tiff("imagej_channels_zero", description);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.dims.c, 1, "a count of zero is read as one channel");
    assert_eq!(scene.notes.len(), 1, "the count the file states is named");
    assert!(scene.notes[0].contains("channels"), "{}", scene.notes[0]);
    assert!(scene.notes[0].contains("as 0"), "{}", scene.notes[0]);
}

/// The slices of an ImageJ stack are its z planes.
#[test]
fn an_imagej_slice_count_of_zero_is_read_as_one_z_plane_and_says_so() {
    let description = "ImageJ=1.54p\nimages=1\nchannels=1\nslices=0\n";
    let (_dir, path) = one_page_tiff("imagej_slices_zero", description);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.dims.z, 1, "a count of zero is read as one z plane");
    assert_eq!(scene.notes.len(), 1, "the count the file states is named");
    assert!(scene.notes[0].contains("slices"), "{}", scene.notes[0]);
}

/// The frames of an ImageJ stack are its time points.
#[test]
fn an_imagej_frame_count_of_zero_is_read_as_one_time_point_and_says_so() {
    let description = "ImageJ=1.54p\nimages=1\nchannels=1\nslices=1\nframes=0\n";
    let (_dir, path) = one_page_tiff("imagej_frames_zero", description);
    let reader = TiffReader::open(&path).expect("open the TIFF");
    let scene = reader.meta().scene(0).expect("one scene");
    assert_eq!(scene.dims.t, 1, "a count of zero is read as one time point");
    assert_eq!(scene.notes.len(), 1, "the count the file states is named");
    assert!(scene.notes[0].contains("frames"), "{}", scene.notes[0]);
}

/// The counts an ImageJ description states are read as they are, and none of them is named.
#[test]
fn an_imagej_description_with_real_counts_earns_no_note() {
    let description = "ImageJ=1.54p\nimages=1\nchannels=1\nslices=1\n";
    let (_dir, path) = one_page_tiff("imagej_counts", description);
    assert!(
        notes(&path).is_empty(),
        "a count the reader uses earns no note"
    );
}
