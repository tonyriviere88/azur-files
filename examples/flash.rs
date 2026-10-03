//! Watch the desktop while the window opens, and say what is on it.
//!
//! The instrument behind the table on `main::cloak`: it is what counted the white frames a
//! maximised window shows on the way up, and the only way to tell one frame of white apart
//! from none. Reads only — it never touches the window it is watching.
//!
//! `cargo run --example flash -- <exe> [ms]`. Reads nine points of the composited desktop as fast
//! as it can — which is thousands of times a second, fast enough to catch one frame — alongside
//! the window's visible and cloaked state, and prints one line per change.

fn main() {
    use windows::core::w;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect, IsWindowVisible};

    let mut args = std::env::args().skip(1);
    let exe = args.next().expect("give me an exe");
    let ms: u128 = args.next().and_then(|a| a.parse().ok()).unwrap_or(4000);
    let points = [
        (60, 60),
        (1280, 40),
        (2500, 60),
        (60, 700),
        (1280, 700),
        (2500, 700),
        (60, 1350),
        (1280, 1350),
        (2500, 1350),
    ];

    // SAFETY: the desktop's own DC, released below; a pure read per sample.
    let dc = unsafe { GetDC(None) };
    let mut child = std::process::Command::new(&exe)
        .spawn()
        .expect("could not start it");
    // After the spawn, which can take seconds the first time a freshly built binary is run.
    let start = std::time::Instant::now();

    let mut log: Vec<(u128, usize, String)> = Vec::new();
    let mut hwnd = HWND::default();
    while start.elapsed().as_millis() < ms {
        let white = points
            .iter()
            .filter(|(x, y)| {
                let c = unsafe { GetPixel(dc, *x, *y) }.0;
                let (r, g, b) = (c & 0xff, (c >> 8) & 0xff, c >> 16);
                r > 240 && g > 240 && b > 240
            })
            .count();
        if hwnd.is_invalid() {
            // SAFETY: a lookup by title; the handle is only read.
            hwnd = unsafe { FindWindowW(None, w!("Azur File Explorer")) }.unwrap_or_default();
        }
        let mut state = "no window".to_owned();
        if !hwnd.is_invalid() {
            let mut cloaked = 0u32;
            // SAFETY: a window handle read into one `u32` of the size declared, and a rect.
            let visible = unsafe {
                let _ = DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAKED,
                    (&raw mut cloaked).cast(),
                    size_of::<u32>() as u32,
                );
                IsWindowVisible(hwnd).as_bool()
            };
            let mut rect = Default::default();
            let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
            state = format!(
                "visible={visible} cloaked={cloaked} rect={},{} {}x{}",
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top
            );
        }
        let at = start.elapsed().as_micros();
        match log.last() {
            Some((_, last, was)) if *last == white && *was == state => {}
            _ => log.push((at, white, state)),
        }
    }
    unsafe { ReleaseDC(None, dc) };
    let _ = child.kill();
    let _ = child.wait();

    println!("{} changes, {ms} ms", log.len());
    for (i, (at, white, state)) in log.iter().enumerate() {
        let until = log.get(i + 1).map(|(t, ..)| *t).unwrap_or(ms * 1000);
        println!(
            "{:8.1}ms for {:7.1}ms  white {white}/9 {}  {state}",
            *at as f64 / 1000.0,
            (until - at) as f64 / 1000.0,
            if *white > 0 { "  <<<<" } else { "" },
        );
    }
}
