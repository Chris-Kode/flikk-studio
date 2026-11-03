//! Icons for FX kinds.

use crate::ui::kit::{Icon, IconName};
use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxGlyph {
    /// Video effect: a wand.
    Effect,
    /// Two pictures blending.
    Transition,
    /// Shader generator: shapes.
    Generator,
    /// HTML title or motion graphic: a "T".
    Title,
    /// Audio effect: waveform lines.
    Audio,
    /// A project's own program: `</>`.
    Code,
}

impl FxGlyph {
    fn icon(self) -> IconName {
        match self {
            Self::Effect => IconName::WandSparkles,
            Self::Transition => IconName::Blend,
            Self::Generator => IconName::Shapes,
            Self::Title => IconName::Type,
            Self::Audio => IconName::AudioLines,
            Self::Code => IconName::CodeXml,
        }
    }
}

/// `glyph`, `size` square, in `color`.
pub fn fx_glyph(glyph: FxGlyph, size: Pixels, color: impl Into<Hsla>) -> Icon {
    Icon::new(glyph.icon()).size(size).color(color)
}
