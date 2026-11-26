//! The buttons, pickers, and marks the views share, and the classes they carry.
//!
//! Tailwind CSS writes the stylesheet from the class names it finds in this
//! crate's sources, which it reads as plain text.
//! So a class is written out in full wherever it is named, and never put
//! together from parts: `text-danger`, not `text-{tone}`.
//!
//! A control is 44 CSS pixels tall under a finger (`pointer-coarse:`), and
//! smaller under a mouse.

use leptos_dom::helpers::event_target_value;
use tachys::{
    html::{
        element::{button, label, optgroup, option, select, span},
        event as ev,
    },
    prelude::*,
};

/// What a button is among its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// One of several things to do.
    Plain,

    /// The one thing the reader came to do.
    Primary,

    /// Something to do that stays out of the way until wanted.
    Quiet,
}

/// The shape of a button.
const BUTTON: &str = "inline-flex h-8 shrink-0 cursor-pointer items-center justify-center gap-1.5 \
                      rounded-md px-3 text-sm font-medium whitespace-nowrap select-none \
                      disabled:cursor-default disabled:opacity-50 pointer-coarse:h-11 \
                      pointer-coarse:px-4 pointer-coarse:text-base";

/// The shape of a button among a card's actions.
const SMALL_BUTTON: &str = "inline-flex h-7 shrink-0 cursor-pointer items-center justify-center \
                            gap-1 rounded-md px-2 text-xs font-medium whitespace-nowrap \
                            select-none disabled:cursor-default disabled:opacity-50 \
                            pointer-coarse:h-11 pointer-coarse:px-3 pointer-coarse:text-sm";

/// The colors of a button.
fn tone(tone: Tone) -> &'static str {
    match tone {
        Tone::Plain => "border border-line bg-canvas text-ink hover:bg-raised",
        Tone::Primary => "bg-accent text-on-accent hover:opacity-90",
        Tone::Quiet => "text-muted hover:bg-raised hover:text-ink",
    }
}

/// The classes of a button.
pub(crate) fn button_class(of: Tone) -> String {
    format!("{BUTTON} {}", tone(of))
}

/// The classes of a button among a card's actions.
pub(crate) fn small_button_class(of: Tone) -> String {
    format!("{SMALL_BUTTON} {}", tone(of))
}

/// A button among several small ones, as a card's actions are, showing `child`,
/// that runs `then`.
pub(crate) fn small_action<C>(
    of: Tone,
    child: C,
    then: impl Fn() + Send + Sync + 'static,
) -> impl RenderHtml
where
    C: RenderHtml + 'static,
{
    button()
        .attr("type", "button")
        .class(small_button_class(of))
        .on(ev::click, move |_| then())
        .child(child)
}

/// A button showing `child` that runs `then`.
pub(crate) fn action<C>(
    of: Tone,
    child: C,
    then: impl Fn() + Send + Sync + 'static,
) -> impl RenderHtml
where
    C: RenderHtml + 'static,
{
    button()
        .attr("type", "button")
        .class(button_class(of))
        .on(ev::click, move |_| then())
        .child(child)
}

/// A note under or beside what it is about.
pub(crate) const NOTE: &str = "text-sm text-muted";

/// Why something asked of the server didn't happen.
pub(crate) const ERROR: &str = "text-sm text-danger";

/// A link in running text.
pub(crate) const LINK: &str = "cursor-pointer text-accent underline underline-offset-2";

/// The page's top bar, which stays in view.
/// What sits beside the page under it is placed by its height.
pub(crate) const BAR: &str = "sticky top-0 z-20 flex h-12 items-center gap-2 border-b border-line \
                              bg-canvas/90 px-3 backdrop-blur";

/// The link to the review list, at the start of the top bar.
pub(crate) const HOME: &str = "flex h-8 shrink-0 items-center rounded-md px-1.5 font-mono text-sm \
                               font-bold text-ink hover:bg-raised pointer-coarse:h-11 \
                               pointer-coarse:px-2";

/// The page's title, in the top bar.
pub(crate) const TITLE: &str = "min-w-0 flex-1 truncate text-sm font-semibold";

/// The column a page's text is read in.
pub(crate) const COLUMN: &str = "mx-auto w-full max-w-[46rem] px-4 py-6";

/// A field's label, over the field.
pub(crate) const FIELD: &str = "grid gap-1 text-sm text-muted";

/// A field to write in. iOS zooms the page in on a focused field whose text is
/// smaller than 16 pixels.
pub(crate) const INPUT: &str =
    "w-full rounded-md border border-line bg-canvas px-2.5 py-2 text-base text-ink";

/// One option of a [`picker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub value: String,
    pub label: String,

    /// The heading the option is listed under, with its neighbours that name
    /// the same one; empty for none.
    pub group: &'static str,
}

/// How a [`picker`] gives way when the row it is in runs out of room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fit {
    /// It stays as wide as what it shows.
    Rigid,

    /// It narrows, and what it shows has to truncate itself.
    Shrinks,
}

/// The classes of every picker.
///
/// It clips what it holds: Safari counts the width of the longest option of the
/// `<select>` laid over it as overflowing, which lets the page scroll sideways.
const PICKER: &str = "relative inline-flex h-8 items-center gap-1 overflow-hidden rounded-md \
                      border border-line bg-canvas pr-2 pl-2.5 text-sm font-medium \
                      whitespace-nowrap hover:bg-raised has-[:focus-visible]:outline-2 \
                      has-[:focus-visible]:outline-accent pointer-coarse:h-11 \
                      pointer-coarse:text-base";

/// A picker named `name`: `shown`, what is chosen now, over the browser's own
/// list of `choices`, which a click or a tap on it opens.
///
/// `chosen` is the value of the choice shown, and `then` takes the value of the
/// one picked.
/// The list is the browser's `<select>`, laid see-through over `shown`, so it
/// opens as the platform's own picker does, and what is chosen can read
/// differently on a narrow screen than in the list.
pub(crate) fn picker<S>(
    name: &'static str,
    fit: Fit,
    shown: S,
    chosen: String,
    choices: Vec<Choice>,
    then: impl Fn(String) + 'static,
) -> impl RenderHtml
where
    S: RenderHtml + 'static,
{
    let options = options(choices, &chosen);
    let class = match fit {
        Fit::Rigid => format!("{PICKER} shrink-0"),
        Fit::Shrinks => format!("{PICKER} min-w-0 shrink"),
    };

    label().class(class).child((
        shown,
        span()
            .class("text-xs text-muted")
            .attr("aria-hidden", "true")
            .child("\u{25be}"),
        select()
            .class("absolute inset-0 h-full w-full cursor-pointer opacity-0")
            .attr("aria-label", name)
            // Chosen again whenever the picker is drawn again, so the list
            // opens on what the page shows after the page moved on its own.
            .prop("value", chosen)
            .on(ev::change, move |event| then(event_target_value(&event)))
            .child(options),
    ))
}

/// The options of a picker, neighbours naming the same group under its heading.
fn options(choices: Vec<Choice>, chosen: &str) -> Vec<AnyView> {
    let mut groups: Vec<(&'static str, Vec<Choice>)> = vec![];
    for choice in choices {
        match groups.last_mut() {
            Some((group, members)) if *group == choice.group => members.push(choice),
            _ => groups.push((choice.group, vec![choice])),
        }
    }

    groups
        .into_iter()
        .map(|(group, members)| {
            let members = members
                .into_iter()
                .map(|choice| {
                    let selected = choice.value == chosen;
                    option()
                        .attr("value", choice.value)
                        .attr("selected", selected)
                        .child(choice.label)
                })
                .collect::<Vec<_>>();
            if group.is_empty() {
                return members.into_any();
            }

            optgroup().attr("label", group).child(members).into_any()
        })
        .collect()
}

/// Two views of the same thing, the first shown from a small screen's width up
/// and the second below it.
pub(crate) fn wide_or_narrow(wide: String, narrow: String) -> impl RenderHtml {
    (
        span().class("max-sm:hidden").child(wide),
        span().class("sm:hidden").child(narrow),
    )
}
