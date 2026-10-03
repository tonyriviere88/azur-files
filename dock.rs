1//! The layout tree: how the panes divide the window.
2//!
3//! A binary tree of splits, which is the smallest structure that expresses
4//! everything dragging a tab to an edge can ask for — side by side, stacked, and
5//! any nesting of the two — while still having exactly one arrangement per state,
6//! so there is nothing to normalise and nothing to get out of sync.
7//!
8//! Splits are addressed by the route taken to reach them (`[0, 1]` = the second
9//! child of the first child) rather than by an id of their own. A route is derived
10//! from the tree during layout and used in the same frame, so it cannot go stale —
11//! whereas an id would need allocating, storing and reclaiming on every collapse.
12
13use egui::{pos2, vec2, Pos2, Rect};
14
15use crate::pane::{PaneId, Side};
16
17/// What shows between two panes: one point of [`crate::ui::seam`], the same line the sidebar
18/// is separated by.
19///
20/// It was `tokens::space::S2` — four points of canvas, which read as a gap between two cards.
21/// A window divided into panes is not a row of cards, so what divides them is a line.
22pub const GAP: f32 = crate::ui::SEAM;
23
24/// How wide a splitter is to the pointer. Far wider than the line it sits on, because a
25/// one-pixel grab target is not a grab target.
26const GRAB: f32 = 4.0;
27
28/// One node of the layout.
29pub enum Node {
30    Leaf(PaneId),
31    Split {
32        /// Children side by side, rather than stacked.
33        horizontal: bool,
34        /// Fraction of the space the first child gets, in [`RATIO_MIN`]`..=`[`RATIO_MAX`].
35        ratio: f32,
36        first: Box<Node>,
37        second: Box<Node>,
38    },
39}
40
41/// A draggable divider, resolved for one frame.
42pub struct Splitter {
43    /// The grab area, which is wider than the visible gap.
44    pub rect: Rect,
45    pub horizontal: bool,
46    /// Route from the root: `0` = first child, `1` = second.
47    pub route: Vec<u8>,
48}
49
50/// What dropping a dragged tab on a pane would do.
51#[derive(Clone, Copy, PartialEq, Eq, Debug)]
52pub enum Zone {
53    /// Add the tab to this pane's own strip.
54    Into,
55    /// Split the pane, putting the tab on this side.
56    Split(Side),
57}
58
59impl Node {
60    /// Divide `rect` up, collecting where each pane goes and where each divider is.
61    pub fn layout(&self, rect: Rect, panes: &mut Vec<(PaneId, Rect)>, splitters: &mut Vec<Splitter>) {
62        panes.clear();
63        splitters.clear();
64        let mut route = Vec::new();
65        self.layout_into(rect, &mut route, panes, splitters);
66    }
67
68    fn layout_into(
69        &self,
70        rect: Rect,
71        route: &mut Vec<u8>,
72        panes: &mut Vec<(PaneId, Rect)>,
73        splitters: &mut Vec<Splitter>,
74    ) {
75        match self {
76            Self::Leaf(id) => panes.push((*id, rect)),
77            Self::Split {
78                horizontal,
79                ratio,
80                first,
81                second,
82            } => {
83                let (a, b, bar) = if *horizontal {
84                    let usable = (rect.width() - GAP).max(0.0);
85                    // Rounded so the divider lands on a whole pixel and its 1px
86                    // edges do not go grey.
87                    let width = (usable * ratio).round();
88                    let a = Rect::from_min_max(rect.min, pos2(rect.left() + width, rect.bottom()));
89                    let b = Rect::from_min_max(pos2(a.right() + GAP, rect.top()), rect.max);
90                    let bar = Rect::from_min_max(
91                        pos2(a.right() - GRAB, rect.top()),
92                        pos2(b.left() + GRAB, rect.bottom()),
93                    );
94                    (a, b, bar)
95                } else {
96                    let usable = (rect.height() - GAP).max(0.0);
97                    let height = (usable * ratio).round();
98                    let a = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + height));
99                    let b = Rect::from_min_max(pos2(rect.left(), a.bottom() + GAP), rect.max);
100                    let bar = Rect::from_min_max(
101                        pos2(rect.left(), a.bottom() - GRAB),
102                        pos2(rect.right(), b.top() + GRAB),
103                    );
104                    (a, b, bar)
105                };
106
107                splitters.push(Splitter {
108                    rect: bar,
109                    horizontal: *horizontal,
110                    route: route.clone(),
111                });
112
113                route.push(0);
114                first.layout_into(a, route, panes, splitters);
115                route.pop();
116                route.push(1);
117                second.layout_into(b, route, panes, splitters);
118                route.pop();
119            }
120        }
121    }
122
123    /// The ratio of the split at `route`, for a divider drag.
124    pub fn ratio_at(&mut self, route: &[u8]) -> Option<&mut f32> {
125        let mut node = self;
126        for &step in route {
127            let Self::Split { first, second, .. } = node else {
128                return None;
129            };
130            node = if step == 0 { first } else { second };
131        }
132        match node {
133            Self::Split { ratio, .. } => Some(ratio),
134            Self::Leaf(_) => None,
135        }
136    }
137
138    /// Split the pane holding `target`, putting `added` on `side` of it.
139    pub fn split(&mut self, target: PaneId, side: Side, added: PaneId) -> bool {
140        match self {
141            Self::Leaf(id) if *id == target => {
142                let existing = Self::Leaf(*id);
143                let new = Self::Leaf(added);
144                let (first, second) = if side.is_first() {
145                    (new, existing)
146                } else {
147                    (existing, new)
148                };
149                *self = Self::Split {
150                    horizontal: side.is_horizontal(),
151                    ratio: 0.5,
152                    first: Box::new(first),
153                    second: Box::new(second),
154                };
155                true
156            }
157            Self::Leaf(_) => false,
158            Self::Split { first, second, .. } => {
159                first.split(target, side, added) || second.split(target, side, added)
160            }
161        }
162    }
163
164    /// Take a pane out, collapsing the split it was half of.
165    ///
166    /// Returns `false` when `pane` is the root — the caller has to decide what an
167    /// empty window means, and here it means the last pane cannot be closed.
168    pub fn remove(&mut self, pane: PaneId) -> bool {
169        let Self::Split { first, second, .. } = self else {
170            return false;
171        };
172
173        for which in [0, 1] {
174            let child = if which == 0 { &**first } else { &**second };
175            if matches!(child, Self::Leaf(id) if *id == pane) {
176                // Promote the sibling into this node's place.
177                let sibling = if which == 0 { second } else { first };
178                let promoted = std::mem::replace(&mut **sibling, Self::Leaf(pane));
179                *self = promoted;
180                return true;
181            }
182        }
183        first.remove(pane) || second.remove(pane)
184    }
185
186    /// Every pane, in layout order.
187    pub fn panes(&self, out: &mut Vec<PaneId>) {
188        match self {
189            Self::Leaf(id) => out.push(*id),
190            Self::Split { first, second, .. } => {
191                first.panes(out);
192                second.panes(out);
193            }
194        }
195    }
196
197    /// How many panes the tree holds.
198    ///
199    /// Only the tests ask: they assert on the shape of the tree itself, which is a
200    /// stronger claim than the length of the pane list built from it. Nothing in the
201    /// window needs it — a pane count is `panes.len()` there.
202    #[cfg(test)]
203    pub fn count(&self) -> usize {
204        match self {
205            Self::Leaf(_) => 1,
206            Self::Split { first, second, .. } => first.count() + second.count(),
207        }
208    }
209
210    // ---- The settings file ---------------------------------------------
211    //
212    // A tree in one line, so that the next launch comes up with the panes this one had
213    // rather than with every tab stacked in a single pane:
214    //
215    // ```text
216    // h0.500(0,v0.667(1,2))
217    // ```
218    //
219    // `h` and `v` are how the split divides its space, the number after it is the share
220    // the first child gets, and a bare integer is a pane. **Panes are numbered by their
221    // position in layout order, not by [`PaneId`]** — an id is a counter that runs for the
222    // life of one window and means nothing to the next one, whereas "the second pane from
223    // the left" survives being written down. `panes` walks the tree in the same order this
224    // numbers it, which is what lets the caller line the two up.
225    //
226    // Hand-rolled, as the rest of the settings file is, and for the same reason: the
227    // grammar is four productions and the parser below is shorter than the derive and the
228    // format crate a serialised version would need.
229
230    /// Write the tree out for the settings file.
231    pub fn encode(&self) -> String {
232        let mut text = String::new();
233        self.write_into(&mut text, &mut 0);
234        text
235    }
236
237    fn write_into(&self, text: &mut String, next: &mut usize) {
238        use std::fmt::Write as _;
239        match self {
240            Self::Leaf(_) => {
241                let _ = write!(text, "{next}");
242                *next += 1;
243            }
244            Self::Split {
245                horizontal,
246                ratio,
247                first,
248                second,
249            } => {
250                let _ = write!(text, "{}{ratio:.3}(", if *horizontal { 'h' } else { 'v' });
251                first.write_into(text, next);
252                text.push(',');
253                second.write_into(text, next);
254                text.push(')');
255            }
256        }
257    }
258
259    /// Read one back, giving the pane numbered `n` the id `ids[n]`.
260    ///
261    /// `None` for anything that is not a tree over exactly these panes — a truncated line, a
262    /// pane named twice, a pane not named at all, a number with no pane to go with it. All of
263    /// those are one answer rather than several because there is only one thing to do about
264    /// them: open the window the way a first run would. A settings file is not worth refusing
265    /// to start over, and a *partly* restored layout would be worse than an honest default —
266    /// it would be a window missing one of the folders that were open in it.
267    pub fn decode(text: &str, ids: &[PaneId]) -> Option<Self> {
268        /// How deeply a settings file may nest. Reached only by a file that was written by
269        /// hand, and the reason for the limit is that this parser recurses: without it, a
270        /// line of ten thousand `h0.5(` would overflow the stack rather than be rejected.
271        const DEPTH: usize = 32;
272
273        fn parse(rest: &mut &str, ids: &[PaneId], taken: &mut [bool], depth: usize) -> Option<Node> {
274            if depth > DEPTH {
275                return None;
276            }
277            let horizontal = match rest.as_bytes().first()? {
278                b'h' => true,
279                b'v' => false,
280                _ => {
281                    // A pane: its number, and then whatever follows it.
282                    let end = rest
283                        .find(|c: char| !c.is_ascii_digit())
284                        .unwrap_or(rest.len());
285                    let which: usize = rest[..end].parse().ok()?;
286                    *rest = &rest[end..];
287                    let id = *ids.get(which)?;
288                    // Named twice: not a tree. Two leaves for one pane would draw the same
289                    // pane in two places and every gesture would find the first of them.
290                    if std::mem::replace(taken.get_mut(which)?, true) {
291                        return None;
292                    }
293                    return Some(Node::Leaf(id));
294                }
295            };
296            *rest = &rest[1..];
297
298            // The ratio runs up to this split's own bracket. Nothing before it can contain
299            // one, because a ratio is digits and a point.
300            let open = rest.find('(')?;
301            let ratio: f32 = rest[..open].parse().ok()?;
302            // `"NaN".parse::<f32>()` succeeds, and a NaN ratio lays out a pane of NaN width
303            // that never comes back. The clamp cannot catch it — `f32::clamp` passes NaN
304            // through — so it is refused here.
305            if !ratio.is_finite() {
306                return None;
307            }
308            *rest = &rest[open + 1..];
309
310            let first = parse(rest, ids, taken, depth + 1)?;
311            *rest = rest.strip_prefix(',')?;
312            let second = parse(rest, ids, taken, depth + 1)?;
313            *rest = rest.strip_prefix(')')?;
314
315            Some(Node::Split {
316                horizontal,
317                ratio: ratio.clamp(RATIO_MIN, RATIO_MAX),
318                first: Box::new(first),
319                second: Box::new(second),
320            })
321        }
322
323        let mut taken = vec![false; ids.len()];
324        let mut rest = text.trim();
325        let tree = parse(&mut rest, ids, &mut taken, 0)?;
326        // Every pane accounted for, and nothing left over. A pane with no leaf would be a
327        // folder the user had open and cannot see; a tail would mean this parsed something
328        // other than what was written.
329        if !rest.is_empty() || taken.iter().any(|&used| !used) {
330            return None;
331        }
332        Some(tree)
333    }
334}
335
336/// The narrowest share a split will give a pane, and its mirror.
337///
338/// What a splitter drag clamps to, and so what [`Node::decode`] accepts: a ratio outside
339/// this is one no gesture in this window can produce, and a pane at 0.02 of the window is a
340/// pane you cannot find the edge of again.
341pub const RATIO_MIN: f32 = 0.12;
342pub const RATIO_MAX: f32 = 0.88;
343
344/// Which part of a pane the pointer is over, for a tab being dragged.
345///
346/// The edge bands are a fraction of the pane rather than a fixed size, so the
347/// gesture feels the same in a narrow pane as in a wide one — but clamped, because
348/// a 30% band on a 2000-pixel pane would mean the centre is unreachable, and a 30%
349/// band on a 200-pixel one would be too small to hit.
350pub fn zone_at(pane: Rect, pointer: Pos2) -> Zone {
351    let band = |extent: f32| (extent * 0.30).clamp(28.0, 140.0);
352    let (bx, by) = (band(pane.width()), band(pane.height()));
353
354    let from_left = pointer.x - pane.left();
355    let from_right = pane.right() - pointer.x;
356    let from_top = pointer.y - pane.top();
357    let from_bottom = pane.bottom() - pointer.y;
358
359    // Nearest edge wins, so the corners resolve to whichever side the pointer is
360    // actually closer to instead of to whichever test ran first.
361    let mut best: Option<(f32, Side)> = None;
362    for (distance, limit, side) in [
363        (from_left, bx, Side::Left),
364        (from_right, bx, Side::Right),
365        (from_top, by, Side::Top),
366        (from_bottom, by, Side::Bottom),
367    ] {
368        if distance < limit && best.is_none_or(|(d, _)| distance < d) {
369            best = Some((distance, side));
370        }
371    }
372
373    match best {
374        Some((_, side)) => Zone::Split(side),
375        None => Zone::Into,
376    }
377}
378
379/// The rect a drop preview should highlight.
380pub fn preview_rect(pane: Rect, zone: Zone) -> Rect {
381    match zone {
382        Zone::Into => pane,
383        Zone::Split(side) => {
384            let half = |extent: f32| (extent - GAP) * 0.5;
385            match side {
386                Side::Left => Rect::from_min_size(pane.min, vec2(half(pane.width()), pane.height())),
387                Side::Right => Rect::from_min_max(
388                    pos2(pane.right() - half(pane.width()), pane.top()),
389                    pane.max,
390                ),
391                Side::Top => Rect::from_min_size(pane.min, vec2(pane.width(), half(pane.height()))),
392                Side::Bottom => Rect::from_min_max(
393                    pos2(pane.left(), pane.bottom() - half(pane.height())),
394                    pane.max,
395                ),
396            }
397        }
398    }
399}
400
401#[cfg(test)]
402mod tests {
403    use super::*;
404
405    fn ids(node: &Node) -> Vec<PaneId> {
406        let mut out = Vec::new();
407        node.panes(&mut out);
408        out
409    }
410
411    #[test]
412    fn splitting_right_puts_the_new_pane_second() {
413        let mut tree = Node::Leaf(1);
414        assert!(tree.split(1, Side::Right, 2));
415        assert_eq!(ids(&tree), [1, 2]);
416    }
417
418    #[test]
419    fn splitting_left_puts_the_new_pane_first() {
420        let mut tree = Node::Leaf(1);
421        assert!(tree.split(1, Side::Left, 2));
422        assert_eq!(ids(&tree), [2, 1]);
423    }
424
425    #[test]
426    fn splitting_a_nested_pane_finds_it() {
427        let mut tree = Node::Leaf(1);
428        tree.split(1, Side::Right, 2);
429        assert!(tree.split(2, Side::Bottom, 3));
430        assert_eq!(ids(&tree), [1, 2, 3]);
431        assert_eq!(tree.count(), 3);
432    }
433
434    #[test]
435    fn removing_collapses_the_split() {
436        let mut tree = Node::Leaf(1);
437        tree.split(1, Side::Right, 2);
438        tree.split(2, Side::Bottom, 3);
439
440        assert!(tree.remove(3));
441        assert_eq!(ids(&tree), [1, 2]);
442        assert!(tree.remove(1));
443        assert_eq!(ids(&tree), [2]);
444        assert!(
445            !tree.remove(2),
446            "the last pane has nowhere to collapse into"
447        );
448    }
449
450    #[test]
451    fn layout_leaves_one_gap_between_panes() {
452        let mut tree = Node::Leaf(1);
453        tree.split(1, Side::Right, 2);
454
455        // Width chosen from `GAP` rather than written down, so the two halves stay whole
456        // numbers and the test says something about the split rather than about the constant.
457        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0 + GAP, 200.0));
458        let (mut panes, mut splitters) = (Vec::new(), Vec::new());
459        tree.layout(rect, &mut panes, &mut splitters);
460
461        assert_eq!(panes.len(), 2);
462        assert_eq!(splitters.len(), 1);
463        assert_eq!(panes[0].1.width(), 200.0);
464        assert_eq!(panes[1].1.width(), 200.0);
465        assert_eq!(panes[1].1.left() - panes[0].1.right(), GAP);
466        assert_eq!(panes[0].1.height(), 200.0, "a side split is full height");
467    }
468
469    #[test]
470    fn ratios_are_reachable_by_route() {
471        let mut tree = Node::Leaf(1);
472        tree.split(1, Side::Right, 2);
473        tree.split(2, Side::Bottom, 3);
474
475        let rect = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0));
476        let (mut panes, mut splitters) = (Vec::new(), Vec::new());
477        tree.layout(rect, &mut panes, &mut splitters);
478
479        for splitter in &splitters {
480            assert!(
481                tree.ratio_at(&splitter.route).is_some(),
482                "route {:?} has to lead to the split it came from",
483                splitter.route
484            );
485        }
486        // The nested split is the second child of the root.
487        *tree.ratio_at(&[1]).unwrap() = 0.25;
488        tree.layout(rect, &mut panes, &mut splitters);
489        assert!(panes[1].1.height() < panes[2].1.height());
490    }
491
492    /// A layout survives being written down and read back.
493    ///
494    /// The panes come back in the same order and the splits with the same shape and the same
495    /// ratios — which is the whole claim the settings file makes. Round-tripped rather than
496    /// compared against a literal, because the text is an implementation detail and the
497    /// window the user gets back is not.
498    #[test]
499    fn a_layout_survives_the_settings_file() {
500        let mut tree = Node::Leaf(1);
501        tree.split(1, Side::Right, 2);
502        tree.split(2, Side::Bottom, 3);
503        *tree.ratio_at(&[]).unwrap() = 0.4;
504        *tree.ratio_at(&[1]).unwrap() = 0.75;
505
506        let text = tree.encode();
507        assert_eq!(text, "h0.400(0,v0.750(1,2))", "{text}");
508
509        // Deliberately not the ids it was written with: the file numbers panes by position,
510        // so a fresh window's ids are what it is read back over.
511        let back = Node::decode(&text, &[7, 8, 9]).expect("its own output has to parse");
512        assert_eq!(ids(&back), [7, 8, 9]);
513        assert_eq!(back.encode(), text, "the shape and the ratios both come back");
514    }
515
516    /// Anything that is not a tree over exactly these panes opens as a first run would.
517    ///
518    /// Each of these is a settings file somebody could produce — by hand, by a crash
519    /// half-way through a write, or by opening a file this program wrote and then closing a
520    /// pane in an older build of it. The failure has to be "the window opens plainly",
521    /// never a panic and never a window with one of the open folders missing from it.
522    #[test]
523    fn a_layout_that_is_not_one_is_refused() {
524        let three = [1, 2, 3];
525        for text in [
526            "",
527            "h0.5(0,1",              // truncated
528            "h0.5(0 1)",             // no comma
529            "h0.5(0,1))",            // a tail
530            "x0.5(0,1)",             // not a direction
531            "h(0,1)",                // no ratio
532            "hNaN(0,1)",             // parses as a float and lays out nothing
533            "h0.5(0,0)",             // the same pane twice
534            "h0.5(0,1)",             // pane 2 has nowhere to be drawn
535            "h0.5(0,v0.5(1,9))",     // a pane that does not exist
536        ] {
537            assert!(
538                Node::decode(text, &three).is_none(),
539                "{text:?} was accepted as a layout over three panes"
540            );
541        }
542        // And the one-pane case, which is the shortest legal line there is.
543        assert!(matches!(Node::decode("0", &[4]), Some(Node::Leaf(4))));
544    }
545
546    /// A ratio from outside comes back inside the range a drag can reach.
547    #[test]
548    fn a_ratio_out_of_range_is_brought_back() {
549        let tree = Node::decode("h0.999(0,1)", &[1, 2]).expect("a wild ratio is not a broken file");
550        let Node::Split { ratio, .. } = tree else {
551            panic!("that is a split")
552        };
553        assert_eq!(ratio, RATIO_MAX);
554    }
555
556    /// Nesting deep enough to recurse into the stack is refused rather than run.
557    #[test]
558    fn a_hand_written_file_cannot_overflow_the_stack() {
559        let deep = "h0.5(0,".repeat(10_000);
560        assert!(Node::decode(&deep, &[1, 2]).is_none());
561    }
562
563    #[test]
564    fn the_centre_of_a_pane_is_a_tab_drop() {
565        let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
566        assert_eq!(zone_at(pane, pane.center()), Zone::Into);
567        assert_eq!(zone_at(pane, pos2(5.0, 200.0)), Zone::Split(Side::Left));
568        assert_eq!(zone_at(pane, pos2(595.0, 200.0)), Zone::Split(Side::Right));
569        assert_eq!(zone_at(pane, pos2(300.0, 3.0)), Zone::Split(Side::Top));
570        assert_eq!(zone_at(pane, pos2(300.0, 397.0)), Zone::Split(Side::Bottom));
571    }
572
573    #[test]
574    fn a_corner_resolves_to_the_nearer_edge() {
575        let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0));
576        // Closer to the top than to the left.
577        assert_eq!(zone_at(pane, pos2(40.0, 6.0)), Zone::Split(Side::Top));
578        // And the other way round.
579        assert_eq!(zone_at(pane, pos2(6.0, 40.0)), Zone::Split(Side::Left));
580    }
581
582    #[test]
583    fn preview_of_a_side_split_is_half_the_pane() {
584        // Width from `GAP`, as in `layout_leaves_one_gap_between_panes`: the preview shows what
585        // the split would give, so it takes the same line out of the middle that the split does.
586        let pane = Rect::from_min_size(Pos2::ZERO, vec2(600.0 + GAP, 400.0));
587        let preview = preview_rect(pane, Zone::Split(Side::Right));
588        assert_eq!(preview.width(), 300.0);
589        assert_eq!(preview.right(), 600.0 + GAP);
590        assert_eq!(preview_rect(pane, Zone::Into), pane);
591    }
592}
