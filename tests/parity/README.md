# Parity reference harness

Ground-truth reference dumps generated via `tifffile` (Python), used to check the Rust
`phasit-imread` decoder against real microscopy files bit-for-bit. The files to dump are
named on the command line — the harness carries no local paths of its own. Run with
`python dump_ref.py PATH [PATH ...] [--max-planes N] [--out DIR]` from this directory
(requires `tifffile` + `numpy`).

`_ref/` (the `.npy` planes and `manifest.json`) is gitignored — regenerate it locally whenever
needed. The Rust parity tests read `manifest.json` plus the referenced `.npy` files to compare
against.

`dump_ref_lif.py` covers LIF via `liffile` (`pip install liffile`) as ground truth.
It writes a separate `_ref/manifest_lif.json` — it never touches `manifest.json`. Run with
`python dump_ref_lif.py PATH [PATH ...] [--max-planes N] [--out DIR]`. Because LIF samples are
multi-GB, it only ever reads individual 2-D planes lazily (`LifImage.frame(...)`), never a whole
multi-dimensional image.

`dump_ref_nd2.py` covers ND2 via the `nd2` reference (`pip install nd2`). It writes the
reference planes into `tests/fixtures/nd2_real/`. Run with `python dump_ref_nd2.py PATH_TO_ND2`.

The Rust tests read their files from the folder named by `PHASIT_REAL_DATA_DIR`; the manifest
names each sample by its file name without the extension, which is how the parity tests tell
the samples apart.
