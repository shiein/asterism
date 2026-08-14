use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
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

    let status = run_pnpm(&["--filter", "@asterism/web", "build"], workspace)
        .unwrap_or_else(|err| panic!("pnpm is required to build the embedded Hub Web UI: {err}"));
    assert!(status.success(), "Hub Web UI build failed");
}

/// `Command::new("pnpm")` does not apply PATHEXT, so Windows misses `pnpm.cmd`.
fn run_pnpm(args: &[&str], dir: &Path) -> std::io::Result<std::process::ExitStatus> {
    let programs: &[&str] =
        if cfg!(windows) { &["pnpm.cmd", "pnpm.exe", "pnpm"] } else { &["pnpm"] };
    let mut last = Error::new(ErrorKind::NotFound, "pnpm");
    for program in programs {
        match Command::new(program).args(args).current_dir(dir).status() {
            Ok(status) => return Ok(status),
            Err(err) if err.kind() == ErrorKind::NotFound => last = err,
            Err(err) => return Err(err),
        }
    }
    Err(last)
}
