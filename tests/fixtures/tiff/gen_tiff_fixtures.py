"""Generate the TIFF fixtures the phasit-imread variant tests read.

One responsibility: write small, exactly-tagged TIFF files for the variants a
real corpus contains — a per-sample `SampleFormat`, a palette, an ImageJ
hyperstack, a multi-scene OME-TIFF — plus the manifest that says what each
fixture states and what a reader must make of it.

The files are written by hand (a few dozen lines of struct packing) because the
variants are exactly the ones a general TIFF writer normalises away: a writer
that emits one `SampleFormat` value instead of one per sample cannot produce
the file this reader has to survive.

Run:  python gen_tiff_fixtures.py
"""

import json
import os
import struct

BYTE, ASCII, SHORT, LONG, RATIONAL = 1, 2, 3, 4, 5

HERE = os.path.dirname(os.path.abspath(__file__))


# --- the TIFF writer --------------------------------------------------------


def _pack(typ, values):
    """The bytes one tag value holds."""
    if typ == ASCII:
        # UTF-8: a real OME writer puts the micro sign in the XML as it is.
        return values.encode("utf-8") + b"\0"
    if typ == BYTE:
        return bytes(values)
    if typ == SHORT:
        return b"".join(struct.pack("<H", v) for v in values)
    if typ == LONG:
        return b"".join(struct.pack("<I", v) for v in values)
    if typ == RATIONAL:
        return b"".join(struct.pack("<II", n, d) for n, d in values)
    raise ValueError("unknown tag type %r" % typ)


def write_tiff(path, pages):
    """Write a classic little-endian TIFF.

    `pages` is a list of dicts: `width`, `height`, `data` (the strip bytes) and
    `tags` (a dict of tag code -> (type, values)). The tags the writer needs
    (width, height, strip offsets, ...) are added here.
    """
    entries = []
    for page in pages:
        tags = dict(page.get("tags", {}))
        tags[256] = (LONG, [page["width"]])            # ImageWidth
        tags[257] = (LONG, [page["height"]])           # ImageLength
        tags[273] = (LONG, [0])                        # StripOffsets (filled in)
        tags[278] = (LONG, [page["height"]])           # RowsPerStrip
        tags[279] = (LONG, [len(page["data"])])        # StripByteCounts
        entries.append(tags)

    # Layout: header, then the IFDs, then the values too big to sit in the
    # IFD entry itself, then the strip data.
    ifd_count = len(entries)
    ifd_offsets = []
    cursor = 8
    for tags in entries:
        ifd_offsets.append(cursor)
        cursor += 2 + 12 * len(tags) + 4

    out_of_line = []      # (page index, tag code, bytes)
    value_cursor = cursor
    for index, tags in enumerate(entries):
        for code, (typ, values) in sorted(tags.items()):
            packed = _pack(typ, values)
            if len(packed) > 4:
                out_of_line.append((index, code, value_cursor))
                value_cursor += len(packed) + (len(packed) % 2)
    strip_cursor = value_cursor
    strips = []
    for page in pages:
        strips.append(strip_cursor)
        strip_cursor += len(page["data"])

    out = bytearray()
    out += b"II" + struct.pack("<H", 42) + struct.pack("<I", ifd_offsets[0])
    extra = {}
    offsets = {}
    for index, code, offset in out_of_line:
        typ, values = entries[index][code]
        extra[(index, code)] = bytearray(_pack(typ, values))
        offsets[(index, code)] = offset
    for index, tags in enumerate(entries):
        entries[index][273] = (LONG, [strips[index]])

    for index, tags in enumerate(entries):
        out += struct.pack("<H", len(tags))
        for code, (typ, values) in sorted(tags.items()):
            packed = _pack(typ, values)
            # An ASCII tag counts BYTES (a UTF-8 character is two of them).
            count = len(packed) if typ == ASCII else len(values)
            if len(packed) > 4:
                out += struct.pack("<HHI", code, typ, count) + struct.pack("<I", offsets[(index, code)])
            else:
                out += struct.pack("<HHI", code, typ, count) + packed.ljust(4, b"\0")
        next_ifd = ifd_offsets[index + 1] if index + 1 < ifd_count else 0
        out += struct.pack("<I", next_ifd)

    for index, code, _offset in out_of_line:
        out += extra[(index, code)]
        if len(extra[(index, code)]) % 2:
            out += b"\0"

    for page in pages:
        out += page["data"]

    with open(path, "wb") as handle:
        handle.write(bytes(out))
    return path


# --- the fixtures -----------------------------------------------------------


def palette_8bit():
    """An indexed page: the pixel is an index into a 256-entry palette."""
    width, height = 4, 3
    data = bytes((y * width + x) % 256 for y in range(height) for x in range(width))
    # A grey ramp: index i -> (i, i, i).
    ramp = [(i << 8, i << 8, i << 8) for i in range(256)]
    # The ColorMap tag holds three RUNS: every red, then every green, then
    # every blue.
    colormap = [r for (r, _g, _b) in ramp] + [g for (_r, g, _b) in ramp] + [
        b for (_r, _g, b) in ramp
    ]
    return write_tiff(os.path.join(HERE, "palette-8bit.tif"), [{
        "width": width,
        "height": height,
        "data": data,
        "tags": {
            258: (SHORT, [8]),          # BitsPerSample
            259: (SHORT, [1]),          # Compression: none
            262: (SHORT, [3]),          # PhotometricInterpretation: RGBPalette
            277: (SHORT, [1]),          # SamplesPerPixel
            320: (SHORT, colormap),     # ColorMap
        },
    }]), {
        "file": "palette-8bit.tif",
        "what": "one indexed page: the pixel is an index into a 256-entry palette",
        "scenes": 1,
        "dims": {"t": 1, "c": 1, "z": 1, "y": 3, "x": 4},
        "dtype": "U8",
        "samples": 1,
        "paletteEntries": 256,
        "planes": [{"t": 0, "c": 0, "z": 0, "hex": data.hex()}],
    }


def rgb_per_sample_format():
    """An RGB page whose SampleFormat is stated once PER SAMPLE."""
    width, height = 4, 3
    data = bytes((x * 16 + y) % 256 for y in range(height) for x in range(width * 3))
    return write_tiff(os.path.join(HERE, "rgb-per-sampleformat.tif"), [{
        "width": width,
        "height": height,
        "data": data,
        "tags": {
            258: (SHORT, [8, 8, 8]),    # BitsPerSample, once per sample
            259: (SHORT, [1]),          # Compression: none
            262: (SHORT, [2]),          # PhotometricInterpretation: RGB
            277: (SHORT, [3]),          # SamplesPerPixel
            284: (SHORT, [1]),          # PlanarConfiguration: chunky
            339: (SHORT, [1, 1, 1]),    # SampleFormat, once per sample
        },
    }]), {
        "file": "rgb-per-sampleformat.tif",
        "what": "an RGB page whose SampleFormat is stated once per sample (the shape the tiff crate refuses)",
        "scenes": 1,
        "dims": {"t": 1, "c": 1, "z": 1, "y": 3, "x": 4},
        "dtype": "U8",
        "samples": 3,
        "planes": [{"t": 0, "c": 0, "z": 0, "hex": data.hex()}],
    }


def rgba_8bit():
    """An RGB page with an alpha sample: four samples in one pixel."""
    width, height = 4, 3
    data = bytes((x * 8 + y) % 256 for y in range(height) for x in range(width * 4))
    return write_tiff(os.path.join(HERE, "rgba-8bit.tif"), [{
        "width": width,
        "height": height,
        "data": data,
        "tags": {
            258: (SHORT, [8]),
            259: (SHORT, [1]),
            262: (SHORT, [2]),
            277: (SHORT, [4]),
            284: (SHORT, [1]),
        },
    }]), {
        "file": "rgba-8bit.tif",
        "what": "an RGB page with an alpha sample: four samples in one pixel",
        "scenes": 1,
        "dims": {"t": 1, "c": 1, "z": 1, "y": 3, "x": 4},
        "dtype": "U8",
        "samples": 4,
        "planes": [{"t": 0, "c": 0, "z": 0, "hex": data.hex()}],
    }


def imagej_hyperstack():
    """An ImageJ hyperstack: 2 channels x 2 slices, channel-first."""
    width, height = 4, 3
    pages = []
    # ImageJ order: page = c + z * channels.
    for z in range(2):
        for c in range(2):
            data = bytes((y * width + x + 10 * c + 100 * z) % 256
                         for y in range(height) for x in range(width))
            pages.append({"width": width, "height": height, "data": data, "tags": {}})
    description = (
        "ImageJ=1.54p\nimages=4\nchannels=2\nslices=2\nhyperstack=true\n"
        "mode=composite\nunit=\\u00B5m\nspacing=1.5\nloop=false\n"
    )
    for page in pages:
        page["tags"] = {
            258: (SHORT, [8]),
            259: (SHORT, [1]),
            262: (SHORT, [1]),
            270: (ASCII, description),
            # 6666.7 pixels per centimetre = 1.5 um per pixel.
            282: (RATIONAL, [(66667, 10)]),
            283: (RATIONAL, [(66667, 10)]),
            296: (SHORT, [3]),
        }
    return write_tiff(os.path.join(HERE, "imagej-hyperstack.tif"), pages), {
        "file": "imagej-hyperstack.tif",
        "what": "an ImageJ hyperstack: 2 channels x 2 slices, stored channel-first",
        "scenes": 1,
        "dims": {"t": 1, "c": 2, "z": 2, "y": 3, "x": 4},
        "dtype": "U8",
        "samples": 1,
        "pxSizeUm": {"x": 1.5, "y": 1.5, "z": 1.5},
        "planes": [
            {"t": 0, "c": 0, "z": 0, "hex": pages[0]["data"].hex()},
            {"t": 0, "c": 1, "z": 0, "hex": pages[1]["data"].hex()},
            {"t": 0, "c": 0, "z": 1, "hex": pages[2]["data"].hex()},
            {"t": 0, "c": 1, "z": 1, "hex": pages[3]["data"].hex()},
        ],
    }


def two_series_ome():
    """An OME-TIFF with two scenes: the second scene's plane is the second IFD."""
    width, height = 4, 3
    first = struct.pack("<12H", *[(x * 100 + y) % 65536 for y in range(height) for x in range(width)])
    second = bytes((x + y) % 256 for y in range(2) for x in range(2))
    description = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">\n'
        '  <Image ID="Image:0" Name="first scene">\n'
        '    <Pixels ID="Pixels:0" DimensionOrder="XYCZT" Type="uint16"'
        ' SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1"'
        ' PhysicalSizeX="0.5" PhysicalSizeXUnit="\u00b5m"'
        ' PhysicalSizeY="0.5" PhysicalSizeYUnit="\u00b5m">\n'
        '      <Channel ID="Channel:0:0" Name="DAPI"/>\n'
        '      <TiffData IFD="0" PlaneCount="1"/>\n'
        '    </Pixels>\n'
        '  </Image>\n'
        '  <Image ID="Image:1" Name="second scene">\n'
        '    <Pixels ID="Pixels:1" DimensionOrder="XYCZT" Type="uint8"'
        ' SizeX="2" SizeY="2" SizeZ="1" SizeC="1" SizeT="1">\n'
        '      <TiffData IFD="1" PlaneCount="1"/>\n'
        '    </Pixels>\n'
        '  </Image>\n'
        '</OME>\n'
    )
    pages = [
        {"width": width, "height": height, "data": first, "tags": {
            258: (SHORT, [16]),
            259: (SHORT, [1]),
            262: (SHORT, [1]),
            270: (ASCII, description),
            339: (SHORT, [1]),
        }},
        {"width": 2, "height": 2, "data": second, "tags": {
            258: (SHORT, [8]),
            259: (SHORT, [1]),
            262: (SHORT, [1]),
            339: (SHORT, [1]),
        }},
    ]
    return write_tiff(os.path.join(HERE, "two-series.ome.tif"), pages), {
        "file": "two-series.ome.tif",
        "what": "an OME-TIFF with two scenes: the second scene's plane is the second IFD",
        "scenes": 2,
        "sceneDims": [
            {"t": 1, "c": 1, "z": 1, "y": 3, "x": 4, "dtype": "U16", "samples": 1},
            {"t": 1, "c": 1, "z": 1, "y": 2, "x": 2, "dtype": "U8", "samples": 1},
        ],
        "planes": [
            {"scene": 0, "t": 0, "c": 0, "z": 0, "hex": first.hex()},
            {"scene": 1, "t": 0, "c": 0, "z": 0, "hex": second.hex()},
        ],
    }


def refused_ome():
    """An OME-TIFF whose only scene states a pixel type the reader cannot model.

    The file is refused whole: the reader names the reason and reads no scene.
    """
    width, height = 4, 3
    data = bytes((x + y) % 256 for y in range(height) for x in range(width))
    description = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">\n'
        '  <Image ID="Image:0" Name="unreadable scene">\n'
        '    <Pixels ID="Pixels:0" DimensionOrder="XYCZT" Type="complex"'
        ' SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1">\n'
        '      <TiffData IFD="0" PlaneCount="1"/>\n'
        '    </Pixels>\n'
        '  </Image>\n'
        '</OME>\n'
    )
    pages = [
        {"width": width, "height": height, "data": data, "tags": {
            258: (SHORT, [8]),
            259: (SHORT, [1]),
            262: (SHORT, [1]),
            270: (ASCII, description),
            339: (SHORT, [1]),
        }},
    ]
    return write_tiff(os.path.join(HERE, "refused-ome.tif"), pages), {
        "file": "refused-ome.tif",
        "what": "an OME-TIFF whose only scene states a pixel type the reader does not model: the whole file is refused",
        "scenes": 0,
        "refused": [{
            "image_index": 0,
            "name": "unreadable scene",
            "reason": "OME PixelType \"complex\" not modelled by phasit-imread",
        }],
    }


def partly_refused_ome():
    """An OME-TIFF of two scenes: the first reads, the second is refused.

    The refusal holds its place in the file: the readable scene keeps its own
    index and the refused one is named with its reason.
    """
    width, height = 4, 3
    first = bytes((x + y) % 256 for y in range(height) for x in range(width))
    second = bytes((x * 2 + y) % 256 for y in range(height) for x in range(width))
    description = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">\n'
        '  <Image ID="Image:0" Name="readable scene">\n'
        '    <Pixels ID="Pixels:0" DimensionOrder="XYCZT" Type="uint8"'
        ' SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1">\n'
        '      <TiffData IFD="0" PlaneCount="1"/>\n'
        '    </Pixels>\n'
        '  </Image>\n'
        '  <Image ID="Image:1" Name="unreadable scene">\n'
        '    <Pixels ID="Pixels:1" DimensionOrder="XYCZT" Type="complex"'
        ' SizeX="4" SizeY="3" SizeZ="1" SizeC="1" SizeT="1">\n'
        '      <TiffData IFD="1" PlaneCount="1"/>\n'
        '    </Pixels>\n'
        '  </Image>\n'
        '</OME>\n'
    )
    pages = [
        {"width": width, "height": height, "data": first, "tags": {
            258: (SHORT, [8]),
            259: (SHORT, [1]),
            262: (SHORT, [1]),
            270: (ASCII, description),
            339: (SHORT, [1]),
        }},
        {"width": width, "height": height, "data": second, "tags": {
            258: (SHORT, [8]),
            259: (SHORT, [1]),
            262: (SHORT, [1]),
            339: (SHORT, [1]),
        }},
    ]
    return write_tiff(os.path.join(HERE, "partly-refused-ome.tif"), pages), {
        "file": "partly-refused-ome.tif",
        "what": "an OME-TIFF of two scenes: the first reads, the second states a pixel type the reader does not model and is refused",
        "scenes": 1,
        "dims": {"t": 1, "c": 1, "z": 1, "y": 3, "x": 4},
        "dtype": "U8",
        "samples": 1,
        "planes": [{"scene": 0, "t": 0, "c": 0, "z": 0, "hex": first.hex()}],
        "refused": [{
            "image_index": 1,
            "name": "unreadable scene",
            "reason": "OME PixelType \"complex\" not modelled by phasit-imread",
        }],
    }


def signed_and_float():
    """A signed 16-bit page and a float page: the other two SampleFormats."""
    width, height = 4, 3
    signed = struct.pack("<12h", *[(x - 6) * 100 for y in range(height) for x in range(width)])
    floats = struct.pack("<12f", *[(x + y) / 4.0 for y in range(height) for x in range(width)])
    int_path, int_entry = write_tiff(os.path.join(HERE, "int16-gray.tif"), [{
        "width": width, "height": height, "data": signed, "tags": {
            258: (SHORT, [16]), 259: (SHORT, [1]), 262: (SHORT, [1]), 339: (SHORT, [2]),
        },
    }]), {
        "file": "int16-gray.tif",
        "what": "a signed 16-bit page (SampleFormat=2)",
        "scenes": 1,
        "dims": {"t": 1, "c": 1, "z": 1, "y": 3, "x": 4},
        "dtype": "I16",
        "samples": 1,
        "planes": [{"t": 0, "c": 0, "z": 0, "hex": signed.hex()}],
    }
    float_path, float_entry = write_tiff(os.path.join(HERE, "float32-gray.tif"), [{
        "width": width, "height": height, "data": floats, "tags": {
            258: (SHORT, [32]), 259: (SHORT, [1]), 262: (SHORT, [1]), 339: (SHORT, [3]),
        },
    }]), {
        "file": "float32-gray.tif",
        "what": "a float page (SampleFormat=3, 32-bit)",
        "scenes": 1,
        "dims": {"t": 1, "c": 1, "z": 1, "y": 3, "x": 4},
        "dtype": "F32",
        "samples": 1,
        "planes": [{"t": 0, "c": 0, "z": 0, "hex": floats.hex()}],
    }
    return int_path, int_entry, float_path, float_entry


def main():
    entries = []
    for maker in (palette_8bit, rgb_per_sample_format, rgba_8bit, imagej_hyperstack):
        _path, entry = maker()
        entries.append(entry)
    _int_path, int_entry, _float_path, float_entry = signed_and_float()
    entries.append(int_entry)
    entries.append(float_entry)
    _ome_path, ome_entry = two_series_ome()
    entries.append(ome_entry)
    _refused_path, refused_entry = refused_ome()
    entries.append(refused_entry)
    _partly_path, partly_entry = partly_refused_ome()
    entries.append(partly_entry)

    manifest = {
        "generator": "gen_tiff_fixtures.py",
        "note": "Every fixture states exactly what its entry says; the reader must publish that.",
        "fixtures": entries,
    }
    with open(os.path.join(HERE, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=1)
        handle.write("\n")
    print("wrote %d fixtures" % len(entries))


if __name__ == "__main__":
    main()
