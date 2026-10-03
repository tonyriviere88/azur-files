1//! The filesystem layer: everything that touches the disk, and nothing that
2//! touches the screen.
3//!
4//! The whole layer is built around one rule — **enumerate once, and never go back
5//! to the disk for something the enumeration already told you.** A directory read
6//! hands over a name, a size, a timestamp and an attribute word per entry; the
7//! moment any code asks `Path::is_dir` or `metadata()` on top of that, it has turned
8//! a single sequential read into one round trip per file. That is not a small
9//! difference: [`scan`]'s benchmark measures it at 202× on a folder of 60,000.
10//!
11//! | module | responsibility |
12//! | --- | --- |
13//! | [`dir`] | the listing: one string of names, one vector of 32-byte records |
14//! | [`scan`] | reading a directory as fast as the platform allows |
15//! | [`sort`] | ordering and filtering, over indices rather than entries |
16//! | [`fmt`] | sizes, dates and type names, written into a reused buffer |
17//! | [`time`] | `FILETIME` to local civil time without a syscall per row |
18//! | [`drives`] | mounted volumes, and the synthetic "This PC" |
19//! | [`places`] | the shell's known folders |
20//! | [`shell`] | handing a path back to the operating system |
21
22pub mod dir;
23pub mod drives;
24pub mod fmt;
25pub mod places;
26pub mod scan;
27pub mod shell;
28pub mod sort;
29pub mod time;
30
31pub use dir::{display_name, Dir};
32pub use sort::Column;
33
34use std::path::{Path, PathBuf};
35
36/// The parent of a path, in navigation terms.
37///
38/// Differs from [`Path::parent`] at the two ends of the tree: a drive root's
39/// parent is "This PC" (the empty path) rather than nothing, and "This PC" has no
40/// parent at all. Without that, Up stops working one level too early.
41pub fn parent_of(path: &Path) -> Option<PathBuf> {
42    if path.as_os_str().is_empty() {
43        return None;
44    }
45    match path.parent() {
46        Some(parent) if !parent.as_os_str().is_empty() => Some(parent.to_path_buf()),
47        // A root: `C:\` or `\\server\share`.
48        _ => Some(PathBuf::new()),
49    }
50}
51
52/// A path that came from outside this program, in the form the *shell* also understands.
53///
54/// Every file API on Windows accepts a forward slash, so `D:/Sources` scans, navigates,
55/// draws a correct breadcrumb and looks entirely fine — and then `SHParseDisplayName`
56/// refuses it, `IContextMenu` is never obtained, and the right-click menu comes up with
57/// this program's own entries and none of the shell's. Nothing reports an error; the menu
58/// is just half a menu.
59///
60/// So a path is rewritten once, at each door it can come in by: the command line and the
61/// path bar. Inside, every path is built by joining onto one of those.
62pub fn normalize(path: &Path) -> PathBuf {
63    #[cfg(windows)]
64    {
65        let text = path.to_string_lossy();
66        if text.contains('/') {
67            // A forward slash cannot be part of a Windows file name, so this can only be
68            // a separator — including inside a `\\?\` or UNC prefix.
69            return PathBuf::from(text.replace('/', "\\"));
70        }
71    }
72    path.to_path_buf()
73}
74
75/// Resolve what the user typed in the breadcrumb into somewhere to go.
76///
77/// Expands `%VARS%` and `~`, accepts either slash, and tolerates a trailing one.
78/// Returns `None` if there is nothing there — the caller shows that as a message
79/// rather than navigating into a void.
80pub fn resolve_input(text: &str) -> Option<PathBuf> {
81    let text = text.trim().trim_matches('"');
82    if text.is_empty() {
83        return None;
84    }
85    if text.eq_ignore_ascii_case("this pc") {
86        return Some(PathBuf::new());
87    }
88
89    let expanded = expand(text);
90    let path = PathBuf::from(&expanded);
91
92    // A drive letter on its own means its root: `D:` is where `D:\` is.
93    if expanded.len() == 2 && expanded.as_bytes()[1] == b':' {
94        return Some(PathBuf::from(format!("{expanded}\\")));
95    }
96
97    if path.is_dir() {
98        return Some(normalize(&path));
99    }
100    // A file: go to its folder and let the caller select it.
101    if path.is_file() {
102        return Some(normalize(&path));
103    }
104    None
105}
106
107/// `%APPDATA%`-style variables and a leading `~`.
108fn expand(text: &str) -> String {
109    let mut out = String::with_capacity(text.len());
110
111    let text = if let Some(rest) = text.strip_prefix('~') {
112        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
113            out.push_str(&home.to_string_lossy());
114        }
115        rest
116    } else {
117        text
118    };
119
120    let mut rest = text;
121    while let Some(open) = rest.find('%') {
122        out.push_str(&rest[..open]);
123        let after = &rest[open + 1..];
124        match after.find('%') {
125            Some(close) => {
126                let name = &after[..close];
127                match std::env::var_os(name) {
128                    Some(value) => out.push_str(&value.to_string_lossy()),
129                    // Leave an unknown variable as it was written, so the user can
130                    // see what did not resolve instead of watching it vanish.
131                    None => {
132                        out.push('%');
133                        out.push_str(name);
134                        out.push('%');
135                    }
136                }
137                rest = &after[close + 1..];
138            }
139            None => {
140                out.push('%');
141                out.push_str(after);
142                rest = "";
143            }
144        }
145    }
146    out.push_str(rest);
147    out
148}
149
150/// Split a path into the segments a breadcrumb shows, each with the path that
151/// reaching it navigates to.
152///
153/// The first segment is always "This PC", so a breadcrumb is a route from the
154/// machine rather than from an arbitrary root.
155pub fn breadcrumb_segments(path: &Path) -> Vec<(String, PathBuf)> {
156    let mut out = vec![("This PC".to_owned(), PathBuf::new())];
157    if path.as_os_str().is_empty() {
158        return out;
159    }
160
161    let mut walked = PathBuf::new();
162    for component in path.components() {
163        use std::path::Component;
164        match component {
165            Component::Prefix(prefix) => {
166                walked.push(prefix.as_os_str());
167                // A prefix on its own is not a directory: `C:` means "the current
168                // directory on C:". The root component that follows completes it,
169                // so the label is deferred until then.
170            }
171            Component::RootDir => {
172                walked.push(std::path::MAIN_SEPARATOR_STR);
173                out.push((display_name(&walked), walked.clone()));
174            }
175            Component::Normal(name) => {
176                walked.push(name);
177                out.push((name.to_string_lossy().into_owned(), walked.clone()));
178            }
179            // `.` and `..` cannot appear: every path here has been resolved.
180            Component::CurDir | Component::ParentDir => {}
181        }
182    }
183    out
184}
185
186#[cfg(test)]
187mod tests {
188    use super::*;
189
190    #[test]
191    fn this_pc_is_the_top() {
192        assert_eq!(parent_of(&PathBuf::new()), None);
193    }
194
195    #[test]
196    #[cfg(windows)]
197    fn a_path_from_outside_comes_back_in_the_form_the_shell_parses() {
198        // The failure this prevents is silent: a forward-slash path lists and navigates
199        // perfectly and then has no shell context menu at all, because
200        // `SHParseDisplayName` will not parse it.
201        assert_eq!(
202            normalize(Path::new("D:/Sources/MyTools")),
203            PathBuf::from("D:\\Sources\\MyTools")
204        );
205        assert_eq!(
206            normalize(Path::new("//server/share/x")),
207            PathBuf::from("\\\\server\\share\\x")
208        );
209        // Already right: returned unchanged, allocation and all.
210        let plain = Path::new("C:\\Users\\tony");
211        assert_eq!(normalize(plain), plain);
212    }
213
214    #[test]
215    #[cfg(windows)]
216    fn a_typed_path_is_normalised_on_the_way_in() {
217        let temp = std::env::temp_dir();
218        let slashed = temp.to_string_lossy().replace('\\', "/");
219        let resolved = resolve_input(&slashed).expect("the temp directory exists");
220        assert!(
221            !resolved.to_string_lossy().contains('/'),
222            "`{}` still has a forward slash in it",
223            resolved.display()
224        );
225    }
226
227    #[test]
228    #[cfg(windows)]
229    fn a_drive_root_goes_up_to_this_pc() {
230        assert_eq!(
231            parent_of(Path::new("C:\\")),
232            Some(PathBuf::new()),
233            "Up from a drive root has to reach This PC, not stop"
234        );
235        assert_eq!(
236            parent_of(Path::new("C:\\Users\\tony")),
237            Some(PathBuf::from("C:\\Users"))
238        );
239    }
240
241    #[test]
242    #[cfg(windows)]
243    fn breadcrumbs_start_at_this_pc_and_keep_the_drive() {
244        let crumbs = breadcrumb_segments(Path::new("C:\\Users\\tony\\Documents"));
245        let labels: Vec<&str> = crumbs.iter().map(|(l, _)| l.as_str()).collect();
246        assert_eq!(labels, ["This PC", "C:", "Users", "tony", "Documents"]);
247        assert_eq!(crumbs[1].1, PathBuf::from("C:\\"));
248        assert_eq!(crumbs[4].1, PathBuf::from("C:\\Users\\tony\\Documents"));
249    }
250
251    #[test]
252    fn breadcrumbs_of_this_pc_are_just_this_pc() {
253        assert_eq!(breadcrumb_segments(&PathBuf::new()).len(), 1);
254    }
255
256    #[test]
257    fn unknown_variables_survive_expansion() {
258        assert_eq!(expand("%NOT_A_REAL_VAR_XYZ%\\x"), "%NOT_A_REAL_VAR_XYZ%\\x");
259        assert_eq!(expand("plain"), "plain");
260        assert_eq!(expand("50% done"), "50% done");
261    }
262
263    #[test]
264    fn resolve_understands_this_pc_and_bare_drives() {
265        assert_eq!(resolve_input("  This PC "), Some(PathBuf::new()));
266        assert_eq!(resolve_input(""), None);
267        #[cfg(windows)]
268        assert_eq!(resolve_input("C:"), Some(PathBuf::from("C:\\")));
269    }
270}
