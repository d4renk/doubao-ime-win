use std::{io::Result, path::Path, process::Command};

fn main() -> Result<()> {
    // Compile protobuf files
    prost_build::compile_protos(&["proto/asr.proto"], &["proto/"])?;

    // Tell Cargo to rerun if the proto file changes
    println!("cargo:rerun-if-changed=proto/asr.proto");
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/package-lock.json");

    if Path::new("frontend/package.json").exists() {
        let status = Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
            .args(["run", "build"])
            .current_dir("frontend")
            .status()
            .expect("Node.js and npm are required; run `npm ci` in frontend first");
        assert!(status.success(), "Vite frontend build failed");
    }

    Ok(())
}
