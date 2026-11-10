// SPDX-License-Identifier: Apache-2.0
//! Compiles the vendored jxrlib C sources for `jpegxr_ffi`; opt-in, so a default build needs no C toolchain.
//! `__ANSI__` builds without `windows.h`; `DISABLE_PERF_MEASUREMENT` compiles out the QueryPerformanceCounter timers.

fn main() {
    if std::env::var_os("CARGO_FEATURE_JPEGXR").is_none() {
        return;
    }

    let vendor = "vendor/jxrlib";

    // strdec_x86.c and strenc_x86.c stay: without WMP_OPT_SSE2 they are no-ops, but their symbols must exist.
    let sources: &[&str] = &[
        "image/sys/adapthuff.c",
        "image/sys/image.c",
        "image/sys/strcodec.c",
        "image/sys/strPredQuant.c",
        "image/sys/strTransform.c",
        "image/sys/perfTimerANSI.c",
        "image/decode/decode.c",
        "image/decode/postprocess.c",
        "image/decode/segdec.c",
        "image/decode/strdec.c",
        "image/decode/strdec_x86.c",
        "image/decode/strInvTransform.c",
        "image/decode/strPredQuantDec.c",
        "image/decode/JXRTranscode.c",
        // JXRTranscode.c references encode symbols, so the full encoder is built.
        "image/encode/encode.c",
        "image/encode/segenc.c",
        "image/encode/strenc.c",
        "image/encode/strenc_x86.c",
        "image/encode/strFwdTransform.c",
        "image/encode/strPredQuantEnc.c",
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
        .include(format!("{vendor}/image/decode"))
        .include(format!("{vendor}/image/encode"))
        .include(format!("{vendor}/image/x86"))
        .include(format!("{vendor}/jxrgluelib"))
        .define("__ANSI__", None)
        .define("DISABLE_PERF_MEASUREMENT", None)
        .warnings(false);

    let is_64bit =
        std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH").as_deref() == Ok("64");

    // jxrlib uses unsafe CRT string functions; C4996 is silenced so a stray /WX cannot make it fatal.
    if build.get_compiler().is_like_msvc() {
        build.define("_CRT_SECURE_NO_WARNINGS", None);
        build.define("_CRT_NONSTDC_NO_WARNINGS", None);

        // jxrlib's ANSI shim takes 8-byte pointers only under `__LP64__`, which MSVC never defines: CT_ASSERT fails.
        // MSVC system headers key off _WIN64, so this define is inert for them.
        if is_64bit {
            build.define("__LP64__", "1");
        }

        // `__in_win` / `__out_win` are jxrlib-only names, defined empty here because the CRT pulls the real <sal.h>.
        build.define("__in_win", Some(""));
        build.define("__out_win", Some(""));
    }

    for src in sources {
        build.file(format!("{vendor}/{src}"));
    }

    build.compile("jxr");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={vendor}/image");
    println!("cargo:rerun-if-changed={vendor}/jxrgluelib");
    println!("cargo:rerun-if-changed={vendor}/common");
}
