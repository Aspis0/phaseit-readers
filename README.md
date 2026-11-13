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
| MRC2014 | ⚠️ one scene; MODE 1, 2 and 6 only; ISPG 0, 1–230 or 401–630 |
| DeltaVision | ⚠️ one scene; MODE 0, 1, 2 and 6 only; ImgSequence 0, 1 or 2; W ≤ 5 |
| ICS/IDS | ⚠️ one scene; versions 1.0 and 2.0; uncompressed and gzip; U8, U16, I16 and F32 |
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
  its own reason. A `.lifext` companion next to the `.lif` holds the children
  of the `.lif`'s own images — their histograms (`*_histo`) and their reduced
  pyramid levels (`*_pmd_N`) — which are auxiliary data of those images and not
  scenes of their own, so they are deliberately neither published nor listed as
  refused, the same as Bio-Formats, which opens the `.lif` and publishes its
  images only.
- **CZI** — uncompressed, Zstd0 and Zstd1 subblocks. JpgFile (1) and LZW (2)
  are refused. JPEG-XR (4) is refused unless the crate is built with
  `--features jpegxr`, which needs a C toolchain (MSVC on Windows, gcc or
  clang on Linux and macOS; MinGW-w64 is not supported, see
  `vendor/jxrlib/PATCHES.md`); even then a
  JPEG-XR-compressed Gray32Float or Bgr48 tile is refused (an uncompressed
  one is decoded), and so are subblocks that pack several values of one axis
  (line-scan style) — that one fails the open of the whole file, it is not a
  per-scene refusal. A packed **mosaic** axis (`M.Size > 1`) is the exception:
  it is accepted, like `czifile` does, because the mosaic index only decides
  the draw order and contributes no pixels. Where two tiles of a plane
  overlap, the tile with the **higher mosaic index (`M`) is drawn on top** —
  the same order `czifile` composites a mosaic in — but only in a plane that
  carries at least one non-negative `M`; a plane whose `M` values are all
  negative is composed in its subblock-directory order instead. A scene that
  LOSES a full-resolution tile — a tile whose
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
- **MRC2014** — one scene: a stack of images (`ISPG 0`), one volume
  (`ISPG 1–230`, with a Z step) or a stack of volumes (`ISPG 401–630`, split into
  time points and sections, refused when `MZ` is zero or does not divide `NZ`).
  MODE 1, 2 and 6 are read as I16, F32 and U16; MODE 0 (signed 8-bit), the
  complex modes (3, 4), 16-bit float (12) and 4-bit packed (101) are refused
  rather than converted, and so are an axis order other than the standard
  1/2/3, a non-positive extent, a negative extended-header length and a data
  size that runs past the end of the file.
- **DeltaVision** — one scene: a stack of sections in ZTW, WZT or ZWT order
  (`ImgSequence 0`, `1` or `2`), split into time points, channels and sections,
  refused when `NZ` is not divisible by `W * T`, `W` is above 5, a dimension
  is non-positive, `NumTimes`/`NumWaves` is negative, `next` is negative or
  smaller than the per-section extended header the header states, or the data
  runs past the end of the file. MODE 0, 1, 2 and 6 are read as U8, I16, F32
  and U16; the complex modes (3, 4), 16-bit integer of unclear meaning (5) and
  32-bit integer (7) are refused rather than converted. The per-section extended
  header (NumIntegers + NumFloats words) lives inside the `next` bytes, so it
  shifts nothing; only the consistency of the two is checked. The emission
  wavelengths are not published because the metadata has no field for them.
- **ICS/IDS** (Image Cytometry Standard) — one scene: the axes of the file's own
  `layout order`, fastest first, so a plane is read as one block only when x and
  y come first and is gathered by the axis strides otherwise. Version 2.0 keeps
  the pixels after `end`; version 1.0 keeps them in a separate `.ids` file with
  the same stem, and an `.ids` is opened through its `.ics` header. The sample
  types unsigned 8 and 16, signed 16 and real 32 are read; every other
  combination (complex, 32- and 64-bit integer, real 64, signed 8) is refused
  rather than converted, and so are a version other than 1.0 or 2.0, an unknown,
  repeated or missing x/y axis, a non-positive or non-integer extent, a
  `layout parameters` that disagrees with `order` or `sizes`, a header with no
  `end` inside 1 MiB, the old Unix `compress`, a byte order that is neither
  `1 … k` nor `k … 2 1` (or an absent one for a wide sample), a gzip file whose two
  fastest axes are not x then y, and data past the end of the file. A
  gzip plane costs the whole stream before it: there is no index into a deflated
  file. `region` is the trait default.
- **MetaMorph STK** (Molecular Devices) — one scene: the UIC2 plane table's count is the Z
  length, and the planes follow plane 0 with no gap. A compression other than none, more than
  one sample per pixel, a sample type other than unsigned 8 or 16 bit, strips of plane 0 that do
  not touch, a UIC2 of no plane, and a plane count whose records or planes run past the end of
  the file are refused. The pixel size comes from UIC1's XCalibration and YCalibration and is
  published only when its own CalibrationUnits entry names a unit this reader converts (µm or
  nm); any other unit, no units entry and no UIC1 leave the scene uncalibrated, never guessed. A
  UIC1 entry whose offset is outside the file is ignored. The UIC2 z distance is not published:
  what it measures varies between writers, and the MetaMorph `.nd` companion file is not read.
  `region` is the trait default.

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
`tests/parity/dump_ref_lif.py` the LIF ones, `tests/parity/dump_ref_nd2.py`
the planes of `tests/fixtures/nd2_real/`, and `tests/parity/dump_ref_lsm.py`
the Bio-Formats reference of the LSM tile scan (see `tests/parity/README.md`).
The `tests/fixtures/jpegxr_real/` folder is made by
`tests/parity/extract_jxr_tiles.py`, and the reference regions of the JPEG-XR
sample by `tests/parity/dump_ref_czi_jxr.py`. The folder is expected to hold:

| Relative path | What it is | Where it comes from |
|---|---|---|
| `czi/brightfield-jxr.czi` | a Zeiss Axioscan 20x brightfield CZI of a DAB-stained human section, JPEG-XR compressed, 1958 full-resolution tiles | Garcia, Mario, *Human DAB staining Axioscan BF 20x*, Zenodo record 11234863, <https://doi.org/10.5281/zenodo.11234863> (CC-BY-4.0) |
| `czi/mosaic.czi` | a Zeiss CZI of several tiles per (C,Z), composed into one scene | — |
| `czi/stitched.czi` | a pre-stitched Zeiss CZI, one tile per (C,Z) | — |
| `lif/control.lif` | a Leica LIF with four images in the `.lif` | — |
| `lif/four-scene.lif` | a Leica LIF with four series and a companion `.lifext` | — |
| `lsm/stack.lsm` | a Zeiss LSM stack | — |
| `lsm/tilescan.lsm` | a Zeiss LSM mosaic shot as a 7x7 grid of tiles, 49 pages | — |
| `nd2/stack.nd2` | a Nikon ND2 stack | — |
| `tiff/rgb.tif` | an RGB TIFF (three interleaved components) | — |
| `tiff/calibrated-a.tif` | a TIFF that states its scale | — |
| `tiff/calibrated-b.tif` | a TIFF that states its scale | — |
| `tiff/uncalibrated.tif` | a TIFF that states no scale | — |

One test names its file directly, through `PHASIT_CZI_BGR_SAMPLE`, because that
sample is a third-party file rather than an acquisition.

The third-party sample under `czi/brightfield-jxr.czi` is published as:

> Garcia, Mario. *Human DAB staining Axioscan BF 20x*. Zenodo, 2024.
> <https://doi.org/10.5281/zenodo.11234863>. Licensed
> [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/legalcode).

The repository ships NO derived data from it: the reference regions and the
extracted tiles are generated locally and are gitignored, so the attribution
above is a courtesy rather than a redistribution obligation. Anyone who
redistributes them must keep the attribution and the CC-BY-4.0 licence.

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
