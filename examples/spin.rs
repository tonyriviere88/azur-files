//! Does a plain eframe window leak while it repaints?
//!
//! Scrolling this program's listing grows the process by megabytes a second, at a rate
//! proportional to how much is on screen, with every cache in it pinned and the Rust heap
//! flat. That points at the paint path — but "the paint path" includes egui's tessellator,
//! eframe's `glow` backend and the GL driver, none of which this program wrote.
//!
//! So this is the control: an eframe window with no part of this application in it, drawing a
//! comparable amount of text and asking for a repaint every frame. If it grows too, the
//! answer is not in `src/`.
//!
//! ```text
//! cargo run --release --example spin
//! ```

fn main() -> eframe::Result {
    eframe::run_native(
        "spin",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
            multisampling: 0,
            renderer: if std::env::var_os("SPIN_WGPU").is_some() {
                eframe::Renderer::Wgpu
            } else {
                eframe::Renderer::Glow
            },
            ..Default::default()
        },
        Box::new(|_| Ok(Box::<Spin>::default())),
    )
}

struct Spin {
    frames: u64,
    reported: std::time::Instant,
    /// A 16x16 texture, like a shell icon, made once.
    icon: Option<egui::TextureHandle>,
}

impl Default for Spin {
    fn default() -> Self {
        Self {
            frames: 0,
            reported: std::time::Instant::now(),
            icon: None,
        }
    }
}

impl eframe::App for Spin {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let icon = self
            .icon
            .get_or_insert_with(|| {
                let image = egui::ColorImage::filled([16, 16], egui::Color32::LIGHT_BLUE);
                ctx.load_texture("icon", image, egui::TextureOptions::LINEAR)
            })
            .id();
        // A row at a time, image then text then image then text — which is what a listing
        // does, and which means the primitive stream alternates between the icon's texture
        // and the font atlas instead of batching.
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let interleave = std::env::var_os("SPIN_INTERLEAVE").is_some();
        for row in 0..40 {
            let y = 4.0 + row as f32 * 16.0;
            let at = egui::Rect::from_min_size(egui::pos2(4.0, y), egui::vec2(16.0, 16.0));
            ui.painter().image(icon, at, uv, egui::Color32::WHITE);
            if interleave {
                ui.painter().text(
                    egui::pos2(28.0, y),
                    egui::Align2::LEFT_TOP,
                    "some text in between",
                    egui::FontId::proportional(12.0),
                    egui::Color32::GRAY,
                );
            }
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Roughly what a listing puts on screen: a few dozen rows of several cells.
            for row in 0..40 {
                ui.horizontal(|ui| {
                    for cell in 0..4 {
                        ui.label(format!("row {row} cell {cell} - some text"));
                    }
                });
            }
        });

        self.frames += 1;
        ctx.request_repaint();

        if self.reported.elapsed() >= std::time::Duration::from_secs(3) {
            self.reported = std::time::Instant::now();
            println!("{:>6} frames   private {:>7} KB", self.frames, private() / 1024);
        }
    }
}

#[cfg(windows)]
fn private() -> usize {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is told its own size; the handle is this process's pseudo-handle.
    unsafe {
        let _ = GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        );
    }
    counters.PrivateUsage
}

#[cfg(not(windows))]
fn private() -> usize {
    0
}
