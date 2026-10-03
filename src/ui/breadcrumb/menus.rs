//! The dropdowns the bar opens: the overflow crumb, the flatten mode, the filter funnel, and
//! which slash the path field writes.

use super::*;

/// The `…` button's place in [`CrumbMenu::open`].
///
/// It is one of the bar's dropdowns and takes part in the same tracking as the chevrons — it
/// simply has no segment of its own to be numbered after, being what stands in for the ones
/// that did not fit.
pub(crate) const OVERFLOW_MENU: usize = usize::MAX;

pub(crate) const MENU_LIMIT: usize = 200;

/// A menu entry with a tick in its icon slot when it is on.
///
/// The icon slot rather than a checkbox, which is what every desktop menu does with a toggle —
/// and `MenuItem` reserves the slot for every entry, so the labels line up whether or not
/// anything is ticked.
pub(crate) fn ticked<'a>(item: MenuItem<'a>, on: bool) -> MenuItem<'a> {
    if on {
        item.icon(&azur_egui_theme::icons::check)
    } else {
        item
    }
}

/// A folder diff's show button's own menu: the three states by name, the one on show ticked.
pub(crate) fn diff_show_menu(
    ui: &Ui,
    trigger: &egui::Response,
    pane: PaneId,
    show: crate::diff::Show,
    out: &mut Vec<Action>,
) {
    use azur_egui_theme::components::ContextMenu;

    ContextMenu::new(trigger).show(ui.ctx(), |ui| {
        for wants in crate::diff::Show::ALL {
            if ui
                .add(ticked(MenuItem::new(wants.label()), show == wants))
                .clicked()
            {
                out.push(Action::SetDiffShow { pane, show: wants });
            }
        }
    });
}

/// The preview button's own menu: whether the panel is showing, and where it goes.
///
/// **Sticky**, which is `azur::components::ContextMenu`'s word for "these are settings, not
/// commands": ticking one of three positions and having the menu vanish means reopening it to see
/// what you did. A menu of commands should close — dismissal is how a reader knows the command was
/// taken — and this one is not.
///
/// Where the panel goes is the *window's* preference and not this folder's, which is why `layout`
/// comes in from `App` while `open` comes off the tab. A radio group in a menu reads as a setting,
/// and a setting that only applied to the folder you happened to be in when you chose it would be
/// a setting nobody could rely on.
pub(crate) fn position_menu(
    ui: &Ui,
    trigger: &egui::Response,
    pane: PaneId,
    open: bool,
    layout: &mut crate::ui::preview::Layout,
    out: &mut Vec<Action>,
) {
    use azur_egui_theme::components::{collection_label, menu_divider, ContextMenu};

    ContextMenu::new(trigger)
        .sticky(true)
        .show(ui.ctx(), |ui| {
            if ui
                .add(ticked(
                    MenuItem::new("Show preview").shortcut("Ctrl+P"),
                    open,
                ))
                .clicked()
            {
                out.push(Action::TogglePreview(pane));
            }
            menu_divider(ui);
            collection_label(ui, "Position");
            for at in crate::ui::preview::Where::ALL {
                if ui
                    .add(ticked(MenuItem::new(at.label()), layout.at == at))
                    .clicked()
                {
                    layout.at = at;
                    out.push(Action::RememberLayout);
                }
            }
        });
}

/// The flatten button's own menu: whether the tree is showing, and which of the two ways.
///
/// The same shape as [`position_menu`] and for the same reasons — **sticky**, because these are
/// settings rather than commands, and the radio group is the *window's* preference while the
/// toggle above it is this folder's. A right click on the control that opens a thing is where
/// people look for the settings of that thing, and it keeps two radio buttons off a path bar with
/// no room for them.
///
/// Picking a mode does not turn the view on, exactly as picking a preview position does not open
/// the panel: the entry above is what does that, it is right there, and a menu whose settings
/// silently perform actions is a menu you stop opening to look at. What it *does* do is take
/// effect at once on every pane already showing a tree — see [`crate::app::Action::SetFlatMode`].
///
/// **Regroup single folders** is the third setting and it belongs to the tree rather than to the
/// button, which is why it sits under the two modes rather than beside the toggle at the top: it is
/// what a `Tree` looks like, and in a `List` there is no shape for it to change. Ticked while it is
/// on, which it is by default — see [`crate::config::Config::regroup`] — and enabled either way,
/// because a setting that greys out in the mode you are not in is a setting you cannot find when you
/// go looking for why the last tree looked like that.
///
/// On This PC there is no menu, because there is no button: `tool_button` senses hover alone while
/// it is disabled, so a right click there never reaches this. That is the same answer the button
/// gives — a machine's volumes are not a tree, and each of them is a place to flatten of its own.
pub(crate) fn flatten_menu(
    ui: &Ui,
    trigger: &egui::Response,
    pane: PaneId,
    flat: bool,
    mode: crate::pane::FlatMode,
    regroup: bool,
    out: &mut Vec<Action>,
) {
    use azur_egui_theme::components::{collection_label, menu_divider, ContextMenu};

    ContextMenu::new(trigger)
        .sticky(true)
        .show(ui.ctx(), |ui| {
            if ui
                .add(ticked(
                    MenuItem::new("Flatten this folder").shortcut("Ctrl+E"),
                    flat,
                ))
                .clicked()
            {
                out.push(Action::ToggleFlat(pane));
            }
            menu_divider(ui);
            collection_label(ui, "Show as");
            for as_what in crate::pane::FlatMode::ALL {
                if ui
                    .add(ticked(MenuItem::new(as_what.label()), mode == as_what))
                    .clicked()
                {
                    out.push(Action::SetFlatMode(as_what));
                }
            }
            // What the tree does with a folder that holds nothing but one folder. **Under the modes
            // but behind a rule**, because it is neither of the two things above it: not a third mode
            // — the group it would join is a radio group, and a tick in the middle of one reads as a
            // mode you can have as well as `Tree` — and not a command like the toggle at the top. Its
            // own line says so before the words do.
            menu_divider(ui);
            if ui
                .add(ticked(MenuItem::new("Regroup single folders"), regroup))
                .clicked()
            {
                out.push(Action::SetRegroup(!regroup));
            }
        });
}

/// The funnel's menu: the listings a *name* cannot ask for.
///
/// **A left click, and `Menu` rather than `ContextMenu`.** The two menus above it hang off buttons
/// that already do something, so theirs is the second gesture and the right button is where it
/// belongs. This button has no first gesture — opening this *is* what it does — and a control whose
/// only purpose is behind the button most people never try there is a control nobody finds. That is
/// the whole reason the `@git` word became this: what it cost was every reader who never learnt it.
///
/// **Not sticky**, unlike those two, because these are commands rather than settings: each entry
/// rebuilds the listing behind the menu, and what a reader wants next is to see it. Dismissal is
/// also how they know the command was taken.
///
/// Ticked, and **ticking the one on show is how it is turned off** — a listing you are already
/// looking at cannot be asked for again, so the tick is the only thing the entry can usefully mean
/// the second time. One at a time for the reason [`Lens`] gives: they are two questions, not two
/// halves of one.
pub(crate) fn funnel_menu(
    ui: &Ui,
    trigger: &egui::Response,
    pane: PaneId,
    lens: Option<Lens>,
    out: &mut Vec<Action>,
) {
    azur_egui_theme::components::Menu::new(trigger).show(ui.ctx(), |ui| {
        for which in Lens::ALL {
            let on = lens == Some(which);
            if ui.add(ticked(MenuItem::new(which.label()), on)).clicked() {
                out.push(Action::SetLens {
                    pane,
                    lens: (!on).then_some(which),
                });
            }
        }
    });
}

/// The path field's own menu: which slash it writes between the parts of a path.
///
/// **Sticky**, like the two button menus above and for the same reason — it holds a setting rather
/// than a command, and a menu that vanished on the tick would have to be reopened to see what the
/// tick did. Here that matters more than it does up there, because what it did is *behind* the menu:
/// the path in the field, rewritten.
///
/// One entry, and it hangs off the field rather than off the bar because the field is the only thing
/// the setting is about — a breadcrumb has no separators in it to change. Which slash you want is a
/// question about where the path is going next: `\` is what Windows shows and what its own dialogs
/// take, and `/` is what a shell, a URL and nearly every source file want, which is a conversion
/// otherwise done by hand every time a path is copied out of here.
///
/// Returns where the menu is while it is showing, because [`edit_field`] has to know whether a click
/// that took the keyboard off the field landed in here or somewhere else entirely.
pub(crate) fn slash_menu(
    ui: &Ui,
    trigger: &egui::Response,
    slashes: bool,
    out: &mut Vec<Action>,
) -> Option<Rect> {
    use azur_egui_theme::components::ContextMenu;

    ContextMenu::new(trigger)
        .sticky(true)
        .show(ui.ctx(), |ui| {
            if ui
                .add(ticked(MenuItem::new("Use / in path"), slashes))
                .clicked()
            {
                out.push(Action::SetForwardSlashes(!slashes));
            }
        })
        .map(|shown| shown.response.rect)
}
