# Third-Party Notices — `phasit-imread`

This crate reproduces structural knowledge and algorithms from permissively-licensed
(BSD) reference readers, and it statically links a number of permissively-licensed
Rust crates. The BSD-3-Clause condition to reproduce copyright notices and
disclaimers applies to any redistribution; this file satisfies it.

No GPL-, LGPL-, or otherwise copyleft-licensed code is used, translated, or
linked by this crate. See the disclaimer at the end.

---

## 1. Ported reference implementations (algorithms / binary-format knowledge)

`phasit-imread` implements its microscopy-format readers in original Rust. Where
a proprietary container has no complete public specification, the layout facts
and decoding algorithms were learned from the following **BSD-3-Clause** reference
readers by Christoph Gohlke. No source code was copied from them; the attribution
travels with the derived knowledge as required by the license.

- **tifffile** — https://github.com/cgohlke/tifffile
  Reference for OME-TIFF series/`DimensionOrder` handling and for the Zeiss LSM
  layout (`CZ-LSM-Info` tag 34412 field offsets, reduced-resolution page
  filtering, planar strip-per-channel layout). Used by `src/tiff_reader.rs`, the
  `src/tiff_ome*.rs` modules and `src/lsm_reader.rs`.

- **liffile** — https://github.com/cgohlke/liffile
  Reference for the Leica LIF container (memory-block framing, UTF-16 XML header,
  per-dimension `BytesInc` plane-offset algorithm). Used by `src/lif_reader.rs`.
  This crate's LIF reader is **not** derived from `readlif` (GPL-3.0) or from
  Bio-Formats `formats-gpl` — see the disclaimer below.

- **czifile** — https://github.com/cgohlke/czifile
  Reference for the Zeiss CZI / ZISRAW container (segment framing, subblock
  directory, the `max(256, 16 + storage_size)` subblock pixel-offset rule, the
  `DirectoryEntryDV` / `DimensionEntryDV1` field layouts, and the Zstd1
  header / Hi-Lo byte-unpack preprocessing). Used by `src/czi_reader.rs`.

The `czifile`, `tifffile`, and `liffile` references above are each licensed under
Christoph Gohlke's BSD-3-Clause terms; the three upstream notices and the shared
terms are reproduced below.

Additionally:

- **nd2** — https://github.com/tlambert03/nd2
  Copyright (c) Talley Lambert, **BSD-3-Clause**. Reference for the Nikon ND2
  container (chunk framing + chunkmap, the `CLxLiteVariant` binary metadata
  format, `load_attributes` / `load_metadata` / `load_experiment` parsing, and
  the `_util.loop_indices` coordinate mapping). Used by `src/nd2_reader.rs`.

  Most of the port is layout and decoding facts: no source code was copied. Two
  parts are more than that, and both are Rust re-implementations of `nd2`
  functions, BSD-3-Clause, Copyright (c) Talley Lambert:

  - the experiment-loop parser (`load_experiment_rec` and its helpers
    `single_experiment_loop`, `xy_point_count` and `as_loop_params` in
    `src/nd2_reader.rs`), derived from `nd2`'s `load_experiment`
    (`nd2/src/nd2/_parse/_parse.py`);
  - the attribute and channel parsers (`parse_attributes` and `parse_channels` in
    `src/nd2_reader.rs`), derived from `nd2`'s `load_attributes` and
    `load_metadata`.

  Their own copyright notice and licence text are reproduced below.

The Gohlke references are licensed under the following terms. Each upstream
project states its own copyright notice and the years differ; the terms
themselves are word-identical in the three LICENSE files, so they are reproduced
once, under all three notices.

| Reference | Copyright notice, as stated by that project's LICENSE |
|---|---|
| `tifffile` | `Copyright (c) 2008-2026, Christoph Gohlke` |
| `czifile` | `Copyright (c) 2013-2026, Christoph Gohlke` |
| `liffile` | `Copyright (c) 2023-2026, Christoph Gohlke` |

```
Copyright (c) 2008-2026, Christoph Gohlke
Copyright (c) 2013-2026, Christoph Gohlke
Copyright (c) 2023-2026, Christoph Gohlke
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

The `nd2` reference is licensed under the following terms:

```
Copyright (c) 2021 Talley Lambert

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

> The parity test harness under `tests/parity/` invokes `tifffile` and `liffile`
> at development time only to produce reference pixel dumps. It is never built
> into the crate; the scripts ship in the source distribution, where they need
> Python packages the crate does not depend on.

> The fixture generators under `tests/fixtures/` invoke `imagecodecs`
> (BSD-3-Clause, https://github.com/cgohlke/imagecodecs) at development time
> only, to encode the reference planes the Rust tests decode. `imagecodecs` is
> never built into the crate; the generator scripts ship in the source
> distribution.

---

## 2. Bundled Rust dependencies (statically linked)

All direct and transitive dependencies of this crate are permissively licensed
(MIT / Apache-2.0 / BSD / Unlicense / Zlib / 0BSD / Unicode-3.0). None are
copyleft. The licence strings below are the ones each crate declares in its own
`Cargo.toml` in the cargo registry.

The tables below cover every target this crate is built for: Windows
(`x86_64-pc-windows-msvc`), Linux (`x86_64-unknown-linux-gnu`) and macOS
(`x86_64-apple-darwin`, `aarch64-apple-darwin`). The four graphs are identical
except for one crate: the unix targets additionally resolve `libc`, which
`memmap2` pulls in for its platform layer. `libc` is therefore listed in the
runtime table, and that row therefore applies to the unix targets.

### Runtime dependencies (default build)

`cargo tree -p phasit-imread -e normal --prefix none`

| Crate | Version | License (as declared) |
|---|---|---|
| `byteorder` | 1.5.0 | Unlicense OR MIT |
| `flate2` | 1.1.9 | MIT OR Apache-2.0 |
| `crc32fast` | 1.5.0 | MIT OR Apache-2.0 |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 |
| `miniz_oxide` | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| `simd-adler32` | 0.3.9 | MIT |
| `memmap2` | 0.9.11 | MIT OR Apache-2.0 |
| `libc` | 0.2.186 | MIT OR Apache-2.0 |
| `quick-xml` | 0.41.0 | MIT |
| `memchr` | 2.8.2 | Unlicense OR MIT |
| `ruzstd` | 0.8.3 | MIT |
| `twox-hash` | 2.1.2 | MIT |
| `thiserror` | 2.0.18 | MIT OR Apache-2.0 |
| `thiserror-impl` | 2.0.18 | MIT OR Apache-2.0 |
| `proc-macro2` | 1.0.106 | MIT OR Apache-2.0 |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| `quote` | 1.0.46 | MIT OR Apache-2.0 |
| `syn` | 2.0.118 | MIT OR Apache-2.0 |
| `tiff` | 0.11.3 | MIT |
| `fax` | 0.2.7 | MIT |
| `half` | 2.7.1 | MIT OR Apache-2.0 |
| `zerocopy` | 0.8.52 | BSD-2-Clause OR Apache-2.0 OR MIT |
| `zerocopy-derive` | 0.8.52 | BSD-2-Clause OR Apache-2.0 OR MIT |
| `quick-error` | 2.0.1 | MIT/Apache-2.0 |
| `weezl` | 0.1.12 | MIT OR Apache-2.0 |
| `zune-jpeg` | 0.5.15 | MIT OR Apache-2.0 OR Zlib |
| `zune-core` | 0.5.1 | MIT OR Apache-2.0 OR Zlib |

### Optional: the `ndarray` feature

`cargo tree -p phasit-imread --all-features -e normal --prefix none`

| Crate | Version | License (as declared) |
|---|---|---|
| `ndarray` | 0.17.2 | MIT OR Apache-2.0 |
| `matrixmultiply` | 0.3.10 | MIT/Apache-2.0 |
| `rawpointer` | 0.2.1 | MIT/Apache-2.0 |
| `num-complex` | 0.4.6 | MIT OR Apache-2.0 |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 |
| `num-integer` | 0.1.46 | MIT OR Apache-2.0 |

### Build dependencies

Crates that a build script compiles while this crate is built. They never end up
linked into the resulting binary. They enter the graph through two features:
`jpegxr` (which compiles the vendored jxrlib) and `ndarray` (whose
`matrixmultiply` and `num-traits` build scripts need `autocfg`).

`cargo tree -p phasit-imread --all-features -e normal,build --prefix none`
lists them together with the runtime crates; the build-only ones are the crates
that this adds over
`cargo tree -p phasit-imread --all-features -e normal --prefix none`.

| Crate | Version | License (as declared) | Reached through |
|---|---|---|---|
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT | `ndarray`: build dependency of `matrixmultiply` and `num-traits` |
| `cc` | 1.2.65 | MIT OR Apache-2.0 | `jpegxr`: build dependency of this crate |
| `find-msvc-tools` | 0.1.9 | MIT OR Apache-2.0 | `jpegxr`: dependency of `cc` |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | `jpegxr`: dependency of `cc` |

### Development dependencies only

Crates that are compiled only while the test suite is built; they are never linked
into a shipped binary. The set is the development graph minus the shipped graph:
every crate of
`cargo tree -p phasit-imread -e normal,dev,build --all-features --prefix none`
that is absent from
`cargo tree -p phasit-imread --all-features -e normal,build --prefix none`.

| Crate | Version | License (as declared) |
|---|---|---|
| `zlib-rs` | 0.6.5 | Zlib |
| `ndarray-npy` | 0.10.0 | MIT OR Apache-2.0 |
| `py_literal` | 0.4.0 | MIT OR Apache-2.0 |
| `num-bigint` | 0.4.6 | MIT OR Apache-2.0 |
| `pest` | 2.8.6 | MIT OR Apache-2.0 |
| `ucd-trie` | 0.1.7 | MIT OR Apache-2.0 |
| `pest_derive` | 2.8.6 | MIT OR Apache-2.0 |
| `pest_generator` | 2.8.6 | MIT OR Apache-2.0 |
| `pest_meta` | 2.8.6 | MIT OR Apache-2.0 |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 |
| `digest` | 0.10.7 | MIT OR Apache-2.0 |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 |
| `generic-array` | 0.14.7 | MIT |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 |
| `version_check` | 0.9.5 | MIT/Apache-2.0 |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 |
| `zip` | 6.0.0 | MIT |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 |
| `zopfli` | 0.8.3 | Apache-2.0 |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 |
| `log` | 0.4.33 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.150 | MIT OR Apache-2.0 |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 |
| `serde_core` | 1.0.228 | MIT OR Apache-2.0 |
| `zmij` | 1.0.21 | MIT |

`zlib-rs` is in the development graph only: the dev-dependency `ndarray-npy`
pulls in `zip`, and `zip`'s default features enable `flate2`'s `zlib-rs` backend,
so the test builds compile and link it. A shipped build never resolves `flate2`
to `zlib-rs` — there it stays on the pure-Rust `miniz_oxide` backend, which is
the crate listed in the runtime table above.

The `sha2` / `digest` / `block-buffer` / `generic-array` / `typenum` /
`crypto-common` / `cpufeatures` / `version_check` chain is a build-time
dependency of `pest_meta`, reached through `ndarray-npy`; it is compiled while
the tests are built and is never linked into the shipped binary.

### Licence texts

This file names every dependency and the licence it declares. It does not
reproduce the full text of every licence, and nothing in it is generated at build
time — the crate's `build.rs` only compiles the vendored jxrlib.

The BSD texts are reproduced in full above (the Gohlke readers and the `nd2`
reader in section 1, jxrlib in section 3), because the BSD conditions require a
binary distribution to reproduce the copyright notice, the conditions and the
disclaimer.

The MIT, Apache-2.0, BSD-2-Clause, Zlib, 0BSD, Unlicense and Unicode-3.0 texts
of the bundled Rust crates are not reproduced here; the MIT and Apache-2.0 texts
are the canonical unmodified OSI templates, and each text ships with the
published crate it belongs to.

A redistributor of a binary built from this crate must ship, with the binary:

- this crate's `LICENSE` and `NOTICE`;
- the BSD texts reproduced above;
- the text of each licence that applies to the crates the binary statically
  links, as listed in the tables of section 2 — one copy of a licence covers
  every crate under it.

The `jpegxr` feature adds the jxrlib text of section 3 to that list. The
development-only crates of section 2 are never linked into the binary and need
no text.

This repository ships the crate's source only, and does not bundle the licence
texts of the permissively-licensed Rust crates; a redistributor of a binary
built from it collects those texts itself. `cargo about` generates that bundle
from the crates the binary links.

---

## 3. Vendored & statically linked C library — jxrlib (feature `jpegxr`)

When built with the opt-in `jpegxr` cargo feature, this crate compiles and
statically links a vendored copy of **jxrlib** (the JPEG XR reference
implementation / Device Porting Kit) to decode CZI JpegXR-compressed subblocks
(`compression id 4`). The specific tree is the **glencoesoftware fork** of
jxrlib, vendored under `vendor/jxrlib/`. It is licensed under the Microsoft
**BSD-style (2-clause)** license reproduced below (from the source headers, e.g.
`vendor/jxrlib/jxrgluelib/JXRGlue.h`; see also `vendor/jxrlib/doc/readme.txt`).
The same text, with its source named, is the file `vendor/jxrlib/LICENSE`.
Default builds do NOT include the `jpegxr` feature and link none of this code.

```
Copyright © Microsoft Corp.
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

- Redistributions of source code must retain the above copyright notice,
  this list of conditions and the following disclaimer.
- Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

---

## 4. Disclaimer — copyleft sources NOT used

For the Leica LIF format, the GPL-3.0 reader `readlif` and the Bio-Formats
`formats-gpl` module were **not** read line-by-line, translated, or linked. They
were, at most, consulted at a documentation level for format understanding. The
LIF reader in this crate is an independent implementation informed by the
BSD-licensed `liffile` reference and the openly-documented container layout. No
copyleft-licensed code is present in, derived into, or linked by `phasit-imread`.
