# phasit-imread

Native, in-process, offline reader for proprietary microscopy image formats.
Decodes files to normalized `(T, C, Z, Y, X)` planes with no JVM, no Bio-Formats,
no network. GPL-free (see `THIRD-PARTY-NOTICES.md`).

## Status

| Format | State |
|---|---|
| TIFF / OME-TIFF | ⚠️ grey, RGB and 8-bit palette pages; pyramids out of scope |
| LSM (Zeiss) | ⚠️ uncompressed, single time point, planar channels |
| LIF (Leica) | ⚠️ an image whose pixels are absent from the `.lif`, whose `DimID` is not modelled, or whose declared size overflows the reader is refused with its reason; the others open |
| CZI (Zeiss) | ⚠️ uncompressed and Zstd; JPEG-XR needs `--features jpegxr`; a scene that lost a full-resolution tile is refused with its reason |
| ND2 (Nikon) | ⚠️ uncompressed frames only (lossless/lossy and legacy JPEG2000 refused) |
| OIB · OIF (Olympus) | detected, `NotImplemented` |

⚠️ means the format opens only part of what it can hold; the reader refuses the
rest rather than guess:

- **TIFF / OME-TIFF** — plain TIFF: grey (`BlackIsZero`/`WhiteIsZero`), RGB and
  8-bit indexed (`RGBPalette`) pages; every other photometric (CMYK, YCbCr,
  LogL), a palette page whose indices are not 8-bit or that carries no
  `ColorMap`, a grey page with more than one sample per pixel, an RGB page with
  fewer than three, a planar (`PlanarConfiguration = 2`) multi-sample page and
  the sample types the crate does not model (U32, F16, F64, I8, I32) are
  refused. OME-TIFF: the scenes it can read open and the others are refused by
  name; indexed pages, an interleaved (`Interleaved = true`) scene and a
  `<Pixels>` whose type or size disagrees with its first page are refused.
  Reduced pages (pyramids, thumbnails, masks) are skipped in both.
- **LSM** — uncompressed only; a single time point (`T > 1` is refused because
  the on-disk page order is unverified); planar channels, not interleaved; not
  BigTIFF.
- **LIF** — an image is refused by name, with its reason, when its data block
  is absent from the `.lif` (the reason calls the block ext-resident when a
  `.lifext` companion sits next to the `.lif`, and absent from the `.lif` when
  there is none), when a `DimID` is outside `{1,2,3,4}`, when its channels are
  interleaved or stride-padded so a frame is not contiguous, when its pixel
  type is not
  modelled, when a numeric attribute is missing or malformed, when its
  dimension list is ambiguous (a duplicate `DimID`, a modelled axis with no
  elements), when its declared size overflows the reader's byte arithmetic or
  exceeds the reader's cap, or when its data block does not hold its whole
  frame. The refused image keeps its place in the file's own list of images,
  so the images after it are not renumbered; the open fails only when no image
  of the file can be read at all, and then the error names every refusal with
  its own reason.
- **CZI** — uncompressed, Zstd0 and Zstd1 subblocks. JpgFile (1) and LZW (2)
  are refused. JPEG-XR (4) is refused unless the crate is built with
  `--features jpegxr`, which needs a C toolchain (MSVC on Windows, gcc or
  clang on Linux and macOS; MinGW-w64 is not supported, see
  `vendor/jxrlib/PATCHES.md`); even then a
  JPEG-XR-compressed Gray32Float or Bgr48 tile is refused (an uncompressed
  one is decoded), and so are subblocks that pack several values of one axis
  (line-scan style) — that one fails the open of the whole file, it is not a
  per-scene refusal. A scene that LOSES a full-resolution tile — a tile whose
  pixel type the reader does not model, or a tile whose pixel span does not
  fit the file — is refused by name, with the count of the tiles that were
  skipped and why they were skipped; it is never published smaller than the
  file says. A scene whose tiles do not share one pixel type (a Gray8 tile
  mixed with a Bgr24 one) is refused by name too. A scene that holds only
  sub-sampled (pyramid) tiles is neither published nor refused: it is not a
  scene of the file this reader composes. When every scene of the file is
  refused the open fails, and the error names each refusal with its own
  reason.
- **ND2** — uncompressed frames only: a lossless- or lossy-compressed ND2 and a
  legacy JPEG2000 ND2 are refused. `uiBpcInMemory` must be 8, 16 or 32 bits, and
  the experiment loops are limited to time, Z-stack and XY-position — a spectral
  loop is skipped, any other loop type is refused.

## API

```rust
let reader = phasit_imread::open(path)?;      // dispatch by magic + extension
let meta = reader.meta();                     // ImageMeta { name, scenes: Vec<SceneMeta> }
let plane = reader.plane(scene, t, c, z)?;    // owned, native-endian Plane { data, shape:[y,x] }
let refused = reader.refused_scenes();        // the scenes this reader refused, with their reason
let index = reader.scene_file_index(scene);   // the scene's own position in the file
```

`meta.scenes` holds the readable scenes only; `reader.refused_scenes()` lists the
refused ones with their file position, and `reader.scene_file_index(s)` maps a
scene back to its position in the file.

Canonical axis order is **TCZYX**. Each scene carries its own dims/dtype/channels
(a LIF holds several differently-shaped images). Buffers are owned and byteswapped
to native endianness; decoding is lazy per-plane over an mmap. Every reader is
`Send + Sync` and sizes an allocation (`checked_plane_bytes`) before making it.

## CLI

```
imread info <file>
imread dump <file> <scene> <t> <c> <z> <out>   # raw native-endian bytes
```

## Tests

`cargo test -p phasit-imread --features ndarray`

The tests that read a real acquisition file read it from the folder named by
the `PHASIT_REAL_DATA_DIR` environment variable; the file itself is named by a
relative path inside that folder. A test whose variable is unset, or whose
file is absent, skips with a message saying which of the two it was, so the
suite stays green without the files. A skipped test still counts as passed in
the cargo summary, so read the `skipping:` messages (run the suite with
`--nocapture`) to tell a skip from a real run. Set
`PHASIT_REAL_DATA_REQUIRED=1` (or `true`) and every one of those skips becomes
a failure that names what is missing, so a run that needs the real data cannot
pass quietly without it. The flag takes two values only: unset, empty or `0`
means the run does not need the data, `1` or `true` means it does; any other
value is a typo and panics rather than failing open. The flag covers the whole
run: the reference dumps, the third-party sample below and the real JPEG-XR
tiles are gated by it as well. A required run therefore needs the generated
artifacts too, and none of them is in the repository: `tests/parity/dump_ref.py`
makes the `_ref/` dumps of the TIFF and LSM samples,
`tests/parity/dump_ref_lif.py` the LIF ones, and `tests/parity/dump_ref_nd2.py`
the planes of `tests/fixtures/nd2_real/` (see `tests/parity/README.md`). The
`tests/fixtures/jpegxr_real/` folder has no producer in this repository, so a
required run stops at the JPEG-XR real-tile check unless that folder is
supplied by hand. The folder is expected to hold:

| Relative path | What it is |
|---|---|
| `czi/mosaic.czi` | a Zeiss CZI of several tiles per (C,Z), composed into one scene |
| `czi/stitched.czi` | a pre-stitched Zeiss CZI, one tile per (C,Z) |
| `lif/control.lif` | a Leica LIF with four images in the `.lif` |
| `lif/four-scene.lif` | a Leica LIF with four series and a companion `.lifext` |
| `lsm/stack.lsm` | a Zeiss LSM stack |
| `nd2/stack.nd2` | a Nikon ND2 stack |
| `tiff/rgb.tif` | an RGB TIFF (three interleaved components) |
| `tiff/calibrated-a.tif` | a TIFF that states its scale |
| `tiff/calibrated-b.tif` | a TIFF that states its scale |
| `tiff/uncalibrated.tif` | a TIFF that states no scale |

One test names its file directly, through `PHASIT_CZI_BGR_SAMPLE`, because that
sample is a third-party file rather than an acquisition.

Real-file bit-exact parity vs the Python BSD references (`tifffile`, `liffile`,
`nd2`) runs when those files and the generated `_ref/` dumps are present; see
`tests/parity/README.md`. The harness is development-only — it needs Python
packages the crate does not depend on — and is never compiled into the library
or the `imread` binary. Its scripts do ship in the source distribution, so a
reader can see how the reference dumps were made.

## License / attribution

**Apache-2.0** — the full text is in `LICENSE`. `NOTICE` names the third
parties; their notices, licences and dependency tables are in
`THIRD-PARTY-NOTICES.md`.

Format knowledge is ported with attribution from BSD reference readers by
Christoph Gohlke; all bundled crates are permissive; no copyleft is linked. See
`THIRD-PARTY-NOTICES.md`.

## Provenance

This crate is published on its own, with its own history. It was written as
part of a larger project that is not published; none of that project's history
travels with this repository, and no part of its code travels with it beyond
this crate.
