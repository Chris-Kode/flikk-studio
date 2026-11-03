//! Flikk, koo-editor's design system, on GPUI Base: the foundation supplies
//! behaviour (focus, keyboard, popups, text editing), these components
//! supply every pixel of presentation. Panels build from these and paint
//! from [`theme`]; they never style a raw control themselves.

pub mod badge;
pub mod button;
pub mod dialog;
pub mod empty;
pub mod icon;
pub mod input;
pub mod kbd;
pub mod layout;
pub mod menu;
pub mod number_field;
pub mod panel;
pub mod popover;
pub mod progress;
pub mod segmented;
pub mod select;
pub mod slider;
pub mod spinner;
pub mod tabs;
pub mod theme;
pub mod toast;
pub mod toggle;
pub mod tooltip;

pub use button::{Button, Size, Variant};
pub use gpui_kit::base::{h_flex, v_flex};
pub use icon::{Icon, IconName};
pub use kbd::Kbd;
pub use spinner::Spinner;
pub use tooltip::Tooltip;

use gpui_kit::*;

/// Install the palette into the foundation layer. Call once after
/// `gpui_kit::init`.
pub fn init(cx: &mut App) {
    theme::install(cx);
}
