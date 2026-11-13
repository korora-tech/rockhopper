//! systemd services, listed and controlled through `systemctl`.
//!
//! `systemctl` is used rather than the D-Bus API so that authorization (the
//! polkit prompt for system units) and error messages are systemd's own.

use std::io;
use std::process::{Command, Stdio};

use serde::Deserialize;

/// Which systemd instance a unit belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    User,
    System,
}

impl Bus {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Self::User => "--user",
            Self::System => "--system",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveState {
    Active,
    Activating,
    Deactivating,
    Failed,
    Inactive,
}

impl ActiveState {
    fn parse(state: &str) -> Self {
        match state {
            "active" | "reloading" => Self::Active,
            "activating" => Self::Activating,
            "deactivating" => Self::Deactivating,
            "failed" => Self::Failed,
            _ => Self::Inactive,
        }
    }

    /// Sort rank: what needs attention first, then what is running.
    fn rank(self) -> u8 {
        match self {
            Self::Failed => 0,
            Self::Activating | Self::Deactivating => 1,
            Self::Active => 2,
            Self::Inactive => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// The full unit name, such as `pipewire.service`.
    pub name: String,
    pub description: String,
    pub bus: Bus,
    pub state: ActiveState,
    /// The unit's finer-grained state, such as `running` or `exited`.
    pub sub_state: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitOp {
    Start,
    Stop,
    Restart,
}

impl UnitOp {
    fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

impl Unit {
    /// The name without the `.service` suffix, which every listed unit has.
    pub fn short_name(&self) -> &str {
        self.name.strip_suffix(".service").unwrap_or(&self.name)
    }

    /// Whether `enter` stops the unit rather than starts it.
    pub fn is_running(&self) -> bool {
        matches!(self.state, ActiveState::Active | ActiveState::Activating)
    }

    /// Stopping or restarting system units can take the machine's services
    /// down, so they ask for a second `enter`.
    pub fn needs_confirmation(&self, op: UnitOp) -> bool {
        self.bus == Bus::System && op != UnitOp::Start
    }

    pub fn status_line(&self) -> String {
        let state = match self.state {
            ActiveState::Failed => "Failed".to_owned(),
            ActiveState::Inactive => "Inactive".to_owned(),
            ActiveState::Activating => "Starting".to_owned(),
            ActiveState::Deactivating => "Stopping".to_owned(),
            ActiveState::Active => format!("Active ({})", self.sub_state),
        };
        let mut line = format!("{state} · {}", self.bus.label());
        if !self.description.is_empty() && self.description != self.name {
            line.push_str(" · ");
            line.push_str(&self.description);
        }
        line
    }

    fn systemctl(&self, args: &[&str]) -> Vec<String> {
        ["systemctl", self.bus.flag(), "--no-ask-password"]
            .into_iter()
            .chain(args.iter().copied())
            .map(String::from)
            .collect()
    }

    /// The command line that follows the unit's journal in a terminal.
    pub fn logs_command(&self) -> Vec<String> {
        ["journalctl", self.bus.flag(), "--follow", "--unit"]
            .into_iter()
            .map(String::from)
            .chain([self.name.clone()])
            .collect()
    }

    /// Run `op`, waiting for systemd to finish. An error carries what
    /// `systemctl` printed.
    pub fn run(&self, op: UnitOp) -> io::Result<()> {
        let argv = self.systemctl(&[op.verb(), &self.name]);
        let output = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .output()?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        Err(io::Error::other(if message.is_empty() {
            format!("systemctl {} failed: {}", op.verb(), output.status)
        } else {
            message.to_owned()
        }))
    }
}

#[derive(Deserialize)]
struct RawUnit {
    unit: String,
    #[serde(default)]
    load: String,
    #[serde(default)]
    active: String,
    #[serde(default)]
    sub: String,
    #[serde(default)]
    description: String,
}

/// Parse `systemctl list-units --output=json`, dropping units that are
/// merely referenced by others and have no unit file.
fn parse(json: &[u8], bus: Bus) -> io::Result<Vec<Unit>> {
    let raw: Vec<RawUnit> = serde_json::from_slice(json)
        .map_err(|err| io::Error::other(format!("unreadable systemctl output: {err}")))?;
    let mut units: Vec<Unit> = raw
        .into_iter()
        .filter(|unit| unit.load != "not-found" && unit.unit.ends_with(".service"))
        .map(|unit| Unit {
            name: unit.unit,
            description: unit.description,
            bus,
            state: ActiveState::parse(&unit.active),
            sub_state: unit.sub,
        })
        .collect();
    units.sort_by(|a, b| (a.state.rank(), a.name.as_str()).cmp(&(b.state.rank(), b.name.as_str())));
    Ok(units)
}

/// Every loaded service of `bus`: running, failed and stopped.
pub fn list(bus: Bus) -> io::Result<Vec<Unit>> {
    let output = Command::new("systemctl")
        .args([
            bus.flag(),
            "list-units",
            "--type=service",
            "--all",
            "--no-pager",
            "--output=json",
        ])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(stderr.trim().to_owned()));
    }
    parse(&output.stdout, bus)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"[
        {"unit":"sshd.service","load":"loaded","active":"active","sub":"running","description":"OpenSSH Daemon"},
        {"unit":"broken.service","load":"loaded","active":"failed","sub":"failed","description":"Broken thing"},
        {"unit":"ghost.service","load":"not-found","active":"inactive","sub":"dead","description":"ghost.service"},
        {"unit":"backup.service","load":"loaded","active":"inactive","sub":"dead","description":"Nightly backup"},
        {"unit":"boot-complete.target","load":"loaded","active":"active","sub":"active","description":"Boot"},
        {"unit":"starting.service","load":"loaded","active":"activating","sub":"start","description":"Warming up"}
    ]"#;

    #[test]
    fn listing_skips_unknown_units_and_puts_failures_first() {
        let units = parse(LISTING.as_bytes(), Bus::User).unwrap();
        let names: Vec<_> = units.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "broken.service",
                "starting.service",
                "sshd.service",
                "backup.service"
            ]
        );
    }

    #[test]
    fn unreadable_output_is_an_error() {
        assert!(parse(b"not json", Bus::User).is_err());
    }

    #[test]
    fn units_describe_their_state() {
        let units = parse(LISTING.as_bytes(), Bus::System).unwrap();
        let by_name = |name: &str| units.iter().find(|u| u.name == name).unwrap();
        assert_eq!(
            by_name("sshd.service").status_line(),
            "Active (running) · system · OpenSSH Daemon"
        );
        assert_eq!(
            by_name("broken.service").status_line(),
            "Failed · system · Broken thing"
        );
        assert!(by_name("sshd.service").is_running());
        assert!(by_name("starting.service").is_running());
        assert!(!by_name("backup.service").is_running());
        assert_eq!(by_name("sshd.service").short_name(), "sshd");
    }

    #[test]
    fn only_disruptive_system_operations_need_confirmation() {
        let system = parse(LISTING.as_bytes(), Bus::System).unwrap().remove(0);
        let user = parse(LISTING.as_bytes(), Bus::User).unwrap().remove(0);
        assert!(system.needs_confirmation(UnitOp::Stop));
        assert!(system.needs_confirmation(UnitOp::Restart));
        assert!(!system.needs_confirmation(UnitOp::Start));
        assert!(!user.needs_confirmation(UnitOp::Stop));
    }

    #[test]
    fn commands_target_the_right_instance() {
        let user = parse(LISTING.as_bytes(), Bus::User).unwrap().remove(0);
        assert_eq!(
            user.systemctl(&["restart", &user.name]),
            [
                "systemctl",
                "--user",
                "--no-ask-password",
                "restart",
                "broken.service"
            ]
        );
        assert_eq!(
            user.logs_command(),
            [
                "journalctl",
                "--user",
                "--follow",
                "--unit",
                "broken.service"
            ]
        );
    }
}
