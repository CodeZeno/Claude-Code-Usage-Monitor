use std::env;
use std::path::PathBuf;

use winres::{VersionInfo, WindowsResource};

fn main() {
    let version = env!("CARGO_PKG_VERSION");

    // Embed the icon and richer PE version metadata into the executable.
    let mut res = WindowsResource::new();
    let numeric_version = pack_version(version);

    res.set_icon("src/icons/icon.ico")
        .set("FileVersion", version)
        .set("ProductVersion", version)
        .set_version_info(VersionInfo::FILEVERSION, numeric_version)
        .set_version_info(VersionInfo::PRODUCTVERSION, numeric_version);

    res.compile().expect("Failed to compile Windows resources");

    // `winres` 0.1's own `cargo:rustc-link-lib=dylib=resource` directive
    // (emitted internally by `compile()`) asks the linker to pull the
    // compiled resource in through ordinary static-library member
    // selection. `rc.exe /fo` only ever produces a raw `.res` payload
    // (never converted to a real COFF object/archive via `cvtres.exe`), so
    // that selection can silently find nothing to pull in — observed on
    // this toolchain: `res.compile()` reports success, but the resulting
    // binary has no `.rsrc` section at all (no icon, no version info).
    // Passing the same file directly as a linker argument instead sidesteps
    // library member selection entirely: `link.exe` recognizes a raw `.res`
    // payload from its content and always embeds it when given this way.
    // Scoped to the GUI binary specifically (not `-bins`, which would also
    // apply to `aum-quota`): `winres`'s own unsuffixed directive above
    // already reaches every binary in this package, so adding the same
    // `.lib` again for every binary here would make `aum-quota`'s link see
    // the resource twice and fail with a duplicate-resource error.
    if let Ok(out_dir) = env::var("OUT_DIR") {
        let resource_lib = PathBuf::from(out_dir).join("resource.lib");
        println!(
            "cargo:rustc-link-arg-bin=claude-code-usage-monitor={}",
            resource_lib.display()
        );
    }
}

fn pack_version(version: &str) -> u64 {
    let core = version.split('-').next().unwrap_or(version);
    let mut parts = core.split('.').map(|part| part.parse::<u64>().unwrap_or(0));

    let major = parts.next().unwrap_or(0).min(u16::MAX as u64);
    let minor = parts.next().unwrap_or(0).min(u16::MAX as u64);
    let patch = parts.next().unwrap_or(0).min(u16::MAX as u64);

    (major << 48) | (minor << 32) | (patch << 16)
}
