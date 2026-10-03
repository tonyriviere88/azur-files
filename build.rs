//! Puts the application icon into the executable's resources.
//!
//! The icon is the design system's — `azur-egui-theme/app-icons/file-explorer/`, generated
//! there by `cargo run --example app_icons` — so this only has to hand it to the linker.
//! Windows shows *that* icon in Explorer, on a pinned shortcut and in Alt-Tab before the
//! process exists; the running window's own taskbar button comes from
//! `ViewportBuilder::with_icon` instead (see `src/brand.rs`), and both have to be set or the
//! mark changes identity the moment the window opens.
//!
//! Nothing here is fatal. A resource compiler is not part of the Rust toolchain, and a
//! machine without one should still be able to build and run this program — it just gets
//! the generic icon on the file. That is a warning, not a failed build.

/// The same relative path `Cargo.toml` uses to find the theme, and `src/brand.rs` to find
/// the 64px bitmap for the window.
const ICON: &str = "../azur-egui-theme/app-icons/file-explorer/app.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    if !std::path::Path::new(ICON).exists() {
        println!(
            "cargo:warning={ICON} is missing; regenerate it with `cargo run --example \
             app_icons` in the theme"
        );
        return;
    }
    if let Err(e) = winresource::WindowsResource::new().set_icon(ICON).compile() {
        println!("cargo:warning=could not embed the icon ({e}); the executable will use the generic one");
    }
}
