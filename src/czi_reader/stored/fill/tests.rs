use super::*;

/// A part that misses the pixel on both axes adds nothing: clamping the product instead of
/// each extent would turn its two negative extents into a positive area.
#[test]
fn a_part_off_the_pixel_on_both_axes_adds_nothing_to_the_divisor() {
    // The pixel's source square is (40..42, 160..162); the first part sits left of it and
    // above it, the second covers it whole.
    let parts = [PixelRect::new(0, 0, 32, 32), PixelRect::new(40, 160, 2, 2)];
    assert_eq!(block_count(80, 20, &parts, 2), 4);
}

#[test]
fn a_part_off_the_pixel_on_one_axis_adds_nothing_to_the_divisor() {
    // Left of the pixel (40..42, 160..162), overlapping it in rows only.
    let parts = [PixelRect::new(0, 158, 32, 6)];
    assert_eq!(block_count(80, 20, &parts, 2), 0);
}
