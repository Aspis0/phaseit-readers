// SPDX-License-Identifier: Apache-2.0
//! The CZI fixture the app reads: a source that stores a pyramid of its own.
//!
//! WHY the fixture is committed: the app's own tests must open a source that stores a
//! pyramid, and a file that only a test can write is a file that can change under them.
//!
//! WHY no test writes it: a test that rewrites a committed file changes what the next run
//! reads, and the change is invisible until the bytes differ. The ignored test at the end
//! is the only writer.

mod support;

use std::path::PathBuf;

use support::czi::{level_tile, source_pixels, Tile};
use support::czi_write::write_czi;

/// The scene of the fixture: 2048 pixels wide and 4 high, so the default recipe makes a
/// ladder of x1, x2 and x4.
const WIDTH: u32 = 2048;
const HEIGHT: u32 = 4;

/// The bytes of the committed fixture.
const COMMITTED: &[u8] = include_bytes!("fixtures/czi/stored-pyramid.czi");

/// The file the fixture lives in.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("czi")
        .join("stored-pyramid.czi")
}

/// The tiles of the fixture: the full resolution and the two coarse levels it stores.
///
/// WHY the coarse levels are stored whole: a level that covers the scene whole is the only
/// one the reader offers, so the app sees a source it can open while it builds.
fn fixture_tiles() -> Vec<Tile> {
    let mut tiles = vec![Tile::full(0, 0, WIDTH, HEIGHT)
        .painted(source_pixels(0, 0, WIDTH, HEIGHT, 1))];
    tiles.push(level_tile(0, 2, 0, 0, WIDTH, HEIGHT, 0));
    tiles.push(level_tile(0, 4, 0, 0, WIDTH, HEIGHT, 0));
    tiles
}

/// The committed fixture is the one the writer makes, and the reader offers its levels.
#[test]
fn the_fixture_stores_the_two_coarse_levels() {
    // WHY a scratch dir: the test must not write into the source tree. The committed file
    // is what the app's tests read, and a test that rewrites it changes it under them.
    let dir = support::scratch_dir("czi_fixture");
    let path = dir.join("stored-pyramid.czi");
    write_czi(&path, &fixture_tiles());

    assert_eq!(
        std::fs::read(&path).expect("the generated fixture reads"),
        COMMITTED,
        "the committed fixture must be the one the writer makes"
    );

    let reader = phasit_imread::open(&path).expect("the fixture opens");
    let stored = reader
        .stored_pyramid(0)
        .expect("the fixture stores a pyramid of its own");
    let factors: Vec<usize> = stored.levels().iter().map(|level| level.factor).collect();

    assert!(
        factors.contains(&2) && factors.contains(&4),
        "the fixture must store the x2 and the x4 level, got {factors:?}"
    );
}

/// Writes the committed fixture. Run it with `cargo test -p phasit-imread --test czi_fixture
/// -- --ignored regenerate_the_stored_pyramid_fixture`.
///
/// WHY it is ignored: a test that runs with the suite must not change the file the suite reads.
#[test]
#[ignore = "writes the committed fixture"]
fn regenerate_the_stored_pyramid_fixture() {
    let path = fixture_path();
    std::fs::create_dir_all(path.parent().expect("the fixtures dir"))
        .expect("create the fixtures dir");
    write_czi(&path, &fixture_tiles());
}
