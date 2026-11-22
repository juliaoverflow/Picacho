//! Windows only: the icon and version resource, and the two system DLLs that
//! not every Windows has, loaded only when used (see `display::windows`).
//! Elsewhere this does nothing.

use std::path::Path;

/// The webauthn.dll functions webauthn-authenticator-rs calls (Windows Hello).
const WEBAUTHN: &[&str] = &[
    "WebAuthNAuthenticatorGetAssertion",
    "WebAuthNAuthenticatorMakeCredential",
    "WebAuthNFreeAssertion",
    "WebAuthNFreeCredentialAttestation",
    "WebAuthNGetApiVersionNumber",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    resources();
    delay_load();
}

/// The icon (resource 1, which GPUI gives its windows) and version
/// information, from packaging/windows/picacho.rc: windres for the GNU
/// target, RC.EXE for MSVC.
fn resources() {
    println!("cargo:rerun-if-changed=packaging/windows/picacho.rc");
    println!("cargo:rerun-if-changed=packaging/windows/picacho.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<String> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse::<u16>().unwrap_or(0).to_string())
        .collect();
    parts.resize(4, "0".into());
    let defines = [
        format!("PICACHO_VERSION=\\\"{version}\\\""),
        format!("PICACHO_VERSION_COMMAS={}", parts.join(",")),
    ];
    embed_resource::compile_for(
        "packaging/windows/picacho.rc",
        ["picacho"],
        embed_resource::ParamsMacros(defines.iter()),
    )
    .manifest_optional()
    .unwrap();
}

/// WebView2Loader.dll (Proton's human verification, through wry) isn't on
/// every Windows and isn't installed with Picacho; webauthn.dll (Windows
/// Hello) is only on Windows 10 1903 and later. Imported normally, either
/// missing stops Picacho before `main`. Delay-loaded, they are only looked
/// for when first called, and Picacho checks they're there first
/// (`display::windows::has_dll`).
///
/// `rustc-link-arg` reaches the app and its unit tests (`cargo test`'s
/// binary links the same imports).
fn delay_load() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // wry links WebView2's static loader on MSVC: only webauthn.dll.
        println!("cargo:rustc-link-arg=/DELAYLOAD:webauthn.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
        return;
    }
    // MinGW. GNU ld can't delay-load, so this links with lld (MSYS2's
    // mingw-w64-x86_64-lld, or LLVM's elsewhere). lld delay-loads only
    // imports from short import libraries, and the `windows` crate's
    // libwindows.a, where webauthn-authenticator-rs's functions come from,
    // has long ones, found first. So its calls are renamed (`--wrap`) to
    // `__wrap_` names that only a short import library of our own defines,
    // importing webauthn.dll's own names.
    let out = std::env::var("OUT_DIR").unwrap();
    let machine = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86_64" => 0x8664,
        "aarch64" => 0xaa64,
        "x86" => 0x014c,
        other => panic!("No Windows import library for {other}"),
    };
    let library = Path::new(&out).join("libpicacho_webauthn.a");
    write_import_library(&library, "webauthn.dll", WEBAUTHN, machine)
        .expect("couldn't write webauthn.dll's import library");
    println!("cargo:rustc-link-arg=-fuse-ld=lld");
    for function in WEBAUTHN {
        println!("cargo:rustc-link-arg=-Wl,--wrap={function}");
    }
    println!("cargo:rustc-link-arg={}", library.display());
    println!("cargo:rustc-link-arg=-Wl,-delayload=webauthn.dll");
    println!("cargo:rustc-link-arg=-Wl,-delayload=WebView2Loader.dll");
}

/// Writes an `ar` archive of COFF short import members (Microsoft PE/COFF
/// specification, "Import Library Format"), as `llvm-dlltool` does for a
/// .def file of `__wrap_name == name` lines: one member per function,
/// defining `__wrap_name` and `__imp___wrap_name` and importing `name`, and
/// the GNU symbol table the linker finds them by. lld makes the import
/// descriptor and thunks itself from these.
fn write_import_library(
    path: &Path,
    dll: &str,
    functions: &[&str],
    machine: u16,
) -> std::io::Result<()> {
    fn header(name: &str, size: usize) -> Vec<u8> {
        // GNU ar: name (16), mtime (12), uid (6), gid (6), mode (8), size (10).
        format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{size:<10}`\n", 0, 0, 0, 644).into_bytes()
    }
    let padded = |size: usize| size + size % 2;
    let members: Vec<Vec<u8>> = functions
        .iter()
        .map(|function| short_import(dll, function, machine))
        .collect();
    let names: Vec<String> = functions
        .iter()
        .flat_map(|f| [format!("__imp___wrap_{f}"), format!("__wrap_{f}")])
        .collect();
    // The symbol table: a count, each symbol's member offset (big-endian),
    // then the names, NUL-terminated.
    let table_size = 4 + 4 * names.len() + names.iter().map(|n| n.len() + 1).sum::<usize>();
    let mut offset = 8 + 60 + padded(table_size);
    let mut offsets = Vec::new();
    for member in &members {
        offsets.extend([offset as u32; 2]);
        offset += 60 + padded(member.len());
    }
    let mut archive = b"!<arch>\n".to_vec();
    archive.extend(header("/", table_size));
    archive.extend((names.len() as u32).to_be_bytes());
    for o in offsets {
        archive.extend(o.to_be_bytes());
    }
    for name in &names {
        archive.extend(name.as_bytes());
        archive.push(0);
    }
    if table_size % 2 == 1 {
        archive.push(b'\n');
    }
    for member in members {
        archive.extend(header(&format!("{dll}/"), member.len()));
        archive.extend(&member);
        if member.len() % 2 == 1 {
            archive.push(b'\n');
        }
    }
    std::fs::write(path, archive)
}

/// One function's short import member: the symbol `__wrap_<function>`,
/// importing `<function>` from `dll` by name (IMPORT_NAME_EXPORTAS).
fn short_import(dll: &str, function: &str, machine: u16) -> Vec<u8> {
    let strings = [
        format!("__wrap_{function}").as_bytes(),
        b"\0",
        dll.as_bytes(),
        b"\0",
        function.as_bytes(),
        b"\0",
    ]
    .concat();
    let mut data = Vec::with_capacity(20 + strings.len());
    data.extend(0u16.to_le_bytes()); // Sig1: IMAGE_FILE_MACHINE_UNKNOWN
    data.extend(0xffffu16.to_le_bytes()); // Sig2
    data.extend(0u16.to_le_bytes()); // Version
    data.extend(machine.to_le_bytes());
    data.extend(0u32.to_le_bytes()); // TimeDateStamp
    data.extend((strings.len() as u32).to_le_bytes()); // SizeOfData
    data.extend(0u16.to_le_bytes()); // Ordinal/Hint
    // Type: IMPORT_CODE (0); Name Type: IMPORT_NAME_EXPORTAS (4), bits 2-4.
    data.extend((4u16 << 2).to_le_bytes());
    data.extend(strings);
    data
}
