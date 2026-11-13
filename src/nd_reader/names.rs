// SPDX-License-Identifier: Apache-2.0
//! The image file name of one (wavelength, stage position, time point), and the guard on the names that go into it.
//! The names are the `.nd` file's own stem, then `_w<i>`, `_s<j>` and `_t<k>` as the dataset has those axes, then
//! the extension: a wavelength with a z-series is a `.STK` file, a wavelength without one a one-plane `.TIF` file.

use crate::error::{Error, Result};

use super::parse::Dataset;

/// The name of the image file of one (wave, stage, time), all three 1-based as the dataset states them.
pub(super) fn file_name(dataset: &Dataset, wave: usize, stage: usize, time: usize) -> String {
    let mut name = String::with_capacity(dataset.base.len() + 24);
    name.push_str(&dataset.base);
    if dataset.do_wave {
        name.push_str("_w");
        name.push_str(&wave.to_string());
        if dataset.wave_in_file_name {
            name.push_str(&dataset.waves[wave - 1].name);
        }
    }
    if dataset.do_stage {
        name.push_str("_s");
        name.push_str(&stage.to_string());
    }
    if dataset.do_time {
        name.push_str("_t");
        name.push_str(&time.to_string());
    }
    name.push_str(if dataset.waves[wave - 1].has_z {
        ".STK"
    } else {
        ".TIF"
    });
    name
}

/// The names the dataset states are checked before any file name is built: a name that holds a path separator or a
/// drive prefix would name a file outside the dataset's own directory.
pub(super) fn guard_names(dataset: &Dataset) -> Result<()> {
    for wave in &dataset.waves {
        guard_name("wavelength", &wave.name)?;
    }
    for stage in &dataset.stages {
        guard_name("stage position", stage)?;
    }
    Ok(())
}

/// One name of the dataset, refused when it could leave the dataset's own directory.
fn guard_name(what: &str, name: &str) -> Result<()> {
    if name.contains('/') || name.contains('\\') || name.contains("..") || has_drive_prefix(name) {
        return Err(Error::Unsupported(format!(
            "MetaMorph .nd: the {what} name {name:?} holds a path separator, a \"..\" or a drive prefix, so no \
             image file of it can be named inside the dataset's own directory"
        )));
    }
    Ok(())
}

/// A name that opens with a drive letter and a colon, as `C:` does.
fn has_drive_prefix(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nd_reader::parse::parse;

    /// The dataset a text states.
    fn dataset(text: &str) -> Dataset {
        parse(text, "acq").expect("a dataset the text states")
    }

    #[test]
    fn a_single_plane_dataset_names_one_file() {
        let parsed = dataset("\"NDInfoFile\", Version 1.0\n\"EndFile\"\n");
        assert_eq!(file_name(&parsed, 1, 1, 1), "acq.TIF");
    }

    #[test]
    fn the_axes_the_dataset_states_name_the_file() {
        let parsed = dataset(
            "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 2\n\
             \"WaveName1\", \"DAPI\"\n\"WaveName2\", \"FITC\"\n\"WaveInFileName\", TRUE\n\
             \"DoStage\", TRUE\n\"NStagePositions\", 2\n\"DoTimelapse\", TRUE\n\"NTimePoints\", 3\n\"EndFile\"\n",
        );
        assert_eq!(file_name(&parsed, 1, 1, 1), "acq_w1DAPI_s1_t1.TIF");
        assert_eq!(file_name(&parsed, 2, 2, 3), "acq_w2FITC_s2_t3.TIF");
    }

    #[test]
    fn a_dataset_that_states_no_wave_in_file_name_keeps_the_index_alone() {
        let parsed = dataset(
            "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 2\n\
             \"WaveName1\", \"DAPI\"\n\"EndFile\"\n",
        );
        assert_eq!(file_name(&parsed, 1, 1, 1), "acq_w1.TIF");
        assert_eq!(file_name(&parsed, 2, 1, 1), "acq_w2.TIF");
    }

    #[test]
    fn a_wavelength_without_its_name_in_the_file_name_keeps_the_index() {
        let parsed = dataset(
            "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 2\n\
             \"WaveName1\", \"DAPI\"\n\"WaveInFileName\", FALSE\n\"EndFile\"\n",
        );
        assert_eq!(file_name(&parsed, 1, 1, 1), "acq_w1.TIF");
        assert_eq!(file_name(&parsed, 2, 1, 1), "acq_w2.TIF");
    }

    #[test]
    fn a_wavelength_with_a_z_series_is_a_stk_file() {
        let parsed = dataset(
            "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 2\n\
             \"DoZSeries\", TRUE\n\"NZSteps\", 4\n\"WaveDoZ2\", FALSE\n\"EndFile\"\n",
        );
        assert_eq!(file_name(&parsed, 1, 1, 1), "acq_w1.STK");
        assert_eq!(file_name(&parsed, 2, 1, 1), "acq_w2.TIF");
    }

    #[test]
    fn a_name_that_leaves_the_directory_is_refused() {
        for name in ["../other", "a/b", "a\\b", "C:", "C:\\pictures"] {
            let parsed = dataset(&format!(
                "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 1\n\
                 \"WaveName1\", \"{name}\"\n\"EndFile\"\n"
            ));
            let error = guard_names(&parsed).expect_err("a name outside the directory");
            assert!(
                error.to_string().contains("own directory"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn a_stage_name_that_leaves_the_directory_is_refused() {
        let parsed = dataset(
            "\"NDInfoFile\", Version 1.0\n\"DoStage\", TRUE\n\"NStagePositions\", 1\n\
             \"Stage1\", \"../other\"\n\"EndFile\"\n",
        );
        assert!(guard_names(&parsed).is_err());
    }

    #[test]
    fn a_name_of_ordinary_text_is_accepted() {
        let parsed = dataset(
            "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 1\n\
             \"WaveName1\", \"DAPI (405)\"\n\"EndFile\"\n",
        );
        assert!(guard_names(&parsed).is_ok());
    }
}
