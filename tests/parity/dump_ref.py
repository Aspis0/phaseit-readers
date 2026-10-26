"""Ground-truth reference dumper for phasit-imread parity tests.

Uses `tifffile` as the trusted, battle-tested decoder to dump a handful of
2-D (Y, X) planes from real microscopy files, plus a manifest describing
exactly how each plane was selected and its sha256. A future Rust test
suite reads `manifest.json` + the `.npy` files and checks the Rust decoder
against these bytes bit-for-bit.

This is dev-only tooling. Never shipped, never imported by production code.

Usage:
    python dump_ref.py PATH [PATH ...] [--max-planes N] [--out DIR]

The files to dump are named on the command line: this script carries no local
paths of its own.

Or programmatically:
    from dump_ref import dump
    entries = dump(path, out_dir, max_planes=8)
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import sys
import traceback
from pathlib import Path

import numpy as np
import tifffile

# Planes dumped per file when the caller does not ask for a specific cap.
DEFAULT_MAX_PLANES = 8


def _select_representative(combos: list[tuple[int, ...]], max_planes: int) -> list[tuple[int, ...]]:
    """Pick up to `max_planes` evenly-spaced combos, always including first and last."""
    n = len(combos)
    if n <= max_planes:
        return combos
    if max_planes <= 1:
        return [combos[0]]
    indices = sorted({round(i * (n - 1) / (max_planes - 1)) for i in range(max_planes)})
    return [combos[i] for i in indices]


def dump(path: str | Path, out_dir: str | Path, max_planes: int | None = None) -> list[dict]:
    """Dump reference 2-D planes for a single file.

    Prints diagnostics to stdout and returns a list of manifest entries
    (does NOT write manifest.json itself -- caller aggregates across files).
    """
    path = Path(path)
    out_dir = Path(out_dir)
    stem = path.stem
    sample_dir = out_dir / stem
    sample_dir.mkdir(parents=True, exist_ok=True)

    entries: list[dict] = []
    is_lsm = path.suffix.lower() == ".lsm"

    print(f"=== {path.name} ===")
    with tifffile.TiffFile(str(path)) as tif:
        n_pages = len(tif.pages)
        print(f"raw page count (len(tif.pages)): {n_pages}")

        try:
            is_reduced_count = sum(1 for p in tif.pages if getattr(p, "is_reduced", False))
            print(f"pages with is_reduced=True: {is_reduced_count}")
        except Exception as exc:  # pragma: no cover - diagnostic only
            print(f"could not evaluate page.is_reduced: {exc!r}")

        for i, series in enumerate(tif.series):
            print(f"series[{i}]: shape={series.shape} axes={series.axes!r} dtype={series.dtype}")

        series = tif.series[0]
        axes = series.axes
        shape = series.shape
        dtype = series.dtype

        if is_lsm:
            real_planes = 1
            for ax, n in zip(axes, shape):
                if ax not in ("Y", "X"):
                    real_planes *= n
            ratio = (n_pages / real_planes) if real_planes else float("nan")
            print(
                f"LSM thumbnail-doubling check: raw_pages={n_pages} "
                f"real_planes(series[0], excl. Y/X)={real_planes} ratio={ratio:.3f}"
            )
            print(f"  -> raw pages ~= 2x real planes: {abs(ratio - 2.0) < 0.5}")

        data = series.asarray()
        if data.ndim != len(axes):
            raise ValueError(
                f"ndim mismatch for {path.name}: data.ndim={data.ndim} axes={axes!r}"
            )

        y_idx = axes.index("Y")
        x_idx = axes.index("X")
        extra_axes = [(ax, i) for i, ax in enumerate(axes) if ax not in ("Y", "X")]

        if extra_axes:
            ranges = [range(shape[i]) for _, i in extra_axes]
            all_combos = list(itertools.product(*ranges))
        else:
            all_combos = [()]

        if max_planes is not None and len(all_combos) > max_planes:
            combos = _select_representative(all_combos, max_planes)
        else:
            combos = all_combos

        dtype_str = data.dtype.str  # e.g. '<u2', preserves byte order

        for combo in combos:
            index: list = [slice(None)] * data.ndim
            plane_index: dict[str, int] = {}
            for (ax, dim_i), val in zip(extra_axes, combo):
                index[dim_i] = val
                plane_index[ax] = int(val)

            plane = data[tuple(index)]
            # After slicing out all extra (scalar) axes, the two remaining
            # dims keep their original relative order. Normalize to (Y, X).
            if x_idx < y_idx:
                plane = plane.T
            plane = np.ascontiguousarray(plane)

            idx_str = "_".join(f"{ax}{val}" for ax, val in plane_index.items()) or "single"
            npy_name = f"{stem}__{idx_str}.npy"
            npy_path = sample_dir / npy_name
            np.save(npy_path, plane)

            sha256 = hashlib.sha256(npy_path.read_bytes()).hexdigest()
            rel_npy_path = npy_path.relative_to(out_dir)

            entries.append(
                {
                    "file": path.name,
                    "sample": stem,
                    "axes": axes,
                    "series_shape": list(shape),
                    "dtype": dtype_str,
                    "plane_index": plane_index,
                    "plane_shape": list(plane.shape),
                    "npy_path": str(rel_npy_path).replace("\\", "/"),
                    "sha256": sha256,
                }
            )

        print(f"dumped {len(entries)} plane(s) for {path.name} (of {len(all_combos)} candidate combos)")

    print()
    return entries


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Dump reference planes from the named files (no default paths)."
    )
    parser.add_argument("paths", nargs="+", help="the files to dump planes from")
    parser.add_argument(
        "--max-planes",
        type=int,
        default=DEFAULT_MAX_PLANES,
        help=f"cap the planes dumped per file (default: {DEFAULT_MAX_PLANES})",
    )
    parser.add_argument(
        "--out",
        default=None,
        help="the folder the dumps go into (default: this script's _ref/)",
    )
    args = parser.parse_args(argv)

    here = Path(__file__).resolve().parent
    out_dir = Path(args.out) if args.out else here / "_ref"
    out_dir.mkdir(parents=True, exist_ok=True)

    all_entries: list[dict] = []
    had_error = False

    for path in args.paths:
        try:
            entries = dump(path, out_dir, max_planes=args.max_planes)
            all_entries.extend(entries)
        except Exception:
            had_error = True
            print(f"ERROR processing {path!r}:", file=sys.stderr)
            traceback.print_exc()
            print()

    manifest_path = out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(all_entries, indent=2), encoding="utf-8")
    print(f"Wrote manifest: {manifest_path} ({len(all_entries)} entries)")

    return 1 if had_error else 0


if __name__ == "__main__":
    raise SystemExit(main())
