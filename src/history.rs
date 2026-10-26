//! Launch history: how often and how recently each item was opened.
//!
//! Stored as JSON in `$XDG_STATE_HOME/rockhopper/history.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    pub count: u32,
    /// Unix seconds.
    pub last: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    items: HashMap<String, Visit>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl History {
    pub fn load() -> Self {
        let path = state_dir().map(|d| d.join("history.json"));
        let mut history: Self = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| match serde_json::from_slice(&b) {
                Ok(h) => Some(h),
                Err(err) => {
                    eprintln!("rockhopper: ignoring unreadable history: {err}");
                    None
                }
            })
            .unwrap_or_default();
        history.path = path;
        history
    }

    pub fn record(&mut self, id: &str, now: u64) {
        let visit = self.items.entry(id.to_owned()).or_default();
        visit.count = visit.count.saturating_add(1);
        visit.last = now;
        self.save();
    }

    pub fn forget(&mut self, id: &str) {
        if self.items.remove(id).is_some() {
            self.save();
        }
    }

    /// Frecency: visit count weighted by how recently the item was last used,
    /// in the spirit of Firefox's URL bar. Zero for unknown items.
    pub fn frecency(&self, id: &str, now: u64) -> f32 {
        let Some(visit) = self.items.get(id) else {
            return 0.0;
        };
        let age = now.saturating_sub(visit.last);
        let weight = match age {
            a if a < 4 * DAY => 1.0,
            a if a < 14 * DAY => 0.7,
            a if a < 31 * DAY => 0.5,
            a if a < 90 * DAY => 0.3,
            _ => 0.1,
        };
        visit.count as f32 * weight
    }

    /// Written atomically so a crash never leaves half a file.
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let result = (|| {
            std::fs::create_dir_all(path.parent().expect("history path has a parent"))?;
            let dir = path.parent().expect("history path has a parent");
            let tmp = path.with_extension("json.tmp");
            let mut file = std::fs::File::create(&tmp)?;
            std::io::Write::write_all(&mut file, &serde_json::to_vec(self)?)?;
            file.sync_all()?;
            std::fs::rename(&tmp, path)?;
            std::fs::File::open(dir)?.sync_all()
        })();
        if let Err(err) = result {
            eprintln!(
                "rockhopper: could not save history to {}: {err}",
                path.display()
            );
        }
    }
}

fn state_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .map(|d| d.join("rockhopper"))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frecency_decays_with_age() {
        let mut h = History::default();
        let now = 1_000 * DAY;
        h.record("a", now);
        h.record("a", now);
        h.record("b", now - 20 * DAY);
        h.record("b", now - 20 * DAY);
        assert_eq!(h.frecency("a", now), 2.0);
        assert_eq!(h.frecency("b", now), 1.0);
        assert_eq!(h.frecency("unknown", now), 0.0);
    }
}
