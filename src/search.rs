//! Ranking: nucleo fuzzy matching boosted by launch history.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::history::History;

/// Matches on anything but the name count for this fraction of their score.
const SECONDARY_WEIGHT: f32 = 0.6;
/// Points per unit of `ln(1 + frecency)`.
const HISTORY_WEIGHT: f32 = 15.0;
/// Extra points when the name starts with the query.
const PREFIX_BONUS: f32 = 20.0;

/// What the ranker sees of an item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    pub name: String,
    /// Generic name, keywords, program name and the like.
    pub secondary: Vec<String>,
    /// History key; `None` for items whose use is not recorded.
    pub key: Option<String>,
}

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

    /// Indices into `candidates`, best match first; ties keep their order.
    /// An empty query matches nothing.
    pub fn rank(
        &mut self,
        candidates: &[Candidate],
        history: &History,
        query: &str,
        now: u64,
    ) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let lower = query.to_lowercase();
        let mut scored: Vec<(usize, f32)> = candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let score = self.score(&pattern, c, &lower)?;
                let frecency = c.key.as_deref().map_or(0.0, |k| history.frecency(k, now));
                Some((i, score + HISTORY_WEIGHT * frecency.ln_1p()))
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.into_iter().map(|(i, _)| i).collect()
    }

    fn score(&mut self, pattern: &Pattern, c: &Candidate, lower_query: &str) -> Option<f32> {
        let mut hit = |s: &str| pattern.score(Utf32Str::new(s, &mut self.buf), &mut self.matcher);

        let name = hit(&c.name).map(|s| {
            let prefix = c.name.to_lowercase().starts_with(lower_query);
            s as f32 + if prefix { PREFIX_BONUS } else { 0.0 }
        });
        let secondary = c
            .secondary
            .iter()
            .filter_map(|s| hit(s))
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

    fn c(key: &str, name: &str, secondary: &[&str]) -> Candidate {
        Candidate {
            name: name.into(),
            secondary: secondary.iter().map(|s| s.to_string()).collect(),
            key: Some(key.into()),
        }
    }

    fn names(cs: &[Candidate], ix: &[usize]) -> Vec<String> {
        ix.iter().map(|&i| cs[i].name.clone()).collect()
    }

    fn candidates() -> Vec<Candidate> {
        vec![
            c("files", "Files", &["folder", "nautilus"]),
            c("firefox", "Firefox", &["web", "browser", "firefox"]),
            c("ghostty", "Ghostty", &["terminal", "shell", "ghostty"]),
            c("profiler", "System Profiler", &["sysprof"]),
        ]
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(
            Ranker::new()
                .rank(&candidates(), &History::default(), "  ", 0)
                .is_empty()
        );
    }

    #[test]
    fn prefix_beats_scattered_match() {
        let cs = candidates();
        let r = Ranker::new().rank(&cs, &History::default(), "fi", 0);
        assert_eq!(names(&cs, &r)[..2], ["Files", "Firefox"]);
        assert!(!names(&cs, &r).contains(&"Ghostty".to_string()));
    }

    #[test]
    fn history_reorders_close_matches() {
        let cs = candidates();
        let mut h = History::default();
        for _ in 0..5 {
            h.record("firefox", 100);
        }
        let r = Ranker::new().rank(&cs, &h, "fi", 100);
        assert_eq!(names(&cs, &r)[0], "Firefox");
    }

    #[test]
    fn matches_secondary() {
        let cs = candidates();
        let mut ranker = Ranker::new();
        let h = History::default();
        assert_eq!(
            names(&cs, &ranker.rank(&cs, &h, "terminal", 0)),
            ["Ghostty"]
        );
        assert_eq!(names(&cs, &ranker.rank(&cs, &h, "nautilus", 0)), ["Files"]);
    }
}
