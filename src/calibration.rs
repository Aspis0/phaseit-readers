// SPDX-License-Identifier: Apache-2.0
//! The calibration rules every reader shares: what a Z step may be, and how a
//! calibration the file stated but the reader could not use is described.
//!
//! One place, one rule. Every reader resolves the Z step it reads through
//! [`Calibration::sized`], so "the file records no Z step" means the same thing
//! in every format, and a step the file stated but that is not a distance a
//! microscope measured is described the same way everywhere.
//!
//! The rule has two ends, and both are distances:
//!
//! * a step below 1e-6 µm is a placeholder — the `1e-19 m` sentinel some
//!   writers put in the Z field is `1e-13 µm`, far below it;
//! * a step above 1e3 µm is a unit mistake — a file that states its step in
//!   micrometres where the reader expects metres (the CZI/LSM `×1e6`
//!   conversion) yields ~`5e5` µm, which no microscope measured.
//!
//! A step outside either end is not published; the note says which end it fell
//! off, because "the file's Z step is not a step" and "the file's Z step is a
//! placeholder" are different facts about the file.

use crate::types::PxSizeUm;

/// The smallest Z step, in micrometres, a microscope could have measured.
///
/// A step below this is not a distance between two focal planes: it is a
/// placeholder the file wrote in place of one. The `1e-19 m` sentinel some
/// writers put in the Z field is `1e-13 µm`, far below this.
const MIN_PLAUSIBLE_Z_STEP_UM: f32 = 1e-6;

/// The largest Z step, in micrometres, a microscope could have measured.
///
/// The largest Z step in the real corpus is 1 µm, and a step between two focal
/// planes is a distance of the order of the field of view — never a
/// millimetre. A step above this bound is a unit mistake (a file that states
/// micrometres where the reader expects metres is off by `1e6`), and
/// publishing it would make a downstream consumer compute distances that are
/// millions of times too large.
const MAX_PLAUSIBLE_Z_STEP_UM: f32 = 1e3;

/// What a reader made of a file's calibration: the pixel size it publishes,
/// and the notes on the calibration it could not use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calibration {
    /// The pixel size the file states, when it states one.
    pub px_size_um: Option<PxSizeUm>,
    /// Notes on the calibration the file stated but the reader could not use.
    ///
    /// Human-readable sentences: no file path, no pixel value. Empty when the
    /// reader used everything the file stated.
    pub notes: Vec<String>,
}

impl Calibration {
    /// A calibration with a pixel size of `x_um` by `y_um`, whose Z step is
    /// `stated_z_um` resolved by the shared rule.
    ///
    /// A stated Z step the rule drops is noted, so the scene says why it has
    /// no Z step.
    pub fn sized(x_um: f32, y_um: f32, stated_z_um: Option<f32>) -> Self {
        let z = resolve_z_step(stated_z_um);
        Self {
            px_size_um: Some(PxSizeUm {
                z: z.um,
                y: y_um,
                x: x_um,
            }),
            notes: z.note.into_iter().collect(),
        }
    }

    /// A calibration with no pixel size, and the notes that say why.
    pub fn uncalibrated(notes: Vec<String>) -> Self {
        Self {
            px_size_um: None,
            notes,
        }
    }
}

/// A Z step a reader read from a file, and what it did with it.
#[derive(Debug, Clone, PartialEq)]
struct ZStep {
    /// The step in micrometres, or `None` when the file records no step the
    /// reader could use.
    um: Option<f32>,
    /// The note saying why a stated step was not used, when one was dropped.
    note: Option<String>,
}

/// The note a stated Z step earns when the shared rule does not use it.
///
/// `stated_um` is the step the file stated, in micrometres. `None` when the
/// rule USES the step (there is nothing to explain) and when the file stated
/// no step at all (nothing to explain there either).
///
/// The wording lives here, once, so that a caller describing the same step the
/// same way — a wire, a report, a test — says the same thing as the reader.
pub fn z_step_note(stated_um: f32) -> Option<String> {
    match classify_step(stated_um) {
        Step::Placeholder => Some(format!(
            "the file's Z step ({} um) is a placeholder and was not used",
            short_exponential(stated_um)
        )),
        Step::TooLarge => Some(format!(
            "the file's Z step ({} um) is too large to be a distance between two \
             focal planes and was not used",
            short_exponential(stated_um)
        )),
        Step::Used | Step::NoStep => None,
    }
}

/// What the shared rule makes of a stated Z step.
enum Step {
    /// A distance a microscope measured: published as the step.
    Used,
    /// A placeholder the file wrote in place of a step.
    Placeholder,
    /// A number too large to be a distance between two focal planes.
    TooLarge,
    /// No step at all: the file stated nothing to resolve.
    NoStep,
}

/// Resolve the Z step a file states into the step to publish.
///
/// `stated_um` is the step the file states, in micrometres, or `None` when it
/// states none at all. A step that is not finite, not positive, below
/// [`MIN_PLAUSIBLE_Z_STEP_UM`] or above [`MAX_PLAUSIBLE_Z_STEP_UM`] is not a
/// distance a microscope measured, so it is published as `None` — the file
/// records no Z step — and never as a number the reader made up.
///
/// A step the file DID state but that is not a distance is noted: that is the
/// only case a note is written for. A file that states no step at all has
/// nothing to explain.
fn resolve_z_step(stated_um: Option<f32>) -> ZStep {
    let Some(value) = stated_um else {
        return ZStep { um: None, note: None };
    };
    if is_plausible_step(value) {
        return ZStep {
            um: Some(value),
            note: None,
        };
    }
    ZStep {
        um: None,
        note: z_step_note(value),
    }
}

/// Classify a stated Z step, in micrometres.
fn classify_step(um: f32) -> Step {
    if !um.is_finite() || um <= 0.0 {
        // A non-positive or non-finite value states no step at all: there is
        // nothing to explain.
        Step::NoStep
    } else if um < MIN_PLAUSIBLE_Z_STEP_UM {
        Step::Placeholder
    } else if um > MAX_PLAUSIBLE_Z_STEP_UM {
        Step::TooLarge
    } else {
        Step::Used
    }
}

/// Whether a Z step, in micrometres, is a distance a microscope measured.
fn is_plausible_step(um: f32) -> bool {
    matches!(classify_step(um), Step::Used)
}

/// A number as a short exponential: `1e-13`, `5e-7`, `2.5e-7`.
///
/// The note says the value is a placeholder, so the point is HOW SMALL it
/// is, not its last digit: a rounded form reads better and says the same.
fn short_exponential(value: f32) -> String {
    let text = format!("{value:.2e}");
    let Some((mantissa, exponent)) = text.split_once('e') else {
        return text;
    };
    let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
    let exponent = match exponent.strip_prefix('-') {
        Some(digits) => format!("-{}", digits.trim_start_matches('0')),
        None => exponent.to_string(),
    };
    format!("{mantissa}e{exponent}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The note a Z step the rule dropped as a placeholder earns — asked of
    /// the rule, never copied from it.
    fn placeholder_note(stated_um: f32) -> String {
        z_step_note(stated_um).expect("a placeholder earns a note")
    }

    #[test]
    fn a_step_of_zero_is_not_a_step() {
        let step = resolve_z_step(Some(0.0));
        assert_eq!(step.um, None, "zero is not a distance");
    }

    #[test]
    fn a_negative_step_is_not_a_step() {
        let step = resolve_z_step(Some(-0.5));
        assert_eq!(step.um, None, "a negative distance is not a distance");
    }

    #[test]
    fn a_nan_step_is_not_a_step() {
        let step = resolve_z_step(Some(f32::NAN));
        assert_eq!(step.um, None, "NaN is not a distance");
    }

    #[test]
    fn a_step_below_the_floor_is_not_a_step() {
        // 1e-7 um is a hundredth of the floor: no microscope measured it.
        let step = resolve_z_step(Some(1e-7));
        assert_eq!(step.um, None, "a step below the floor is not a step");
        assert_eq!(
            step.note.as_deref(),
            Some(placeholder_note(1e-7).as_str()),
            "a step the file stated and the rule dropped is named"
        );
    }

    #[test]
    fn a_step_at_the_floor_is_a_step() {
        let step = resolve_z_step(Some(MIN_PLAUSIBLE_Z_STEP_UM));
        assert_eq!(step.um, Some(1e-6), "the floor itself is a step");
        assert!(step.note.is_none(), "a step at the floor is used, not dropped");
    }

    #[test]
    fn a_step_above_the_ceiling_is_not_a_step() {
        // 5e5 um is what a file that states its step in micrometres where
        // the reader expects metres yields (the CZI/LSM x1e6 conversion): no
        // microscope measured that, so it is not published.
        let step = resolve_z_step(Some(5e5));
        assert_eq!(step.um, None, "a step above the ceiling is not a step");
        assert!(
            step.note.is_some(),
            "a step the file stated and the rule dropped is named"
        );
    }

    #[test]
    fn a_too_large_step_is_not_called_a_placeholder() {
        // "too large" and "a placeholder" are different facts about a file,
        // and the note must not mix them up.
        let note = z_step_note(5e5).expect("a too-large step earns a note");
        assert!(
            note.contains("too large"),
            "the note says the step is too large: {note}"
        );
        assert!(
            !note.contains("placeholder"),
            "the note must not call a too-large step a placeholder: {note}"
        );
    }

    #[test]
    fn a_step_at_the_ceiling_is_a_step() {
        let step = resolve_z_step(Some(MAX_PLAUSIBLE_Z_STEP_UM));
        assert_eq!(
            step.um,
            Some(MAX_PLAUSIBLE_Z_STEP_UM),
            "the ceiling itself is a step"
        );
        assert!(
            step.note.is_none(),
            "a step at the ceiling is used, not dropped"
        );
    }

    #[test]
    fn the_placeholder_wording_is_pinned_once() {
        // The wording is pinned HERE, in the crate that owns it; every other
        // test — and every other crate — asks the rule for it.
        assert_eq!(
            z_step_note(1e-13).as_deref(),
            Some("the file's Z step (1e-13 um) is a placeholder and was not used"),
            "the placeholder note says what the file stated"
        );
    }

    #[test]
    fn a_real_step_is_a_step() {
        let step = resolve_z_step(Some(0.5));
        assert_eq!(step.um, Some(0.5));
        assert!(step.note.is_none(), "a used step is not noted");
    }

    #[test]
    fn a_file_that_states_no_step_has_nothing_to_explain() {
        let step = resolve_z_step(None);
        assert_eq!(step.um, None);
        assert!(
            step.note.is_none(),
            "a file that states no step has nothing to explain"
        );
    }

    #[test]
    fn a_non_positive_step_states_no_step_rather_than_a_placeholder() {
        // Zero and a negative are a file stating no step, not a placeholder
        // standing in for one: there is nothing to explain.
        for value in [0.0f32, -1.0] {
            let step = resolve_z_step(Some(value));
            assert_eq!(step.um, None, "{value} is not a step");
            assert!(step.note.is_none(), "{value} states no step");
        }
    }

    #[test]
    fn a_calibration_carries_the_note_for_the_step_it_dropped() {
        let calibration = Calibration::sized(0.5, 0.25, Some(1e-13));
        let px = calibration.px_size_um.expect("the pixel size is published");
        assert_eq!(px.z, None, "the placeholder is not a step");
        assert_eq!(px.x, 0.5);
        assert_eq!(px.y, 0.25);
        assert_eq!(calibration.notes, vec![placeholder_note(1e-13)]);
    }

    #[test]
    fn a_calibration_with_nothing_dropped_carries_no_note() {
        let calibration = Calibration::sized(0.5, 0.25, Some(0.5));
        assert_eq!(calibration.notes, Vec::<String>::new());
        let calibration = Calibration::sized(0.5, 0.25, None);
        assert_eq!(calibration.notes, Vec::<String>::new());
    }
}
