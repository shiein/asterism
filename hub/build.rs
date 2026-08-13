use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = manifest_dir.parent().expect("hub must be inside workspace");
    for path in [
        "web/index.html",
        "web/package.json",
        "web/tsconfig.json",
        "web/vite.config.ts",
        "web/src",
        "pnpm-lock.yaml",
    ] {
        println!("cargo:rerun-if-changed={}", workspace.join(path).display());
    }

    let status = Command::new("pnpm")
        .args(["--filter", "@asterism/web", "build"])
        .current_dir(workspace)
        .status()
        .expect("pnpm is required to build the embedded Hub Web UI");
    assert!(status.success(), "Hub Web UI build failed");
}
