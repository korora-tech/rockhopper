//! Emoji for the picker, from Unicode's emoji list with GitHub shortcodes.

use serde::Deserialize;

use crate::search::Candidate;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SkinTone {
    #[default]
    Default,
    Light,
    MediumLight,
    Medium,
    MediumDark,
    Dark,
}

impl From<SkinTone> for emojis::SkinTone {
    fn from(tone: SkinTone) -> Self {
        match tone {
            SkinTone::Default => Self::Default,
            SkinTone::Light => Self::Light,
            SkinTone::MediumLight => Self::MediumLight,
            SkinTone::Medium => Self::Medium,
            SkinTone::MediumDark => Self::MediumDark,
            SkinTone::Dark => Self::Dark,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Emoji {
    /// What is copied, in the configured skin tone where it has one.
    pub glyph: &'static str,
    pub name: &'static str,
    pub shortcode: Option<&'static str>,
}

impl Emoji {
    /// History is kept per emoji, whatever the skin tone.
    pub fn key(&self) -> String {
        format!("emoji:{}", self.name)
    }

    pub fn candidate(&self) -> Candidate {
        Candidate {
            name: self.name.to_owned(),
            secondary: emojis::get(self.glyph)
                .map(|e| e.shortcodes().map(|s| s.replace('_', " ")).collect())
                .unwrap_or_default(),
            key: Some(self.key()),
        }
    }
}

/// Every emoji in Unicode order. The list has no skin tone swatches or other
/// components that are not meant to be used alone.
pub fn all(tone: SkinTone) -> Vec<Emoji> {
    emojis::iter()
        .map(|e| {
            let toned = e.with_skin_tone(tone.into()).unwrap_or(e);
            Emoji {
                glyph: toned.as_str(),
                name: e.name(),
                shortcode: e.shortcode(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_skin_tones() {
        let plain = all(SkinTone::Default);
        assert!(plain.len() > 1800, "{}", plain.len());
        let wave = |list: &[Emoji]| list.iter().find(|e| e.name == "waving hand").unwrap().glyph;
        assert_eq!(wave(&plain), "👋");
        assert_eq!(wave(&all(SkinTone::Dark)), "👋🏿");
        // Emoji without skin tones are unchanged.
        let rocket = plain.iter().find(|e| e.name == "rocket").unwrap();
        assert_eq!(rocket.shortcode, Some("rocket"));
        assert_eq!(all(SkinTone::Dark).iter().find(|e| e.name == "rocket").unwrap().glyph, "🚀");
        assert!(!plain.iter().any(|e| e.name.contains("skin tone") && e.name.split(' ').count() <= 3));
    }

    #[test]
    fn candidates_search_shortcodes() {
        let tada = all(SkinTone::Default).into_iter().find(|e| e.glyph == "🎉").unwrap();
        let c = tada.candidate();
        assert_eq!(c.name, "party popper");
        assert!(c.secondary.contains(&"tada".to_string()));
        assert_eq!(c.key.as_deref(), Some("emoji:party popper"));
    }
}
