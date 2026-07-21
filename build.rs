//! Embeds the built web app (`web/dist`, produced by `npm run build` in
//! `web/`) into the binary as a generated static table consumed by
//! `src/webapp.rs` — the same "one self-contained binary" property the
//! `include_str!` OpenAPI fixture and the `/ui` dashboard already have,
//! without adding an embedding crate dependency.
//!
//! `web/dist` being ABSENT is a supported, first-class state: local `cargo`
//! runs and the CI `rust` job never need node — the table is just empty and
//! `GET /app` explains how to build. The Docker image and releases run the
//! node build first, so they ship with the app embedded.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "map" => "application/json",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            out.push((rel, content_type(&path).to_string(), path.clone()));
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=web/dist");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist = manifest_dir.join("web/dist");

    let mut assets = Vec::new();
    if dist.is_dir() {
        collect(&dist, &dist, &mut assets);
        assets.sort();
    }

    let mut table = String::from(
        "/// (relative path, content type, bytes) for every file in `web/dist`\n\
         /// at compile time — empty when the web app was not built.\n\
         pub static WEB_ASSETS: &[(&str, &str, &[u8])] = &[\n",
    );
    for (rel, ct, path) in &assets {
        // `include_bytes!` needs an absolute path here since the generated
        // file lives in OUT_DIR, not in the source tree.
        table.push_str(&format!(
            "    ({rel:?}, {ct:?}, include_bytes!({:?}) as &[u8]),\n",
            path.display().to_string(),
        ));
    }
    table.push_str("];\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("web_assets.rs"), table).expect("write web_assets.rs");
}
