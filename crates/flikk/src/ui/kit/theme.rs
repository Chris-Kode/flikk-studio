//! Flikk design tokens: surfaces, text, accent and status colours, track
//! tints, radii and control heights. Every kit component and panel paints
//! from these; nothing else picks a colour.

use gpui_kit::base::{SemanticThemeTokens, Theme, ThemeAppearance};
use gpui_kit::*;

/// `0xRRGGBB` as a colour, usable in constants.
pub const fn hex(v: u32) -> Rgba {
    hexa((v << 8) | 0xff)
}

/// `0xRRGGBBAA` as a colour, usable in constants.
const fn hexa(v: u32) -> Rgba {
    Rgba {
        r: ((v >> 24) & 0xff) as f32 / 255.,
        g: ((v >> 16) & 0xff) as f32 / 255.,
        b: ((v >> 8) & 0xff) as f32 / 255.,
        a: (v & 0xff) as f32 / 255.,
    }
}

/// `c` at `alpha` (0–1).
pub const fn alpha(c: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..c }
}

/// Window and viewer surround.
pub const G: Rgba = hex(0x0B0B0C);
/// Panels, title and status bar.
pub const P: Rgba = hex(0x111113);
/// Raised surfaces, secondary buttons, menus.
pub const R1: Rgba = hex(0x18181B);
/// Hover, active icon.
pub const HV: Rgba = hex(0x1F1F23);
/// Pressed.
pub const PS: Rgba = hex(0x26262B);
/// Hairlines and panel gaps.
pub const LN: Rgba = hex(0x232327);
/// Hovered field border.
pub const LN_HV: Rgba = hex(0x34343A);
/// Unchecked checkbox and radio border.
pub const LN_CTL: Rgba = hex(0x3A3A41);
/// Hovered unchecked checkbox and radio border.
pub const LN_CTL_HV: Rgba = hex(0x55555D);
/// Dashed drop-zone outline.
pub const LN_DASHED: Rgba = hex(0x2E2E34);
/// Switch track when off, and hovered.
pub const SW: Rgba = hex(0x2C2C31);
pub const SW_HV: Rgba = hex(0x38383E);
/// Tooltip surface and its border.
pub const TIP: Rgba = hex(0x202024);
pub const TIP_LN: Rgba = hex(0x2C2C31);
/// The scrim behind a modal dialog.
pub const SCRIM: Rgba = hexa(0x000000A6);

/// Primary text.
pub const T1: Rgba = hex(0xEDEDEF);
/// Labels, secondary text, idle icons.
pub const T2: Rgba = hex(0x9A9AA2);
/// Meta, placeholders.
pub const T3: Rgba = hex(0x5E5E66);
/// Text on an accent fill.
pub const ON_AC: Rgba = hex(0xFFFFFF);
/// Text on a danger fill; white on `ERR` does not reach 4.5:1.
pub const ON_ERR: Rgba = hex(0x2A0A0A);

/// Signal blue: focus, selection, primary actions.
pub const AC: Rgba = hex(0x3D8BFF);
pub const AC_HV: Rgba = hex(0x5A9CFF);
pub const AC_PS: Rgba = hex(0x2F76E0);
/// Accent wash: focus rings, range fills, selected rows.
pub const AC_WASH: Rgba = alpha(AC, 0.15);
/// The playhead, and nothing else.
pub const PH: Rgba = hex(0xFF4D4D);
/// Saved, cached, added.
pub const OK: Rgba = hex(0x3DDC97);
/// Offline, conflict, solo.
pub const WARN: Rgba = hex(0xF5C04A);
/// Failed, removed, clipping.
pub const ERR: Rgba = hex(0xFF6B6B);
/// Danger text on a danger wash.
pub const ERR_TX: Rgba = hex(0xFF8585);
/// Danger fill, hovered and pressed.
pub const ERR_HV: Rgba = hex(0xFF8080);
pub const ERR_PS: Rgba = hex(0xE85A5A);

/// Video track tint.
pub const V: Rgba = hex(0x5B8DEF);
/// Audio track tint.
pub const A: Rgba = hex(0x2EC4B6);
/// Captions track tint.
pub const C: Rgba = hex(0xB57CF0);
/// Titles and generators track tint.
pub const TI: Rgba = hex(0xD7B4FF);
/// Timeline lanes, under the clips.
pub const LANE: Rgba = hex(0x0E0E10);
/// Timeline markers and the marquee band.
pub const MK: Rgba = hex(0x7FB0FF);
/// Scope and curve-editor wells.
pub const SCOPE: Rgba = hex(0x09090A);
/// Red, green and blue channels: curves, parade, histogram.
pub const CH_R: Rgba = hex(0xFF8A80);
pub const CH_G: Rgba = hex(0x7FDC8F);
pub const CH_B: Rgba = hex(0x82A8FF);
/// Stands in for a picture behind caption lettering previews.
pub const BACKDROP: Rgba = hex(0x2F343D);
/// A dialog's sidebar and footer, a step below its body.
pub const INSET: Rgba = hex(0x0F0F11);

/// Keycaps, checkboxes and small handles inside a control.
pub const RADIUS_XS: Pixels = px(3.);
/// Clips and fields.
pub const RADIUS_SM: Pixels = px(4.);
/// Buttons.
pub const RADIUS: Pixels = px(5.);
/// Segmented controls.
pub const RADIUS_SEG: Pixels = px(7.);
/// Floating surfaces: menus, popovers, dialogs.
pub const RADIUS_LG: Pixels = px(8.);

/// Panel header height.
pub const PANEL_HEADER: Pixels = px(28.);
/// Title bar height.
pub const TITLE_BAR: Pixels = px(40.);
/// Status bar height.
pub const STATUS_BAR: Pixels = px(24.);

/// Monospace face for timecodes and numbers.
pub fn mono() -> SharedString {
    if cfg!(target_os = "macos") {
        "Menlo".into()
    } else if cfg!(target_os = "windows") {
        "Consolas".into()
    } else {
        "DejaVu Sans Mono".into()
    }
}

/// A shadow for floating surfaces.
pub fn floating_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., 0.67),
        offset: point(px(0.), px(12.)),
        blur_radius: px(32.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// Two-pixel accent focus ring around a control, one pixel off its edge.
/// Shadows paint in order, so the gap goes last, over the ring.
pub fn focus_ring() -> Vec<BoxShadow> {
    let ring = |color: Rgba, spread: f32| BoxShadow {
        color: color.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(spread),
        inset: false,
    };
    vec![ring(alpha(AC, 0.6), 3.), ring(G, 1.)]
}

/// Three-pixel accent wash around a focused field.
pub fn field_ring() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: AC_WASH.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(3.),
        inset: false,
    }]
}

/// Point the foundation controls (text selection, scrollbars, resize
/// handles) at the Flikk palette.
pub(super) fn install(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    theme.appearance = ThemeAppearance::Dark;
    let mut tokens = SemanticThemeTokens::default();
    let c = &mut tokens.colors;
    c.background = G.into();
    c.foreground = T1.into();
    c.surface = P.into();
    c.surface_foreground = T1.into();
    c.primary = AC.into();
    c.primary_foreground = ON_AC.into();
    c.secondary = R1.into();
    c.secondary_foreground = T1.into();
    c.muted = R1.into();
    c.muted_foreground = T2.into();
    c.accent = HV.into();
    c.accent_foreground = T1.into();
    c.destructive = ERR.into();
    c.destructive_foreground = ON_ERR.into();
    c.border = LN.into();
    c.input = LN.into();
    c.ring = AC.into();
    c.selection = alpha(AC, 0.3).into();
    tokens.radius.sm = RADIUS_SM;
    tokens.radius.md = RADIUS;
    tokens.radius.lg = RADIUS_LG;
    theme.tokens = tokens;
    theme.resizable.handle = Some(LN.into());
    theme.resizable.active_handle = Some(AC.into());
}
