//! Lists every SVG in `assets/` (icons, their shimmer versions, glyphs,
//! provider logos and the Three voices mark frames) as a table of
//! `include_bytes!`, so `src/assets.rs` does not keep a list by hand.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            // The fonts load through `src/fonts.rs`.
            if path.file_name().is_some_and(|n| n == "fonts") {
                continue;
            }
            collect(&path, root, out)?;
        } else if path.extension().is_some_and(|e| e == "svg") {
            let rel = path
                .strip_prefix(root)
                .map_err(std::io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let manifest =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").map_err(std::io::Error::other)?);
    let root = manifest.join("assets");
    println!("cargo:rerun-if-changed=assets");
    let mut files = Vec::new();
    collect(&root, &root, &mut files)?;
    let mut code = String::new();
    let _ = writeln!(
        code,
        "pub(crate) const FILES: [(&str, &[u8]); {}] = [",
        files.len()
    );
    for (rel, path) in &files {
        let _ = writeln!(
            code,
            "    ({rel:?}, include_bytes!({:?})),",
            path.to_string_lossy()
        );
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(std::io::Error::other)?);
    fs::write(out.join("asset_files.rs"), code)
}
