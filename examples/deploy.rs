//! `cargo deploy` — build the release binary and put it where you run it from.
//!
//! ```text
//! cargo deploy                     release build -> D:\Programs
//! cargo deploy E:\Tools            somewhere else
//! cargo deploy --debug             the debug build, for once
//! ```
//!
//! The work is [`azur_egui_theme::deploy`], which every Azur application shares — including
//! why any of this is an example target rather than a shell script. What has to be here is
//! the two things only this package knows, and `env!` can only answer them where it is
//! written: asked inside the theme crate it would deploy the theme crate.

fn main() -> std::process::ExitCode {
    azur_egui_theme::deploy::main(azur_egui_theme::deploy::Program {
        bin: env!("CARGO_PKG_NAME"),
        manifest: env!("CARGO_MANIFEST_DIR"),
    })
}
