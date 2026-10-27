# Parity reference harness

Ground-truth reference dumps generated via `tifffile` (Python), used to check the Rust
`phasit-imread` decoder against real microscopy files bit-for-bit. The files to dump are
named on the command line — the harness carries no local paths of its own. Run with
`python dump_ref.py PATH [PATH ...] [--max-planes N] [--out DIR]` from this directory
(requires `tifffile` + `numpy`).

`_ref/` (the `.npy` planes and `manifest.json`) is gitignored — regenerate it locally whenever
needed. The Rust parity tests read `manifest.json` plus the referenced `.npy` files to compare
against.

Committed tests never carry values derived from private files; such references live in gitignored
dumps.

`dump_ref_lif.py` covers LIF via `liffile` (`pip install liffile`) as ground truth.
It writes a separate `_ref/manifest_lif.json` — it never touches `manifest.json`. Run with
`python dump_ref_lif.py PATH [PATH ...] [--max-planes N] [--out DIR]`. Because LIF samples are
multi-GB, it only ever reads individual 2-D planes lazily (`LifImage.frame(...)`), never a whole
multi-dimensional image.

`dump_ref_lsm.py` covers the LSM tile scan against Bio-Formats as ground truth — Bio-Formats is
the reference, never this crate's own output. It drives the Bio-Formats harness (the small Java
probe that writes one plane as raw little-endian samples) and records the SHA-256 of every
series' first plane into `_ref/<stem>/manifest.json`: one entry per series, with its index, the
plane's shape and the hash. `tests/lsm_real.rs` reads that manifest and compares the hashes with
the reader's own planes, so the check is bit-exact against a reference this crate did not produce.
Run with `python dump_ref_lsm.py PATH_TO_LSM --java JAVA --jar BIOFORMATS_JAR --classes
HARNESS_CLASSES [--out DIR]`; the harness is named on the command line, so the script carries no
local path of its own. The hashes are of a PRIVATE acquisition file, so they are a fingerprint of
it: the dump is gitignored and the committed test states no hash of its own.

`dump_ref_nd2.py` covers ND2 via the `nd2` reference (`pip install nd2`). It writes the
reference planes into `tests/fixtures/nd2_real/`. Run with `python dump_ref_nd2.py PATH_TO_ND2`.

`extract_jxr_tiles.py` covers the real Zeiss JPEG-XR tiles. It samples a real Axioscan CZI
(Zenodo record 11234863, CC-BY-4.0 — the attribution is below), writing the raw JPEG-XR
codestreams and their expected decode into `tests/fixtures/jpegxr_real/`, where the
`jpegxr_fixtures_match_ground_truth` unit test in `src/jpegxr_ffi.rs` checks them byte for
byte. Run with `python extract_jxr_tiles.py PATH_TO_CZI` (needs `czifile` and `imagecodecs`).
It samples every k-th subblock, so the sample spreads over the whole file, and it decodes only
the sampled subblocks, one at a time: the mosaic is never materialized. The samples, the
dtype and the channel order of every fixture come from the subblock's own pixel type, and
`stored_x`/`stored_y` are the size the CONTAINER declares for the subblock, not the size the
decoder happened to return.

`dump_ref_czi_jxr.py` covers the CZI JPEG-XR path against `czifile` as ground truth. It
writes nine 1024x1024 regions of the composed scene into `_ref/czi_jxr/` (one `.npy` per
region plus `manifest.json`), where `tests/czi_jxr_real.rs` compares them byte for byte with
the reader's own `region()` output. Run with `python dump_ref_czi_jxr.py PATH_TO_CZI` (needs
`czifile` and `imagecodecs`); the window picking lives in `czi_jxr_geometry.py`, which is pure
geometry and opens no file. The regions are picked so that the mosaic itself is exercised:

* four single-tile interiors (top left, top right, bottom right, centre) — covered by exactly
  ONE tile, so the comparison cannot depend on the order in which overlapping tiles are
  blitted;
* two windows straddling the seam of two side-by-side tiles, inside the band the two tiles
  share;
* one window straddling a corner where FOUR tiles meet;
* one window at the bottom-right corner of a tile's interior, far from the top-left corner the
  other single-tile windows sit in;
* one window hugging the scene bounding box, partly outside every tile, so the fill value of
  the gap shows.

Only the tiles that intersect a region are ever decoded: `dump_ref_czi_jxr.py` composes the
intersecting tiles itself, and `dump_ref_czi.py` lets `czifile` compose the window it reads.
Neither of them ever materialises the whole mosaic.

COMPOSITION ORDER: the reference composes the tiles of a region the way `czifile` does — in
ascending mosaic index, so the tile with the higher M ends up on top (`czifile.py:1308`,
`czifile.py:1309`, applied at `czifile.py:2810` and `czifile.py:2811`). `phasit-imread`
composes them in the same order (`src/czi_reader.rs`, `CziReader::open` — the per-plane mosaic
sort). The two orders are not the same
in every file — in the Axioscan sample 993 of the 7350 overlapping tile pairs pick a
different winner under the subblock-directory order — so the manifest records the
subblock-directory order next to the mosaic order, per region, and the seam and corner
regions are chosen where the order decides the bytes. `dump_ref_czi_jxr.py
--order subblock_directory_order` writes the same regions in the subblock-directory order,
which is how the difference is pinned down — and the manifest then records THAT order, so the
parity test fails loudly instead of certifying a directory-order reference. Uncovered pixels
take `czifile`'s default fill
value 0 (`czifile.py:556`), which is also what the reader fills them with
(`src/czi_reader.rs`, `CziReader::plane` and `CziReader::region` — the composed buffer starts
zeroed).

`crosscheck_ref_czi.py` re-reads a reference set with `czifile` ITSELF —
`czifile.imread(path, roi=..., maxworkers=1)` — and compares the bytes with the dump, so the
reference is not only czifile's rule re-implemented but checked against czifile. Run with
`python crosscheck_ref_czi.py PATH_TO_CZI REF_DIR`. It reports two reads per window: one with
the plane selected (the order-preserving path, `czifile.py:1852-1865`) and one with a BARE
`roi=` (the cached spatial-index path, `czifile.py:1849`, which re-sorts the intersecting tiles
by X start and therefore composites in a DIFFERENT order than czifile's own full-plane path —
`czifile.py:2810-2811`). The bare read also keeps the dimensions a selection would squeeze
away, so its shape can differ from the reference's. A window whose PLANE-SELECTED read differs
from the dump makes the script exit non-zero: the reference is then not what `czifile` reads. A
bare-roi difference is only reported — a bare `roi=` composites in another order than czifile's
own composition path, so it disagrees wherever two tiles overlap and the two orders pick
different winners, which says nothing about the reference.

Both scripts name their input on the command line and carry no local path of their own. The
JPEG-XR sample is a third-party file, not an acquisition: link it into the folder named by
`PHASIT_REAL_DATA_DIR` as `czi/brightfield-jxr.czi`.

`dump_ref_czi.py` covers the owner's own CZI mosaic — `czi/mosaic.czi` — the same way, with
`czifile` as ground truth. It writes three 512x512 windows per plane for two planes into
`_ref/czi_mosaic/` (one `.npy` per window plus `manifest.json`), where
`tests/czi_mosaic_real.rs` compares them byte for byte with the reader's own `region()`
output. Run with `python dump_ref_czi.py PATH_TO_CZI` (needs `czifile`). The windows are picked
FROM THE DIRECTORY: one across the narrow band two side-by-side tiles share, one across the band
two stacked tiles share, and one inside a single tile. The window is read by
`czifile.imread(path, roi=..., maxworkers=1, C=c, Z=z, T=t)` — the plane is selected on
purpose, because a bare `roi=` takes `czifile`'s cached spatial-index path
(`czifile.py:1849`), which re-sorts the intersecting tiles by X start (`czifile.py:3157`,
`:3188-3197`) and composites in a different order than `czifile`'s own full-plane path
(`czifile.py:2810-2811`). `maxworkers=1` is what makes the winner of an overlap
deterministic (`czifile.py:2793-2801`).

WHAT THE OWNER'S MOSAIC REFERENCE CERTIFIES: the owner's mosaic cannot tell the two composition
orders apart. The manifest records the census — for every overlapping pair of tiles of the
file, the mosaic order and the subblock-directory order pick the SAME winner — so the reference
bytes are identical whichever of the two orders a reader uses. The reference therefore certifies
the GEOMETRY, the STRIDE, the DTYPE and the ORIGIN of the owner's mosaic, and it does NOT
certify the composition order. The composition order is certified by the JPEG-XR reference
(`tests/czi_jxr_real.rs`, the Axioscan sample), where 993 of the 7350 overlapping tile pairs do
pick a different winner. The manifest records, per window, whether the two orders pick the same
winner for every pixel (`orders_agree`), so this is visible in the reference itself and not only
in the test.

BOTH MANIFESTS USE THE SAME METHOD for `orders_agree`: the winner per pixel, computed from the
tile rectangles of the window. The two orders can list the tiles differently and still win the
same pixel, and the winner is what decides the bytes — so the field means the same thing in
`_ref/czi_mosaic/manifest.json` and in `_ref/czi_jxr/manifest.json`, and both can be read with one
rule. The top-level `composition.reference_order` of both manifests is the order the reference was
ACTUALLY made in: the mosaic order when the file carries a non-negative mosaic index, the
subblock-directory order when it does not (`czifile.py:1308-1310`).

The third-party sample is published as:

> Garcia, Mario. *Human DAB staining Axioscan BF 20x*. Zenodo, 2024.
> <https://doi.org/10.5281/zenodo.11234863>. Licensed
> [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/legalcode).

The dumps and the extracted tiles are derived data; they are generated locally and are
gitignored, so nothing derived from the sample is redistributed with this repository. Anyone
who redistributes them must keep the attribution above and the CC-BY-4.0 licence.

CHANNEL ORDER: the CZI container stores B,G,R (`PixelType = Bgr24`), but `czifile` returns the
JPEG-XR tiles as R,G,B — it does not swap BGR->RGB for compression 4, because the JPEG-XR
decoder already returns R,G,B — and that is the order `phasit-imread` publishes. The `.npy`
files therefore hold R,G,B, while the `.raw` files of the tile fixtures hold the container's
native B,G,R, which is what `decode_jpegxr` returns.

The Rust tests read their files from the folder named by `PHASIT_REAL_DATA_DIR`; the manifest
names each sample by its file name without the extension, which is how the parity tests tell
the samples apart.

## Renaming a sample

The sample paths and stems are compiled into the tests, not read from the folder: each
test names its file by a relative path constant, and the parity tests match the manifest
against that constant's stem.

Rename a file under `PHASIT_REAL_DATA_DIR` and the test that reads it finds nothing,
prints `skipping:` and passes. Run the suite with `--nocapture` and grep for `skipping:`
to tell a skip from a real run. With `PHASIT_REAL_DATA_REQUIRED=1` (or `true`) the same
missing file panics instead of skipping: the flag turns every skip into a failure, so a
required run cannot pass without the dumps below.

Dumps made from files whose stems differ from the test's constant are the other case:
`lif_real` panics on a sample name the manifest does not know, `tiff_real` fails its
`checked >= 3` assertion and `lsm_real` its `checked >= 8`.

Regenerate the TIFF/LSM dumps with `python dump_ref.py PATH [PATH ...]` and the LIF dumps
with `python dump_ref_lif.py PATH [PATH ...]`, naming every sample the manifest must hold:
each script rewrites its manifest from the files it is given, so a partial run drops the
other samples. The ND2 dumps come from `python dump_ref_nd2.py PATH_TO_ND2`, which writes
into `tests/fixtures/nd2_real/`, where `tests/nd2_real.rs` looks for them.
