"""Cross-check a CZI reference set against `czifile.imread` ITSELF.

The JPEG-XR reference set is czifile's rule RE-IMPLEMENTED: the producer
composes the intersecting tiles of a window itself, so it can pick the windows
from the subblock directory and decode only the tiles it needs. This re-reads
the same windows with czifile and compares the bytes.

TWO READS PER WINDOW:
* with the plane selected (`C=`, `Z=`, `T=`) — the order-preserving path
  (`czifile.py:1852-1865`), which is czifile's mosaic order;
* with a BARE `roi=` — the cached spatial-index path (`czifile.py:1849`), which
  returns the intersecting tiles sorted by X start (`czifile.py:3157`,
  `:3188-3197`) and therefore composites in a DIFFERENT order than czifile's own
  full-plane path (`czifile.py:2810-2811`).

Both are reported, so the difference is visible instead of hidden. A window
where the two reads disagree is a window where two tiles overlap AND the two
orders pick a different winner.

EXIT CODE: a window whose PLANE-SELECTED read differs from the reference makes
the script exit non-zero — the reference is then not what `czifile` reads, and
the run must fail. A BARE-roi difference is reported and is not fatal, because
a bare `roi=` takes `czifile`'s cached spatial-index path, which composites in
another order than `czifile`'s own composition path; it disagrees wherever two
tiles overlap and the two orders pick different winners, which says nothing
about the reference.

Run: python crosscheck_ref_czi.py PATH_TO_CZI REF_DIR [--c N] [--z N] [--t N]
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import czifile
import numpy as np


def read_window_bare(path, region):
    """The window's pixels, read by czifile with a BARE roi (no selection)."""
    return czifile.imread(path, roi=(region["x"], region["y"], region["w"], region["h"]),
                          maxworkers=1)


def available_dims(path) -> set:
    """The dimension names the file actually has (a CZI need not have Z or T)."""
    with czifile.CziFile(path) as czi:
        scene = next(iter(czi.scenes.values()))
        return set(scene._raw_sizes)


def compare_region(path, ref_dir, region, dims, defaults) -> tuple[bool, bool]:
    """Compare one region with the dump, by both reads.

    Returns whether the plane-selected read and the bare-roi read match the
    dump.
    """
    expected = np.load(ref_dir / region["npy_path"])
    # The plane of THIS region, when the manifest names it (the owner's
    # mosaic has one plane per region); otherwise the one from the command
    # line. Only the dimensions the file HAS are selected.
    spec = region.get("plane", {})
    values = {"C": spec.get("c_value", defaults.c), "Z": spec.get("z_value", defaults.z),
              "T": spec.get("t_value", defaults.t)}
    selection = {k: v for k, v in values.items() if k in dims}
    got = czifile.imread(path,
                         roi=(region["x"], region["y"], region["w"], region["h"]),
                         maxworkers=1, **selection)
    bare = read_window_bare(path, region)
    same = got.shape == expected.shape and np.array_equal(got, expected)
    bare_same = bare.shape == expected.shape and np.array_equal(bare, expected)
    # A bare read keeps the dimensions the selection would have squeezed
    # away, so its shape can differ from the reference's; that is a shape
    # difference, not a pixel difference.
    note = "" if bare.shape == expected.shape else f" bare shape {bare.shape}"
    print(f"  {region['name']}: plane-selected equal={same} "
          f"bare-roi equal={bare_same} tiles={region.get('tile_count', '?')}{note}")
    return same, bare_same


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Cross-check a CZI reference set.")
    parser.add_argument("path", help="the .czi file to read")
    parser.add_argument("ref", help="the reference folder (with manifest.json)")
    parser.add_argument("--c", type=int, default=0, help="the C value (default: 0)")
    parser.add_argument("--z", type=int, default=0, help="the Z value (default: 0)")
    parser.add_argument("--t", type=int, default=0, help="the T value (default: 0)")
    args = parser.parse_args(argv)

    ref_dir = Path(args.ref)
    manifest = json.loads((ref_dir / "manifest.json").read_text(encoding="utf-8"))
    regions = manifest["regions"]
    dims = available_dims(args.path)
    selected = 0
    bare_equal = 0
    differing = []
    for region in regions:
        same, bare_same = compare_region(args.path, ref_dir, region, dims, args)
        selected += 1 if same else 0
        bare_equal += 1 if bare_same else 0
        if not same:
            differing.append(region["name"])
    print(f"plane-selected: {selected}/{len(regions)} equal; "
          f"bare-roi: {bare_equal}/{len(regions)} equal")
    if differing:
        print(f"the reference does NOT match czifile's own plane-selected read "
              f"for: {', '.join(differing)}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
