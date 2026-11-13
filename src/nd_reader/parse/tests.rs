// SPDX-License-Identifier: Apache-2.0
//! The `.nd` parser on the texts a MetaMorph acquisition writes.

use super::*;

/// The dataset a text states, with the axes the text gives.
fn dataset(text: &str) -> Dataset {
    parse(text, "base").expect("a dataset the text states")
}

#[test]
fn the_axes_come_from_the_do_keys() {
    let text = "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 3\n\
                \"DoStage\", TRUE\n\"NStagePositions\", 2\n\"DoTimelapse\", TRUE\n\
                \"NTimePoints\", 4\n\"DoZSeries\", TRUE\n\"NZSteps\", 5\n\"EndFile\"\n";
    let parsed = dataset(text);
    assert_eq!(parsed.waves.len(), 3);
    assert_eq!(parsed.stages.len(), 2);
    assert_eq!(parsed.times, 4);
    assert_eq!(parsed.z_steps, 5);
    assert!(parsed.do_wave && parsed.do_stage && parsed.do_time);
}

#[test]
fn a_missing_do_key_means_one_plane_on_that_axis() {
    let parsed = dataset("\"NDInfoFile\", Version 1.0\n\"NWavelengths\", 7\n\"EndFile\"\n");
    assert_eq!(
        parsed.waves.len(),
        1,
        "the count of an axis that is not stated is ignored"
    );
    assert_eq!(parsed.times, 1);
    assert_eq!(parsed.z_steps, 1);
    assert!(!parsed.do_wave);
}

#[test]
fn a_quoted_value_is_text_and_a_bare_one_is_read_by_its_kind() {
    let parsed = dataset(
        "\"NDInfoFile\", Version 1.0\n\"WaveName1\", \"DAPI\"\n\"WaveDoZ1\", FALSE\n\
         \"ZStepSize\", 0.5\n\"EndFile\"\n",
    );
    assert_eq!(parsed.waves[0].name, "DAPI");
    assert!(!parsed.waves[0].has_z, "the wavelength states no z-series");
    assert_eq!(parsed.z_step_um, Some(0.5));
}

#[test]
fn a_missing_wave_do_z_key_means_the_wavelength_has_a_z_series() {
    let parsed = dataset("\"NDInfoFile\", Version 1.0\n\"DoZSeries\", TRUE\n\"EndFile\"\n");
    assert!(parsed.waves[0].has_z);
}

#[test]
fn a_malformed_line_is_refused() {
    for text in [
        "NDInfoFile, Version 1.0\n\"EndFile\"\n",
        "\"NDInfoFile\n\"EndFile\"\n",
        "\"NDInfoFile\" Version 1.0\n\"EndFile\"\n",
        "\"NDInfoFile\", \"unterminated\n\"EndFile\"\n",
    ] {
        assert!(parse(text, "base").is_err(), "{text:?} is refused");
    }
}

#[test]
fn a_file_that_does_not_start_with_ndinfofile_or_end_with_endfile_is_refused() {
    let no_start = parse("\"EndFile\"\n", "base").expect_err("no NDInfoFile key");
    assert!(no_start.to_string().contains("NDInfoFile"), "{no_start}");
    let no_end = parse("\"NDInfoFile\", Version 1.0\n", "base").expect_err("no EndFile key");
    assert!(no_end.to_string().contains("EndFile"), "{no_end}");
}

#[test]
fn a_count_of_zero_is_refused() {
    let error = parse(
        "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 0\n\"EndFile\"\n",
        "base",
    )
    .expect_err("no wavelength at all");
    assert!(error.to_string().contains("NWavelengths"), "{error}");
}

#[test]
fn a_dataset_above_the_plane_cap_is_refused() {
    // Every count is within its own bound; the product of the three axes is not.
    let text = "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 32\n\
                \"DoStage\", TRUE\n\"NStagePositions\", 512\n\"DoTimelapse\", TRUE\n\
                \"NTimePoints\", 512\n\"EndFile\"\n";
    let error = parse(text, "base").expect_err("too many planes");
    assert!(error.to_string().contains("cap"), "{error}");
}

#[test]
fn a_count_above_its_own_bound_is_refused() {
    let text = format!(
        "\"NDInfoFile\", Version 1.0\n\"DoStage\", TRUE\n\"NStagePositions\", {}\n\"EndFile\"\n",
        MAX_ND_AXIS_LEN + 1
    );
    let error = parse(&text, "base").expect_err("too many stage positions");
    assert!(error.to_string().contains("NStagePositions"), "{error}");
}

#[test]
fn a_byte_order_mark_before_the_first_key_is_accepted() {
    // The detection tolerates the mark, so the parser accepts the same file.
    let parsed = dataset("\u{feff}\"NDInfoFile\", Version 1.0\n\"EndFile\"\n");
    assert_eq!(parsed.times, 1);
}

#[test]
fn a_key_spelled_in_another_case_is_read() {
    let text = "\"NDInfoFile\", Version 1.0\n\"dowave\", TRUE\n\"NWAVELENGTHS\", 2\n\
                \"WaveName1\", \"DAPI\"\n\"EndFile\"\n";
    let parsed = dataset(text);
    assert_eq!(parsed.waves.len(), 2, "the count is read whatever its case");
    assert!(parsed.do_wave, "the axis is read whatever its case");
    assert_eq!(parsed.waves[0].name, "DAPI");
}

#[test]
fn a_wavelength_count_above_its_own_cap_is_refused() {
    let text = "\"NDInfoFile\", Version 1.0\n\"DoWave\", TRUE\n\"NWavelengths\", 1000000\n\
                \"EndFile\"\n";
    let error = parse(text, "base").expect_err("a million wavelengths");
    let message = error.to_string();
    assert!(message.contains("NWavelengths"), "{message}");
    assert!(message.contains(&MAX_ND_WAVELENGTHS.to_string()), "{message}");
}
