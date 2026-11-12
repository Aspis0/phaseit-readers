// SPDX-License-Identifier: Apache-2.0
//! Every way a DeltaVision header is refused: the magic, the sample type, the extents, the counts, the extended
//! header and a data window that runs past the end of the file. The files are written by `tests/support/dv.rs` and
//! removed by `Scratch`, so a failing assertion leaves nothing behind.

mod support;

use phasit_imread::dv_reader::DvReader;
use phasit_imread::{Error, ImageReader};

use support::dv::{sample_at, DvFile};
use support::scratch::Scratch;

/// The samples the builders write for section `section`, x fastest.
fn section_i16(nx: usize, ny: usize, section: usize) -> Vec<i16> {
    (0..ny)
        .flat_map(|y| (0..nx).map(move |x| sample_at(section, y, x)))
        .collect()
}

fn scratch(name: &str) -> Scratch {
    Scratch::new(std::env::temp_dir().join(format!("phasit_dv_{name}_{}.dv", std::process::id())))
}

/// The sentence a file is refused with, whatever the kind: an unknown feature is `Unsupported`, a header that
/// disagrees with itself is `Parse`.
fn refusal_text(file: &DvFile, name: &str) -> String {
    let path = scratch(name);
    file.write(&path);
    match DvReader::open(&path) {
        Err(Error::Unsupported(message)) | Err(Error::Parse(message)) => message,
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the file opened"),
    }
}

/// The refusal sentence, or `None` when the file opens: this grid needs both outcomes.
fn sentence(file: &DvFile, name: &str) -> Option<String> {
    let path = scratch(name);
    file.write(&path);
    match DvReader::open(&path) {
        Err(Error::Unsupported(message)) | Err(Error::Parse(message)) => Some(message),
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => None,
    }
}

/// The two counts are i16 and attacker-controlled, so this grid walks every sign and magnitude either field can hold,
/// through `DvReader::open`: a negative count is a refusal, never a panic. `0` means one, so the non-negative rows
/// stay readable and must not earn the negative-count sentence.
#[test]
fn a_count_that_cannot_be_a_count_is_refused_without_a_panic() {
    const VALUES: [i16; 8] = [i16::MIN, -32_767, -5, -2, -1, 0, 1, 32_767];
    let mut negative = 0;
    let mut usable = 0;
    for num_times in VALUES {
        for num_waves in VALUES {
            for nz in [2usize, 10, 100] {
                let file = DvFile {
                    num_times,
                    num_waves,
                    ..DvFile::i16(4, 3, nz, 1, 1, 0)
                };
                let message = sentence(&file, "counts");
                if num_times < 0 || num_waves < 0 {
                    negative += 1;
                    let message = message
                        .unwrap_or_else(|| panic!("T={num_times} W={num_waves} NZ={nz} opened"));
                    // `geometry` reads NumWaves before NumTimes, so that is the field the sentence names.
                    let named = if num_waves < 0 { num_waves } else { num_times };
                    assert!(
                        message.contains("cannot be negative"),
                        "T={num_times} W={num_waves} NZ={nz}: {message}"
                    );
                    // The sentence names the header's own signed value, never 2^64 - k.
                    assert!(message.contains(&named.to_string()), "{message}");
                    assert!(!message.contains("1844674407370955"), "{message}");
                    continue;
                }
                usable += 1;
                assert!(!message.unwrap_or_default().contains("cannot be negative"));
            }
        }
    }
    // 5 of the 8 values are negative, so 55 of the 64 pairs are, and 9 are not, times 3 extents.
    assert_eq!((negative, usable), (165, 27));
}

#[test]
fn the_same_counts_are_refused_through_the_dispatcher() {
    for (num_times, num_waves) in [(i16::MIN, 5), (-1, 2), (-32_767, 5), (-2, 1)] {
        let file = DvFile {
            num_times,
            num_waves,
            ..DvFile::i16(4, 3, 10, 1, 1, 0)
        };
        let path = scratch("dispatch_counts");
        file.write(&path);
        match phasit_imread::open(&path) {
            Err(Error::Unsupported(message)) => {
                assert!(message.contains("cannot be negative"), "{message}")
            }
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("T={num_times} W={num_waves} opened"),
        }
    }
}

#[test]
fn a_file_without_the_ndvid_magic_is_refused() {
    let message = refusal_text(
        &DvFile {
            n_dvid: 0,
            ..DvFile::i16(4, 3, 1, 1, 1, 0)
        },
        "non_dvid",
    );
    assert!(message.contains("nDVID"), "{message}");
    assert!(message.contains("not a DeltaVision file"), "{message}");
}

#[test]
fn the_modes_without_a_dtype_are_refused_by_name() {
    for mode in [3, 4, 5, 7] {
        let message = refusal_text(
            &DvFile {
                mode,
                ..DvFile::i16(4, 3, 1, 1, 1, 0)
            },
            &format!("mode{mode}"),
        );
        assert!(message.contains(&format!("MODE {mode}")), "{message}");
    }
}

#[test]
fn non_positive_extents_are_refused() {
    for axis in 0..3 {
        let mut file = DvFile::i16(4, 3, 1, 1, 1, 0);
        match axis {
            0 => file.nx = 0,
            1 => file.ny = 0,
            _ => file.nz = 0,
        }
        let message = refusal_text(&file, &format!("extent{axis}"));
        assert!(message.contains("must be positive"), "{message}");
    }
}

#[test]
fn a_negative_next_is_refused() {
    let file = DvFile {
        next_word: Some(-1),
        ..DvFile::i16(4, 3, 1, 1, 1, 0)
    };
    let message = refusal_text(&file, "negative_next");
    assert!(message.contains("next"), "{message}");
}

#[test]
fn num_waves_above_five_is_refused() {
    let mut file = DvFile::i16(4, 3, 1, 1, 1, 0);
    file.num_waves = 6;
    let message = refusal_text(&file, "six_waves");
    assert!(message.contains("NumWaves is 6"), "{message}");
    assert!(message.contains("five wavelength"), "{message}");
}

#[test]
fn an_indivisible_nz_is_refused() {
    let file = DvFile::i16(4, 3, 5, 2, 1, 0);
    let message = refusal_text(&file, "indivisible_nz");
    assert!(message.contains("not divisible"), "{message}");
}

#[test]
fn an_unknown_img_sequence_is_refused() {
    for seq in [-1, 3, 4] {
        let mut file = DvFile::i16(4, 3, 1, 1, 1, 0);
        file.img_sequence = seq;
        let message = refusal_text(&file, &format!("seq{seq}"));
        assert!(message.contains(&format!("ImgSequence {seq}")), "{message}");
    }
}

#[test]
fn data_past_the_end_of_the_file_is_refused() {
    let file = DvFile::i16(4, 3, 2, 1, 1, 0);
    let path = scratch("truncated");
    let mut bytes = file.bytes();
    bytes.truncate(bytes.len() - 1);
    std::fs::write(&*path, &bytes).expect("write the truncated DV");
    match DvReader::open(&path) {
        Err(Error::Parse(message)) => assert!(message.contains("past the end of the"), "{message}"),
        Err(other) => panic!("expected Parse, got {other:?}"),
        Ok(_) => panic!("expected Parse, got a reader"),
    }
}

#[test]
fn an_extended_header_larger_than_next_is_refused() {
    // The per-section words live inside the `next` bytes: 2 sections of 3 integer and 4 float words need
    // 2 * (3 + 4) * 4 = 56 bytes, so a `next` below that describes a header that disagrees with itself.
    let file = DvFile {
        next: 56,
        num_integers: 3,
        num_floats: 4,
        ..DvFile::i16(4, 3, 2, 1, 1, 0)
    };
    let values = dispatcher_plane(&file, "fits");
    assert_eq!(values, section_i16(4, 3, 0));

    let short = DvFile {
        next: 55,
        num_integers: 3,
        num_floats: 4,
        ..DvFile::i16(4, 3, 2, 1, 1, 0)
    };
    let message = refusal_text(&short, "too_small");
    assert!(message.contains("NEXT is 55"), "{message}");
    assert!(message.contains("56 bytes"), "{message}");

    for (name, integers, floats) in [("neg_int", -1, 0), ("neg_float", 0, -1)] {
        let file = DvFile {
            num_integers: integers,
            num_floats: floats,
            ..DvFile::i16(4, 3, 1, 1, 1, 0)
        };
        let message = refusal_text(&file, name);
        assert!(message.contains("cannot be negative"), "{message}");
        assert!(!message.contains("1844674407370955"), "{message}");
    }
}

/// One plane read through the dispatcher, so the dispatcher's own open path is exercised too.
fn dispatcher_plane(file: &DvFile, name: &str) -> Vec<i16> {
    let path = scratch(name);
    file.write(&path);
    let reader = phasit_imread::open(&path).expect("open through the dispatcher");
    match reader.plane(0, 0, 0, 0).expect("read the plane").data {
        phasit_imread::PlaneBuf::I16(values) => values,
        other => panic!("expected I16, got {other:?}"),
    }
}
