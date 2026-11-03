# Tokens

The values each pass reaches for. The source of truth is `crates/flikk/src/ui/kit/theme.rs`; if this file and the code disagree, the code wins and this file gets fixed. Add a token there rather than a literal in a panel.

## Spacing scale

2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48

Flikk is dense, so it uses the 2px steps below 12 that a web page would not.

- Inside a control: 4 to 10
- Between related items, like rows in a list or a label and its control: 2 to 6
- Between groups, like two inspector sections: 8 to 16
- Panel padding: 8 to 12; dialog body: 16 to 24
- Row heights: 22 compact (timeline, dense lists), 26 default (inspector, menus), 30 for primary lists like recent projects

## Fixed heights

| Token | Value | Use |
| --- | --- | --- |
| `TITLE_BAR` | 40 | Window title bar |
| `PANEL_HEADER` | 28 | Every panel header |
| `STATUS_BAR` | 24 | Status bar |

## Surfaces

| Token | Value | Use |
| --- | --- | --- |
| `SCOPE` | #09090A | Scope and curve-editor wells |
| `G` | #0B0B0C | Window and viewer surround |
| `LANE` | #0E0E10 | Timeline lanes under the clips |
| `INSET` | #0F0F11 | Dialog sidebar and footer |
| `P` | #111113 | Panels, title and status bar |
| `R1` | #18181B | Raised surfaces, secondary buttons, menus |
| `HV` | #1F1F23 | Hover |
| `PS` | #26262B | Pressed |
| `LN` | #232327 | Hairlines and panel gaps |
| `LN_HV` | #34343A | Hovered field border |

## Text

| Token | Value | Use |
| --- | --- | --- |
| `T1` | #EDEDEF | Primary text |
| `T2` | #9A9AA2 | Labels, secondary text, idle icons |
| `T3` | #5E5E66 | Meta, placeholders |
| `ON_AC` | #FFFFFF | Text on an accent or danger fill |

`T3` is for meta and placeholders only. Anything people must read is `T1` or `T2`.

## Accent and meaning

| Token | Use |
| --- | --- |
| `AC`, `AC_HV`, `AC_PS` | Focus, selection, the one primary action |
| `AC_WASH` | Focus rings, range fills, selected rows |
| `PH` | The playhead, and nothing else |
| `OK`, `WARN`, `ERR`, `ERR_TX` | Saved / offline, conflict / failed, removed |
| `V`, `A`, `C`, `TI` | Video, audio, captions, titles tracks |
| `MK` | Timeline markers and the marquee band |
| `CH_R`, `CH_G`, `CH_B` | Color channels in curves and scopes |

## Type

Interface: the system face. Figures, timecodes, sizes and counts: `t::mono()`.

| Use | Size | Weight |
| --- | --- | --- |
| Ruler ticks, tiny meta on clips | 9 to 10 | `NORMAL` |
| Labels, rows, buttons, menus | 11 | `NORMAL` or `MEDIUM` |
| Panel headers, list titles | 12 | `MEDIUM` |
| Dialog body, emphasis | 13 | `NORMAL` |
| Dialog titles | 14 to 16 | `MEDIUM`, `SEMIBOLD` at most |
| Page titles, like the welcome page | 18 to 30 | `MEDIUM` |

No `BOLD` or heavier anywhere. Letter spacing stays at the default.

## Radius

| Token | Value | Use |
| --- | --- | --- |
| `RADIUS_SM` | 4 | Clips, fields, keycaps |
| `RADIUS` | 5 | Buttons |
| `RADIUS_SEG` | 7 | Segmented controls |
| `RADIUS_LG` | 8 | Menus, popovers, dialogs |

Nested radius: inner = outer − padding, never below 2.

## Icons

Kit `Icon` with a lucide `IconName`, 14px, `T2` idle, `T1` on hover, `AC` when on. 6 to 8 between icon and label.

## Depth and focus

- Depth is surface steps plus an `LN` hairline.
- `floating_shadow()` for menus, popovers and dialogs only, with `SCRIM` behind a modal.
- `focus_ring()` on controls, `field_ring()` on fields.

## Motion

| Use | Duration |
| --- | --- |
| Hover, press, toggles | instant, or up to 120ms |
| Menus, popovers, tooltips | 120 to 180ms fade |
| Dialogs, sheets | 180 to 240ms |

One ease-out curve. Loops only for live work: `Spinner`, `Progress`, playback, recording.

---

Adapted from unslop by Damir Sokolovsky ([@heydamir](https://x.com/heydamir)), https://github.com/heydamir/unslop, CC BY 4.0. Changed for Flikk's design tokens. Keep this credit in copies and adaptations.
