//! The window's content: the search box, the result list, the clipboard
//! preview and the actions menu.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::theme::Theme;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Launcher, Start};
use crate::apps::App as DesktopApp;
use crate::clipboard::{Clip, Content};
use crate::emoji::Emoji;
use crate::history;
use crate::items::{Action, ActionDef, Entry, Item, ItemIcon, Symbol};
use crate::search::{Candidate, Ranker};

/// Most results listed for a query.
const MAX_RESULTS: usize = 50;
const ROW_HEIGHT: f32 = 56.;
/// Rows shown before the list scrolls; the half row hints that it does.
const VISIBLE_ROWS: f32 = 7.5;
const ICON_SIZE: f32 = 32.;
const SEARCH_HEIGHT: f32 = 72.;
const SEARCH_TEXT_SIZE: f32 = 22.;
/// The clipboard view needs room for its preview.
const CLIPBOARD_MIN_WIDTH: f32 = 920.;
/// Characters of a text entry that are searched and previewed.
const CLIP_SEARCH_CHARS: usize = 2_000;
const CLIP_PREVIEW_CHARS: usize = 10_000;
const CONTEXT: &str = "Rockhopper";
/// Added to [`CONTEXT`] while the emoji grid shows, for its arrow keys.
const GRID_CONTEXT: &str = "Rockhopper EmojiGrid";
const EMOJI_TILE: f32 = 64.;
const EMOJI_ROWS: f32 = 5.5;
/// Most recently used emoji shown first for an empty query.
const FREQUENT_EMOJI: usize = 20;
/// The least time between the two presses that confirm a destructive
/// command, so a double click is not a confirmation.
const CONFIRM_DELAY: Duration = Duration::from_millis(300);
const PLACEHOLDER: &str = "Search apps, windows and commands…";
const CLIPBOARD_PLACEHOLDER: &str = "Search clipboard history…";
const EMOJI_PLACEHOLDER: &str = "Search emoji…";

actions!(
    rockhopper,
    [SelectNext, SelectPrevious, SelectDown, SelectUp, SelectLeft, SelectRight, Open, Dismiss, ToggleActions]
);

pub fn bind_keys(cx: &mut gpui_kit::App) {
    // Bound on the search box itself, so these win over the input's own
    // bindings for the same keys (deepest context first, then latest).
    let input = Some("Rockhopper > Input");
    cx.bind_keys([
        KeyBinding::new("down", SelectDown, input),
        KeyBinding::new("up", SelectUp, input),
        KeyBinding::new("ctrl-n", SelectDown, input),
        KeyBinding::new("ctrl-p", SelectUp, input),
        KeyBinding::new("tab", SelectNext, input),
        KeyBinding::new("shift-tab", SelectPrevious, input),
        KeyBinding::new("enter", Open, input),
        KeyBinding::new("escape", Dismiss, input),
        KeyBinding::new("ctrl-k", ToggleActions, input),
        // Only in the emoji grid; elsewhere they move the text cursor.
        KeyBinding::new("left", SelectLeft, Some("EmojiGrid > Input")),
        KeyBinding::new("right", SelectRight, Some("EmojiGrid > Input")),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Apps, windows and commands; results index the launcher's entries.
    Search,
    /// Results index the launcher's clips.
    Clipboard,
    /// Results index the launcher's emoji, shown as a grid.
    Emoji,
}

#[derive(Clone, Debug, PartialEq)]
enum MenuAction {
    Item(ActionDef),
    CopyClip,
    DeleteClip,
    DeleteAllClips,
    CopyEmoji,
    CopyShortcode,
}

impl MenuAction {
    fn label(&self) -> String {
        match self {
            Self::Item(def) => def.label.clone(),
            Self::CopyClip => "Copy".into(),
            Self::DeleteClip => "Delete Entry".into(),
            Self::DeleteAllClips => "Delete All Entries".into(),
            Self::CopyEmoji => "Copy Emoji".into(),
            Self::CopyShortcode => "Copy Shortcode".into(),
        }
    }
}

/// The open actions menu for one result.
struct Menu {
    /// Index into the results' source: entries or clips.
    target: usize,
    title: String,
    actions: Vec<MenuAction>,
    selected: usize,
}

/// Something waiting for a second `enter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Confirm {
    Entry(usize),
    DeleteAllClips,
}

pub struct LauncherView {
    launcher: Entity<Launcher>,
    input: Entity<InputState>,
    scroll: UniformListScrollHandle,
    ranker: Ranker,
    mode: Mode,
    query: String,
    /// Indices into entries or clips, best match first.
    results: Vec<usize>,
    selected: usize,
    menu: Option<Menu>,
    confirming: Option<Confirm>,
    confirm_since: Instant,
    /// Whether `enter` was released since `confirming` was set. A held key
    /// repeats, and a repeat must never confirm.
    confirm_released: bool,
    /// Emoji per grid row, as last laid out.
    columns: usize,
    /// Emoji drawn from the colour font, or `None` where it has no image.
    emoji_images: HashMap<&'static str, Option<Arc<Image>>>,
    /// Decrypted, decoded clipboard images by clip id.
    images: HashMap<u64, Arc<Image>>,
    _subscriptions: Vec<Subscription>,
}

impl LauncherView {
    pub fn new(launcher: Entity<Launcher>, start: Start, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));
        input.update(cx, |state, cx| state.focus(window, cx));
        let subscriptions = vec![
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = input.read(cx).value().to_string();
                    this.selected = 0;
                    this.rerank(cx);
                }
            }),
            // The index can finish, a window close or a copy land while open.
            cx.observe(&launcher, |this, _, cx| this.rerank(cx)),
        ];
        let mut this = Self {
            launcher,
            input,
            scroll: UniformListScrollHandle::new(),
            ranker: Ranker::new(),
            mode: Mode::Search,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            menu: None,
            confirming: None,
            confirm_since: Instant::now(),
            confirm_released: false,
            columns: 1,
            emoji_images: HashMap::new(),
            images: HashMap::new(),
            _subscriptions: subscriptions,
        };
        match start {
            Start::Search => {}
            Start::Clipboard => this.set_mode(Mode::Clipboard, window, cx),
            Start::Emoji => this.set_mode(Mode::Emoji, window, cx),
        }
        this
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        let placeholder = match mode {
            Mode::Search => PLACEHOLDER,
            Mode::Clipboard => CLIPBOARD_PLACEHOLDER,
            Mode::Emoji => EMOJI_PLACEHOLDER,
        };
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
        self.query.clear();
        self.selected = 0;
        self.rerank(cx);
    }

    fn rerank(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        self.confirming = None;
        let launcher = self.launcher.read(cx);
        self.results = match self.mode {
            Mode::Search if self.query.trim().is_empty() => Vec::new(),
            Mode::Search => {
                let candidates: Vec<_> = launcher.entries.iter().map(|e| e.candidate.clone()).collect();
                let mut ranked = self
                    .ranker
                    .rank(&candidates, &launcher.history, &self.query, history::now());
                ranked.truncate(MAX_RESULTS);
                ranked
            }
            Mode::Clipboard if self.query.trim().is_empty() => (0..launcher.clips.len()).collect(),
            Mode::Clipboard => {
                let candidates: Vec<_> = launcher.clips.iter().map(clip_candidate).collect();
                self.ranker
                    .rank(&candidates, &launcher.history, &self.query, history::now())
            }
            Mode::Emoji if self.query.trim().is_empty() => frequent_first(&launcher.emojis, &launcher.history),
            Mode::Emoji => {
                let candidates: Vec<_> = launcher.emojis.iter().map(Emoji::candidate).collect();
                let mut ranked = self
                    .ranker
                    .rank(&candidates, &launcher.history, &self.query, history::now());
                ranked.truncate(MAX_RESULTS * 4);
                ranked
            }
        };
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        self.scroll_to_selected(ScrollStrategy::Top);
        cx.notify();
    }

    /// The list row holding the selection: in the grid, rows hold several.
    fn scroll_to_selected(&self, strategy: ScrollStrategy) {
        let row = match self.mode {
            Mode::Emoji => self.selected / self.columns.max(1),
            _ => self.selected,
        };
        self.scroll.scroll_to_item(row, strategy);
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.results.len() && ix != self.selected && self.menu.is_none() {
            self.selected = ix;
            self.confirming = None;
            self.scroll_to_selected(ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            let n = menu.actions.len();
            menu.selected = (menu.selected + if forward { 1 } else { n - 1 }) % n;
            self.confirming = None;
            cx.notify();
        } else if !self.results.is_empty() {
            let n = self.results.len();
            self.select((self.selected + if forward { 1 } else { n - 1 }) % n, cx);
        }
    }

    /// Up and down: by one row in the grid, by one result elsewhere.
    fn step_rows(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.mode != Mode::Emoji || self.menu.is_some() {
            return self.step(down, cx);
        }
        let columns = self.columns.max(1);
        let last = self.results.len().saturating_sub(1);
        let target = if down {
            // From a short last row's neighbourhood, land on the last emoji.
            (self.selected + columns).min(last)
        } else {
            self.selected.saturating_sub(columns)
        };
        self.select(target, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(true, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(false, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.step_rows(true, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.step_rows(false, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_none() && self.selected > 0 {
            self.select(self.selected - 1, cx);
        }
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_none() {
            self.select(self.selected + 1, cx);
        }
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &self.menu {
            let (target, action) = (menu.target, menu.actions[menu.selected].clone());
            self.run(target, action, window, cx);
        } else if let Some(&target) = self.results.get(self.selected) {
            let action = match self.mode {
                Mode::Search => MenuAction::Item(ActionDef {
                    action: Action::Primary,
                    label: String::new(),
                }),
                Mode::Clipboard => MenuAction::CopyClip,
                Mode::Emoji => MenuAction::CopyEmoji,
            };
            self.run(target, action, window, cx);
        }
    }

    fn open_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = ix;
        // A click completes with the button released.
        self.confirm_released = true;
        self.open(&Open, window, cx);
    }

    /// Whether `what` is confirmed. The first call asks for confirmation and
    /// returns false; a later one confirms only after `enter` was released
    /// and some time passed.
    fn confirm(&mut self, what: Confirm, cx: &mut Context<Self>) -> bool {
        if self.confirming == Some(what) {
            return self.confirm_released && self.confirm_since.elapsed() >= CONFIRM_DELAY;
        }
        self.confirming = Some(what);
        self.confirm_since = Instant::now();
        self.confirm_released = false;
        cx.notify();
        false
    }

    fn run(&mut self, target: usize, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        let launcher = self.launcher.read(cx);
        match action {
            MenuAction::Item(def) => {
                let item = launcher.entries[target].item.clone();
                match item {
                    Item::Clipboard => return self.set_mode(Mode::Clipboard, window, cx),
                    Item::Emoji => return self.set_mode(Mode::Emoji, window, cx),
                    _ => {}
                }
                if let (Item::System(cmd), Action::Primary) = (&item, def.action)
                    && cmd.needs_confirmation()
                    && !self.confirm(Confirm::Entry(target), cx)
                {
                    self.menu = None;
                    return;
                }
                self.menu = None;
                self.confirming = None;
                self.launcher.update(cx, |l, cx| l.perform(&item, def.action, cx));
            }
            MenuAction::CopyClip => {
                let clip = launcher.clips[target].clone();
                self.launcher.update(cx, |l, cx| l.copy_clip(&clip, cx));
            }
            MenuAction::DeleteClip => {
                let id = launcher.clips[target].id;
                self.images.remove(&id);
                self.launcher.update(cx, |l, cx| l.delete_clip(id, cx));
            }
            MenuAction::CopyEmoji | MenuAction::CopyShortcode => {
                let emoji = launcher.emojis[target].clone();
                let text = match action {
                    MenuAction::CopyShortcode => match emoji.shortcode {
                        Some(code) => format!(":{code}:"),
                        None => return,
                    },
                    _ => emoji.glyph.to_owned(),
                };
                self.launcher
                    .update(cx, |l, cx| l.copy_text(&text, Some(emoji.key()), cx));
            }
            MenuAction::DeleteAllClips => {
                if !self.confirm(Confirm::DeleteAllClips, cx) {
                    return;
                }
                self.images.clear();
                self.launcher.update(cx, |l, cx| l.clear_clips(cx));
            }
        }
    }

    fn toggle_actions(&mut self, _: &ToggleActions, _: &mut Window, cx: &mut Context<Self>) {
        self.confirming = None;
        if self.menu.take().is_none()
            && let Some(&target) = self.results.get(self.selected)
        {
            let launcher = self.launcher.read(cx);
            let (title, actions) = match self.mode {
                Mode::Search => {
                    let item = &launcher.entries[target].item;
                    let actions = item
                        .actions(&launcher.apps, &launcher.history)
                        .into_iter()
                        .map(MenuAction::Item)
                        .collect();
                    (item.title(&launcher.apps), actions)
                }
                Mode::Clipboard => (
                    clip_title(&launcher.clips[target]),
                    vec![MenuAction::CopyClip, MenuAction::DeleteClip, MenuAction::DeleteAllClips],
                ),
                Mode::Emoji => {
                    let emoji = &launcher.emojis[target];
                    let mut actions = vec![MenuAction::CopyEmoji];
                    if emoji.shortcode.is_some() {
                        actions.push(MenuAction::CopyShortcode);
                    }
                    (title_case(emoji.name), actions)
                }
            };
            self.menu = Some(Menu {
                target,
                title,
                actions,
                selected: 0,
            });
        }
        cx.notify();
    }

    /// Escape closes the actions menu, then clears a query, then leaves the
    /// clipboard history, then closes.
    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.confirming = None;
            cx.notify();
        } else if !self.query.is_empty() {
            // set_value does not emit InputEvent::Change.
            self.input.update(cx, |input, cx| input.set_value("", window, cx));
            self.query.clear();
            self.rerank(cx);
        } else if self.mode != Mode::Search {
            self.set_mode(Mode::Search, window, cx);
        } else {
            self.launcher.update(cx, |l, cx| l.hide(cx));
        }
    }

    /// The selected clip's image, decrypted and decoded on first view.
    fn clip_image(&mut self, id: u64, cx: &App) -> Option<Arc<Image>> {
        if let Some(image) = self.images.get(&id) {
            return Some(image.clone());
        }
        let png = self.launcher.read(cx).clip_image(id)?;
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, png.to_vec()));
        self.images.insert(id, image.clone());
        Some(image)
    }

    fn row(&self, ix: usize, theme: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = ix == self.selected;
        h_flex()
            .id(ix)
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_3()
            .gap_3()
            .items_center()
            .rounded(px(10.))
            .when(selected, |row| row.bg(theme.accent))
            .on_mouse_move(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
            .on_click(cx.listener(move |this, _, window, cx| this.open_at(ix, window, cx)))
    }

    fn render_entry_row(&self, ix: usize, entries: &[Entry], apps: &[DesktopApp], cx: &mut Context<Self>) -> AnyElement {
        let entry = self.results[ix];
        let item = &entries[entry].item;
        let theme = cx.theme().clone();
        let confirming = self.confirming == Some(Confirm::Entry(entry));
        let subtitle = if confirming {
            Some(format!("Press ↵ again to {}", item.title(apps).to_lowercase()))
        } else {
            item.subtitle(apps)
        };
        self.row(ix, &theme, cx)
            .child(item_icon(item.icon(apps), &theme))
            .child(row_text(item.title(apps), subtitle, confirming, ix == self.selected, &theme))
            .child(kind_label(item.kind(apps), &theme))
            .into_any_element()
    }

    fn render_clip_row(&self, ix: usize, clips: &[Clip], cx: &mut Context<Self>) -> AnyElement {
        let clip = &clips[self.results[ix]];
        let theme = cx.theme().clone();
        let (symbol, kind, detail) = match &clip.content {
            Content::Text { text } => {
                let chars = text.chars().count();
                (Symbol::Text, "Text", format!("{chars} character{}", if chars == 1 { "" } else { "s" }))
            }
            Content::Image { width, height } => (Symbol::Image, "Image", format!("{width}×{height}")),
        };
        let subtitle = format!("{} · {detail}", ago(clip.time));
        self.row(ix, &theme, cx)
            .child(item_icon(ItemIcon::Symbol(symbol), &theme))
            .child(row_text(clip_title(clip), Some(subtitle), false, ix == self.selected, &theme))
            .child(kind_label(kind, &theme))
            .into_any_element()
    }

    /// The emoji as an image from the colour font, once that has loaded.
    fn emoji_image(&mut self, glyph: &'static str, cx: &App) -> Option<Arc<Image>> {
        let font = self.launcher.read(cx).emoji_font.clone()?;
        self.emoji_images
            .entry(glyph)
            .or_insert_with(|| font.png(glyph).map(|png| Arc::new(Image::from_bytes(ImageFormat::Png, png))))
            .clone()
    }

    /// An emoji drawn `size` pixels square: the font's image, else text.
    fn emoji_glyph(&mut self, glyph: &'static str, size: f32, cx: &App) -> AnyElement {
        match self.emoji_image(glyph, cx) {
            Some(image) => img(image).size(px(size)).into_any_element(),
            None => div().text_size(px(size * 0.85)).child(glyph).into_any_element(),
        }
    }

    /// One row of the emoji grid.
    fn render_emoji_row(&mut self, row: usize, emojis: &[Emoji], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let columns = self.columns.max(1);
        let start = row * columns;
        let end = (start + columns).min(self.results.len());
        let glyphs: Vec<AnyElement> = (start..end)
            .map(|ix| self.emoji_glyph(emojis[self.results[ix]].glyph, 36., cx))
            .collect();
        h_flex()
            .h(px(EMOJI_TILE))
            .children((start..end).zip(glyphs).map(|(ix, glyph)| {
                div()
                    .id(ix)
                    .size(px(EMOJI_TILE))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(12.))
                    .when(ix == self.selected, |tile| tile.bg(theme.accent))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_at(ix, window, cx)))
                    .child(glyph)
            }))
            .into_any_element()
    }

    fn render_menu(&self, menu: &Menu, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let confirming_all = self.confirming == Some(Confirm::DeleteAllClips);
        v_flex()
            .id("actions")
            .occlude()
            .absolute()
            .right_3()
            .bottom(px(48.))
            .w(px(300.))
            .p_1()
            .rounded(px(12.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_xl()
            .child(
                div()
                    .px_2()
                    .py_1p5()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(menu.title.clone()),
            )
            .children(menu.actions.iter().enumerate().map(|(i, action)| {
                let selected = i == menu.selected;
                let danger = matches!(action, MenuAction::DeleteClip | MenuAction::DeleteAllClips);
                let label = if *action == MenuAction::DeleteAllClips && confirming_all {
                    "Press ↵ again to delete all".to_owned()
                } else {
                    action.label()
                };
                h_flex()
                    .id(("action", i))
                    .h(px(34.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(px(8.))
                    .text_sm()
                    .when(danger, |row| row.text_color(theme.danger))
                    .when(selected, |row| row.bg(theme.accent))
                    .when(selected && !danger, |row| row.text_color(theme.accent_foreground))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(menu) = &mut this.menu
                            && menu.selected != i
                        {
                            menu.selected = i;
                            this.confirming = None;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(menu) = &mut this.menu {
                            menu.selected = i;
                        }
                        this.confirm_released = true;
                        this.open(&Open, window, cx);
                    }))
                    .child(div().flex_1().truncate().child(label))
                    .when(i == 0, |row| row.child(Kbd::new(key("enter"))))
            }))
    }

    fn render_preview(&mut self, clip: &Clip, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let body = match &clip.content {
            Content::Text { text } => {
                let shown: String = text.chars().take(CLIP_PREVIEW_CHARS).collect();
                div()
                    .id("preview-text")
                    .size_full()
                    .overflow_y_scroll()
                    .p_4()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(shown)
                    .into_any_element()
            }
            Content::Image { .. } => match self.clip_image(clip.id, cx) {
                Some(image) => div()
                    .size_full()
                    .p_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(img(image).max_w_full().max_h_full().object_fit(ObjectFit::Contain))
                    .into_any_element(),
                None => div()
                    .p_4()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("This image can no longer be read.")
                    .into_any_element(),
            },
        };
        v_flex().flex_1().min_w_0().h_full().bg(theme.muted.opacity(0.25)).child(body).into_any_element()
    }
}

impl Render for LauncherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let launcher = self.launcher.read(cx);
        let apps = launcher.apps.clone();
        let entries = launcher.entries.clone();
        let clips = launcher.clips.clone();
        let emojis = launcher.emojis.clone();
        let appearance = launcher.config.appearance.clone();
        let indexing = launcher.indexing && apps.is_empty();
        let theme = cx.theme().clone();
        let viewport = window.viewport_size();
        let clipboard = self.mode == Mode::Clipboard;
        let emoji = self.mode == Mode::Emoji;
        let wanted = if clipboard {
            appearance.width.max(CLIPBOARD_MIN_WIDTH)
        } else {
            appearance.width
        };
        let width = px(wanted).min(viewport.width - px(32.));
        let top = viewport.height * appearance.top.clamp(0., 0.8);
        let searching = !self.query.trim().is_empty();
        // The grid fills the panel less its padding (px_2 each side).
        self.columns = (f32::from(width - px(16.) - px(2.)) / EMOJI_TILE).floor().max(1.) as usize;
        let pill = match self.mode {
            Mode::Search => None,
            Mode::Clipboard => Some("Clipboard"),
            Mode::Emoji => Some("Emoji"),
        };

        let search = h_flex()
            .h(px(SEARCH_HEIGHT))
            .px_5()
            .gap_3()
            .items_center()
            .child(
                Icon::new(IconName::Search)
                    .size(px(22.))
                    .text_color(theme.muted_foreground),
            )
            .when_some(pill, |search, pill| {
                search.child(
                    div()
                        .flex_none()
                        .px_2()
                        .py_0p5()
                        .rounded(px(6.))
                        .bg(theme.accent)
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.accent_foreground)
                        .child(pill),
                )
            })
            .child(
                div().flex_1().child(
                    Input::new(&self.input)
                        .appearance(false)
                        .h(px(SEARCH_HEIGHT))
                        .p_0()
                        .text_size(px(SEARCH_TEXT_SIZE))
                        .line_height(px(SEARCH_TEXT_SIZE * 1.4)),
                ),
            );

        let message = |text: &'static str| {
            div()
                .py_6()
                .w_full()
                .text_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        };
        let list_height = |rows: usize| {
            let rows = if clipboard { VISIBLE_ROWS } else { (rows as f32).min(VISIBLE_ROWS) };
            px(ROW_HEIGHT * rows)
        };
        let body = if !searching && self.mode == Mode::Search {
            None
        } else if emoji && !self.results.is_empty() {
            let columns = self.columns;
            let rows = self.results.len().div_ceil(columns);
            let row_emojis = emojis.clone();
            let grid = uniform_list(
                "emoji",
                rows,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range.map(|row| this.render_emoji_row(row, &row_emojis, cx)).collect()
                }),
            )
            .track_scroll(&self.scroll)
            .h(px(EMOJI_TILE * EMOJI_ROWS.min(rows as f32)))
            .px_2();
            let selected = self.results.get(self.selected).map(|&i| emojis[i].clone());
            let info = selected.map(|emoji| {
                let glyph = self.emoji_glyph(emoji.glyph, 32., cx);
                emoji_info(&emoji, glyph, &theme)
            });
            Some(
                v_flex()
                    .child(div().py_2().child(grid))
                    .children(info)
                    .into_any_element(),
            )
        } else if self.results.is_empty() {
            Some(message(match (clipboard, indexing) {
                (true, _) if clips.is_empty() => "Nothing copied yet",
                (false, true) => "Indexing apps…",
                _ => "No matches",
            }))
        } else {
            let list = if clipboard {
                let row_clips = clips.clone();
                uniform_list(
                    "results",
                    self.results.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|ix| this.render_clip_row(ix, &row_clips, cx)).collect()
                    }),
                )
            } else {
                let (row_entries, row_apps) = (entries.clone(), apps.clone());
                uniform_list(
                    "results",
                    self.results.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|ix| this.render_entry_row(ix, &row_entries, &row_apps, cx))
                            .collect()
                    }),
                )
            }
            .track_scroll(&self.scroll)
            .h(list_height(self.results.len()))
            .px_2();
            let list = v_flex().py_2().child(list);
            let selected_clip = clipboard
                .then(|| self.results.get(self.selected).map(|&i| clips[i].clone()))
                .flatten();
            Some(match selected_clip {
                Some(clip) => h_flex()
                    .w_full()
                    .h(list_height(0) + px(16.))
                    .child(list.w(relative(0.45)).flex_none())
                    .child(div().w(px(1.)).h_full().bg(theme.border))
                    .child(self.render_preview(&clip, &theme, cx))
                    .into_any_element(),
                None => list.into_any_element(),
            })
        };

        let footer_verb = match self.mode {
            Mode::Search => self
                .results
                .get(self.selected)
                .filter(|_| searching)
                .map(|&e| entries[e].item.verb()),
            Mode::Clipboard | Mode::Emoji => self.results.get(self.selected).map(|_| "Copy"),
        };
        let menu = self.menu.as_ref().map(|menu| self.render_menu(menu, &theme, cx));

        let panel = v_flex()
            .id("panel")
            .relative()
            .w(width)
            .occlude()
            .overflow_hidden()
            .rounded(px(18.))
            .border_1()
            .border_color(theme.border.opacity(0.8))
            .bg(theme.popover)
            .shadow_2xl()
            .child(search)
            .when_some(body, |panel, body| {
                panel.child(div().border_t_1().border_color(theme.border).child(body))
            })
            .when_some(footer_verb, |panel, verb| {
                panel.child(footer(verb, self.menu.is_some(), &theme))
            })
            .children(menu)
            .with_animation(
                "panel-in",
                Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
                |panel, t| panel.opacity(t).mt(px(8. * (1. - t))),
            );

        let backdrop = if appearance.dim {
            hsla(0., 0., 0., 0.2)
        } else {
            hsla(0., 0., 0., 0.)
        };
        let launcher = self.launcher.clone();
        div()
            .size_full()
            .key_context(if emoji { GRID_CONTEXT } else { CONTEXT })
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::toggle_actions))
            .capture_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| {
                if event.keystroke.key == "enter" {
                    this.confirm_released = true;
                }
            }))
            .flex()
            .flex_col()
            .items_center()
            .pt(top)
            .bg(backdrop)
            .font_family(theme.font_family.clone())
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                launcher.update(cx, |l, cx| l.hide(cx));
            })
            .child(panel)
    }
}

/// Recently used emoji, most frecent first, then the rest in Unicode order.
fn frequent_first(emojis: &[Emoji], history: &crate::history::History) -> Vec<usize> {
    let now = history::now();
    let mut frequent: Vec<(usize, f32)> = emojis
        .iter()
        .enumerate()
        .map(|(i, e)| (i, history.frecency(&e.key(), now)))
        .filter(|(_, f)| *f > 0.0)
        .collect();
    frequent.sort_by(|a, b| b.1.total_cmp(&a.1));
    frequent.truncate(FREQUENT_EMOJI);
    let first: Vec<usize> = frequent.into_iter().map(|(i, _)| i).collect();
    let rest = (0..emojis.len()).filter(|i| !first.contains(i));
    first.iter().copied().chain(rest).collect()
}

/// The selected emoji, large, with its name and shortcode.
fn emoji_info(emoji: &Emoji, glyph: AnyElement, theme: &Theme) -> AnyElement {
    h_flex()
        .px_4()
        .py_3()
        .gap_3()
        .items_center()
        .border_t_1()
        .border_color(theme.border)
        .child(glyph)
        .child(
            v_flex()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(title_case(emoji.name)),
                )
                .when_some(emoji.shortcode, |info, code| {
                    info.child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(format!(":{code}:")),
                    )
                }),
        )
        .into_any_element()
}

/// "party popper" → "Party Popper".
fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn clip_candidate(clip: &Clip) -> Candidate {
    match &clip.content {
        Content::Text { text } => Candidate {
            name: text.chars().take(CLIP_SEARCH_CHARS).collect(),
            ..Candidate::default()
        },
        Content::Image { width, height } => Candidate {
            name: format!("Image {width}×{height}"),
            secondary: vec!["image".into(), "picture".into(), "screenshot".into()],
            key: None,
        },
    }
}

/// The first non-blank line of a text entry, or an image's description.
fn clip_title(clip: &Clip) -> String {
    match &clip.content {
        Content::Text { text } => text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect(),
        Content::Image { width, height } => format!("Image {width}×{height}"),
    }
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
fn ago(time: u64) -> String {
    let secs = history::now().saturating_sub(time);
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s if s < 2 * 86_400 => "yesterday".into(),
        s => format!("{} days ago", s / 86_400),
    }
}

fn key(keys: &str) -> Keystroke {
    Keystroke::parse(keys).expect("valid key hint")
}

fn row_text(title: String, subtitle: Option<String>, alert: bool, selected: bool, theme: &Theme) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(if selected { theme.accent_foreground } else { theme.foreground })
                .truncate()
                .child(title),
        )
        .when_some(subtitle, |this, subtitle| {
            this.child(
                div()
                    .text_xs()
                    .text_color(if alert { theme.danger } else { theme.muted_foreground })
                    .truncate()
                    .child(subtitle),
            )
        })
}

fn kind_label(kind: &'static str, theme: &Theme) -> impl IntoElement {
    div().flex_none().text_xs().text_color(theme.muted_foreground).child(kind)
}

fn item_icon(icon: ItemIcon, theme: &Theme) -> AnyElement {
    let size = px(ICON_SIZE);
    match icon {
        ItemIcon::File(path) => img(path).size(size).flex_none().into_any_element(),
        ItemIcon::Initial(name) => {
            let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
            let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            div()
                .size(size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(hsla(hue as f32 / 360., 0.55, 0.5, 1.))
                .text_color(white())
                .font_weight(FontWeight::BOLD)
                .child(initial)
                .into_any_element()
        }
        ItemIcon::Symbol(symbol) => {
            let name = match symbol {
                Symbol::Lock => Lucide::Lock,
                Symbol::Suspend => Lucide::Moon,
                Symbol::LogOut => Lucide::LogOut,
                Symbol::Restart => Lucide::RotateCcw,
                Symbol::Power => Lucide::Power,
                Symbol::Window => Lucide::AppWindow,
                Symbol::Clipboard => Lucide::ClipboardList,
                Symbol::Emoji => Lucide::FaceSlightlySmiling,
                Symbol::Text => Lucide::Type,
                Symbol::Image => Lucide::Image,
            };
            div()
                .size(size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(theme.muted)
                .child(Icon::new(name).size(px(18.)).text_color(theme.foreground))
                .into_any_element()
        }
    }
}

fn footer(verb: &str, menu_open: bool, theme: &Theme) -> AnyElement {
    let hint = |keys: &str, label: &str| {
        h_flex()
            .gap_1p5()
            .items_center()
            .child(div().child(label.to_owned()))
            .child(Kbd::new(key(keys)))
    };
    h_flex()
        .px_4()
        .py_2()
        .gap_4()
        .items_center()
        .border_t_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.4))
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(div().font_weight(FontWeight::SEMIBOLD).child("Rockhopper"))
        .child(div().flex_1())
        .when(!menu_open, |footer| footer.child(hint("enter", verb)))
        .child(hint("ctrl-k", "Actions"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in GPUI's `test` macro, shadowing Rust's.
    use super::{frequent_first, title_case};
    use crate::emoji::{self, SkinTone};
    use crate::history::{self, History};

    #[test]
    fn frequent_emoji_lead_without_repeats() {
        let emojis = emoji::all(SkinTone::Default);
        let rocket = emojis.iter().position(|e| e.name == "rocket").unwrap();
        let tada = emojis.iter().position(|e| e.name == "party popper").unwrap();
        let mut history = History::default();
        let now = history::now();
        history.record(&emojis[tada].key(), now);
        for _ in 0..3 {
            history.record(&emojis[rocket].key(), now);
        }
        let order = frequent_first(&emojis, &history);
        assert_eq!(&order[..2], [rocket, tada]);
        assert_eq!(order.len(), emojis.len());
        assert_eq!(order.iter().filter(|&&i| i == rocket).count(), 1);
    }

    #[test]
    fn titles() {
        assert_eq!(title_case("party popper"), "Party Popper");
        assert_eq!(title_case("flag: United Kingdom"), "Flag: United Kingdom");
    }
}
