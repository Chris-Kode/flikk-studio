# DESIGN.md

How Flikk's interface looks and reads. Follow it for every change to the UI
in `crates/flikk/src/ui/`. The values live in `crates/flikk/src/ui/kit/theme.rs`;
if this file and the code disagree, the code wins and this file gets fixed.

## Principles

- **The footage is the bright element.** The viewer, clip thumbnails,
  waveforms, scopes and colour wheels carry the colour. The chrome around them
  is graphite, hairlines and one blue accent, so the picture is always the
  brightest thing on screen.
- **Quiet chrome.** Panels, headers, buttons and text use the graphite
  surfaces, three text roles and the one accent. Nothing glows.
- **Colour means something.** Track tints, the playhead red, channel colours
  and status colours each mark one thing, everywhere, and are never
  decoration.
- **Dense and exact.** A pro editor is looked at for hours. Rows are compact,
  numbers are mono and right-aligned, and every control lines up with its
  neighbours.
- **Let the layout do the explaining.** Grouping, placement, icons and state
  make a control clear on their own. Text is the last resort.
- **Only Flikk.** A screen should not fit another app if you swapped the
  name. Character comes from the copy, the content shown and the signature
  details, never from a second brand colour, a gradient or a glow.

## Building blocks

- Every colour, radius and fixed bar height comes from `crate::ui::kit::theme`
  (imported as `t`). Panel code never writes a raw `hex(…)`, `rgb(…)`,
  `rgba(…)` or `hsla(…)`. If a value is missing, add a named token to
  `theme.rs` with a one-line doc comment saying what it is for.
- Panels are built from the kit components in `crate::ui::kit`: `Button`,
  `Icon`, `Panel`, `PanelHeader`, `SectionHeader`, `Select`, `NumberField`,
  `Badge`, `Kbd`, `EmptyState`, `Tooltip` and the rest. Never style a raw
  `div()` as a control. If a component lacks a variant, extend the component
  rather than restyling it at the call site.

## Copy

- No secondary labels, subtitles, helper text or descriptions that restate
  what the layout shows. If a control needs explaining, its label is wrong.
- Labels are short, one or two words where possible: `Search projects`, not
  `Search anything...`; `Export`, not `Export your amazing video`.
- Sentence case everywhere. Labels, buttons, headers and tooltips end without
  a period; only multi-sentence paragraphs keep periods.
- Show state through the control itself (disabled, selected, a spinner)
  rather than in words. Prefer a tooltip to a visible explanation.
- Titles name the thing, in Flikk's own nouns.
- No exclamation marks, greetings or emoji. No hype words such as
  supercharge, seamless, unlock, magic, effortless or AI-powered, and no ✨ on
  agent features: say what the feature does.
- Errors say what happened and what to do, plainly, without blame or apology.
  Status and errors go in the existing status and error lines, briefly.
- Mock and preview data looks real: `Interview_A_cam2.mov`, `00:03:12:08`,
  `1080p · 25 fps`, never `[X]`, `video1.mp4` or `123,456`.
- No `PRO`, `New` or `Beta` badges unless the product needs one, and then a
  quiet `Badge`.

## Spacing

- Spacing sits on a 2px grid with 4 as the main step:
  2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48.
  - inside a control: 4 to 10;
  - between related items (rows in a list, a label and its control): 2 to 6;
  - between groups (two inspector sections): 8 to 16;
  - panel padding: 8 to 12; dialog body: 16 to 24;
  - row heights: 22 compact (timeline, dense lists), 26 default (inspector,
    menus), 30 for primary lists such as recent projects.
- Odd values like 3, 5, 7 or 13 are allowed only to centre something exactly,
  with a short comment.
- Equal things have equal sizes: one row height and one gap per list, one
  padding per set of sibling sections.
- Space shows grouping: gaps inside a group are smaller than gaps between
  groups.
- Things line up: icons in one column, labels on one line, numbers right
  aligned in one column.
- Bars use `t::TITLE_BAR` (40), `t::PANEL_HEADER` (28) and `t::STATUS_BAR`
  (24), never their own heights.

## Colour

| Token | Use |
| --- | --- |
| `SCOPE` | Scope and curve-editor wells |
| `G` | Window and viewer surround |
| `LANE` | Timeline lanes under the clips |
| `INSET` | Dialog sidebar and footer |
| `P` | Panels, title and status bar |
| `R1` | Raised surfaces, secondary buttons, menus |
| `HV` / `PS` | Hover / pressed |
| `LN` / `LN_HV` | Hairlines and panel gaps / hovered field border |
| `T1` / `T2` / `T3` | Primary text / labels, secondary text, idle icons / meta, placeholders |
| `ON_AC` | Text on an accent or danger fill |
| `AC`, `AC_HV`, `AC_PS` | Focus, selection, the one primary action |
| `AC_WASH` | Focus rings, range fills, selected rows |
| `PH` | The playhead, and nothing else |
| `OK`, `WARN`, `ERR`, `ERR_TX` | Saved or online, conflict or failed, removed |
| `V`, `A`, `C`, `TI` | Video, audio, caption and title tracks |
| `MK` | Timeline markers and the marquee band |
| `CH_R`, `CH_G`, `CH_B` | Colour channels in curves and scopes |

- Depth comes from surface steps and the `LN` hairline, not shadows. Only
  floating surfaces (menus, popovers, dialogs) use `t::floating_shadow()`,
  with `SCRIM` behind a modal.
- Three text roles, `T1`, `T2` and `T3`, not a range of near-identical greys.
  `T3` is for meta and placeholders only; anything people must read is `T1`
  or `T2`.
- One accent, `AC`, used once or twice per view.
- Meaning colours carry their meaning only. Never borrow one as decoration.
- No hue gradients, coloured glows, coloured shadows, colour washes behind
  content or bright borders. A gradient is allowed only as a small tone shift
  inside one neutral.
- Focus is `t::focus_ring()` on controls and `t::field_ring()` on fields.

## Icons

- Lucide icons through the kit `Icon` (`IconName`). No emoji, no other icon
  set, no inline SVG paths for something lucide has.
- 14px, larger only for an empty-state glyph.
- Icons take their text role's colour: `T2` idle, `T1` on hover, `AC` when
  toggled on. Their own colour only when it carries meaning (a track tint, a
  status).
- The same icon box width in every row and 6 to 8 between icon and label, so
  labels align.
- No icon that adds nothing, such as one before every section label. A
  category that needs colour gets a small `Indicator` dot.

## Type

| Use | Size | Weight |
| --- | --- | --- |
| Ruler ticks, tiny meta on clips | 9 to 10 | `NORMAL` |
| Labels, rows, buttons, menus | 11 | `NORMAL` or `MEDIUM` |
| Panel headers, list titles | 12 | `MEDIUM` |
| Dialog body, emphasis | 13 | `NORMAL` |
| Dialog titles | 14 to 16 | `MEDIUM`, `SEMIBOLD` at most |
| Page titles, such as the welcome page | 18 to 30 | `MEDIUM` |

- The interface uses the system face. Figures, timecodes, frame counts, sizes
  and counts use `t::mono()`. No other families.
- No `BOLD` or heavier. Hierarchy comes from size and colour, not weight.
- No uppercase tracked labels (`AUDIO TRACKS` is `Audio tracks`), no
  negative letter spacing on small text, no italic for emphasis.
- One number format per screen (`845 MB` next to `1.2 GB`, not `1.2GB`), in
  mono, right-aligned in columns.

## Layout

- Panels fill their resizable slot (`h_resizable`, `v_resizable`,
  `resizable_panel`). No fixed-width islands centred in a panel.
- Inspector rows put the label left and the control right, labels in one
  column of one width.
- Dialogs: title, body, then a footer on `INSET` with the primary action
  rightmost and at most one secondary next to it.
- One `Button::primary()` per view; everything else is default or `ghost()`.
  Destructive actions use `Button::destructive()` and are never the default.
- Long text aligns left. Centring is for short empty states only.
- A panel holds up at its smallest usable size and at a large size: nothing
  overlaps or clips, and text truncates with an ellipsis.
- Show the real material where a screen can: thumbnails, durations and
  timecodes rather than grey boxes and labels.

## Details

| Token | Value | Use |
| --- | --- | --- |
| `RADIUS_XS` | 3 | Keycaps, checkboxes, small handles inside a control |
| `RADIUS_SM` | 4 | Clips, fields |
| `RADIUS` | 5 | Buttons |
| `RADIUS_SEG` | 7 | Segmented controls |
| `RADIUS_LG` | 8 | Menus, popovers, dialogs |

- Nested radius: inner = outer − padding, never below 2. Nothing is a pill
  except a toggle track.
- Counts and meta are quiet mono `T3` text, not pills.
- Keyboard shortcuts are `Kbd` keycaps.
- Scopes, curves and meters sit on `SCOPE` wells with muted grid lines and
  mono labels. One series leads; channel colours only for channels.
- The active state is a surface step (`HV` or `R1`) with a hairline, or
  `AC_WASH` for a selected row.
- Everything clickable has hover, pressed and focus states inside the palette.
- Empty states use `EmptyState`: one icon, one short line, at most one action.
- Signature details belong where they mean something: mono timecodes,
  track-tint dots, keycaps for shortcuts, the playhead red only on the
  playhead.

## Motion

| Use | Duration |
| --- | --- |
| Hover, press, toggles | instant, or up to 120ms |
| Menus, popovers, tooltips | 120 to 180ms fade |
| Dialogs, sheets | 180 to 240ms |

- One ease-out curve. GPUI animations go through `with_animation`; keep them
  short and never animate layout sizes every frame.
- Loops only for live work: playback, recording, a `Spinner` while work
  runs, a `Progress` bar.
- No bouncy springs, hover scaling or hover movement: a hover is an instant
  colour or surface change.
- Playback, scrubbing and timeline drags never wait on an animation.

## Reviewing a screen

When polishing or redesigning a screen, keep what it does, its content and
the order of its sections. Fix in this order, across the whole screen, one
layer at a time: spacing, colour, icons, copy, type, layout, details, motion.
Copy comes before type and layout because a label's length decides its size
and place. Don't replace removed decoration with new decoration, and don't
change a screen that already follows this file just to make it different.

Then check:

- [ ] Every spacing value is on the scale; equal things have equal sizes
- [ ] Icons, labels and numbers each sit in one aligned column
- [ ] Every colour is a named token; no raw `hex`, `rgb` or `hsla` in panel code
- [ ] One accent, for focus, selection and the one primary action
- [ ] Meaning colours carry only their meaning; `PH` only on the playhead
- [ ] No gradients, glows, coloured shadows or colour washes
- [ ] No emoji; every icon is a kit `Icon` at 14px
- [ ] Weights `NORMAL` and `MEDIUM`, `SEMIBOLD` at most; no uppercase tracked labels, no italic
- [ ] Numbers and timecodes in `t::mono()`, one format, right-aligned
- [ ] No exclamation marks, greetings, hype words, trailing periods on short strings or explanatory subtitles
- [ ] Radii come from the tokens and nested radii add up
- [ ] The panel holds up at its narrowest and widest sizes
- [ ] Nothing loops that is not live; no hover scaling
- [ ] Would the screen fit another app if you swapped the name? If yes, push the copy, the content shown and the signature details

---

Parts of this file are adapted from unslop by Damir Sokolovsky
([@heydamir](https://x.com/heydamir)), https://github.com/heydamir/unslop,
licensed under CC BY 4.0, changed for Flikk's GPUI code and design tokens.
Keep this credit in copies and adaptations.
