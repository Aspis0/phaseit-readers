"""Dev-only: extract a few decoded planes from the real Nikon ND2 sample as
native-endian raw bytes, so the pure-Rust ND2 reader can be checked byte-exact
against the `nd2` (BSD-3, Lambert) reference decode — WITHOUT committing the
411 MB third-party file or the multi-MB plane dumps (both gitignored).

Emits, into tests/fixtures/nd2_real/ :
  nd2_plane_s{scene}_t{t}_c{c}_z{z}.raw  -- the exact native-LE bytes phasit's
                                            reader.plane(scene,t,c,z).data must equal
  manifest_real.json                     -- geometry + list of dumped planes

Frames in ND2 are laid out (Y, X, C) interleaved; nd2.asarray() returns the
de-interleaved per-component plane, which is precisely what phasit returns.
Run:  python dump_ref_nd2.py PATH_TO_ND2
"""
import json, os, sys
import numpy as np
import nd2

SRC = sys.argv[1]
OUT = os.path.dirname(os.path.abspath(__file__))

f = nd2.ND2File(SRC)
sizes = dict(f.sizes)              # e.g. {'Z':25,'Y':2868,'X':2868}
nT = sizes.get("T", 1)
nC = f.attributes.componentCount
nZ = sizes.get("Z", 1)
Y, X = sizes["Y"], sizes["X"]
dtype = str(f.dtype)              # 'uint16'
arr = f.asarray()                 # shape follows f.sizes order (no leading 1s)

# Map the numpy array to (t,c,z) lookups. This sample is single-scene, single-
# channel, Z-stack: arr shape (Z, Y, X). Handle the general case by using the
# ordered axis labels from f.sizes (minus Y/X) plus the component axis.
axis_labels = [k for k in sizes if k not in ("Y", "X")]  # outer->inner
print(f"sizes={sizes} nT={nT} nC={nC} nZ={nZ} dtype={dtype} arr={arr.shape} axes={axis_labels}")

def plane_of(t, c, z):
    """Return the (Y,X) plane for coord (t,c,z) from `arr`, whatever its axes."""
    idx = []
    for lab in axis_labels:
        if lab == "T":
            idx.append(t)
        elif lab == "Z":
            idx.append(z)
        elif lab == "C":
            idx.append(c)
        else:
            idx.append(0)  # positions/other -> first
    sub = arr[tuple(idx)] if idx else arr
    # if there's a trailing component axis (multi-channel interleaved), select c
    if sub.ndim == 3:
        sub = sub[..., c]
    assert sub.shape == (Y, X), f"{sub.shape} != {(Y, X)}"
    return sub

# choose coords to dump: first, middle, last z of (t=0,c=0)
zs = sorted({0, nZ // 2, nZ - 1})
planes = []
for z in zs:
    p = np.ascontiguousarray(plane_of(0, 0, z))
    name = f"nd2_plane_s0_t0_c0_z{z}.raw"
    p.tofile(os.path.join(OUT, name))
    planes.append({"name": name, "scene": 0, "t": 0, "c": 0, "z": z,
                   "y": Y, "x": X, "dtype": dtype, "samples": 1,
                   "first8": p.ravel()[:8].tolist()})
    print(f"  wrote {name}  first8={p.ravel()[:8].tolist()}")

manifest = {"source": os.path.basename(SRC), "sizes": sizes, "nT": nT, "nC": nC,
            "nZ": nZ, "Y": Y, "X": X, "dtype": dtype, "planes": planes}
json.dump(manifest, open(os.path.join(OUT, "manifest_real.json"), "w"), indent=2)
print("wrote manifest_real.json")
f.close()
