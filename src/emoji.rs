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

/// Colour emoji fonts with PNG glyphs (CBDT or sbix), in order of
/// preference.
const COLOR_FONTS: &[&str] = &[
    "Noto Color Emoji",
    "JoyPixels",
    "Apple Color Emoji",
    "EmojiOne Color",
];
/// Pixels per em to ask for; bitmap fonts return their nearest strike.
const PIXELS_PER_EM: u16 = 160;

/// A colour emoji font, for drawing emoji as images.
///
/// GPUI cannot draw them as text: it never uses a font without a Latin "m"
/// as the primary font, and its fallback reaches text fonts with
/// black-and-white emoji, such as DejaVu Sans, first.
pub struct EmojiFont {
    data: Vec<u8>,
    index: u32,
    pub family: String,
}

impl EmojiFont {
    /// The configured family, else the first installed font in
    /// [`COLOR_FONTS`]. Reads every system font's metadata, so it is slow.
    pub fn load(configured: Option<&str>) -> Option<Self> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let families: Vec<&str> = match configured {
            Some(family) => vec![family],
            None => COLOR_FONTS.to_vec(),
        };
        families.into_iter().find_map(|family| {
            let id = db.query(&fontdb::Query {
                families: &[fontdb::Family::Name(family)],
                ..Default::default()
            })?;
            let (source, index) = db.face_source(id)?;
            let data = match source {
                fontdb::Source::File(path) => std::fs::read(path).ok()?,
                fontdb::Source::Binary(data) | fontdb::Source::SharedFile(_, data) => {
                    (*data).as_ref().to_vec()
                }
            };
            Some(Self {
                data,
                index,
                family: family.to_owned(),
            })
        })
    }

    /// The PNG this font draws for `glyph`. `None` when the font has no single
    /// image for it, such as a newer emoji, or a sequence it does not join.
    pub fn png(&self, glyph: &str) -> Option<Vec<u8>> {
        let face = rustybuzz::Face::from_slice(&self.data, self.index)?;
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(glyph);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        // Variation selectors shape to invisible glyphs without an image.
        let mut images = shaped.glyph_infos().iter().filter_map(|info| {
            let id = rustybuzz::ttf_parser::GlyphId(u16::try_from(info.glyph_id).ok()?);
            face.glyph_raster_image(id, PIXELS_PER_EM)
                .filter(|image| image.format == rustybuzz::ttf_parser::RasterImageFormat::PNG)
        });
        let image = images.next()?;
        images.next().is_none().then(|| image.data.to_vec())
    }

    /// Whether this font draws `glyph` as one image.
    pub fn draws(&self, glyph: &str) -> bool {
        self.png(glyph).is_some()
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
        assert_eq!(
            all(SkinTone::Dark)
                .iter()
                .find(|e| e.name == "rocket")
                .unwrap()
                .glyph,
            "🚀"
        );
        assert!(
            !plain
                .iter()
                .any(|e| e.name.contains("skin tone") && e.name.split(' ').count() <= 3)
        );
    }

    /// Needs a colour emoji font installed; skipped without one.
    #[test]
    fn pngs_from_the_installed_font() {
        let Some(font) = EmojiFont::load(None) else {
            eprintln!("no colour emoji font installed; skipping");
            return;
        };
        let png = |g: &str| font.png(g);
        let signature = b"\x89PNG\r\n\x1a\n";
        assert!(png("😀").unwrap().starts_with(signature), "{}", font.family);
        // Skin tones and joined sequences are one image each.
        assert!(png("👋🏿").is_some());
        assert!(png("👩‍💻").is_some());
        assert!(png("❤️").is_some());
        assert!(png("🏳️‍🌈").is_some());
        assert_ne!(png("👋🏿"), png("👋"));
        assert!(png("a").is_none());
        assert!(font.draws("🚀"));
        // Newer than any Noto Color Emoji release here: no glyph at all.
        assert!(!font.draws("\u{1FAEB}"));
    }

    #[test]
    fn candidates_search_shortcodes() {
        let tada = all(SkinTone::Default)
            .into_iter()
            .find(|e| e.glyph == "🎉")
            .unwrap();
        let c = tada.candidate();
        assert_eq!(c.name, "party popper");
        assert!(c.secondary.contains(&"tada".to_string()));
        assert_eq!(c.key.as_deref(), Some("emoji:party popper"));
    }
}
