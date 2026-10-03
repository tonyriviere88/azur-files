//! Puts the application icon into the executable's resources.
//!
//! The icon is this program's own — `assets/app-icon/`, rasterised from `master.png` — so
//! this only has to hand it to the linker. Windows shows *that* icon in Explorer, on a
//! pinned shortcut and in Alt-Tab before the process exists; the running window's own
//! taskbar button comes from `ViewportBuilder::with_icon` instead (see `src/brand.rs`), and
//! both have to be set or the mark changes identity the moment the window opens.
//!
//! Nothing here is fatal. A resource compiler is not part of the Rust toolchain, and a
//! machine without one should still be able to build and run this program — it just gets
//! the generic icon on the file. That is a warning, not a failed build.

/// The nine-size icon, in the folder `src/brand.rs` takes the window bitmap and the title
/// bar's ladder from.
const ICON: &str = "assets/app-icon/app.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    if !std::path::Path::new(ICON).exists() {
        println!("cargo:warning={ICON} is missing; see assets/app-icon/README.md to rebuild it");
        return;
    }
    if let Err(e) = winresource::WindowsResource::new().set_icon(ICON).compile() {
        println!("cargo:warning=could not embed the icon ({e}); the executable will use the generic one");
    }
}
