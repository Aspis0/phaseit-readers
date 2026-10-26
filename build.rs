// SPDX-License-Identifier: Apache-2.0
//! Build script for `phasit-imread`.
//!
//! It does exactly ONE thing, and only when the opt-in `jpegxr` feature is
//! enabled: compile the vendored jxrlib (glencoesoftware fork, BSD) C sources
//! into a static library that the `jpegxr_ffi` module links against for CZI
//! JPEG-XR (compression id 4) subblock decode.
//!
//! A default build (no `jpegxr`) skips this entirely and needs no C toolchain.
//!
//! The flags mirror the authoritative upstream `Makefile.reference`:
//!   -I. -Icommon/include -Iimage/sys [-Ijxrgluelib for glue] -D__ANSI__
//!   -DDISABLE_PERF_MEASUREMENT -w
//! `__ANSI__` makes the tree build without `windows.h` (using the vendored
//! GUID/SAL shims under common/include); `DISABLE_PERF_MEASUREMENT` compiles out
//! the QueryPerformanceCounter timers. imagecodecs uses the same two defines on
//! every platform, including MSVC.

fn main() {
    // Nothing to do unless the JPEG-XR backend was requested. Keeping the whole
    // C build behind the feature flag means a plain `cargo build` stays
    // pure-Rust and needs no cc/cl.exe on the machine.
    if std::env::var_os("CARGO_FEATURE_JPEGXR").is_none() {
        return;
    }

    let vendor = "vendor/jxrlib";

    // The 24 translation units that make up libjpegxr + libjxrglue. The x86
    // files (strdec_x86.c / strenc_x86.c) are compiled as their OWN translation
    // units: they define StrDecOpt/StrEncOpt (declared+called from strdec.c /
    // strenc.c). Without `WMP_OPT_SSE2` (which we never define) their bodies
    // compile to portable no-ops, so no SSE intrinsics or windows.h are pulled
    // in — but the symbols must still exist, hence building them here.
    let sources: &[&str] = &[
        // image/sys
        "image/sys/adapthuff.c",
        "image/sys/image.c",
        "image/sys/strcodec.c",
        "image/sys/strPredQuant.c",
        "image/sys/strTransform.c",
        "image/sys/perfTimerANSI.c",
        // image/decode
        "image/decode/decode.c",
        "image/decode/postprocess.c",
        "image/decode/segdec.c",
        "image/decode/strdec.c",
        "image/decode/strdec_x86.c",
        "image/decode/strInvTransform.c",
        "image/decode/strPredQuantDec.c",
        "image/decode/JXRTranscode.c",
        // image/encode (JXRTranscode.c references encode symbols, so we build
        // the full encoder to avoid unresolved symbols at link time)
        "image/encode/encode.c",
        "image/encode/segenc.c",
        "image/encode/strenc.c",
        "image/encode/strenc_x86.c",
        "image/encode/strFwdTransform.c",
        "image/encode/strPredQuantEnc.c",
        // jxrgluelib (the PK* API we FFI into)
        "jxrgluelib/JXRGlue.c",
        "jxrgluelib/JXRMeta.c",
        "jxrgluelib/JXRGluePFC.c",
        "jxrgluelib/JXRGlueJxr.c",
    ];

    let mut build = cc::Build::new();
    build
        .include(vendor)
        .include(format!("{vendor}/common/include"))
        .include(format!("{vendor}/image/sys"))
        // decode/encode headers are normally found via `#include "..."` relative
        // to the including .c file, but adding the dirs explicitly is harmless
        // and robust across compilers.
        .include(format!("{vendor}/image/decode"))
        .include(format!("{vendor}/image/encode"))
        .include(format!("{vendor}/image/x86"))
        .include(format!("{vendor}/jxrgluelib"))
        .define("__ANSI__", None)
        .define("DISABLE_PERF_MEASUREMENT", None)
        .warnings(false);

    let is_64bit =
        std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH").as_deref() == Ok("64");

    // MSVC uses "unsafe" CRT string funcs (strncpy/sscanf/…) that jxrlib relies
    // on; silence the C4996 deprecation noise so a stray /WX in the environment
    // cannot turn it into an error. No-op on GCC/Clang.
    if build.get_compiler().is_like_msvc() {
        build.define("_CRT_SECURE_NO_WARNINGS", None);
        build.define("_CRT_NONSTDC_NO_WARNINGS", None);

        // jxrlib's ANSI shim (image/sys/ansi.h) sizes UINTPTR_T / INTPTR_T with
        // `#if __LP64__`, a macro MSVC never defines. On 64-bit Windows (LLP64)
        // pointers are 8 bytes, so without this the shim picks a 4-byte type and
        // the `CT_ASSERT(sizeof(UINTPTR_T) == sizeof(void*))` in strcodec.h fails
        // to compile. Defining __LP64__ makes the shim select the 8-byte type. It
        // feeds ONLY jxrlib's own pointer-width selection — MSVC system headers
        // key off _WIN64, not __LP64__, so this is inert for them. On GCC/Clang
        // (Linux/Mac, LP64) the compiler already defines __LP64__ correctly, so
        // we must NOT redefine it there.
        if is_64bit {
            build.define("__LP64__", "1");
        }

        // jxrlib defines its own SAL-style annotations `__in_win` / `__out_win`
        // in the vendored SAL shim (common/include/wmspecstring.h), which
        // JXRMeta.h pulls in only when WIN32 is NOT defined. On MSVC the CRT
        // headers (via <string.h> etc.) transitively include the real Windows
        // <sal.h>, which already defines the shim's guard macro SPECSTRINGS_H —
        // so the vendored wmspecstring.h skips its body and never defines
        // `__in_win` / `__out_win`. The stock SAL macros it needs (`__in_ecount`
        // …) come from the real sal.h, but these two jxrlib-only names do not,
        // leaving them as bare identifiers and breaking JXRMeta.c's function
        // signatures. Define them to nothing ourselves. They are jxrlib-invented
        // names (never in the Windows SDK), so an empty definition is safe; where
        // the shim does run it merely re-defines them to the same empty result.
        build.define("__in_win", Some(""));
        build.define("__out_win", Some(""));
    }

    for src in sources {
        build.file(format!("{vendor}/{src}"));
    }

    build.compile("jxr");

    // Rebuild if any vendored source or header changes (cheap: the tree is small
    // and only walked when the feature is on).
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={vendor}/image");
    println!("cargo:rerun-if-changed={vendor}/jxrgluelib");
    println!("cargo:rerun-if-changed={vendor}/common");
}
