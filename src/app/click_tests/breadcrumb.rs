//! The breadcrumb's chevrons and the dropdowns they open.

use super::*;

#[test]
fn a_breadcrumb_chevron_opens_the_folder_it_points_at() {
    // It could not, and nothing about the code said so: the dropdown was built only
    // when it was *already* open, and the thing that opens it is the same call that
    // builds it. So this asserts the popup is open after the click and that the folder
    // behind it was actually read.
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
    assert!(crumbs.len() > 2, "the test folder has a path to walk");

    // The chevron before the last segment: its menu lists the parent's subfolders,
    // one of which is the folder now open.
    let index = crumbs.len() - 1;
    let id = Id::new(("crumb-chevron", pane, index));
    let y = h.path_bar_y(0);
    let at = (0..1200)
        .step_by(2)
        .map(|x| pos2(x as f32, y))
        .find(|at| h.hovers(id, *at))
        .expect("the chevron before the current folder is not reachable");

    h.wait();
    h.click_at(at);
    h.frame(Vec::new());

    assert_eq!(
        h.app.crumbs.showing(),
        Some((pane, index)),
        "clicking the chevron did not open anything"
    );
    let (read, count) = h
        .app
        .crumbs
        .listing()
        .expect("the dropdown never read a folder");
    assert_eq!(read, crumbs[index - 1].1, "it read the wrong folder");
    assert!(count > 0, "this crate's parent has subfolders in it");
}

/// An open chevron is welded to the segment before it: one fill over both.
///
/// They are one control — the chevron lists that folder's subfolders, so the pair reads
/// "this folder, and what is inside it" — and a fill each would put a seam down the middle
/// of it. The rect is what makes this checkable: no arrangement of two fills lands a single
/// rectangle on the union of the two.
#[test]
fn an_open_chevron_and_the_name_before_it_are_highlighted_together() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
    let index = crumbs.len() - 1;

    let y = h.path_bar_y(0);
    let at = (0..1200)
        .step_by(2)
        .map(|x| pos2(x as f32, y))
        .find(|at| h.hovers(Id::new(("crumb-chevron", pane, index)), *at))
        .expect("the chevron before the current folder is not reachable");
    h.wait();
    h.click_at(at);
    h.frame(Vec::new());
    assert_eq!(h.app.crumbs.showing(), Some((pane, index)), "nothing opened");

    let of = |id: Id| {
        h.ctx
            .read_response(id)
            .map(|r| r.rect)
            .unwrap_or_else(|| panic!("{id:?} was not drawn"))
    };
    let text = of(Id::new(("crumb", pane, index - 1)));
    let chevron = of(Id::new(("crumb-chevron", pane, index)));

    // In the hover grey, not the pressed one: the pair, the dropdown's own entries and a
    // plain hover on the bar are one gesture and wear one colour.
    assert_eq!(
        h.fill_at(text.union(chevron)).map(|(_, fill)| fill),
        Some(crate::ui::hover_fill(&h.app.theme)),
        "the open chevron and the name before it are not one shape"
    );
}

/// Once a dropdown is open the whole bar is one control: the pointer carries it along.
///
/// Explorer's address bar does this, and it is the difference between reading a menu to find
/// a sibling folder and running the pointer along the trail until the right list appears.
/// Hovering a *name* opens the chevron that belongs to it — the one after it, which lists
/// that folder's subfolders — so the name and its chevron are one target in both directions.
#[test]
fn with_a_dropdown_open_hovering_the_bar_moves_it() {
    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let crumbs = crate::fs::breadcrumb_segments(&h.app.panes[0].tab().path);
    assert!(crumbs.len() > 3, "the test folder has a path to walk");
    let index = crumbs.len() - 1;

    let y = h.path_bar_y(0);
    let sweep = |h: &mut Harness, id: Id| {
        (0..1200)
            .step_by(2)
            .map(|x| pos2(x as f32, y))
            .find(|at| h.hovers(id, *at))
            .unwrap_or_else(|| panic!("{id:?} is not reachable"))
    };

    // Open the last chevron, the ordinary way.
    let at = sweep(&mut h, Id::new(("crumb-chevron", pane, index)));
    h.wait();
    h.click_at(at);
    h.frame(Vec::new());
    assert_eq!(h.app.crumbs.showing(), Some((pane, index)));

    // Now hover a name further back along the trail. No click.
    let target = index - 2;
    let over = sweep(&mut h, Id::new(("crumb", pane, target)));
    h.frame(vec![Event::PointerMoved(over)]);
    // One more, because the switch is applied at the end of the frame that notices it: the
    // dropdown going away is already painted by then.
    h.frame(Vec::new());

    assert_eq!(
        h.app.crumbs.showing(),
        Some((pane, target + 1)),
        "the dropdown did not follow the pointer to the name at {target}"
    );
    let (read, _) = h
        .app
        .crumbs
        .listing()
        .expect("the dropdown that moved never read a folder");
    assert_eq!(
        read, crumbs[target].1,
        "it moved to the chevron of the wrong name"
    );

    // And it lets go when something is clicked. The name under the pointer is a link, so
    // this also navigates — which is the same click doing both, as it does in Explorer.
    h.click_at(over);
    assert_eq!(
        h.app.crumbs.showing(),
        None,
        "the bar is still tracking after a click"
    );
}

/// The bar says where a click will open the path field: a pen, and a border.
///
/// The pointer already turned into an I-beam over the empty space past the last segment and
/// the bar itself said nothing that could be seen — its hairline was `stroke-subtle`, which
/// is the colour the bar is *painted*, so it was a line drawn in the colour behind it.
///
/// Two cues now, sharing one ink. The **pen** is always there, because a hint that appears
/// only once the pointer has arrived is not a hint; the **border** appears with the pointer,
/// over the whole shape the field will take, and the pen brightens to match it. No fill:
/// washing the bar to announce something that is only an announcement was too much.
#[test]
fn the_breadcrumb_shows_where_the_path_field_opens() {
    use crate::ui::breadcrumb::pen_ink;

    let mut h = Harness::new();
    let pane = h.app.panes[0].id;
    let (rest, lit) = (pen_ink(&h.app.theme, false), pen_ink(&h.app.theme, true));
    assert_ne!(rest, lit, "the two states of the hint are the same colour");
    assert_ne!(
        rest,
        crate::ui::seam(&h.app.theme),
        "the hint is drawn in the colour of the bar it is drawn on"
    );

    let of = |id: Id| {
        h.ctx
            .read_response(id)
            .map(|r| r.rect)
            .unwrap_or_else(|| panic!("{id:?} was not drawn"))
    };
    let empty = of(Id::new(("crumb-empty", pane)));
    // The pen sits at the right-hand end of the bar, in the room reserved out of the trail's.
    // A couple of points of slack around it: the glyph fills its box corner to corner.
    let pen = Rect::from_center_size(
        pos2(empty.right() - crate::ui::breadcrumb::PEN * 0.5, empty.center().y),
        egui::Vec2::splat(crate::ui::breadcrumb::PEN + 6.0),
    );
    // The field's own shape: the whole path area, so it ends where the bar does and starts
    // well before the empty part the pointer is over.
    let bar_wide = |(rect, _): &(Rect, egui::Color32)| {
        (rect.right() - empty.right()).abs() < 0.5 && rect.left() < empty.left() - 1.0
    };

    // ---- At rest: the pen, and no border ------------------------------
    assert_eq!(
        h.glyph_inks(pen),
        vec![rest, rest],
        "the pen is not drawn at rest, or not in the resting ink"
    );

    // **Centred on the row by its ink**, which is not the same claim as its box being
    // centred: the box was right the whole time the drawing inside it sat two units low.
    // See `icons::pencil`, and the rule this is an instance of — anything in a row is
    // vertically centred, and text on its baseline.
    let ink = h.glyph_bounds(pen).expect("the pen's own shapes");
    let off = ink.center().y - empty.center().y;
    assert!(
        off.abs() < 0.5,
        "the pen's ink sits {off:+.2} points off the middle of the bar ({:?} in {:?})",
        ink,
        empty
    );
    assert!(
        !h.outlines().iter().any(bar_wide),
        "the bar is outlined before the pointer is anywhere near it"
    );

    // ---- Under the pointer: both, in the lit ink ----------------------
    h.frame(vec![Event::PointerMoved(pos2(
        empty.center().x,
        h.path_bar_y(0),
    ))]);
    assert_eq!(
        h.cursor,
        egui::CursorIcon::Text,
        "this is not the part of the bar that opens the field"
    );
    assert_eq!(
        h.glyph_inks(pen),
        vec![lit, lit],
        "the pen did not light up with the border"
    );
    let outline = h
        .outlines()
        .into_iter()
        .find(bar_wide)
        .expect("the pointer says the field opens here and the bar does not");
    assert_eq!(outline.1, lit, "the border is not the hint's own ink");
    assert!(
        outline.0.contains(pen.center()),
        "the border stops short of the pen: {:?} against {:?}",
        outline.0,
        pen
    );

    // And no fill: the bar does not change colour to say this. Against the hover grey, which is
    // what a segment of the bar is washed with -- naming `control_fills`'s hover, as this once
    // did, stopped naming a colour the bar is ever painted, and the assertion passed because
    // nothing could match it rather than because nothing was washed.
    let hover = crate::ui::hover_fill(&h.app.theme);
    assert!(
        !h.rects()
            .into_iter()
            .any(|(rect, _, fill)| fill == hover && bar_wide(&(rect, fill))),
        "the bar is washed as well as outlined"
    );
}

/// A field's border lights up wherever the pointer is over it, not only over its icon.
///
/// The design system's fault, and worth a test here because this is where it showed: a
/// field's wrapper is allocated *before* the `TextEdit` that goes inside it, so the edit is
/// the topmost widget over the text area and took the hover from it. The border therefore lit
/// up only where the edit was not — the prefix icon and the padding — so hovering the part of
/// the filter box you type in did nothing, which reads as a control that does not respond.
///
/// Asserted at the *far* end of the box from its icon, which is the part that was dead.
#[test]
fn a_field_lights_up_over_all_of_itself() {
    let mut h = Harness::new();
    let y = h.path_bar_y(0);
    let right = h.pane_rect(0).right();

    // The run of x where the pointer says "text", coming in from the right-hand end of the
    // bar: the filter box. Found by sweeping rather than by deriving its rect, for the same
    // reason `arriving_in_a_field_selects_what_is_there` does it — the box's own id is
    // generated inside the design system.
    let mut run: Vec<f32> = Vec::new();
    for dx in (8..400).step_by(2) {
        let at = pos2(right - dx as f32, y);
        h.frame(vec![Event::PointerMoved(at)]);
        if h.cursor == egui::CursorIcon::Text {
            run.push(at.x);
        } else if !run.is_empty() {
            break;
        }
    }
    assert!(run.len() > 8, "the filter box is not reachable by the pointer");

    // The rightmost end of the run: the text area. The icon is at the other end, and the
    // clearable ✕ only exists once something has been typed.
    let text_area = pos2(run[0], y);
    h.frame(vec![Event::PointerMoved(text_area)]);
    let border = |h: &Harness, at: Pos2| {
        h.outlines()
            .into_iter()
            .find(|(rect, _)| rect.contains(at))
            .map(|(_, color)| color)
    };
    assert_eq!(
        border(&h, text_area),
        Some(h.app.theme.stroke.strong),
        "hovering the part of the field you type in did not light its border"
    );

    // And it goes out again, which is what makes the assertion above about the hover rather
    // than about the border always being that colour.
    h.frame(vec![Event::PointerMoved(pos2(right - 500.0, y))]);
    assert_eq!(
        border(&h, text_area),
        Some(h.app.theme.stroke.control),
        "the field stayed lit with the pointer somewhere else"
    );
}
