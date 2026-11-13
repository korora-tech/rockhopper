//! Starting applications, detached from Rockhopper.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::apps::App;
use crate::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemdRunAvailability {
    Available,
    Unavailable,
}

/// The full command line for `app`, including the terminal and systemd scope
/// wrappers.
pub fn command_line(
    app: &App,
    config: &Config,
    systemd_run: SystemdRunAvailability,
) -> Vec<String> {
    let mut argv = Vec::new();
    if config.launch.systemd_scope && systemd_run == SystemdRunAvailability::Available {
        argv.extend(scope_prefix(&unit_id(&app.id), &app.name));
    }
    if app.terminal {
        argv.extend(config.terminal.iter().cloned());
    }
    argv.extend(app.argv.iter().cloned());
    argv
}

pub fn launch(app: &App, config: &Config) -> std::io::Result<()> {
    launch_argv(app, &app.argv, config)
}

/// Start `app` with another command line, such as a desktop action's.
pub fn launch_argv(app: &App, argv: &[String], config: &Config) -> std::io::Result<()> {
    let app = App {
        argv: argv.to_vec(),
        ..app.clone()
    };
    let app = &app;
    let argv = command_line(app, config, has_systemd_run());
    spawn_detached(&argv, app.cwd.as_deref())
}

/// Start `argv` detached, in its own systemd scope named for `id` when
/// scopes are available and enabled.
pub fn run_scoped(
    argv: &[String],
    id: &str,
    description: &str,
    config: &Config,
) -> std::io::Result<()> {
    let mut full = Vec::new();
    if config.launch.systemd_scope && has_systemd_run() == SystemdRunAvailability::Available {
        full.extend(scope_prefix(&unit_id(id), description));
    }
    full.extend(argv.iter().cloned());
    spawn_detached(&full, None)
}

/// Run `argv` in its own session, from `cwd` (else the home directory), with
/// no standard streams.
fn spawn_detached(argv: &[String], cwd: Option<&std::path::Path>) -> std::io::Result<()> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cwd.filter(|dir| dir.is_dir()) {
        Some(dir) => {
            cmd.current_dir(dir);
        }
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                cmd.current_dir(home);
            }
        }
    }
    // SAFETY: `pre_exec` requires async-signal-safe work after fork. The callback
    // only invokes the setsid syscall and touches no Rust-managed state.
    unsafe {
        cmd.pre_exec(crate::syscalls::setsid);
    }
    let mut child = cmd.spawn()?;
    // Reap it so it never lingers as a zombie of the daemon.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// `systemd-run` arguments that start a command in its own user scope. Unit
/// names follow the XDG convention for launched apps:
/// `app-<launcher>-<id>-<random>.scope`.
fn scope_prefix(id: &str, description: &str) -> Vec<String> {
    let mut argv: Vec<String> = [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        "--same-dir",
    ]
    .map(String::from)
    .into();
    argv.push(format!("--unit=app-rockhopper-{id}-{}", nonce()));
    argv.push(format!("--description={description}"));
    argv.push("--".into());
    argv
}

fn has_systemd_run() -> SystemdRunAvailability {
    if std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("systemd-run").is_file()))
    {
        SystemdRunAvailability::Available
    } else {
        SystemdRunAvailability::Unavailable
    }
}

/// Desktop file ID without `.desktop`, limited to characters systemd allows
/// in unit names; `-` separates fields, so it is escaped too.
fn unit_id(id: &str) -> String {
    let stem = id.strip_suffix(".desktop").unwrap_or(id);
    stem.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' => (b as char).to_string(),
            b => format!("\\x{b:02x}"),
        })
        .collect()
}

fn nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{:08x}", nanos ^ std::process::id().rotate_left(16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(terminal: bool) -> App {
        App {
            id: "org.foo-bar.desktop".into(),
            name: "Foo".into(),
            generic_name: None,
            comment: None,
            keywords: vec![],
            argv: vec!["htop".into()],
            terminal,
            cwd: None,
            icon: None,
            icon_path: None,
            wm_class: None,
            actions: vec![],
        }
    }

    #[test]
    fn plain_and_terminal() {
        let config = Config {
            terminal: vec!["ghostty".into(), "-e".into()],
            ..Config::default()
        };
        assert_eq!(
            command_line(&app(false), &config, SystemdRunAvailability::Unavailable),
            ["htop"]
        );
        assert_eq!(
            command_line(&app(true), &config, SystemdRunAvailability::Unavailable),
            ["ghostty", "-e", "htop"]
        );
    }

    #[test]
    fn systemd_scope_wraps() {
        let argv = command_line(
            &app(false),
            &Config::default(),
            SystemdRunAvailability::Available,
        );
        assert_eq!(argv[0], "systemd-run");
        let unit = argv.iter().find(|a| a.starts_with("--unit=")).unwrap();
        assert!(
            unit.starts_with(r"--unit=app-rockhopper-org.foo\x2dbar-"),
            "{unit}"
        );
        assert_eq!(&argv[argv.len() - 2..], ["--", "htop"]);
    }

    #[test]
    fn unit_id_escapes_bytes() {
        assert_eq!(unit_id("café.desktop"), r"caf\xc3\xa9");
    }
}
