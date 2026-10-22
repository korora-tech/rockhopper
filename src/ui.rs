//! The launcher window: a full-screen layer-shell overlay with a search box
//! that grows a result list as you type.
//!
//! [`Launcher`] lives for the whole daemon and owns the app index, history and
//! config. Each time it is shown it opens a fresh [`LauncherView`] window, and
//! hiding closes it, so every opening starts with an empty query.

use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gpui_kit::base::Root;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::apps::{self, App as DesktopApp};
use crate::config::{self, Config, ThemeChoice};
use crate::history::{self, History};
use crate::ipc::Request;
use crate::search::Ranker;
use crate::{icons, launch};

/// Most results listed for a query.
const MAX_RESULTS: usize = 50;
const ROW_HEIGHT: f32 = 56.;
/// Rows shown before the list scrolls; the half row hints that it does.
const VISIBLE_ROWS: f32 = 7.5;
const ICON_SIZE: f32 = 32.;
const SEARCH_HEIGHT: f32 = 72.;
const SEARCH_TEXT_SIZE: f32 = 22.;
/// gpui-kit wraps every window in a client-side frame: a 20px shadow gutter
/// and a 1px border. GPUI reports layer surfaces as client-decorated, so the
/// overlay gets that frame too, and we push it past the output's edges.
const FRAME_INSET: Pixels = px(21.);
const CONTEXT: &str = "Rockhopper";

actions!(rockhopper, [SelectNext, SelectPrevious, Open, Dismiss]);

pub fn bind_keys(cx: &mut gpui_kit::App) {
    // Bound on the search box itself, so these win over the input's own
    // bindings for the same keys (deepest context first, then latest).
    let input = Some("Rockhopper > Input");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, input),
        KeyBinding::new("up", SelectPrevious, input),
        KeyBinding::new("ctrl-n", SelectNext, input),
        KeyBinding::new("ctrl-p", SelectPrevious, input),
        KeyBinding::new("ctrl-j", SelectNext, input),
        KeyBinding::new("ctrl-k", SelectPrevious, input),
        KeyBinding::new("tab", SelectNext, input),
        KeyBinding::new("shift-tab", SelectPrevious, input),
        KeyBinding::new("enter", Open, input),
        KeyBinding::new("escape", Dismiss, input),
    ]);
}

pub struct Launcher {
    config: Config,
    config_stamp: config::Stamp,
    apps: Rc<Vec<DesktopApp>>,
    fingerprint: Vec<(std::path::PathBuf, Option<SystemTime>)>,
    indexing: bool,
    history: History,
    window: Option<AnyWindowHandle>,
    opening: bool,
    hide_when_open: bool,
}

impl Launcher {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Forget a window the compositor closed under us.
        cx.observe_window_closed_by_id();
        let mut this = Self {
            config: config::load(),
            config_stamp: config::stamp(),
            apps: Rc::new(Vec::new()),
            fingerprint: Vec::new(),
            indexing: false,
            history: History::load(),
            window: None,
            opening: false,
            hide_when_open: false,
        };
        this.refresh_index(cx);
        this
    }

    pub fn handle(&mut self, request: Request, cx: &mut Context<Self>) {
        match request {
            Request::Toggle if self.window.is_some() || self.opening => self.hide(cx),
            Request::Toggle | Request::Show => self.show(cx),
            Request::Hide => self.hide(cx),
            Request::Quit => {
                crate::ipc::remove_socket();
                cx.quit();
            }
        }
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.hide_when_open = false;
        if self.window.is_some() || self.opening {
            return;
        }
        let stamp = config::stamp();
        if stamp != self.config_stamp {
            self.config_stamp = stamp;
            self.config = config::load();
        }
        self.refresh_index(cx);
        match self.config.appearance.theme {
            ThemeChoice::System => Theme::sync_system_appearance(None, cx),
            ThemeChoice::Light => Theme::change(ThemeMode::Light, None, cx),
            ThemeChoice::Dark => Theme::change(ThemeMode::Dark, None, cx),
        }

        // Cover the whole output, so the backdrop can dim it and catch clicks
        // outside the panel. The renderer cannot start at the zero size that
        // would let the compositor pick, so open at the output's size, grown
        // by the frame gpui-kit draws (see FRAME_INSET) and pulled back by
        // negative margins so the frame lies off-screen.
        let display = target_display(self.config.appearance.output.as_deref(), cx);
        let output = display.as_ref().map_or(size(px(1920.), px(1080.)), |d| d.bounds().size);
        let options = WindowOptions {
            titlebar: None,
            display_id: display.as_ref().map(|d| d.id()),
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(output.width + FRAME_INSET * 2., output.height + FRAME_INSET * 2.),
            })),
            app_id: Some("rockhopper".into()),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "rockhopper".into(),
                layer: Layer::Overlay,
                anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                exclusive_zone: Some(px(-1.)),
                margin: Some((-FRAME_INSET, -FRAME_INSET, -FRAME_INSET, -FRAME_INSET)),
                keyboard_interactivity: KeyboardInteractivity::Exclusive,
                ..Default::default()
            }),
            ..Default::default()
        };
        // Deferred: the view reads this launcher while it is built, which it
        // cannot do inside this update.
        self.opening = true;
        let launcher = cx.entity();
        cx.defer(move |cx| {
            let view_launcher = launcher.clone();
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| LauncherView::new(view_launcher, window, cx));
                // gpui-kit's root paints the theme background; the backdrop
                // is ours to draw.
                cx.new(|cx| Root::new(view, window, cx).bg(transparent_black()))
            });
            launcher.update(cx, |this, cx| {
                this.opening = false;
                match opened {
                    Ok(handle) => {
                        this.window = Some(handle.into());
                        // Hidden while it was opening.
                        if this.hide_when_open {
                            this.hide_when_open = false;
                            this.hide(cx);
                        }
                    }
                    Err(err) => eprintln!("rockhopper: could not open the launcher window: {err:#}"),
                }
            });
        });
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if self.opening {
            self.hide_when_open = true;
        }
        let Some(handle) = self.window.take() else {
            return;
        };
        // Deferred: the request may come from inside that window's own update.
        cx.defer(move |cx| {
            _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    }

    fn launch(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(app) = self.apps.get(ix).cloned() else {
            return;
        };
        match launch::launch(&app, &self.config) {
            Ok(()) => self.history.record(&app.id, history::now()),
            Err(err) => eprintln!("rockhopper: could not start {}: {err}", app.name),
        }
        self.hide(cx);
    }

    /// Rescan desktop entries in the background when their directories
    /// changed since the last scan.
    fn refresh_index(&mut self, cx: &mut Context<Self>) {
        let dirs = apps::application_dirs();
        let fingerprint = apps::fingerprint(&dirs);
        if self.indexing || fingerprint == self.fingerprint {
            return;
        }
        self.indexing = true;
        let theme = icons::theme(self.config.icon_theme.as_deref());
        cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let found = cx
                .background_spawn(async move {
                    let mut found = apps::scan(&dirs, &apps::Env::current());
                    icons::resolve_all(&mut found, theme.as_deref());
                    found
                })
                .await;
            eprintln!("rockhopper: indexed {} apps in {:?}", found.len(), started.elapsed());
            _ = this.update(cx, |this, cx| {
                this.apps = Rc::new(found);
                this.fingerprint = fingerprint;
                this.indexing = false;
                cx.notify();
            });
        })
        .detach();
    }
}

/// The configured output, else the one niri says has focus, else the primary.
fn target_display(configured: Option<&str>, cx: &gpui_kit::App) -> Option<Rc<dyn PlatformDisplay>> {
    let by_name = |name: String| {
        let uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes());
        cx.displays().into_iter().find(|d| d.uuid().is_ok_and(|u| u == uuid))
    };
    configured
        .map(str::to_owned)
        .and_then(by_name)
        .or_else(|| crate::niri::focused_output().and_then(by_name))
        .or_else(|| cx.primary_display())
}

trait ObserveWindowClosed {
    fn observe_window_closed_by_id(&mut self);
}

impl ObserveWindowClosed for Context<'_, Launcher> {
    fn observe_window_closed_by_id(&mut self) {
        let this = self.weak_entity();
        self.on_window_closed(move |cx, id| {
            _ = this.update(cx, |this, _| {
                if this.window.is_some_and(|w| w.window_id() == id) {
                    this.window = None;
                }
            });
        })
        .detach();
    }
}

struct LauncherView {
    launcher: Entity<Launcher>,
    input: Entity<InputState>,
    scroll: UniformListScrollHandle,
    ranker: Ranker,
    query: String,
    /// App indices, best match first.
    results: Vec<usize>,
    selected: usize,
    _subscriptions: Vec<Subscription>,
}

impl LauncherView {
    fn new(launcher: Entity<Launcher>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search apps…"));
        input.update(cx, |state, cx| state.focus(window, cx));
        let subscriptions = vec![
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = input.read(cx).value().to_string();
                    this.selected = 0;
                    this.rerank(cx);
                }
            }),
            // The index can finish after the window opened.
            cx.observe(&launcher, |this, _, cx| this.rerank(cx)),
        ];
        Self {
            launcher,
            input,
            scroll: UniformListScrollHandle::new(),
            ranker: Ranker::new(),
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            _subscriptions: subscriptions,
        }
    }

    fn rerank(&mut self, cx: &mut Context<Self>) {
        if self.query.trim().is_empty() {
            self.results.clear();
        } else {
            let launcher = self.launcher.read(cx);
            self.results = self
                .ranker
                .rank(&launcher.apps, &launcher.history, &self.query, history::now());
            self.results.truncate(MAX_RESULTS);
        }
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
        cx.notify();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.results.len() && ix != self.selected {
            self.selected = ix;
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.results.is_empty() {
            self.select((self.selected + 1) % self.results.len(), cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        if !self.results.is_empty() {
            let n = self.results.len();
            self.select((self.selected + n - 1) % n, cx);
        }
    }

    fn open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        self.open_at(self.selected, cx);
    }

    fn open_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(&app) = self.results.get(ix) {
            self.launcher.update(cx, |l, cx| l.launch(app, cx));
        }
    }

    /// Escape clears a query first, then closes.
    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.query.is_empty() {
            self.launcher.update(cx, |l, cx| l.hide(cx));
        } else {
            // set_value does not emit InputEvent::Change.
            self.input.update(cx, |input, cx| input.set_value("", window, cx));
            self.query.clear();
            self.rerank(cx);
        }
    }

    fn render_row(&self, ix: usize, apps: &[DesktopApp], cx: &mut Context<Self>) -> AnyElement {
        let app = &apps[self.results[ix]];
        let theme = cx.theme();
        let selected = ix == self.selected;
        let detail = app.comment.as_ref().or(app.generic_name.as_ref()).cloned();
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
            .on_click(cx.listener(move |this, _, _, cx| this.open_at(ix, cx)))
            .child(app_icon(&app.name, app.icon_path.clone()))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if selected { theme.accent_foreground } else { theme.foreground })
                            .truncate()
                            .child(app.name.clone()),
                    )
                    .when_some(detail, |this, detail| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .truncate()
                                .child(detail),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if app.terminal { "Terminal" } else { "Application" }),
            )
            .into_any_element()
    }
}

impl Render for LauncherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let launcher = self.launcher.read(cx);
        let apps = launcher.apps.clone();
        let appearance = launcher.config.appearance.clone();
        let indexing = launcher.indexing && apps.is_empty();
        let theme = cx.theme().clone();
        let viewport = window.viewport_size();
        let width = px(appearance.width).min(viewport.width - px(32.));
        let top = viewport.height * appearance.top.clamp(0., 0.8);
        let searching = !self.query.trim().is_empty();

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

        let body = if !searching {
            None
        } else if self.results.is_empty() {
            Some(
                div()
                    .py_6()
                    .w_full()
                    .text_center()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if indexing { "Indexing apps…" } else { "No matches" })
                    .into_any_element(),
            )
        } else {
            let rows = self.results.len() as f32;
            let list = uniform_list(
                "results",
                self.results.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range.map(|ix| this.render_row(ix, &apps, cx)).collect()
                }),
            )
            .track_scroll(&self.scroll)
            .h(px(ROW_HEIGHT * rows.min(VISIBLE_ROWS)))
            .px_2();
            Some(
                v_flex()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(list)
                    .into_any_element(),
            )
        };

        let panel = v_flex()
            .id("panel")
            .w(width)
            .occlude()
            .overflow_hidden()
            .rounded(px(18.))
            .border_1()
            .border_color(theme.border.opacity(0.8))
            .bg(theme.popover)
            .shadow_2xl()
            .child(search)
            .children(body)
            .when(searching && !self.results.is_empty(), |panel| panel.child(footer(&theme)))
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
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::dismiss))
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

/// The theme icon, or a coloured initial when the app has none.
fn app_icon(name: &str, path: Option<std::path::PathBuf>) -> AnyElement {
    let size = px(ICON_SIZE);
    if let Some(path) = path {
        return img(path).size(size).flex_none().into_any_element();
    }
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

fn footer(theme: &Theme) -> AnyElement {
    let hint = |key: &str, label: &'static str| {
        h_flex()
            .gap_1p5()
            .items_center()
            .child(div().child(label))
            .child(Kbd::new(Keystroke::parse(key).expect("valid key hint")))
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
        .child(hint("enter", "Open"))
        .child(hint("escape", "Close"))
        .into_any_element()
}
