#!/usr/bin/env python
"""Generate JPEG-XR fixtures for the CZI JpegXR (compression id 4) decode path.

Uses imagecodecs (BSD) — the same library that decodes real CZI JpegXR subblocks
in the reference tooling — to LOSSLESSLY encode a handful of small known arrays
(`level=1.0` == lossless). For each case we dump:
  <name>.jxr  = the JPEG-XR codestream (what a CZI subblock would store)
  <name>.raw  = the expected decode output as little-endian bytes (ground truth)
and record dtype / samples / stored_x / stored_y in manifest.json.

The Rust side (`src/jpegxr_ffi.rs`, `#[cfg(feature = "jpegxr")]`) feeds each
<name>.jxr through `decode_jpegxr` and asserts byte-equality with <name>.raw.

Channel order (the load-bearing part for color): `decode_jpegxr` asks jxrlib for
`24bppBGR` output, so for a color tile it returns interleaved **BGR** bytes — the
CZI reader swaps BGR->RGB afterwards. imagecodecs treats a 3-channel array as RGB
(verified below by an exact round-trip), so the ground truth for OUR decoder is
the RGB array with its channels reversed to BGR (`arr[..., ::-1]`). This verifies
the "native BGR, swap happens later" contract end to end.

Gray32Float is intentionally NOT emitted: JPEG-XR float encoding is lossy even at
level=1.0, so it cannot be asserted byte-exact. That code path exists but is not
fixture-verified (see decode_jpegxr).

Run:  python gen_jpegxr_fixtures.py    (writes fixtures next to this script)
"""
import json
import os

import numpy as np
import imagecodecs

HERE = os.path.dirname(os.path.abspath(__file__))


def gray_cases():
    # Odd, non-square, non-16-multiple dims stress jxrlib's 16x16 macroblock
    # cropping in the format converter's Copy.
    # gray8: a wrapping ramp.
    g8 = (np.arange(24 * 37, dtype=np.uint8) % 251).reshape(24, 37)
    yield "gray8_ramp", g8, "u8"

    # gray16: a 16-bit ramp (values span the full range via *257).
    g16 = ((np.arange(19 * 41, dtype=np.uint32) * 257) & 0xFFFF).astype(np.uint16)
    g16 = g16.reshape(19, 41)
    yield "gray16_gradient", g16, "u16"

    # gray16 extremes: 0 / max / high-byte / low-byte, to catch byte-order bugs.
    row = np.array([0x0000, 0xFFFF, 0x00FF, 0xFF00, 0x1234, 0xABCD] * 6, np.uint16)
    g16e = np.tile(row[:32], (16, 1))  # 16x32
    yield "gray16_extremes", np.ascontiguousarray(g16e), "u16"


def color_case():
    # Bgr24: distinct per-channel bases so a wrong channel order is unmistakable,
    # plus a few unique pixels. Array is RGB (axis 0=R, 1=G, 2=B).
    h, w = 12, 20
    rgb = np.zeros((h, w, 3), np.uint8)
    rgb[..., 0] = 0x11  # R
    rgb[..., 1] = 0x22  # G
    rgb[..., 2] = 0x33  # B
    rgb[0, 0] = (0xDE, 0xAD, 0xBE)
    rgb[h - 1, w - 1] = (0x01, 0x02, 0x03)
    rgb[5, 7] = (0xFF, 0x00, 0x80)
    return "bgr24_channels", rgb


def main():
    manifest = []

    for name, arr, dtype in gray_cases():
        arr = np.ascontiguousarray(arr)
        enc = imagecodecs.jpegxr_encode(arr, level=1.0)  # lossless
        dec = imagecodecs.jpegxr_decode(enc).reshape(arr.shape)
        assert np.array_equal(arr, dec), f"{name}: python round-trip not lossless"
        raw = arr.tobytes()  # C-order little-endian (host is LE)
        _write(name, enc, raw, dtype, 1, arr.shape[1], arr.shape[0], manifest,
               note="grayscale; raw == decode output directly")

    name, rgb = color_case()
    rgb = np.ascontiguousarray(rgb)
    enc = imagecodecs.jpegxr_encode(rgb, level=1.0)
    dec = imagecodecs.jpegxr_decode(enc)
    assert np.array_equal(rgb, dec), f"{name}: imagecodecs is not RGB-ordered as assumed"
    # decode_jpegxr returns native BGR, so ground truth = channels reversed.
    bgr = np.ascontiguousarray(rgb[..., ::-1])
    raw = bgr.tobytes()
    _write(name, enc, raw, "u8", 3, rgb.shape[1], rgb.shape[0], manifest,
           note="color; raw is native BGR (RGB reversed) — the CZI reader swaps to RGB later")

    doc = {
        "generator": "gen_jpegxr_fixtures.py",
        "note": "Every fixture is an imagecodecs JPEG-XR codestream; the .raw file is the expected decode.",
        "fixtures": manifest,
    }
    with open(os.path.join(HERE, "manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=2)
    print(f"wrote {len(manifest)} fixtures + manifest.json to {HERE}")


def _write(name, enc, raw, dtype, samples, stored_x, stored_y, manifest, note):
    enc = bytes(enc)
    with open(os.path.join(HERE, name + ".jxr"), "wb") as fh:
        fh.write(enc)
    with open(os.path.join(HERE, name + ".raw"), "wb") as fh:
        fh.write(raw)
    manifest.append({
        "name": name,
        "dtype": dtype,
        "samples": samples,
        "stored_x": stored_x,
        "stored_y": stored_y,
        "jxr": name + ".jxr",
        "raw": name + ".raw",
        "jxr_size": len(enc),
        "raw_size": len(raw),
        "note": note,
    })
    print(f"  {name:20s} jxr={len(enc):6d}  raw={len(raw):6d}  "
          f"{dtype} x{samples}  {stored_x}x{stored_y}")


if __name__ == "__main__":
    main()
