---
name: unslop
description: Turns vibecoded, AI-looking UI in koo-editor (Flikk, GPUI) into designed interface with its own character. Audits the screen, sets a direction from the product, then fixes spacing, color, icons, copy, type, layout, details and motion in that order, using the Flikk tokens in crates/koo-editor/src/ui/kit/theme.rs. Use when the user asks to unslop, polish, clean up or redesign a panel, dialog or page so it stops looking AI-generated, or when a screen shows the usual tells like raw hex colors, glows, emoji labels, uppercase tracked labels, stray one-off sizes, pulsing elements, explanatory subtitles or shouty copy.
license: CC-BY-4.0
metadata:
  author: Damir Sokolovsky
  x: https://x.com/heydamir
  source: https://github.com/heydamir/unslop
  adapted-for: koo-editor (GPUI, Flikk design system)
---

# Unslop

Adapted from unslop by Damir Sokolovsky ([@heydamir](https://x.com/heydamir)), https://github.com/heydamir/unslop, licensed under CC BY 4.0. This version is changed for koo-editor: it works on GPUI Rust code and the Flikk tokens instead of web pages, and drops the landing-page, image-generation and story-video parts of the original. Keep this credit in any copy or adaptation.

Vibecoded UI fails in the same predictable ways. Every surface glows, every label has an emoji, spacing is whatever the model typed that day, and the copy shouts. Unslopping fixes the craft and keeps the product. The features, content and order of sections stay. What changes is that every size, color and word becomes deliberate.

Removing slop is only half the job. A panel that is clean but could belong to any app is the next template. Every run ends with a screen that only Flikk could have.

The difference between slop and craft is where the richness goes. Slop spreads color and effects over the chrome. Craft keeps the chrome quiet and puts the richness into the content. In a video editor the content is the footage: the viewer, clip thumbnails, waveforms, scopes and the color wheels. The chrome around them is graphite, hairlines and one blue accent, so the picture is always the brightest thing on screen.

Work like a designer doing a redline pass, one layer at a time, across the whole screen.

## Ground rules for this repo

- Every color, radius and fixed bar height comes from `crate::ui::kit::theme` (imported as `t`). A panel never writes a raw `hex(…)`, `rgb(…)` or `hsla(…)`. If a needed value does not exist, add a named token to `theme.rs` with a one-line doc comment saying what it is for, in the same style as the others.
- Panels build from the kit components in `crate::ui::kit` (`Button`, `Icon`, `Panel`, `PanelHeader`, `SectionHeader`, `Select`, `NumberField`, `Badge`, `Kbd`, `EmptyState`, `Tooltip` and the rest). They never style a raw control themselves. If a kit component is missing a variant, extend the component rather than restyling it at the call site.
- Let the layout do the explaining. No secondary labels or descriptions that restate what the layout already shows (CLAUDE.md).
- Do not write tests, even when an issue asks for them. Say so in the report.
- Do not rename the product. The app is Flikk.
- Manual runs use `KOOEDITOR_HOME=/tmp/koo-editor-dev cargo run -p koo-editor`. Finish with `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings`.

## What good looks like

- **The footage is the bright element.** The viewer, thumbnails, waveforms and scopes carry the color. Everything around them is quiet so the picture reads instantly.
- **Quiet chrome.** Panels, headers, buttons and text use the graphite surfaces, three text roles and the one accent. Nothing glows.
- **Color means something.** The track tints (`V`, `A`, `C`, `TI`), the playhead red (`PH`), the channel colors and the status colors each mark one thing, everywhere, and are never used as decoration.
- **Dense and exact.** A pro editor is looked at for hours. Rows are compact, numbers are mono and right-aligned, every control lines up with its neighbours.
- **Real material.** Previews and empty states show plausible footage names, durations and timecodes, never `Lorem` or `[X]`.

## Run it

1. **Look at the screen.** Find the files under `crates/koo-editor/src/ui/` that render it, and the kit components they use. If you can run the app and see it, do it, so you judge what people see and not only the code. On Linux, `scripts/screenshot.sh out.png` builds, launches and captures the window. On macOS, run the app and ask the user for a screenshot, or take one with `screencapture` if the user allows it. Without either, judge from the code and say so in the report.
2. **Audit.** Go through [references/signs.md](references/signs.md) and list every sign you find, one line each with `file:line`.
3. **Decide how much to change.** Read the audit honestly before planning anything.
   - **Heavy slop**, many signs across color, type, icons and copy. Run the full process below.
   - **Light slop**, a screen that is mostly Flikk with a few signs left. Polish mode. Fix only what the audit found and keep the layout. Bigger ideas go into the report as suggestions, not changes.
   - **No slop.** Say so plainly and suggest two or three small refinements at most, with exact values. Apply them only if the user agrees. Never change a good screen just to make it different.
4. **Set the direction and stop.** Write the direction lines below. Show the audit and the direction to the user and wait for their answer before editing, since changing the direction later means redoing everything. Ask through the question tool if you have one, otherwise in plain text, and stop. Skip the wait only when the user already said to just do it.
5. **Fix in eight passes, in order.** Finish a pass across the whole screen before starting the next. When nearly every line changes, plan the passes in order and then write the new code in one go, as long as every pass is applied. Spacing comes first because everything else reads differently once the rhythm is right. Copy comes before type and layout because the length of a label decides its size and where it sits. Motion comes last because it should fit a screen that already works without it.
6. **Check.** Build, run the checklist at the end of this file, look at the screen again and compare with the audit list. Anything still on the list gets fixed. Run `cargo fmt` and clippy.
7. **Report** the direction, a few lines per pass about what changed, any new tokens added to `theme.rs`, and anything you could not verify visually.

Keep a run quick. Launch the app to check at most three times.

Exact values for every pass live in [references/tokens.md](references/tokens.md).

## Direction

Flikk already has a direction: a dark, dense, graphite editor with signal blue as the one accent, where the footage carries the color. For most screens, write the lines below to confirm how this screen fits it. Only a new kind of surface, like an onboarding flow or an export summary, may push further.

- **Screen.** What this screen does and for whom, in one plain sentence.
- **Character.** Two or three concrete words for this screen inside Flikk, like `fast, exact, out of the way` or `calm, reassuring` for a recovery dialog. Words like clean, modern, sleek, minimal and premium are not allowed, because they describe every template.
- **Moments.** One or two places where the screen gets rich color or detail, taken from what it shows. A clip thumbnail, a waveform, a scope, a color swatch, a caption preview over `BACKDROP`. Write what they are. Many screens have none, and that is fine.
- **Signature.** The detail that makes it Flikk, used where it belongs: mono timecodes, track-tint dots, keycaps for shortcuts, the playhead red reserved for the playhead.

Never introduce a second brand color, a gradient or a glow to give a screen character. Character comes from the copy, the moments and the signature.

## 1. Spacing

- Flikk is dense and sits on a 2px grid, with 4 as the main step. Margins, paddings, gaps and fixed heights are even numbers from the scale in the tokens. Odd values like 3, 5, 7 or 13 are slop unless they exactly center something, and then they stay with a short comment.
- Same thing, same size. All rows in a list share one height and one gap, all sections in an inspector share one padding.
- Space shows grouping. Space inside a group is smaller than space between groups, so a list reads as one block and sections read as separate ones.
- Things line up. Icons sit in one column, labels start on one line, numbers align right in one column. Nothing is nudged off the line with a stray margin.
- Panel headers, title bar and status bar use `t::PANEL_HEADER`, `t::TITLE_BAR` and `t::STATUS_BAR`, never their own heights.
- Remove one-off values that break the rhythm and replace them with steps from the scale.

## 2. Color

- Surfaces come from the graphite ramp in `theme.rs`: `G` for the window and viewer surround, `INSET` and `P` for panels, `R1` for raised, `HV` for hover, `PS` for pressed. Depth comes from these steps and the `LN` hairline, not from shadows.
- Three text roles, `T1`, `T2` and `T3`. Not ten slightly different greys.
- One accent, `AC`, used rarely. It marks focus, selection and the one primary action in a view. Its hover and press are `AC_HV` and `AC_PS`, its wash is `AC_WASH`.
- Meaning colors carry meaning only: `OK`, `WARN`, `ERR` for status, the track tints for track kinds, `PH` for the playhead and nothing else, `CH_R`/`CH_G`/`CH_B` for channels. Never borrow one as decoration because it looks nice.
- Remove hue gradients, colored glows, colored shadows, color washes behind content and neon borders.
- Borders are `LN` hairlines, `LN_HV` when hovered. Floating surfaces (menus, popovers, dialogs) use `t::floating_shadow()`; nothing else gets a shadow.
- Focus is `t::focus_ring()` on controls and `t::field_ring()` on fields.
- A gradient is allowed only as a small tone shift inside one neutral. Never from one hue to another.

## 3. Icons

- Every emoji used as an icon or as decoration becomes a lucide icon through the kit `Icon` (`IconName`). No other icon set, no inline SVG paths for things lucide has.
- One size, the kit default of 14px. Larger only for an empty-state glyph.
- Icons take the color of their text role: `T2` idle, `T1` on hover, `AC` when toggled on. They never get their own colors, except a track tint or status color that carries meaning.
- The icon box has the same width in every row and the gap to the label is fixed, so labels align.
- Drop icons that add nothing, like an icon before every section label. Categories that need color get a small status dot (`Indicator`).

## 4. Copy

- Titles name the thing, in Flikk's words, so they could not fit another app.
- No exclamation marks, no greetings, no `Welcome back! 👋`.
- Short labels. `Search projects`, not `Search anything...`. `Export`, not `Export your amazing video`.
- No subtitles or helper lines that repeat what the layout already shows. If a control needs explaining, its label is wrong.
- Labels, buttons, headers and tooltips end without a period. Only multi-sentence paragraphs keep periods.
- No hype words like supercharge, seamless, unlock, magic or effortless, and no `AI-powered` or ✨ on the agent features. Say what the feature does.
- Errors say what happened and what to do, plainly, without blame or apology.
- Mock data looks real: `Interview_A_cam2.mov`, `00:03:12:08`, `1080p · 25 fps`, never `[X]` or `123,456`.
- Badges like `PRO`, `New ✨` or `Beta 🚀` go away unless the product really needs them, and then they are a quiet `Badge`.

## 5. Type

- The interface uses the system face that GPUI gives; figures, timecodes, frame counts, sizes and other numbers use `t::mono()`. No other families.
- A few sizes from the scale in the tokens and two weights, `NORMAL` and `MEDIUM`, with `SEMIBOLD` at most for rare emphasis like a dialog title. No `BOLD` or heavier.
- Hierarchy comes from size and color (`T1`, `T2`, `T3`), not weight.
- Sentence case everywhere. Labels like `NAVIGATION` or `AUDIO TRACKS` with wide tracking become `Navigation` and `Audio tracks`.
- No negative letter spacing on small text.
- Numbers use one format across the screen and sit in mono, right-aligned in columns.
- No italic for emphasis.

## 6. Layout

- Keep the sections and their order. Inside a section, alignment and proportions may change when that makes the screen clearer.
- Panels fill their resizable slot (`h_resizable`, `v_resizable`, `resizable_panel`). No fixed-width islands centered in a panel.
- Inspector rows are label left, control right, labels in one column with one width.
- Dialogs: title, body, then a footer on `INSET` with the primary action rightmost and at most one secondary next to it. Destructive actions use `Button::destructive()` and are never the default.
- One `Button::primary()` per view. Everything else is a default or `ghost()` button.
- Long text aligns left. Centering is for short empty states only.
- Compose the panel as a whole at its smallest usable size and at a large size. Nothing overlaps or clips when a resizable panel is dragged narrow; text truncates with an ellipsis instead.

## 7. Details

- Nested radius. Use `RADIUS_SM` for clips and fields, `RADIUS` for buttons, `RADIUS_SEG` for segmented controls, `RADIUS_LG` for floating surfaces. An inner radius equals the outer radius minus the padding between them, never less than 2. Nothing is a pill except a toggle track.
- Counts and meta become quiet mono text in `T3`, not glowing pills.
- Keyboard shortcuts are `Kbd` keycaps.
- Scopes, curves and meters sit on `SCOPE` wells with muted grid lines and mono labels. One series leads; the rest are quiet.
- The active state is a surface step (`HV` or `R1`) with a hairline, or `AC_WASH` for a selected row. Not a gradient with a glow.
- Hover, pressed and focus states exist on everything clickable and stay inside the palette.
- Empty states use `EmptyState`: one icon, one short line, at most one action.
- The signature details from the direction appear where they belong and nowhere else.

## 8. Motion

- Remove motion that loops for no reason. Looping is only for things that are live: playback, recording, a `Spinner` while work runs, a `Progress` bar.
- No bouncy springs, no hover scaling, nothing that moves more than a few pixels on hover.
- Keep motion that explains something: a hover is an instant color or surface change, a popover fades in from where it opens, a changed number updates in place.
- One easing and a few durations, from the tokens. GPUI animations go through `with_animation`; keep them short and never animate layout sizes on every frame.
- Playback, scrubbing and timeline drags must never wait on an animation.

## Do not

- Do not rename the product or redraw its mark.
- Do not change what the screen does. Keep its features, content and order unless the user asks for more. Placeholder content that means nothing may be replaced with content that does.
- Do not replace removed decoration with new decoration, and do not swap one trend for another like glass, grain, noise or neon.
- Do not invent features, claims or data.
- Do not add colors outside `theme.rs`, or style raw controls outside the kit.
- Do not write tests.

## Checklist

- [ ] The direction is written and every pass follows it
- [ ] Every spacing value is on the scale and equal things have equal sizes
- [ ] Icons, labels and numbers each sit in one aligned column
- [ ] Every color is a named token from `theme.rs`; no raw `hex`, `rgb` or `hsla` in panel code
- [ ] One accent, used for focus, selection and the one primary action
- [ ] Meaning colors only carry their meaning; `PH` only on the playhead
- [ ] No gradients, glows, colored shadows or color washes
- [ ] No emoji left in the interface; every icon is a kit `Icon` at 14px
- [ ] Weights `NORMAL` and `MEDIUM`, `SEMIBOLD` at most; no uppercase tracked labels, no italic
- [ ] Numbers and timecodes in `t::mono()`, one format, right-aligned in columns
- [ ] No exclamation marks, trailing periods on short strings, greetings, hype words or explanatory subtitles
- [ ] Nested radii add up and come from the radius tokens
- [ ] The panel holds up at its narrowest and widest sizes
- [ ] No looping motion on things that are not live, no hover scaling
- [ ] `cargo fmt --check` and clippy pass
- [ ] Template check. If you swapped Flikk for another app's name, would the screen still fit? If yes, push harder in the copy, the moments and the signature
