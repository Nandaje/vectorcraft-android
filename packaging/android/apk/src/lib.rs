//! Android `NativeActivity` entry point (`libvectorcraft.so`).
//!
//! `android-activity`'s glue exports `ANativeActivity_onCreate` and calls `android_main(AndroidApp)`.
//! This thin crate lives outside the workspace's `unsafe_code = "deny"` lint so it can export
//! `#[unsafe(no_mangle)] pub fn android_main` while every workspace crate stays 100% safe Rust.

#![cfg(target_os = "android")]

#[unsafe(no_mangle)]
pub fn android_main(app: vectorcraft::AndroidApp) {
    if let Some(dir) = app.external_data_path().or_else(|| app.internal_data_path()) {
        // SAFETY: called on the main thread at process start before worker threads are spawned.
        unsafe {
            std::env::set_var("VECTORCRAFT_CONFIG_DIR", &dir);
            if std::env::var_os("HOME").is_none() {
                std::env::set_var("HOME", &dir);
            }
        }
    }
    let code = vectorcraft::run_android(app);
    if code != std::process::ExitCode::SUCCESS {
        log::error!("vectorcraft android exited with non-zero status");
    }
}
