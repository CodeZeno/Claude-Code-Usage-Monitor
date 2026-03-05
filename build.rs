fn main() {
    // Derive version from the latest Git tag (e.g. "v1.0.7" → "1.0.7").
    let version = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| String::from("0.0.0"));

    let version = version.trim().trim_start_matches('v');
    println!("cargo:rustc-env=APP_VERSION={version}");
}
