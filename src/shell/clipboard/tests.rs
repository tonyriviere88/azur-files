use super::*;

/// A round trip through the real clipboard, which is the only test worth having
/// here: it is interoperability that matters, and interoperability with a mock is
/// not a fact about anything.
/// The data object a cut or a copy hands over: the shell's own, with `CF_HDROP` and
/// the `Preferred DropEffect` block that tells the target which of the two it was.
///
/// Built and read back directly rather than through the clipboard. The clipboard is one
/// object shared by every process on the desktop — Windows' own history service reads
/// each new item the moment it lands — and a test that goes through it is a test that
/// fails one run in four for reasons that have nothing to do with this program. What is
/// worth checking is the object, and that needs no lock at all.
#[test]
#[cfg(windows)]
fn the_data_object_carries_the_files_and_the_effect() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let here = crate::sandbox::dir("clip");
    std::fs::create_dir_all(&here).expect("temp dir");
    let one = here.join("one.txt");
    let two = here.join("two.txt");
    std::fs::write(&one, b"1").expect("write");
    std::fs::write(&two, b"2").expect("write");

    for effect in [Effect::Copy, Effect::Move] {
        let data = win::data_object(&[one.clone(), two.clone()], effect)
            .expect("the shell should build a data object for two real files");
        let read = win::read(&data).expect("and it should read back as files");
        assert_eq!(
            read.effect, effect,
            "a cut has to come back as a cut, or a paste would copy when it should move"
        );
        assert_eq!(read.items.len(), 2);
        // The shell normalises the case of what it hands back, so compare that way.
        let names: Vec<String> = read
            .items
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            })
            .collect();
        assert!(names.contains(&"one.txt".to_owned()), "{names:?}");
        assert!(names.contains(&"two.txt".to_owned()), "{names:?}");
    }

    assert!(
        win::data_object(&[], Effect::Copy).is_err(),
        "and nothing is not something to put on a clipboard"
    );
    crate::sandbox::remove(&here);
}

/// The same, but through the real clipboard, which is what a paste into Explorer
/// actually uses.
///
/// Ignored by default: it needs the desktop's one clipboard to stay still for a moment,
/// and nothing on a working machine promises that. Run it on purpose:
///
/// ```text
/// cargo test -- --ignored the_real_clipboard
/// ```
#[test]
#[ignore]
#[cfg(windows)]
fn the_real_clipboard_round_trips() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    win::settle();

    let here = crate::sandbox::dir("clip-real");
    std::fs::create_dir_all(&here).expect("temp dir");
    let one = here.join("one.txt");
    std::fs::write(&one, b"1").expect("write");

    put(std::slice::from_ref(&one), Effect::Move).expect("put on the clipboard");
    assert!(has_files(), "the clipboard should be offering files");
    let read = get().expect("read back off the clipboard");
    assert_eq!(read.effect, Effect::Move);
    assert_eq!(read.items.len(), 1);

    clear();
    crate::sandbox::remove(&here);
}

/// A zip holding one stored `inner.txt`, so the test that needs a non-file shell item
/// need no archiver.
///
/// Made by `zipfile` and pasted in: a hand-written one had a bad central directory, which
/// Windows treats as a plain file rather than a folder -- and the probe that found this
/// duly reported that the shell could not resolve a path inside a zip.
#[cfg(all(test, windows))]
const ZIP_WITH_ONE_ENTRY: &[u8] = &[
    0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb2, 0x8e, 0x03, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x69, 0x6e, 0x6e, 0x65, 0x72, 0x2e, 0x74, 0x78, 0x74, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb2, 0x8e, 0x03, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x69, 0x6e, 0x6e, 0x65, 0x72, 0x2e, 0x74, 0x78, 0x74, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x37, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Copying something that is not a file: an entry inside a `.zip`.
///
/// This is the case that made a paste read the shell rather than `CF_HDROP`. Explorer
/// offers `Shell IDList Array`, `FileGroupDescriptorW` and `FileContents` for an archive
/// entry and no `CF_HDROP` at all, so a paste that only knew `CF_HDROP` read nothing back
/// and did nothing at all -- copy a file out of a zip in Explorer, press Ctrl+V here, and
/// the answer was silence.
///
/// What is asserted is the whole way through: the data object reads back as the item, and
/// the name it reads back as is one the shell can resolve again, which is what lets
/// `IFileOperation` extract it.
#[test]
#[cfg(windows)]
fn an_entry_inside_a_zip_reads_back_as_something_pasteable() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("zip");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    let zip = root.join("bundle.zip");
    std::fs::write(&zip, ZIP_WITH_ONE_ENTRY).expect("write the zip");
    let inside = zip.join("inner.txt");

    let data = win::data_object(std::slice::from_ref(&inside), Effect::Copy)
        .expect("the shell should build a data object for an item inside an archive");
    let read = win::read(&data).expect(
        "and a paste should read it back -- if this is None, the read has gone back to \
         `CF_HDROP`, which an archive entry does not offer",
    );
    assert_eq!(read.items.len(), 1, "{:?}", read.items);

    // The name has to be one the shell can resolve, since that is what the copy engine
    // is handed on the other side.
    let name = &read.items[0];
    assert!(
        name.to_string_lossy().to_lowercase().contains("bundle.zip"),
        "expected a path through the archive, got {}",
        name.display()
    );
    // SAFETY: a pure lookup; nothing is retained.
    unsafe {
        assert!(
            crate::shell::ops::item(name).is_ok(),
            "the shell cannot resolve {} back, so a paste of it would fail",
            name.display()
        );
    }

    crate::sandbox::remove(&root);
}

/// Interoperability, across a process boundary, in both directions.
///
/// The claim worth checking is not that this program can read its own clipboard -- it is
/// that *another* process sees what it puts there, and that it sees what another process
/// puts. PowerShell stands in for Explorer: `Set-Clipboard -Path` writes `CF_HDROP` the
/// same way a copy in a folder window does, and `Get-Clipboard -Format FileDropList`
/// reads it back the same way a paste does.
///
/// Ignored by default, because it takes over the desktop's one clipboard. Run it on
/// purpose:
///
/// ```text
/// cargo test -- --ignored --test-threads=1 explorer_and_this_program
/// ```
#[test]
#[ignore = "takes over the real clipboard; run explicitly"]
#[cfg(windows)]
fn explorer_and_this_program_read_each_other_s_clipboard() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    win::settle();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("interop");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    let one = root.join("one.txt");
    let two = root.join("two.txt");
    std::fs::write(&one, b"1").expect("write");
    std::fs::write(&two, b"2").expect("write");

    /// Run a snippet of PowerShell in its own STA, pumping messages while it runs.
    ///
    /// The pumping is not incidental. `OleSetClipboard` does not copy anything: it leaves
    /// the clipboard holding a reference to the data object *in this process*, and another
    /// process asking for the bytes is a marshalled call back into this apartment, which
    /// arrives as a window message. A thread that is not dispatching messages therefore
    /// hands out nothing at all -- which is exactly what the first version of this test
    /// measured, and it would have been wrong to conclude from it that interoperability
    /// was broken. The real window pumps continuously.
    fn powershell(script: &str) -> String {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };

        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", script])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("powershell should be on the path");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            // SAFETY: a plain pump over this thread's own queue.
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let out = child.wait_with_output().expect("powershell finished");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    // ---- this program copies, another process pastes ----
    put(&[one.clone(), two.clone()], Effect::Copy).expect("put on the clipboard");
    let seen = powershell(
        "(Get-Clipboard -Format FileDropList | ForEach-Object { $_.Name }) -join ','",
    );
    assert!(
        seen.to_lowercase().contains("one.txt") && seen.to_lowercase().contains("two.txt"),
        "another process read `{seen}` off the clipboard, not the two files put there"
    );

    // ---- another process copies, this program pastes ----
    let script = format!(
        "Set-Clipboard -Path '{}','{}'",
        one.display(),
        two.display()
    );
    powershell(&script);
    let read = get().expect("this program should read a clipboard another process wrote");
    assert_eq!(read.effect, Effect::Copy, "no preferred effect means copy");
    let mut names: Vec<String> = read
        .items
        .iter()
        .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_lowercase())
        .collect();
    names.sort();
    assert_eq!(names, ["one.txt", "two.txt"], "{:?}", read.items);

    clear();
    crate::sandbox::remove(&root);
}

/// The end of a cut: the clipboard is emptied, and only when it is still the same cut.
#[test]
#[ignore = "takes over the real clipboard; run explicitly"]
#[cfg(windows)]
fn a_cut_that_has_been_pasted_empties_the_clipboard() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    win::settle();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("cut");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    let one = root.join("one.txt");
    let other = root.join("other.txt");
    std::fs::write(&one, b"1").expect("write");
    std::fs::write(&other, b"2").expect("write");

    // A clipboard that has changed since the paste began is not this program's to throw
    // away, however much it looks like the one it was given.
    put(std::slice::from_ref(&one), Effect::Move).expect("put");
    let stale = sequence();
    put(std::slice::from_ref(&other), Effect::Move).expect("put something else");
    cut_pasted(stale);
    assert!(
        has_files(),
        "the clipboard moved on between the paste and its finish, and this emptied it \
         anyway -- that is somebody else's data"
    );

    // The real thing: the same clipboard the paste was given.
    put(std::slice::from_ref(&one), Effect::Move).expect("put");
    let ours = sequence();
    assert!(has_files());
    cut_pasted(ours);
    assert!(
        !has_files(),
        "a cut that has been pasted has to leave the clipboard empty, or Ctrl+V would \
         move files that are no longer there"
    );

    clear();
    crate::sandbox::remove(&root);
}

/// Puts two files on the clipboard and returns, so the process exits with them on it.
///
/// Half of a test: the other half is another process reading the clipboard afterwards.
/// Driven from the shell, because what is being checked is what survives *this* program
/// closing, and that cannot be checked from inside it:
///
/// ```text
/// cargo test --release -- --ignored --exact \
///   shell::clipboard::tests::leaves_a_copy_behind_and_exits
/// powershell -NoProfile -STA -Command "Get-Clipboard -Format FileDropList"
/// ```
#[test]
#[ignore = "half of a cross-process check; see the note"]
#[cfg(windows)]
fn leaves_a_copy_behind_and_exits() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("survives");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    let one = root.join("survivor.txt");
    std::fs::write(&one, b"1").expect("write");

    put(std::slice::from_ref(&one), Effect::Copy).expect("put on the clipboard");
    if std::env::var_os("YAFE_NO_FLUSH").is_none() {
        crate::shell::flush();
    }
    // Deliberately leaves the file: the reader on the other side names it.
}

/// One copy after another has to keep working, at every cadence.
///
/// This is the regression test for the least obvious bug in this file. A copy leaves the
/// clipboard holding a reference to a data object *here*, so Windows' clipboard history
/// comes asking for the bytes a couple of hundred milliseconds later -- and it asks while
/// holding the clipboard. A thread that does not answer that call leaves the lock taken and
/// the next copy refused, and the `HRESULT` for it says `OpenClipboard failed`, which reads
/// like somebody else's fault.
///
/// The shape is what gives it away, and it is the shape asserted here: back-to-back copies
/// were fine, because the history service had not started yet, and copies 200 ms apart
/// failed eight to eleven times in twelve. So a version of this test that only hammered
/// would pass against the bug. `answering_calls` is the fix; see the note on it.
///
/// Ignored because it takes over the desktop's one clipboard.
#[test]
#[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
#[cfg(windows)]
fn one_copy_after_another_keeps_working() {
    let _serialised = crate::shell::serialised();
    crate::shell::init();
    win::settle();

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox")
        .join("consecutive");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("sandbox");
    let a = root.join("a.txt");
    let b = root.join("b.txt");
    std::fs::write(&a, b"a").expect("write");
    std::fs::write(&b, b"b").expect("write");

    // 200 ms is the one that mattered, so it is in the list; the others are there because
    // a fix that only worked at one cadence would not be a fix.
    for gap in [0u64, 20, 50, 200] {
        const TIMES: usize = 8;
        for step in 0..TIMES {
            let which = if step % 2 == 0 { &a } else { &b };
            put(std::slice::from_ref(which), Effect::Copy).unwrap_or_else(|why| {
                panic!("copy {step} of {TIMES}, {gap} ms apart, was refused: {why}")
            });
            let read = get().unwrap_or_else(|| {
                panic!("copy {step} of {TIMES}, {gap} ms apart, read back as nothing")
            });
            assert_eq!(read.items.len(), 1);
            std::thread::sleep(std::time::Duration::from_millis(gap));
        }
    }

    clear();
    crate::sandbox::remove(&root);
}

#[test]
fn putting_nothing_is_refused_rather_than_clearing() {
    let _serialised = crate::shell::serialised();
    assert!(put(&[], Effect::Copy).is_err());
}
