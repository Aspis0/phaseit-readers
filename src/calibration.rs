// SPDX-License-Identifier: Apache-2.0
//! The Z-step rule shared by every reader: a stated step is published only if a microscope could have measured it.
//! Both ends are distances: below 1e-6 µm is a placeholder, above 1e3 µm a unit mistake (micrometres read as metres,
//! `×1e6`).
//! A dropped step is noted with the end it fell off: "no step" and "a placeholder" are different facts about a file.

use crate::types::PxSizeUm;

/// The smallest Z step (µm) a microscope could have measured; below it the file wrote a placeholder.
/// Some writers use the `1e-19 m` sentinel, which is `1e-13 µm`, far below this.
const MIN_PLAUSIBLE_Z_STEP_UM: f32 = 1e-6;

/// The largest Z step in the real corpus is 1 µm. Above this bound a step is a unit mistake (off by `1e6`);
/// publishing it would make downstream distances millions of times too large.
const MAX_PLAUSIBLE_Z_STEP_UM: f32 = 1e3;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calibration {
    pub px_size_um: Option<PxSizeUm>,
    /// Human-readable sentences with no file path and no pixel value.
    pub notes: Vec<String>,
}

impl Calibration {
    /// A dropped Z step is noted, so the scene says why it has none.
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

    pub fn uncalibrated(notes: Vec<String>) -> Self {
        Self {
            px_size_um: None,
            notes,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ZStep {
    um: Option<f32>,
    note: Option<String>,
}

/// `None` when the rule uses the step or the file states none. The wording lives here, once, so every caller
/// (wire, report, test) describes a step the same way as the reader.
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

enum Step {
    Used,
    Placeholder,
    TooLarge,
    NoStep,
}

/// A step that is not finite, not positive, or outside the plausible range is published as `None`, never invented.
/// It is noted only when the file stated a step; a file that states none has nothing to explain.
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

fn classify_step(um: f32) -> Step {
    if !um.is_finite() || um <= 0.0 {
        // Non-positive or non-finite means no step was stated: nothing to explain.
        Step::NoStep
    } else if um < MIN_PLAUSIBLE_Z_STEP_UM {
        Step::Placeholder
    } else if um > MAX_PLAUSIBLE_Z_STEP_UM {
        Step::TooLarge
    } else {
        Step::Used
    }
}

fn is_plausible_step(um: f32) -> bool {
    matches!(classify_step(um), Step::Used)
}

/// Rounded: a note cares how small a placeholder is, not its last digits.
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
        // 5e5 µm is the unit mistake of a micrometre file read as metres (the CZI/LSM ×1e6).
        let step = resolve_z_step(Some(5e5));
        assert_eq!(step.um, None, "a step above the ceiling is not a step");
        assert!(
            step.note.is_some(),
            "a step the file stated and the rule dropped is named"
        );
    }

    #[test]
    fn a_too_large_step_is_not_called_a_placeholder() {
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
        // The wording is pinned here, in the crate that owns it; other tests ask the rule for it.
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
