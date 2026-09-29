fn main() {
    // tauri's generate_context! embeds ../dist, so it must exist even before `pnpm build`.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist");
    let _ = std::fs::create_dir_all(dist);
    tauri_build::build()
}
