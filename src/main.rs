mod apps;
mod clipboard;
mod config;
mod emoji;
mod history;
mod icons;
mod ipc;
mod items;
mod launch;
mod niri;
mod search;
mod system;
mod ui;

use gpui_kit::{AppContext as _, QuitMode};

use ipc::Request;

const USAGE: &str = "usage: rockhopper [toggle|show|hide|quit|daemon]

  toggle   open the launcher, or close it if open (default)
  show     open the launcher
  hide     close the launcher
  clipboard  open on the clipboard history
  emoji    open on the emoji picker
  quit     stop the running launcher
  daemon   run without opening, for autostart

toggle, show, clipboard and emoji start the launcher when it is not running yet.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match args.as_slice() {
        [] => "toggle",
        [one] => one.as_str(),
        _ => usage_error(),
    };
    match command {
        "-h" | "--help" | "help" => println!("{USAGE}"),
        "-V" | "--version" => println!("rockhopper {}", env!("CARGO_PKG_VERSION")),
        "daemon" => run_daemon(None),
        other => {
            let Some(request) = Request::parse(other) else {
                usage_error()
            };
            match ipc::send(request) {
                Ok(true) => {}
                Ok(false) => match request {
                    Request::Toggle | Request::Show | Request::Clipboard | Request::Emoji => {
                        run_daemon(Some(request))
                    }
                    Request::Hide | Request::Quit => {}
                },
                Err(err) => {
                    eprintln!("rockhopper: {err}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn usage_error() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

/// Run the daemon; `first` is handled once it is up.
fn run_daemon(first: Option<Request>) {
    let listener = match ipc::bind() {
        Ok(l) => l,
        // Not a failure, so a service manager does not restart-loop.
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("rockhopper: {err}");
            return;
        }
        Err(err) => {
            eprintln!("rockhopper: {err}");
            std::process::exit(1);
        }
    };
    let (tx, rx) = async_channel::unbounded();
    ipc::serve(listener, tx);

    // Explicit: hiding closes the only window, which must not quit.
    gpui_kit::application()
        .with_assets(ui::Assets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            ui::bind_keys(cx);
            let launcher = cx.new(ui::Launcher::new);
            if let Some(request) = first {
                launcher.update(cx, |l, cx| l.handle(request, cx));
            }
            cx.spawn(async move |cx| {
                while let Ok(request) = rx.recv().await {
                    launcher.update(cx, |l, cx| l.handle(request, cx));
                }
            })
            .detach();
        });
}
