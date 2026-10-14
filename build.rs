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

/// What Windows calls the program where it reads the executable rather than the window:
/// Properties → Details, and the Task Manager's process list.
///
/// The same string as `brand::NAME`, and it has to be written again rather than read from
/// there, because a build script is compiled before the crate it builds and cannot see into
/// it. Left to itself `winresource` fills these from `CARGO_PKG_NAME` and
/// `CARGO_PKG_DESCRIPTION` — `azur-files`, which is the executable's name and not the
/// program's, and a sentence too long for the column it lands in.
const NAME: &str = "Azur Files";

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");

    // `cfg(ci)`, on a GitHub Actions runner and nowhere else. A test that passes on a desktop and
    // cannot pass on the runner says so with `#[cfg_attr(ci, ignore = "why")]`: the runner lists it
    // as ignored, with the reason, and a local `cargo test` runs it like any other. Read from the
    // build script rather than handed over in `RUSTFLAGS`, so that it reaches this crate alone and
    // does not rebuild every dependency under a second set of flags.
    println!("cargo:rustc-check-cfg=cfg(ci)");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    if std::env::var_os("GITHUB_ACTIONS").is_some_and(|v| v == "true") {
        println!("cargo:rustc-cfg=ci");
    }

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    if !std::path::Path::new(ICON).exists() {
        println!("cargo:warning={ICON} is missing; see assets/app-icon/README.md to rebuild it");
        return;
    }
    let resource = winresource::WindowsResource::new()
        .set_icon(ICON)
        .set("ProductName", NAME)
        .set("FileDescription", NAME)
        .compile();
    if let Err(e) = resource {
        println!("cargo:warning=could not embed the icon ({e}); the executable will use the generic one");
    }
}
