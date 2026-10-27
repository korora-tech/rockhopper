//! Session and power commands.

use std::process::{Command, Stdio};

use crate::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemCommand {
    Lock,
    Suspend,
    LogOut,
    Reboot,
    PowerOff,
}

impl SystemCommand {
    pub const ALL: [Self; 5] = [
        Self::Lock,
        Self::Suspend,
        Self::LogOut,
        Self::Reboot,
        Self::PowerOff,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Lock => "Lock Screen",
            Self::Suspend => "Suspend",
            Self::LogOut => "Log Out",
            Self::Reboot => "Restart",
            Self::PowerOff => "Shut Down",
        }
    }

    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Lock => &["lock"],
            Self::Suspend => &["sleep", "suspend"],
            Self::LogOut => &["logout", "sign out", "exit", "quit session"],
            Self::Reboot => &["reboot", "restart"],
            Self::PowerOff => &["shutdown", "power off", "poweroff", "halt"],
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Lock => "system:lock",
            Self::Suspend => "system:suspend",
            Self::LogOut => "system:logout",
            Self::Reboot => "system:reboot",
            Self::PowerOff => "system:poweroff",
        }
    }

    /// Commands that end the session ask for a second `enter`.
    pub fn needs_confirmation(self) -> bool {
        matches!(self, Self::LogOut | Self::Reboot | Self::PowerOff)
    }

    pub fn run(self, config: &Config) -> std::io::Result<()> {
        let configured = match self {
            Self::Lock => config.system.lock.as_ref(),
            Self::LogOut => config.system.logout.as_ref(),
            _ => None,
        };
        if let Some(argv) = configured.filter(|a| !a.is_empty()) {
            return spawn(argv);
        }
        let argv: Vec<String> = match self {
            Self::Lock => vec!["loginctl".into(), "lock-session".into()],
            Self::Suspend => vec!["systemctl".into(), "suspend".into()],
            Self::Reboot => vec!["systemctl".into(), "reboot".into()],
            Self::PowerOff => vec!["systemctl".into(), "poweroff".into()],
            Self::LogOut if crate::niri::available() => return crate::niri::quit(),
            Self::LogOut => match std::env::var("XDG_SESSION_ID") {
                Ok(id) => vec!["loginctl".into(), "terminate-session".into(), id],
                Err(_) => vec!["loginctl".into(), "terminate-user".into(), whoami()],
            },
        };
        spawn(&argv)
    }
}

fn whoami() -> String {
    std::env::var("USER").unwrap_or_default()
}

/// Start `argv` detached, without waiting for it.
pub fn spawn(argv: &[String]) -> std::io::Result<()> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}
