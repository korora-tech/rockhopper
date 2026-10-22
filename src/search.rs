//! Ranking: nucleo fuzzy matching boosted by launch history.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::apps::App;
use crate::history::History;

/// Matches on anything but the name count for this fraction of their score.
const SECONDARY_WEIGHT: f32 = 0.6;
/// Points per unit of `ln(1 + frecency)`.
const HISTORY_WEIGHT: f32 = 15.0;
/// Extra points when the name starts with the query.
const PREFIX_BONUS: f32 = 20.0;

pub struct Ranker {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Ranker {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        }
    }

    /// Indices into `apps`, best match first. An empty query matches nothing.
    pub fn rank(&mut self, apps: &[App], history: &History, query: &str, now: u64) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        let pattern = Pattern::new(query, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy);
        let lower = query.to_lowercase();
        let mut scored: Vec<(usize, f32)> = apps
            .iter()
            .enumerate()
            .filter_map(|(i, app)| {
                let score = self.score(&pattern, app, &lower)?;
                let boost = HISTORY_WEIGHT * history.frecency(&app.id, now).ln_1p();
                Some((i, score + boost))
            })
            .collect();
        // Stable: equal scores keep alphabetical order.
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.into_iter().map(|(i, _)| i).collect()
    }

    fn score(&mut self, pattern: &Pattern, app: &App, lower_query: &str) -> Option<f32> {
        let mut hit = |s: &str| pattern.score(Utf32Str::new(s, &mut self.buf), &mut self.matcher);

        let name = hit(&app.name).map(|s| {
            let prefix = app.name.to_lowercase().starts_with(lower_query);
            s as f32 + if prefix { PREFIX_BONUS } else { 0.0 }
        });
        let program = app.argv.first().map(|p| p.rsplit('/').next().unwrap_or(p));
        let secondary = app
            .generic_name
            .iter()
            .map(String::as_str)
            .chain(app.keywords.iter().map(String::as_str))
            .chain(program)
            .filter_map(&mut hit)
            .max()
            .map(|s| s as f32 * SECONDARY_WEIGHT);

        match (name, secondary) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, argv0: &str, keywords: &[&str]) -> App {
        App {
            id: id.into(),
            name: name.into(),
            generic_name: None,
            comment: None,
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            argv: vec![argv0.into()],
            terminal: false,
            cwd: None,
            icon: None,
            icon_path: None,
        }
    }

    fn names(apps: &[App], ix: &[usize]) -> Vec<String> {
        ix.iter().map(|&i| apps[i].name.clone()).collect()
    }

    fn apps() -> Vec<App> {
        vec![
            app("files", "Files", "nautilus", &["folder"]),
            app("firefox", "Firefox", "firefox", &["web", "browser"]),
            app("ghostty", "Ghostty", "ghostty", &["terminal", "shell"]),
            app("profiler", "System Profiler", "sysprof", &[]),
        ]
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(Ranker::new().rank(&apps(), &History::default(), "  ", 0).is_empty());
    }

    #[test]
    fn prefix_beats_scattered_match() {
        let apps = apps();
        let r = Ranker::new().rank(&apps, &History::default(), "fi", 0);
        assert_eq!(names(&apps, &r)[..2], ["Files", "Firefox"]);
        assert!(!names(&apps, &r).contains(&"Ghostty".to_string()));
    }

    #[test]
    fn history_reorders_close_matches() {
        let apps = apps();
        let mut h = History::default();
        for _ in 0..5 {
            h.record("firefox", 100);
        }
        let r = Ranker::new().rank(&apps, &h, "fi", 100);
        assert_eq!(names(&apps, &r)[0], "Firefox");
    }

    #[test]
    fn matches_keywords_and_program() {
        let apps = apps();
        let mut ranker = Ranker::new();
        let h = History::default();
        assert_eq!(names(&apps, &ranker.rank(&apps, &h, "terminal", 0)), ["Ghostty"]);
        assert_eq!(names(&apps, &ranker.rank(&apps, &h, "nautilus", 0)), ["Files"]);
    }
}
