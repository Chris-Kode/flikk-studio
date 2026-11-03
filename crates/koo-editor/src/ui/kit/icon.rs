//! Lucide icons at Flikk's 14 px, coloured from the surrounding text unless
//! told otherwise: `--t2` idle, `--t1` on hover, `--ac` when toggled on.

use gpui_kit::*;

pub use gpui_kit::assets::IconName;

#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Pixels,
    color: Option<Hsla>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: px(14.),
            color: None,
        }
    }

    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = size.into();
        self
    }

    pub fn color(mut self, color: impl Into<Hsla>) -> Self {
        self.color = Some(color.into());
        self
    }
}

impl From<IconName> for Icon {
    fn from(name: IconName) -> Self {
        Self::new(name)
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        svg()
            .path(self.name.path())
            .flex_none()
            .size(self.size)
            .text_color(self.color.unwrap_or_else(|| window.text_style().color))
    }
}
