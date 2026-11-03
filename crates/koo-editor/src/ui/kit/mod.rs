//! Flikk, koo-editor's design system, on GPUI Base: the foundation supplies
//! behaviour (focus, keyboard, popups, text editing), these components
//! supply every pixel of presentation. Panels build from these and paint
//! from [`theme`]; they never style a raw control themselves.

pub mod badge;
pub mod button;
pub mod color;
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
pub mod toggle;
pub mod tooltip;

pub use button::{Button, Size, Variant};
pub use color::ColorField;
pub use dialog::{Dialog, DialogExt};
pub use gpui_kit::base::{h_flex, v_flex};
pub use icon::{Icon, IconName};
pub use kbd::Kbd;
pub use menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
pub use popover::{Popover, PopoverPanel};
pub use select::{Select, SelectEvent, SelectItem, SelectState};
pub use spinner::Spinner;
pub use tooltip::Tooltip;

pub use badge::{Badge, Indicator, Tone};
pub use empty::EmptyState;
pub use layout::{ScrollArea, ScrollableElement, h_resizable, resizable_panel, v_resizable};
pub use panel::{Panel, PanelHeader, SectionHeader, SectionLabel};
pub use progress::Progress;
pub use segmented::Segmented;
pub use tabs::{PanelTabs, Tab, Tabs};
pub use toggle::{Checkbox, Radio, Switch};

pub use input::{Input, Textarea};
pub use number_field::{
    Keyframe, KeyframeNav, NumberField, NumberFieldEvent, NumberFieldState, ScrubLabel,
};
pub use slider::Slider;

use gpui_kit::*;

/// Install the palette into the foundation layer. Call once after
/// `gpui_kit::init`.
pub fn init(cx: &mut App) {
    theme::install(cx);
    dialog::init(cx);
    menu::init(cx);
    select::init(cx);
}
