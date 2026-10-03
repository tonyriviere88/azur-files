1//! The known folders the sidebar offers, resolved once at startup.
2//!
3//! Asked of the shell rather than assembled from `%USERPROFILE%`: Downloads,
4//! Documents and the rest can be redirected anywhere — to another volume, or to
5//! OneDrive, which is the common case on a managed machine — and a hard-coded
6//! `~/Documents` would quietly point at an empty folder beside the real one.
7
8use std::path::PathBuf;
9
10use crate::fs::fmt::Kind;
11
12/// Which glyph a place gets. Distinct from [`Kind`] because these are *places*,
13/// not file types: Downloads is not "a folder", it is Downloads.
14#[derive(Clone, Copy, PartialEq, Eq, Debug)]
15pub enum PlaceIcon {
16    ThisPc,
17    Home,
18    Desktop,
19    Documents,
20    Downloads,
21    Music,
22    Pictures,
23    Videos,
24    Trash,
25}
26
27/// One sidebar entry under Places.
28#[derive(Clone, Debug)]
29pub struct Place {
30    pub label: String,
31    /// Where it goes. Empty means the synthetic "This PC".
32    pub path: PathBuf,
33    pub icon: PlaceIcon,
34    /// Hand this to the shell instead of listing it ourselves.
35    ///
36    /// The Recycle Bin is not a directory — it is a shell namespace extension
37    /// stitched together from a per-volume `$Recycle.Bin\<SID>` plus an index of
38    /// original paths. Enumerating it with `FindFirstFile` shows the mangled
39    /// `$R…` names and no way to restore anything, so this row opens the real
40    /// thing rather than lying about it.
41    pub shell_only: bool,
42}
43
44/// The standard places, in the order the sidebar shows them.
45///
46/// Anything the shell will not resolve — a machine with no Videos folder — is
47/// dropped rather than shown as a dead row.
48pub fn standard() -> Vec<Place> {
49    let mut places = vec![Place {
50        label: "This PC".to_owned(),
51        path: PathBuf::new(),
52        icon: PlaceIcon::ThisPc,
53        shell_only: false,
54    }];
55
56    for (folder, label, icon) in known_folders() {
57        if let Some(path) = folder {
58            places.push(Place {
59                label: label.to_owned(),
60                path,
61                icon,
62                shell_only: false,
63            });
64        }
65    }
66
67    places.push(Place {
68        label: "Recycle Bin".to_owned(),
69        path: PathBuf::from("shell:RecycleBinFolder"),
70        icon: PlaceIcon::Trash,
71        shell_only: true,
72    });
73    places
74}
75
76#[cfg(windows)]
77fn known_folders() -> Vec<(Option<PathBuf>, &'static str, PlaceIcon)> {
78    use windows_sys::Win32::UI::Shell as shell;
79
80    vec![
81        (
82            known_folder(&shell::FOLDERID_Profile),
83            "Home",
84            PlaceIcon::Home,
85        ),
86        (
87            known_folder(&shell::FOLDERID_Desktop),
88            "Desktop",
89            PlaceIcon::Desktop,
90        ),
91        (
92            known_folder(&shell::FOLDERID_Documents),
93            "Documents",
94            PlaceIcon::Documents,
95        ),
96        (
97            known_folder(&shell::FOLDERID_Downloads),
98            "Downloads",
99            PlaceIcon::Downloads,
100        ),
101        (
102            known_folder(&shell::FOLDERID_Pictures),
103            "Pictures",
104            PlaceIcon::Pictures,
105        ),
106        (
107            known_folder(&shell::FOLDERID_Music),
108            "Music",
109            PlaceIcon::Music,
110        ),
111        (
112            known_folder(&shell::FOLDERID_Videos),
113            "Videos",
114            PlaceIcon::Videos,
115        ),
116    ]
117}
118
119/// Resolve one `FOLDERID_*`.
120#[cfg(windows)]
121fn known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
122    use std::os::windows::ffi::OsStringExt as _;
123    use windows_sys::Win32::System::Com::CoTaskMemFree;
124    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
125
126    let mut raw = std::ptr::null_mut();
127    // SAFETY: `raw` is written only on success, and freed on every path out.
128    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
129    if hr < 0 || raw.is_null() {
130        return None;
131    }
132
133    let mut len = 0;
134    // SAFETY: the shell returns a null-terminated string it owns until we free it.
135    unsafe {
136        while *raw.add(len) != 0 {
137            len += 1;
138        }
139    }
140    let wide = unsafe { std::slice::from_raw_parts(raw, len) };
141    let path = PathBuf::from(std::ffi::OsString::from_wide(wide));
142    unsafe { CoTaskMemFree(raw.cast()) };
143
144    (!path.as_os_str().is_empty()).then_some(path)
145}
146
147#[cfg(not(windows))]
148fn known_folders() -> Vec<(Option<PathBuf>, &'static str, PlaceIcon)> {
149    let home = std::env::var_os("HOME").map(PathBuf::from);
150    let under = |name: &str| home.as_ref().map(|h| h.join(name));
151    vec![
152        (home.clone(), "Home", PlaceIcon::Home),
153        (under("Desktop"), "Desktop", PlaceIcon::Desktop),
154        (under("Documents"), "Documents", PlaceIcon::Documents),
155        (under("Downloads"), "Downloads", PlaceIcon::Downloads),
156        (under("Pictures"), "Pictures", PlaceIcon::Pictures),
157        (under("Music"), "Music", PlaceIcon::Music),
158        (under("Videos"), "Videos", PlaceIcon::Videos),
159    ]
160}
161
162/// Where a new window opens if nothing was remembered.
163pub fn default_start() -> PathBuf {
164    #[cfg(windows)]
165    {
166        use windows_sys::Win32::UI::Shell::FOLDERID_Profile;
167        if let Some(home) = known_folder(&FOLDERID_Profile) {
168            return home;
169        }
170        PathBuf::from("C:\\")
171    }
172    #[cfg(not(windows))]
173    {
174        std::env::var_os("HOME")
175            .map(PathBuf::from)
176            .unwrap_or_else(|| PathBuf::from("/"))
177    }
178}
179
180/// The [`Kind`] a place's icon stands in for, when a listing row needs one.
181pub fn kind_of(icon: PlaceIcon) -> Kind {
182    match icon {
183        PlaceIcon::Trash => Kind::Other,
184        _ => Kind::Folder,
185    }
186}
