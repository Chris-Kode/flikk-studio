# Signs of slop

Go through every group and note each sign you find with `file:line`. Each line pairs the sign with its fix.

## Surfaces and color

- Raw `hex(…)`, `rgb(…)`, `rgba(…)` or `hsla(…)` in panel code → a named token from `kit/theme.rs`, added there if it is missing
- Hue-to-hue gradients on backgrounds, headers, buttons or the active row → one graphite surface, accent only where it means something
- Colored glows, like a `BoxShadow` with an accent or tint color and a large blur → remove, depth from surface steps and `LN`
- Shadows on things that do not float → none; only menus, popovers and dialogs get `floating_shadow()`
- Color washes or blurred shapes behind content → remove
- Bright or colored borders → `LN` hairline
- Many near-identical greys for text, made with `alpha(T1, …)` in five places → `T1`, `T2`, `T3`
- Every element highlighted, so nothing stands out → `AC` once or twice per view
- A meaning color used as decoration, like the playhead red on a delete button or a track tint on a header → the neutral, or `ERR` for danger
- Purple used as an accent because it looks "AI" → `AC`, or the caption tint `C` only where it means captions

## Type

- `FontWeight::BOLD`, `EXTRA_BOLD` or `BLACK` → `MEDIUM`, size and color for hierarchy
- Uppercase labels with wide tracking, like `AUDIO TRACKS` → sentence case, normal tracking
- Too many sizes on one screen → a few from the scale
- Timecodes, durations, frame counts or sizes in the interface face → `t::mono()`
- Numbers in mixed formats, like `1.2GB` next to `845 MB`, or `00:03:12` next to `3:12:08` → one format
- Italic for emphasis → color or weight instead

## Icons

- Emoji as icons, label decoration or status, like 🎬 🎵 ✂️ 🚀 ⚠️ ✨ → kit `Icon` with a lucide `IconName`, or nothing
- Emoji inside counts, like `12 🎞` → plain number
- Icons in many sizes and colors → 14px, text role color
- Inline SVG paths for things lucide has → `IconName`
- Icons nudged off the column → same icon box width in every row
- A sparkle for anything AI or agent → remove, or a plain icon only where it explains something

## Spacing

- Odd one-off values, like `px(3.)`, `px(7.)`, `px(13.)` → steps on the scale
- Rows in one list with different heights or gaps → one height, one gap
- Groups that sit closer to their neighbours than to their own items → smaller gaps inside groups, bigger between them
- Uneven padding inside sibling sections → one padding
- A panel header or bar with its own height → `PANEL_HEADER`, `TITLE_BAR`, `STATUS_BAR`

## Copy

- Greetings, like `Welcome back! 👋` → the thing the screen shows as its title
- Exclamation marks → plain statement
- Subtitles and helper lines that repeat what the layout shows → remove
- Wordy placeholders, like `Search anything...` → `Search projects`
- Hype words, like supercharge, seamless, magic, effortless, AI-powered → what the feature does
- Trailing periods on labels, buttons and tooltips → none
- Fake looking data, like `video1.mp4`, `[X]` or `123,456` → plausible file names, timecodes and sizes
- Errors that apologise or blame, like `Oops! Something went wrong` → what failed and what to do

## Components

- A raw `div()` styled as a button, field, menu or tab → the kit component
- Glowing pills for counts, status or shortcuts → mono `T3` text, `Indicator` dot or `Kbd`
- `PRO`, `New ✨`, `Beta 🚀` badges → remove, or one quiet `Badge`
- Every element fully rounded → the radius tokens, nested radius math
- Several primary buttons in one view → one `primary()`, the rest default or `ghost()`
- A destructive action as the default or primary button → `destructive()`, never the default
- Scopes or meters with rainbow fills, glows or uppercase labels → `SCOPE` well, muted grid, mono labels, channel colors only for channels
- Empty states with a big emoji, a bold title and a paragraph → `EmptyState`: one icon, one line, one action

## Motion

- Pulsing, breathing or shimmering things that are not live → static
- Spinners where nothing is running → remove
- Hover scaling or movement → color or surface step
- Bouncy or long animations on simple UI → one short ease-out
- Animations that delay playback, scrubbing or dragging → remove

## Template look

These are not slop, but they are what a cleaned-up screen falls back to when nobody made a decision. Watch for them after the passes.

- A panel that is only grey boxes and labels where it could show the footage, like a project list without thumbnails or durations → show the real material
- A title that would fit another app if you swapped the name → say what this screen does, with Flikk's own nouns
- Every dialog laid out the same way regardless of what it asks → let the content decide the body

---

Adapted from unslop by Damir Sokolovsky ([@heydamir](https://x.com/heydamir)), https://github.com/heydamir/unslop, CC BY 4.0. Changed for koo-editor's GPUI code. Keep this credit in copies and adaptations.
