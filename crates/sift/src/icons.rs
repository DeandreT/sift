//! Lucide icon font integration. All UI glyphs come from Lucide so the set is
//! consistent and nothing renders as a missing-glyph box.

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

pub use lucide_icons::Icon;
use lucide_icons::LUCIDE_FONT_BYTES;

/// Install the Lucide font as a fallback on both the proportional and
/// monospace families, so icon glyphs render inline within normal text.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "lucide".to_owned(),
        Arc::new(FontData::from_static(LUCIDE_FONT_BYTES)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let keys = fonts.families.entry(family).or_default();
        // After the primary font so text still uses the default face, but
        // ahead of any other fallbacks.
        let at = keys.len().min(1);
        keys.insert(at, "lucide".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// The glyph for a Lucide icon, ready to embed in button or label text.
#[must_use]
pub fn icon(i: Icon) -> String {
    char::from(i).to_string()
}
