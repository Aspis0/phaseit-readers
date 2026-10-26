"""Ground-truth reference dumper for phasit-imread LIF parity tests.

Uses `liffile` (cgohlke's BSD-3 LIF reader) as the trusted, battle-tested
decoder to dump a handful of 2-D (Y, X) planes from real Leica .lif files,
plus a manifest describing exactly how each plane was selected and its
sha256. A future Rust test suite reads `manifest_lif.json` + the `.npy`
files and checks the Rust LIF decoder against these bytes bit-for-bit.

This is dev-only tooling. Never shipped, never imported by production code.
Deliberately kept SEPARATE from `dump_ref.py` (F0, TIFF/LSM) -- writes its
own `_ref/manifest_lif.json` and never touches `_ref/manifest.json`.

MEMORY SAFETY: LIF files here are multi-GB. This script NEVER calls
`LifImage.asarray()` on a whole multi-dimensional image (that would load
the entire series into RAM). It only ever pulls individual 2-D (Y, X)
planes lazily via `LifImage.frame(**indices)`, which liffile implements as
a targeted seek + read of just that plane's bytes.

Key diagnostic goal: report each image's NATIVE axis order as liffile
reports it (`LifImage.dims`), since a LIF image's dimension order is not
fixed (unlike TIFF/LSM planes normalized in dump_ref.py) -- the Rust reader
needs this to map native order to canonical TCZYX.

Also reports whether a `.lif` + `.lifext` companion pair is read
transparently by liffile. Empirically (see README / session notes): it is
NOT. Opening `some.lif` alone never reads `some.lifext`; `LifFile.images`
on the plain .lif is unaffected by the companion's presence or absence.
The `.lifext` must be opened as its own `LifFile`, and its images
reference the parent image via a `MemoryBlockID`-prefixed `path`
(`<block_id>/<name>`, e.g. `MemBlock_471/Series001_pmd_0`) that matches
`parent_image.memory_block.id` in the main .lif. This script exploits that
convention to find, and dump a plane from, an image that physically lives
in the .lifext file.

Usage:
    python dump_ref_lif.py PATH [PATH ...] [--max-planes N] [--out DIR]

The files to dump are named on the command line: this script carries no local
paths of its own.

Or programmatically:
    from dump_ref_lif import dump_lif
    entries = dump_lif(path, out_dir, max_planes=6)
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import re
import sys
import traceback
from pathlib import Path

import liffile
import numpy as np


def _select_representative(combos: list[tuple[int, ...]], max_planes: int) -> list[tuple[int, ...]]:
    """Pick up to `max_planes` evenly-spaced combos, always including first and last."""
    n = len(combos)
    if n <= max_planes:
        return combos
    if max_planes <= 1:
        return [combos[0]]
    indices = sorted({round(i * (n - 1) / (max_planes - 1)) for i in range(max_planes)})
    return [combos[i] for i in indices]


def _sanitize(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_-]+", "_", name).strip("_") or "unnamed"


def _pixel_sizes(img) -> dict:
    """Best-effort per-axis pixel spacing (meters) from LifImage.coords.

    Returns {} if coords are unavailable (never raises -- diagnostic only).
    """
    sizes: dict = {}
    try:
        coords = img.coords
    except Exception as exc:  # pragma: no cover - diagnostic only
        return {"error": repr(exc)}
    for ax in ("X", "Y", "Z"):
        arr = coords.get(ax)
        if arr is None or len(arr) < 2:
            continue
        try:
            spacing_m = float(arr[1]) - float(arr[0])
        except Exception:  # pragma: no cover - diagnostic only
            continue
        sizes[ax] = {"spacing_m": spacing_m, "spacing_um": spacing_m * 1e6}
    return sizes


def _print_image_diagnostics(img, index: int, lif_path: Path) -> str | None:
    """Print diagnostics for one LifImage. Returns its memory_block id, if resolvable."""
    dims_str = "".join(img.dims)
    print(
        f"  image[{index}] name={img.name!r} path={img.path!r} "
        f"dims={dims_str!r} shape={img.shape} dtype={img.dtype.str!r}"
    )
    ps = _pixel_sizes(img)
    if ps:
        formatted = ", ".join(
            f"{ax}={v['spacing_um']:.4f}um" if "spacing_um" in v else f"{ax}={v}"
            for ax, v in ps.items()
        )
        print(f"    pixel sizes: {formatted}")

    block_id: str | None = None
    try:
        mb = img.memory_block
        fits_inside = mb.offset >= 0 and (mb.offset + mb.size) <= lif_path.stat().st_size
        print(
            f"    memory_block id={mb.id!r} type={mb.type.name} offset={mb.offset} "
            f"size={mb.size} (fits fully inside {lif_path.name}: {fits_inside})"
        )
        block_id = mb.id
    except Exception as exc:  # pragma: no cover - diagnostic only
        print(f"    memory_block lookup failed: {exc!r}")
    return block_id


def _dump_planes_for_image(
    img,
    source_path: Path,
    sample_stem: str,
    sample_dir: Path,
    out_dir: Path,
    max_planes: int,
) -> list[dict]:
    """Dump up to `max_planes` 2-D (Y, X) planes from a single LifImage, lazily.

    Uses `img.frame(**indices)` exclusively -- never `img.asarray()`.
    """
    dims = img.dims
    sizes = img.sizes

    if "Y" not in dims or "X" not in dims:
        print(f"    skip {img.name!r}: no Y/X dims (dims={''.join(dims)!r}), not a 2-D-plane image")
        return []
    if "S" in dims:
        print(f"    skip {img.name!r}: sample/RGB axis 'S' present, not covered by this harness")
        return []

    last_two = dims[-2:]
    if last_two == ("Y", "X"):
        transpose = False
    elif last_two == ("X", "Y"):
        transpose = True
    else:
        print(
            f"    skip {img.name!r}: innermost dims {last_two} are not (Y,X)/(X,Y), "
            f"unsupported by this harness"
        )
        return []

    extra_axes = dims[:-2]
    if extra_axes:
        ranges = [range(sizes[ax]) for ax in extra_axes]
        all_combos = list(itertools.product(*ranges))
    else:
        all_combos = [()]

    combos = (
        _select_representative(all_combos, max_planes)
        if len(all_combos) > max_planes
        else all_combos
    )

    dtype_str = img.dtype.str
    axes_str = "".join(dims)
    safe_name = _sanitize(img.name)
    entries: list[dict] = []

    for combo in combos:
        plane_index: dict[str, int] = {ax: int(val) for ax, val in zip(extra_axes, combo)}
        frame = img.frame(**plane_index)
        if transpose:
            frame = frame.T
        frame = np.ascontiguousarray(frame)

        idx_str = "_".join(f"{ax}{val}" for ax, val in plane_index.items()) or "single"
        npy_name = f"{sample_stem}__{safe_name}__{idx_str}.npy"
        npy_path = sample_dir / npy_name
        np.save(npy_path, frame)

        sha256 = hashlib.sha256(npy_path.read_bytes()).hexdigest()
        rel_npy_path = npy_path.relative_to(out_dir)

        entries.append(
            {
                "file": source_path.name,
                "sample": sample_stem,
                "image_name": img.name,
                "axes": axes_str,
                "series_shape": list(img.shape),
                "dtype": dtype_str,
                "plane_index": plane_index,
                "plane_shape": list(frame.shape),
                "npy_path": str(rel_npy_path).replace("\\", "/"),
                "sha256": sha256,
            }
        )

    print(
        f"    dumped {len(entries)} plane(s) for image {img.name!r} "
        f"(of {len(all_combos)} candidate combos)"
    )
    return entries


def dump_lif(path: str | Path, out_dir: str | Path, max_planes: int | None = None) -> list[dict]:
    """Dump reference 2-D planes for a single .lif file (and its .lifext companion, if any).

    Prints diagnostics to stdout and returns a list of manifest entries
    (does NOT write manifest_lif.json itself -- caller aggregates across files).
    """
    path = Path(path)
    out_dir = Path(out_dir)
    stem = path.stem
    sample_dir = out_dir / stem
    sample_dir.mkdir(parents=True, exist_ok=True)
    max_planes = max_planes if max_planes is not None else 6

    entries: list[dict] = []

    print(f"=== {path.name} ===")

    ext_path = path.with_suffix(".lifext")
    has_ext = ext_path.exists()
    if has_ext:
        print(f"companion .lifext present: {ext_path.name} ({ext_path.stat().st_size} bytes)")
    else:
        print("companion .lifext present: no")

    primary_block_id: str | None = None
    primary_name: str | None = None

    with liffile.LifFile(str(path)) as lif:
        print(f"LifFile.type={lif.type} version={lif.version}")
        n_images = len(lif.images)
        print(f"image/series count: {n_images}")

        for i, img in enumerate(lif.images):
            block_id = _print_image_diagnostics(img, i, path)
            if i == 0:
                primary_block_id = block_id
                primary_name = img.name

        if has_ext:
            print(
                f"NOTE: liffile does NOT auto-merge {ext_path.name} when opening "
                f"{path.name} alone -- lif.images stays at {n_images} regardless of "
                f"the companion's presence. Caller must open the .lifext explicitly "
                f"and match children by path prefix == parent memory_block.id."
            )

        # Dump reference planes from EVERY image whose pixels physically live in
        # this .lif (so every channel count / dimension layout in the file is
        # exercised, not just image[0]). Images named "*_pmd_0" are ext-resident
        # placeholders -- their bytes live in the .lifext companion (handled
        # separately below) and our Rust reader never exposes them as a scene,
        # so they are skipped here too.
        for i, img in enumerate(lif.images):
            if img.name.endswith("_pmd_0"):
                print(
                    f"  skip image[{i}] {img.name!r}: ext-resident (_pmd_0), "
                    f"pixels are not in {path.name}"
                )
                continue
            entries.extend(
                _dump_planes_for_image(img, path, stem, sample_dir, out_dir, max_planes)
            )

    if has_ext and primary_block_id is not None:
        with liffile.LifFile(str(ext_path)) as lifext:
            n_ext_images = len(lifext.images)
            print(f"--- {ext_path.name} (companion) ---")
            print(f"image/series count: {n_ext_images}")
            ext_child = None
            for i, img in enumerate(lifext.images):
                _print_image_diagnostics(img, i, ext_path)
                first_component = img.path.split("/")[0]
                is_child_of_primary = first_component == primary_block_id
                print(
                    f"    references parent block {first_component!r} "
                    f"(matches primary image {primary_name!r}: {is_child_of_primary})"
                )
                if (
                    ext_child is None
                    and is_child_of_primary
                    and "Y" in img.dims
                    and "X" in img.dims
                ):
                    ext_child = img

            if ext_child is not None:
                print(
                    f"EXT-FILE PLANE: dumping from {ext_child.name!r}, bytes physically "
                    f"stored in {ext_path.name} (resolved via MemoryBlockID={primary_block_id!r})"
                )
                entries.extend(
                    _dump_planes_for_image(ext_child, ext_path, stem, sample_dir, out_dir, 1)
                )
            else:
                print(
                    f"WARNING: no Y/X-bearing child of block {primary_block_id!r} found in "
                    f"{ext_path.name}; no ext-file plane dumped for this sample."
                )

    print(f"dumped {len(entries)} plane(s) total for {path.name}")
    print()
    return entries


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Dump reference planes from the named .lif files (no default paths)."
    )
    parser.add_argument("paths", nargs="+", help="the .lif files to dump planes from")
    parser.add_argument(
        "--max-planes",
        type=int,
        default=6,
        help="cap the planes dumped per image (default: 6)",
    )
    parser.add_argument(
        "--out",
        default=None,
        help="the folder the dumps go into (default: this script's _ref/)",
    )
    args = parser.parse_args(argv)

    print(f"liffile version: {liffile.__version__}")
    print()

    here = Path(__file__).resolve().parent
    out_dir = Path(args.out) if args.out else here / "_ref"
    out_dir.mkdir(parents=True, exist_ok=True)

    all_entries: list[dict] = []
    had_error = False

    for path in args.paths:
        try:
            entries = dump_lif(path, out_dir, max_planes=args.max_planes)
            all_entries.extend(entries)
        except Exception:
            had_error = True
            print(f"ERROR processing {path!r}:", file=sys.stderr)
            traceback.print_exc()
            print()

    manifest_path = out_dir / "manifest_lif.json"
    manifest_path.write_text(json.dumps(all_entries, indent=2), encoding="utf-8")
    print(f"Wrote manifest: {manifest_path} ({len(all_entries)} entries)")

    return 1 if had_error else 0


if __name__ == "__main__":
    raise SystemExit(main())
