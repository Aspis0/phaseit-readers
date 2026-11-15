//! The sentences a refused level carries: an overlap, and a level whose overlap was not checked.

use super::*;

/// The reason a level is refused for its tiles.
fn reason(overlap: Overlap) -> String {
    refuse_overlap(overlap)
        .expect("the verdict refuses the level")
        .reason
}

#[test]
fn two_tiles_of_a_plane_at_the_same_place_are_an_overlap() {
    assert_eq!(
        reason(Overlap::Overlapping),
        "the tiles of the level overlap"
    );
}

/// A level whose overlap was not checked is not called an overlap: the sentence says what happened.
#[test]
fn a_level_whose_overlap_was_not_checked_says_so_and_names_the_count() {
    let refusal = reason(Overlap::TooManyTiles {
        tiles: 129,
        comparisons: 8_256,
    });
    assert_eq!(
        refusal,
        "a cell of the level holds 129 tiles; comparing them would cost 8256 comparisons, more than \
         the 8128 one cell may cost, so the overlap of the level is not checked"
    );
    assert!(
        !refusal.contains("the tiles of the level overlap"),
        "a level that was not checked is not called an overlap: {refusal}"
    );
}

/// A level whose tiles do not overlap is not refused.
#[test]
fn a_level_whose_tiles_do_not_overlap_is_not_refused() {
    assert!(refuse_overlap(Overlap::Clear).is_none());
}
