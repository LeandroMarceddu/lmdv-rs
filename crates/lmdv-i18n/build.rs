//! Generates the typed message catalog from `locales/`.

use std::path::{Path, PathBuf};

fn main() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let locales = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
    rune_i18n::codegen::generate(&locales, &out).expect("generate the i18n catalog");

    // `codegen::generate` watches the directory, whose mtime does not change
    // when a catalog file is rewritten. Watch the manifest and every layer file
    // too, so editing a message rebuilds the crate.
    println!("cargo:rerun-if-changed={}", locales.join("locales.json").display());
    let Ok(layers) = std::fs::read_dir(&locales) else {
        return;
    };
    for layer in layers.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()) {
        println!("cargo:rerun-if-changed={}", layer.display());
        let Ok(files) = std::fs::read_dir(&layer) else {
            continue;
        };
        for file in files.flatten().map(|entry| entry.path()) {
            if file.extension().is_some_and(|extension| extension == "json" || extension == "html") {
                println!("cargo:rerun-if-changed={}", file.display());
            }
        }
    }
}
