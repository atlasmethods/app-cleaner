//! rust-embed requires its folder to exist at compile time; make sure `dist/` is present
//! (it is empty until `pnpm build` runs) and rebuild the embed when the frontend changes.

fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist");
    let _ = std::fs::create_dir_all(&dist);
    println!("cargo:rerun-if-changed=../../dist");
    println!("cargo:rerun-if-changed=build.rs");
}
