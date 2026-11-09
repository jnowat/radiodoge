// Tauri 2 build script - Windows static VC runtime via the supported API
// (replaces the deprecated STATIC_VCRUNTIME env var that tauri-cli still sets).
fn main() {
    let attrs = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new().static_vc_runtime(true));
    tauri_build::try_build(attrs).expect("failed to run tauri-build");
}
