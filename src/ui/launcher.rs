//! The launcher window: a full-screen layer-shell overlay with a search box
//! that grows a result list as you type.
//!
//! [`Launcher`] lives for the whole daemon and owns the app index, history and
//! config. Each time it is shown it opens a fresh [`view::LauncherView`]
//! window, and hiding closes it, so every opening starts with an empty query.

use super::view;

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::Root;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ClipboardItem, Context, Pixels, PlatformDisplay,
    Point, Styled as _, WeakEntity, WindowBackgroundAppearance, WindowBounds, WindowId, WindowKind,
    WindowOptions, px, size, transparent_black,
};

use crate::apps::{self, App as DesktopApp};
use crate::clipboard::{Clip, Clipboard, ImageCapture};
use crate::commands::{self, Command};
use crate::config::{self, Config, ThemeChoice};
use crate::emoji::{self, Emoji, EmojiFont};
use crate::history::{self, History};
use crate::ipc::{Request, TestInput};
use crate::items::{self, Action, Entry, Item};
use crate::{icons, launch, niri};

/// gpui-kit wraps every window in a client-side frame: a 20px shadow gutter
/// and a 1px border. GPUI reports layer surfaces as client-decorated, so the
/// overlay gets that frame too, and we push it past the output's edges.
const FRAME_INSET: Pixels = px(21.);

pub struct Launcher {
    pub(super) config: Config,
    config_stamp: config::Stamp,
    pub(super) apps: Rc<Vec<DesktopApp>>,
    /// Open windows, read from niri each time the launcher opens.
    windows: Vec<niri::Window>,
    /// Workspaces and outputs, snapshotted when the launcher opens.
    pub(super) workspaces: Vec<niri::Workspace>,
    pub(super) outputs: Vec<niri::Output>,
    /// The workspace focused when the launcher opened.
    focused_workspace: Option<u64>,
    /// The configured commands, built-in ones included.
    pub(super) commands: Rc<Vec<Command>>,
    /// Focus captured before the overlay receives keyboard focus.
    focused_window: Option<niri::FocusedWindowSnapshot>,
    /// Everything searchable, rebuilt when apps or windows change.
    pub(super) entries: Rc<Vec<Entry>>,
    fingerprint: Vec<config::FileStamp>,
    pub(super) indexing: bool,
    pub(super) history: History,
    pub(super) clipboard: Option<Clipboard>,
    /// Clipboard history, newest first, refreshed after each copy.
    pub(super) clips: Rc<Vec<Clip>>,
    /// Every emoji, in the configured skin tone.
    pub(super) emojis: Rc<Vec<Emoji>>,
    /// The colour emoji font the picker draws images from; loaded in the
    /// background, and `None` until then or if there is none.
    pub(super) emoji_font: Option<std::sync::Arc<EmojiFont>>,
    /// Where the next window opens.
    open_on: Start,
    window: Option<AnyWindowHandle>,
    /// The open window's content, for test input.
    view: Option<WeakEntity<view::LauncherView>>,
    opening: bool,
    hide_when_open: bool,
    /// Last failed niri action, shown in the launcher.
    pub(super) action_error: Option<String>,
    /// A focus-dependent command held until this overlay is actually closed.
    pending_niri_command: Option<PendingNiriCommand>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingNiriCommand {
    overlay_window: WindowId,
    command: niri::NiriCommand,
    window_id: Option<u64>,
    is_floating: bool,
}

fn take_pending_for_closed_window(
    pending: &mut Option<PendingNiriCommand>,
    closed_window: WindowId,
) -> Option<PendingNiriCommand> {
    if pending.is_some_and(|command| command.overlay_window == closed_window) {
        pending.take()
    } else {
        None
    }
}

fn focused_window_for_open(
    result: std::io::Result<Option<niri::FocusedWindowSnapshot>>,
) -> (Option<niri::FocusedWindowSnapshot>, Option<String>) {
    match result {
        Ok(snapshot) => (snapshot, None),
        Err(err) => (
            None,
            Some(format!("Could not read focused window from niri: {err}")),
        ),
    }
}

impl Launcher {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Forget a window the compositor closed under us.
        observe_window_closed_by_id(cx);
        let config = config::load();
        let clipboard = config.clipboard.enabled.then(|| {
            let (tx, rx) = async_channel::bounded::<()>(1);
            let image_capture = if config.clipboard.images {
                ImageCapture::IncludeImages
            } else {
                ImageCapture::TextOnly
            };
            let clipboard =
                Clipboard::start(config.clipboard.max_entries, image_capture, move || {
                    _ = tx.try_send(());
                });
            cx.spawn(async move |this, cx| {
                while rx.recv().await.is_ok() {
                    if this.update(cx, |this, cx| this.refresh_clips(cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            clipboard
        });
        let clips = Rc::new(clipboard.as_ref().map(Clipboard::clips).unwrap_or_default());
        let emojis = Rc::new(emoji::all(config.emoji.skin_tone));
        let commands = Rc::new(commands::resolve(&config.commands));
        let mut this = Self {
            config,
            config_stamp: config::stamp(),
            apps: Rc::new(Vec::new()),
            windows: Vec::new(),
            workspaces: Vec::new(),
            outputs: Vec::new(),
            focused_workspace: None,
            commands,
            focused_window: None,
            entries: Rc::new(Vec::new()),
            fingerprint: Vec::new(),
            indexing: false,
            history: History::load(),
            clipboard,
            clips,
            emojis,
            emoji_font: None,
            open_on: Start::Search,
            window: None,
            view: None,
            opening: false,
            hide_when_open: false,
            action_error: None,
            pending_niri_command: None,
        };
        this.refresh_index(cx);
        this.load_emoji_font(cx);
        this
    }

    pub fn handle(&mut self, request: Request, cx: &mut Context<Self>) {
        match request {
            Request::Toggle if self.window.is_some() || self.opening => self.hide(cx),
            Request::Toggle | Request::Show => self.show(cx),
            Request::Clipboard | Request::Emoji => {
                let start = if request == Request::Clipboard {
                    if self.clipboard.is_none() {
                        eprintln!("rockhopper: clipboard history is turned off");
                    }
                    Start::Clipboard
                } else {
                    Start::Emoji
                };
                // An open launcher is replaced so it starts there.
                self.hide(cx);
                self.open_on = start;
                self.show(cx);
            }
            Request::Hide => self.hide(cx),
            Request::Test(input) => self.test_input(input, cx),
            Request::Quit => {
                crate::ipc::remove_socket();
                cx.quit();
            }
        }
    }

    fn show_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.show(cx);
        self.action_error = Some(message);
        cx.notify();
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.hide_when_open = false;
        self.action_error = None;
        if self.window.is_some() || self.opening {
            return;
        }
        let start = match std::mem::replace(&mut self.open_on, Start::Search) {
            Start::Clipboard if self.clipboard.is_none() => Start::Search,
            start => start,
        };
        let stamp = config::stamp();
        if stamp != self.config_stamp {
            self.config_stamp = stamp;
            let config = config::load();
            let emoji_changed = config.emoji != self.config.emoji;
            self.commands = Rc::new(commands::resolve(&config.commands));
            self.config = config;
            if emoji_changed {
                self.load_emoji_font(cx);
            }
        }
        self.refresh_index(cx);
        self.focused_window = if niri::available() {
            let (snapshot, error) = focused_window_for_open(niri::focused_window_snapshot());
            if let Some(message) = error {
                eprintln!("rockhopper: {message}");
                self.action_error = Some(message);
            }
            snapshot
        } else {
            None
        };
        // Read before the launcher takes keyboard focus, so the focused
        // window is the one window commands act on.
        self.windows = niri::windows();
        self.workspaces = niri::workspaces();
        self.outputs = niri::outputs();
        self.focused_workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.is_focused)
            .map(|workspace| workspace.id);
        self.rebuild_entries();
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
        let output = display
            .as_ref()
            .map_or(size(px(1920.), px(1080.)), |d| d.bounds().size);
        let options = WindowOptions {
            titlebar: None,
            display_id: display.as_ref().map(|d| d.id()),
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(
                    output.width + FRAME_INSET * 2.,
                    output.height + FRAME_INSET * 2.,
                ),
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
                let view =
                    cx.new(|cx| view::LauncherView::new(view_launcher.clone(), start, window, cx));
                let weak = view.downgrade();
                view_launcher.update(cx, |this, _| this.view = Some(weak));
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
                    Err(err) => {
                        eprintln!("rockhopper: could not open the launcher window: {err:#}")
                    }
                }
            });
        });
    }

    /// Apply test input to the open window, after this update, so the
    /// view's handlers can update the launcher in turn.
    fn test_input(&mut self, input: TestInput, cx: &mut Context<Self>) {
        let (Some(handle), Some(view)) = (self.window, self.view.clone()) else {
            eprintln!("rockhopper: test input with no window open");
            return;
        };
        cx.defer(move |cx| {
            _ = handle.update(cx, |_, window, cx| match input {
                TestInput::Type(text) => {
                    _ = view.update(cx, |view, cx| view.type_text(&text, window, cx));
                }
                TestInput::Key(key) => match view::test_action(&key) {
                    Some(action) => window.dispatch_action(action, cx),
                    None => eprintln!("rockhopper: unknown test key {key:?}"),
                },
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

    fn rebuild_entries(&mut self) {
        self.entries = Rc::new(items::entries(
            &self.apps,
            &self.windows,
            &self.commands,
            self.clipboard.is_some(),
            niri::available(),
            self.focused_window,
        ));
    }

    fn refresh_clips(&mut self, cx: &mut Context<Self>) {
        if let Some(clipboard) = &self.clipboard {
            self.clips = Rc::new(clipboard.clips());
            cx.notify();
        }
    }

    pub(super) fn clip_image(&self, id: u64) -> Option<std::sync::Arc<Vec<u8>>> {
        self.clipboard.as_ref()?.image(id)
    }

    pub(super) fn copy_clip(&mut self, clip: &Clip, cx: &mut Context<Self>) {
        if let Some(clipboard) = &self.clipboard
            && let Err(err) = clipboard.copy(clip)
        {
            eprintln!("rockhopper: could not copy that entry: {err}");
        }
        self.history.record("rockhopper:clipboard", history::now());
        self.hide(cx);
    }

    /// Copy `text`, through the clipboard watcher when there is one so it
    /// outlives this window, and close.
    pub(super) fn copy_text(
        &mut self,
        text: &str,
        history_key: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let copied = match &self.clipboard {
            Some(clipboard) => clipboard.copy_text(text),
            None => Err(std::io::Error::other("no clipboard watcher")),
        };
        if copied.is_err() {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
        if let Some(key) = history_key {
            self.history.record(&key, history::now());
        }
        self.hide(cx);
    }

    /// Load the colour emoji font in the background, and with it the list
    /// of emoji it can draw: newer emoji than the font knows would show as
    /// blank boxes.
    fn load_emoji_font(&mut self, cx: &mut Context<Self>) {
        let configured = self.config.emoji.font.clone();
        let tone = self.config.emoji.skin_tone;
        cx.spawn(async move |this, cx| {
            let (font, emojis) = cx
                .background_spawn(async move {
                    let font = EmojiFont::load(configured.as_deref());
                    let mut emojis = emoji::all(tone);
                    if let Some(font) = &font {
                        emojis.retain(|e| font.draws(e.glyph));
                    }
                    (font, emojis)
                })
                .await;
            match &font {
                Some(font) => eprintln!("rockhopper: drawing emoji from {}", font.family),
                None => eprintln!(
                    "rockhopper: no colour emoji font found; emoji are drawn as text. Install Noto Color Emoji or set emoji.font"
                ),
            }
            _ = this.update(cx, |this, cx| {
                this.emoji_font = font.map(std::sync::Arc::new);
                // Unless the skin tone changed meanwhile, which reloads.
                if this.config.emoji.skin_tone == tone {
                    this.emojis = Rc::new(emojis);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn delete_clip(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(clipboard) = &self.clipboard {
            clipboard.delete(id);
        }
        self.refresh_clips(cx);
    }

    pub(super) fn clear_clips(&mut self, cx: &mut Context<Self>) {
        if let Some(clipboard) = &self.clipboard {
            clipboard.clear();
        }
        self.refresh_clips(cx);
    }

    /// Run an niri command against the window captured when the launcher
    /// opened. Focus-dependent commands wait until the overlay closes and
    /// niri restores focus to that same window.
    fn perform_niri_command(
        &mut self,
        command: niri::NiriCommand,
        window_id: Option<u64>,
        is_floating: bool,
        cx: &mut Context<Self>,
    ) {
        self.action_error = None;
        let label = command.name(is_floating).to_owned();
        if command.scope == niri::CommandScope::Window {
            match command.perform(window_id) {
                Ok(()) => {
                    self.history.record(command.key, history::now());
                    self.hide(cx);
                }
                Err(err) => {
                    let message = format!("{label}: {err}");
                    eprintln!("rockhopper: {message}");
                    self.action_error = Some(message);
                    cx.notify();
                }
            }
            return;
        }

        let Some(handle) = self.window else {
            let message = format!("{label}: the launcher window is not open");
            self.action_error = Some(message);
            cx.notify();
            return;
        };
        self.pending_niri_command = Some(PendingNiriCommand {
            overlay_window: handle.window_id(),
            command,
            window_id,
            is_floating,
        });
        self.hide(cx);
    }

    fn run_niri_command_after_overlay_closed(
        &mut self,
        pending: PendingNiriCommand,
        cx: &mut Context<Self>,
    ) {
        let PendingNiriCommand {
            command,
            window_id,
            is_floating,
            ..
        } = pending;
        let label = command.name(is_floating).to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if command.scope == niri::CommandScope::Focused {
                        let expected = window_id.ok_or_else(|| {
                            std::io::Error::other("no focused window was captured")
                        })?;
                        let deadline = Instant::now() + Duration::from_secs(1);
                        while niri::focused_window_id()? != Some(expected) {
                            if Instant::now() >= deadline {
                                return Err(std::io::Error::other(
                                    "focus did not return to the captured window; command was not sent",
                                ));
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                    command.perform(None)
                })
                .await;
            match result {
                Ok(()) => {
                    _ = this.update(cx, |launcher, _| {
                        launcher.history.record(command.key, history::now());
                    });
                }
                Err(err) => {
                    let message = format!("{label}: {err}");
                    eprintln!("rockhopper: {message}");
                    _ = this.update(cx, |launcher, cx| launcher.show_error(message, cx));
                }
            }
        })
        .detach();
    }

    /// Carry out `action` on `item`. Most actions close the launcher; those
    /// that change the list in place leave it open.
    pub(super) fn perform(&mut self, item: &Item, action: Action, cx: &mut Context<Self>) {
        if let (
            Item::NiriCommand {
                command,
                window_id,
                is_floating,
            },
            Action::Primary,
        ) = (item, action)
        {
            self.perform_niri_command(*command, *window_id, *is_floating, cx);
            return;
        }
        let result = match (item, action) {
            (Item::App(i), Action::Primary) => {
                let app = &self.apps[*i];
                let policy = if self.config.launcher.focus_existing {
                    items::AppLaunchPolicy::PreferExisting
                } else {
                    items::AppLaunchPolicy::AlwaysLaunch
                };
                match items::choose_app_open(
                    app,
                    &self.windows,
                    policy,
                    items::AppLaunchIntent::Default,
                ) {
                    items::AppOpen::FocusWindow(id) => niri::focus_window(id),
                    items::AppOpen::Launch => launch::launch(app, &self.config),
                }
            }
            (Item::App(i), Action::NewInstance) => {
                let app = &self.apps[*i];
                launch::launch(app, &self.config)
                    .inspect(|()| self.history.record(&app.id, history::now()))
            }
            (Item::App(i), Action::Desktop(n)) => {
                let app = &self.apps[*i];
                launch::launch_argv(app, &app.actions[n].argv, &self.config)
            }
            (Item::App(i), Action::CopyCommand) => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.apps[*i].argv.join(" ")));
                Ok(())
            }
            (Item::Window { window, .. }, Action::Primary) => niri::focus_window(window.id),
            (Item::Window { app: Some(a), .. }, Action::NewInstance) => {
                let app = &self.apps[*a];
                launch::launch(app, &self.config)
                    .inspect(|()| self.history.record(&app.id, history::now()))
            }
            (Item::Window { window, .. }, Action::CloseWindow) => {
                let closed = niri::close_window(window.id);
                if closed.is_ok() {
                    self.windows.retain(|w| w.id != window.id);
                    self.rebuild_entries();
                    cx.notify();
                }
                self.finish_window_action(closed, "Close Window", cx);
                return;
            }
            (Item::Window { window, .. }, Action::CopyTitle) => {
                cx.write_to_clipboard(ClipboardItem::new_string(window.title.clone()));
                Ok(())
            }
            (Item::Window { window, .. }, Action::ToggleFullscreen) => {
                niri::perform_window_action(window.id, niri::WindowAction::ToggleFullscreen)
            }
            (Item::Window { window, .. }, Action::ToggleWindowedFullscreen) => {
                niri::perform_window_action(window.id, niri::WindowAction::ToggleWindowedFullscreen)
            }
            (Item::Window { window, .. }, Action::ToggleFloating) => {
                niri::perform_window_action(window.id, niri::WindowAction::ToggleFloating)
            }
            (Item::Window { window, .. }, Action::CenterWindow) => {
                niri::perform_window_action(window.id, niri::WindowAction::Center)
            }
            (Item::Window { window, .. }, Action::SetWidthThird) => {
                niri::perform_window_action(window.id, niri::WindowAction::SetWidth(1.0 / 3.0))
            }
            (Item::Window { window, .. }, Action::SetWidthHalf) => {
                niri::perform_window_action(window.id, niri::WindowAction::SetWidth(0.5))
            }
            (Item::Window { window, .. }, Action::SetWidthTwoThirds) => {
                niri::perform_window_action(window.id, niri::WindowAction::SetWidth(2.0 / 3.0))
            }
            (Item::Window { window, .. }, Action::SetWidthFull) => {
                niri::perform_window_action(window.id, niri::WindowAction::SetWidth(1.0))
            }
            (Item::Window { window, .. }, Action::BringToThisWorkspace) => {
                self.focused_workspace.map_or_else(
                    || Err(std::io::Error::other("no focused workspace was recorded")),
                    |workspace| niri::move_window_to_workspace(window.id, workspace),
                )
            }
            (Item::Window { .. }, Action::MoveToWorkspace | Action::MoveToMonitor) => Err(
                std::io::Error::other("choose a workspace or monitor from its picker"),
            ),
            (Item::Window { window, .. }, Action::ScreenshotWindow) => {
                niri::perform_window_action(window.id, niri::WindowAction::Screenshot)
            }
            (Item::System(cmd), Action::Primary) => cmd.run(&self.config),
            (
                Item::Command {
                    command, window, ..
                },
                Action::Primary,
            ) => {
                self.history.record(&command.history_key(), history::now());
                return self.run_command(command, window.as_ref(), cx);
            }
            (Item::Window { window, .. }, Action::Command(n)) => {
                let Some(command) = self.commands.get(n).cloned() else {
                    return;
                };
                return self.run_command(&command, Some(window), cx);
            }
            (_, Action::Forget) => {
                if let Some(key) = item.history_key(&self.apps) {
                    self.history.forget(&key);
                }
                cx.notify();
                return;
            }
            (item, action) => {
                eprintln!("rockhopper: {action:?} does not apply to {item:?}");
                return;
            }
        };
        // Primary app activation counts for ranking whether it launched or focused a window.
        let records_usage = matches!(action, Action::Primary | Action::Desktop(_));
        if result.is_ok()
            && records_usage
            && let Some(key) = item.history_key(&self.apps)
        {
            self.history.record(&key, history::now());
        }
        if matches!(item, Item::Window { .. }) {
            self.finish_window_action(result, "Window action", cx);
            return;
        }
        report(result, item, &self.apps);
        self.hide(cx);
    }

    /// Finish a window action, leaving the launcher open with an error on
    /// failure and hiding it after success.
    pub(super) fn finish_window_action(
        &mut self,
        result: std::io::Result<()>,
        label: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match result {
            Ok(()) => {
                self.action_error = None;
                self.hide(cx);
                true
            }
            Err(err) => {
                let message = format!("{label}: {err}");
                eprintln!("rockhopper: {message}");
                self.action_error = Some(message);
                cx.notify();
                false
            }
        }
    }

    /// Close, then run `command` on `window`: commands that act on the
    /// focused window without naming it need the launcher's focus gone.
    fn run_command(
        &mut self,
        command: &Command,
        window: Option<&niri::Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(argv) = command.argv(window) else {
            eprintln!("rockhopper: {}: no window to act on", command.name);
            return;
        };
        self.hide(cx);
        let name = command.name.clone();
        // After hide's own deferred close.
        cx.defer(move |_| {
            if let Err(err) = crate::system::spawn(&argv) {
                eprintln!("rockhopper: {name}: {err}");
            }
        });
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
            eprintln!(
                "rockhopper: indexed {} apps in {:?}",
                found.len(),
                started.elapsed()
            );
            _ = this.update(cx, |this, cx| {
                this.apps = Rc::new(found);
                this.rebuild_entries();
                this.fingerprint = fingerprint;
                this.indexing = false;
                cx.notify();
            });
        })
        .detach();
    }
}

/// Where a window opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    Search,
    Clipboard,
    Emoji,
}

fn report(result: std::io::Result<()>, item: &Item, apps: &[DesktopApp]) {
    if let Err(err) = result {
        eprintln!("rockhopper: {}: {err}", item.title(apps));
    }
}

/// The configured output, else the one niri says has focus, else the primary.
fn target_display(configured: Option<&str>, cx: &gpui_kit::App) -> Option<Rc<dyn PlatformDisplay>> {
    let by_name = |name: String| {
        let uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes());
        cx.displays()
            .into_iter()
            .find(|d| d.uuid().is_ok_and(|u| u == uuid))
    };
    configured
        .map(str::to_owned)
        .and_then(by_name)
        .or_else(|| niri::focused_output().and_then(by_name))
        .or_else(|| cx.primary_display())
}

fn observe_window_closed_by_id(cx: &mut Context<Launcher>) {
    let this = cx.weak_entity();
    cx.on_window_closed(move |cx, id| {
        _ = this.update(cx, |this, cx| {
            if this.window.is_some_and(|w| w.window_id() == id) {
                this.window = None;
            }
            if let Some(pending) =
                take_pending_for_closed_window(&mut this.pending_niri_command, id)
            {
                this.run_niri_command_after_overlay_closed(pending, cx);
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod searchable_command_tests {
    use super::{PendingNiriCommand, focused_window_for_open, take_pending_for_closed_window};
    use crate::niri;
    use gpui_kit::WindowId;

    #[test]
    fn failed_initial_niri_snapshot_becomes_a_launcher_error() {
        let (snapshot, error) =
            focused_window_for_open(Err(std::io::Error::other("niri socket unavailable")));

        assert_eq!(snapshot, None);
        assert_eq!(
            error.as_deref(),
            Some("Could not read focused window from niri: niri socket unavailable")
        );
    }

    #[test]
    fn focus_dependent_commands_wait_for_their_overlay_close_event() {
        let command = *niri::NIRI_COMMANDS
            .iter()
            .find(|command| command.scope == niri::CommandScope::Focused)
            .unwrap();
        let pending = PendingNiriCommand {
            overlay_window: WindowId::from(11),
            command,
            window_id: Some(41),
            is_floating: false,
        };
        let mut queued = Some(pending);

        assert!(take_pending_for_closed_window(&mut queued, WindowId::from(12)).is_none());
        assert_eq!(queued, Some(pending));
        assert_eq!(
            take_pending_for_closed_window(&mut queued, WindowId::from(11)),
            Some(pending)
        );
        assert!(queued.is_none());
    }
}
