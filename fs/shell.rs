1//! Handing a path to the operating system.
2//!
3//! Everything here is a fire-and-forget request to the shell. Nothing waits for
4//! the launched program, and nothing in this program ever deletes, moves or
5//! overwrites a file — a first release that can browse fast is worth more than one
6//! that can also destroy things, and destructive operations want undo,
7//! progress and a recycle bin before they want to exist at all.
8
9use std::path::Path;
10
11/// Open a file or folder with whatever the shell thinks owns it.
12pub fn open(path: &Path) {
13    #[cfg(windows)]
14    {
15        run(None, path.as_os_str(), None);
16    }
17    #[cfg(not(windows))]
18    {
19        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
20    }
21}
22
23/// Open the shell's own window on a path, with the entry selected.
24///
25/// For the Recycle Bin and anything else that is a namespace extension rather
26/// than a directory.
27pub fn reveal(path: &Path) {
28    #[cfg(windows)]
29    {
30        use std::ffi::OsString;
31
32        let text = path.to_string_lossy();
33        // `shell:` monikers and `::{GUID}` paths are not files, so `/select,`
34        // would be nonsense — open them directly.
35        if text.starts_with("shell:") || text.starts_with("::{") {
36            let mut args = OsString::from("\"");
37            args.push(path.as_os_str());
38            args.push("\"");
39            run(Some("open"), std::ffi::OsStr::new("explorer.exe"), Some(&args));
40            return;
41        }
42        let mut args = OsString::from("/select,\"");
43        args.push(path.as_os_str());
44        args.push("\"");
45        run(Some("open"), std::ffi::OsStr::new("explorer.exe"), Some(&args));
46    }
47    #[cfg(not(windows))]
48    {
49        // No portable "reveal": show the containing directory instead.
50        open(path.parent().unwrap_or(path));
51    }
52}
53
54/// Open a terminal in a folder.
55pub fn open_terminal(dir: &Path) {
56    #[cfg(windows)]
57    {
58        use std::ffi::OsStr;
59        // Windows Terminal if it is installed, and `cmd` if the shell cannot find
60        // it. `ShellExecuteW` reports failure through its return value, which is
61        // an `HINSTANCE` for historical reasons and <= 32 on error.
62        if !run_ok(Some("open"), OsStr::new("wt.exe"), None, Some(dir)) {
63            run(Some("open"), OsStr::new("cmd.exe"), None);
64        }
65    }
66    #[cfg(not(windows))]
67    {
68        let _ = std::process::Command::new("x-terminal-emulator")
69            .current_dir(dir)
70            .spawn();
71    }
72}
73
74#[cfg(windows)]
75fn run(verb: Option<&str>, file: &std::ffi::OsStr, args: Option<&std::ffi::OsString>) {
76    run_ok(verb, file, args, None);
77}
78
79/// `ShellExecuteW`, with every string null-terminated and the working directory
80/// optional. Returns whether the shell managed to start something.
81#[cfg(windows)]
82fn run_ok(
83    verb: Option<&str>,
84    file: &std::ffi::OsStr,
85    args: Option<&std::ffi::OsString>,
86    dir: Option<&Path>,
87) -> bool {
88    use std::os::windows::ffi::OsStrExt as _;
89    use windows_sys::Win32::UI::Shell::ShellExecuteW;
90    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
91
92    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
93        text.encode_wide().chain(std::iter::once(0)).collect()
94    }
95
96    let verb = verb.map(|v| wide(std::ffi::OsStr::new(v)));
97    let file = wide(file);
98    let args = args.map(|a| wide(a));
99    let dir = dir.map(|d| wide(d.as_os_str()));
100
101    let ptr = |v: &Option<Vec<u16>>| v.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
102
103    // SAFETY: every pointer is either null or into a buffer that outlives the
104    // call, and each is null-terminated.
105    let result = unsafe {
106        ShellExecuteW(
107            std::ptr::null_mut(),
108            ptr(&verb),
109            file.as_ptr(),
110            ptr(&args),
111            ptr(&dir),
112            SW_SHOWNORMAL,
113        )
114    };
115    // Documented contract: anything above 32 is success.
116    result as isize > 32
117}
