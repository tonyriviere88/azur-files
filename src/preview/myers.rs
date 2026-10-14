//! Myers' shortest edit script, over anything that can be compared for equality.
//!
//! A pure algorithm, with no idea what it is comparing: the lines of two files in [`super::diff`], and
//! the words and letters of two versions of a line in `crate::ui::preview::diff`. Kept apart from both
//! so neither has to reach into the other for it.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

/// How many edits [`diff`] will look for before it gives up.
///
/// **What bounds the memory**, not the time: the walk keeps every round's furthest reaches to trace
/// the path back through, which is `EDITS²` entries — 16 MB at this figure, for as long as the diff
/// takes. Two sequences that differ in more places than this are two different sequences, and whoever
/// asked has a better answer for that than a script.
const EDITS: usize = 2000;

/// One step of an edit script: an item both have, one only the first has, one only the second has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Op {
    Keep,
    Remove,
    Add,
}

/// The shortest edit script from `a` to `b`, or `None` past [`EDITS`].
///
/// Each distinct item is given a number first, so a step along a diagonal is an integer compare
/// rather than a string one.
pub(crate) fn diff<T: Eq + Hash>(a: &[T], b: &[T]) -> Option<Vec<Op>> {
    fn numbered<'a, T: Eq + Hash>(ids: &mut HashMap<&'a T, u32>, side: &'a [T]) -> Vec<u32> {
        side.iter()
            .map(|item| {
                let next = ids.len() as u32;
                *ids.entry(item).or_insert(next)
            })
            .collect()
    }
    let mut ids = HashMap::new();
    let x = numbered(&mut ids, a);
    let y = numbered(&mut ids, b);
    walk(&x, &y)
}

/// The script's changed stretches: for each run of edits between two kept items, the range of `a` it
/// takes away and the range of `b` it puts in. Either may be empty.
pub(crate) fn runs(ops: &[Op]) -> Vec<(Range<usize>, Range<usize>)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    for stretch in ops.split(|&op| op == Op::Keep) {
        let removes = stretch.iter().filter(|&&op| op == Op::Remove).count();
        let adds = stretch.len() - removes;
        if !stretch.is_empty() {
            out.push((i..i + removes, j..j + adds));
        }
        // Past the stretch, and past the kept item that ended it.
        (i, j) = (i + removes + 1, j + adds + 1);
    }
    out
}

/// Forward Myers, keeping each round's furthest reaches — the diagonals `-d..=d` only, which is what
/// the trace back reads — and then walking back from the far corner.
fn walk(a: &[u32], b: &[u32]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let limit = (n + m).min(EDITS as isize);
    let off = limit + 1;
    let at = |k: isize| (k + off) as usize;
    // Whether diagonal `k` is reached from the one above, given each diagonal's reach.
    let from_above = |k: isize, d: isize, reach: &dyn Fn(isize) -> isize| {
        k == -d || (k != d && reach(k - 1) < reach(k + 1))
    };
    // `v[at(k)]` is the furthest `x` reached on diagonal `k = x - y`.
    let mut v = vec![0i32; (2 * off + 1) as usize];
    // Every round's reaches, one after another: round `d` is `2d + 1` of them, starting at `d²`.
    let mut trace: Vec<i32> = Vec::new();
    let mut found = None;
    'rounds: for d in 0..=limit {
        trace.extend_from_slice(&v[at(-d)..=at(d)]);
        for k in (-d..=d).step_by(2) {
            let above = from_above(k, d, &|k| v[at(k)] as isize);
            let mut x = if above { v[at(k + 1)] as isize } else { v[at(k - 1)] as isize + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x as i32;
            if x >= n && y >= m {
                found = Some(d);
                break 'rounds;
            }
        }
    }
    let last = found?;

    let mut ops = Vec::with_capacity((n + m) as usize);
    let (mut x, mut y) = (n, m);
    for d in (1..=last).rev() {
        // The reaches as they stood when round `d` began, indexed from diagonal `-d`.
        let before = &trace[(d * d) as usize..];
        let reach = |k: isize| before[(k + d) as usize] as isize;
        let k = x - y;
        let above = from_above(k, d, &reach);
        let prev_k = if above { k + 1 } else { k - 1 };
        let prev_x = reach(prev_k);
        let prev_y = prev_x - prev_k;
        // The diagonal run that followed the edit, then the edit itself.
        let (start_x, start_y) = if above { (prev_x, prev_y + 1) } else { (prev_x + 1, prev_y) };
        while x > start_x && y > start_y {
            ops.push(Op::Keep);
            x -= 1;
            y -= 1;
        }
        ops.push(if above { Op::Add } else { Op::Remove });
        x = prev_x;
        y = prev_y;
    }
    // Round nought is a diagonal run from the corner and nothing else.
    while x > 0 && y > 0 {
        ops.push(Op::Keep);
        x -= 1;
        y -= 1;
    }
    ops.reverse();
    Some(ops)
}
