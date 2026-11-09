//! Prepares the web UI for embedding: copies the built frontend (`web-dist/`, produced by
//! `npm --prefix web run build`) into `$OUT_DIR/web`, or — when it has not been built — a small
//! fallback page, so `cargo build` works without Node.js.

use std::path::Path;
use std::{env, fs, io};

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("web");
    let dist = Path::new(&manifest).join("web-dist");
    println!("cargo:rerun-if-changed=web-dist");
    println!("cargo:rerun-if-changed=fallback.html");
    println!("cargo:rerun-if-changed=fallback.js");
    if out.exists() {
        fs::remove_dir_all(&out)?;
    }
    if dist.join("index.html").is_file() {
        copy_dir(&dist, &out)?;
    } else {
        println!(
            "cargo:warning=web UI not built (run `npm --prefix web ci && npm --prefix web run build`); embedding a placeholder page"
        );
        fs::create_dir_all(&out)?;
        fs::copy(
            Path::new(&manifest).join("fallback.html"),
            out.join("index.html"),
        )?;
        fs::copy(
            Path::new(&manifest).join("fallback.js"),
            out.join("fallback.js"),
        )?;
    }
    Ok(())
}
