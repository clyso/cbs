// Read .git-version from workspace root at compile time.
// Production container builds write this file via the Containerfile.
// Dev builds have no file — defaults to "unknown".

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    let workspace_root = std::path::Path::new(&manifest_dir)
        .parent()
        .expect("crate must be in a workspace");

    let version_file = workspace_root.join(".git-version");

    let git_meta = std::fs::read_to_string(&version_file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|sha| format!("g{sha}"))
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=CBS_BUILD_META={git_meta}");
    println!(
        "cargo:rerun-if-changed={}",
        version_file.display()
    );
}
