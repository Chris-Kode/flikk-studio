//! The loading spinner: 10, 12 or 16 px, in the surrounding text colour.

use super::icon::IconName;
use gpui_kit::*;
use std::time::Duration;

#[derive(IntoElement)]
pub struct Spinner {
    size: Pixels,
    color: Option<Hsla>,
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            size: px(12.),
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

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        svg()
            .path(IconName::LoaderCircle.path())
            .flex_none()
            .size(self.size)
            .text_color(self.color.unwrap_or_else(|| window.text_style().color))
            .with_animation(
                "spinner",
                Animation::new(Duration::from_millis(900)).repeat(),
                |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
            )
    }
}
