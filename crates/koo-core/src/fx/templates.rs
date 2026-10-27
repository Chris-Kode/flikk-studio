//! Built-in HTML generators: titles, lower thirds and other motion graphics.
//! See the module docs of [`super`] for how time and parameters reach the
//! page.
//!
//! Every template lays out on a 1920 × 1080 canvas with system fonts only,
//! keeps its background transparent (full-screen cards take a `background`
//! color), and times its exit against `--duration`, so it ends cleanly
//! however long the clip is.

use super::{
    FxAsset, FxKind, FxLanguage, FxRef, ParamKind, ParamSpec, ParamValue, color, float, text,
};

fn html(
    key: &str,
    name: &str,
    category: &str,
    description: &str,
    code: &str,
    params: Vec<ParamSpec>,
) -> FxAsset {
    FxAsset {
        id: FxRef::builtin(key),
        name: name.into(),
        kind: FxKind::Generator,
        language: FxLanguage::Html,
        category: category.into(),
        description: description.into(),
        code: code.into(),
        params,
    }
}

fn int(name: &str, label: &str, min: i64, max: i64, default: i64) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Int { min, max },
        default: ParamValue::Number(default as f64),
    }
}

fn choice(name: &str, label: &str, options: &[&str], default: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Choice {
            options: options.iter().map(|o| o.to_string()).collect(),
        },
        default: ParamValue::Text(default.into()),
    }
}

/// `[x, y]` in uv space (bottom-left origin).
fn point(name: &str, label: &str, xy: [f64; 2]) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Point,
        default: ParamValue::List(xy.to_vec()),
    }
}

fn image(name: &str, label: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Image,
        default: ParamValue::Text(String::new()),
    }
}

const WHITE: [f64; 4] = [1.0, 1.0, 1.0, 1.0];
const INK: [f64; 4] = [0.07, 0.08, 0.1, 1.0];
const BLUE: [f64; 4] = [0.2, 0.45, 0.95, 1.0];
const CORAL: [f64; 4] = [1.0, 0.35, 0.21, 1.0];
const GOLD: [f64; 4] = [1.0, 0.76, 0.2, 1.0];
const RED: [f64; 4] = [0.9, 0.11, 0.14, 1.0];

const CORNERS: &[&str] = &["Bottom left", "Bottom right", "Top left", "Top right"];

pub(super) fn all() -> Vec<FxAsset> {
    vec![
        // ---- lower thirds
        html(
            "lower_third",
            "Lower Third",
            "Lower thirds",
            "Name and role on a dark panel with an accent bar, wiping in at the bottom left.",
            r#"<style>
  .lt { position: absolute; left: 112px; bottom: 128px; display: flex; align-items: stretch;
        font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
        animation: out .5s cubic-bezier(.7, 0, .84, 0) forwards;
        animation-delay: calc(var(--duration) * 1s - .5s); }
  .bar { width: 10px; border-radius: 5px; background: var(--accent); transform-origin: 50% 100%;
         animation: grow .5s cubic-bezier(.16, 1, .3, 1) both; }
  .panel { margin-left: 14px; padding: 20px 40px 22px 30px; border-radius: 6px;
           background: linear-gradient(90deg, rgba(12, 14, 20, .88), rgba(12, 14, 20, .72));
           box-shadow: 0 12px 40px rgba(0, 0, 0, .25);
           animation: wipe .75s .12s cubic-bezier(.16, 1, .3, 1) both; }
  .name { color: #fff; font-size: 64px; font-weight: 750; letter-spacing: -.01em; line-height: 1.05;
          animation: rise .7s .3s cubic-bezier(.16, 1, .3, 1) both; }
  .role { color: rgba(255, 255, 255, .78); font-size: 30px; font-weight: 600; letter-spacing: .1em;
          text-transform: uppercase; margin-top: 12px;
          animation: rise .7s .42s cubic-bezier(.16, 1, .3, 1) both; }
  @keyframes grow { from { transform: scaleY(0) } }
  @keyframes wipe { from { clip-path: inset(0 100% 0 0) } to { clip-path: inset(0 0 0 0) } }
  @keyframes rise { from { transform: translateY(24px); opacity: 0 } }
  @keyframes out { to { transform: translateX(-48px); opacity: 0 } }
</style>
<div class="lt"><div class="bar"></div><div class="panel"><div class="name">{{name}}</div><div class="role">{{role}}</div></div></div>"#,
            vec![
                text("name", "Name", "Ada Lovelace"),
                text("role", "Role", "Mathematician"),
                color("accent", "Accent", BLUE),
            ],
        ),
        html(
            "lower_third_line",
            "Lower Third Line",
            "Lower thirds",
            "Light name over a thin drawn line, role in spaced capitals; left or right.",
            r#"<style>
  .lt { position: absolute; bottom: 136px; color: var(--color);
        font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
        text-shadow: 0 2px 16px rgba(0, 0, 0, .45);
        animation: out .6s ease-in forwards; animation-delay: calc(var(--duration) * 1s - .6s); }
  [data-side="Left"] { left: 120px; }
  [data-side="Right"] { right: 120px; text-align: right; }
  .clip { overflow: hidden; padding: 0 4px; }
  .name { display: inline-block; font-size: 60px; font-weight: 300; letter-spacing: .01em;
          animation: up .9s .25s cubic-bezier(.16, 1, .3, 1) both; }
  .line { height: 3px; margin: 14px 4px 16px; border-radius: 2px; background: var(--accent);
          box-shadow: 0 1px 8px rgba(0, 0, 0, .25); transform-origin: left;
          animation: draw .9s cubic-bezier(.65, 0, .35, 1) both; }
  [data-side="Right"] .line { transform-origin: right; }
  .role { display: inline-block; font-size: 26px; font-weight: 600; letter-spacing: .3em;
          text-transform: uppercase; opacity: .85;
          animation: down .9s .35s cubic-bezier(.16, 1, .3, 1) both; }
  @keyframes draw { from { transform: scaleX(0) } }
  @keyframes up { from { transform: translateY(110%) } }
  @keyframes down { from { transform: translateY(-120%) } }
  @keyframes out { to { opacity: 0; filter: blur(8px); transform: translateY(8px) } }
</style>
<div class="lt" data-side="{{side}}">
  <div class="clip"><span class="name">{{name}}</span></div>
  <div class="line"></div>
  <div class="clip"><span class="role">{{role}}</span></div>
</div>"#,
            vec![
                text("name", "Name", "Grace Hopper"),
                text("role", "Role", "Rear Admiral"),
                color("color", "Text", WHITE),
                color("accent", "Line", GOLD),
                choice("side", "Side", &["Left", "Right"], "Left"),
            ],
        ),
        html(
            "lower_third_box",
            "Lower Third Box",
            "Lower thirds",
            "Name and role in a solid box with an optional logo and an accent stripe.",
            r#"<style>
  .lt { position: absolute; left: 112px; bottom: 120px; display: flex; align-items: center; gap: 28px;
        padding: 22px 44px 26px 26px; background: var(--box); color: var(--ink); border-radius: 14px;
        box-shadow: 0 18px 50px rgba(0, 0, 0, .28); overflow: hidden;
        font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
        transform-origin: 0 50%;
        animation: open .7s cubic-bezier(.34, 1.4, .64, 1) both, out .45s cubic-bezier(.7, 0, .84, 0) forwards;
        animation-delay: 0s, calc(var(--duration) * 1s - .45s); }
  .lt::after { content: ""; position: absolute; left: 0; right: 0; bottom: 0; height: 6px; background: var(--accent);
               transform-origin: left; animation: draw .8s .35s cubic-bezier(.65, 0, .35, 1) both; }
  .logo { width: 96px; height: 96px; object-fit: contain; border-radius: 10px;
          animation: pop .6s .3s cubic-bezier(.34, 1.56, .64, 1) both; }
  .logo[src=""] { display: none; }
  .text { animation: rise .6s .38s cubic-bezier(.16, 1, .3, 1) both; }
  .name { font-size: 54px; font-weight: 800; letter-spacing: -.015em; line-height: 1.05; }
  .role { font-size: 28px; font-weight: 500; opacity: .7; margin-top: 8px; }
  @keyframes open { from { transform: scaleX(.2); opacity: 0 } }
  @keyframes draw { from { transform: scaleX(0) } }
  @keyframes pop { from { transform: scale(.4); opacity: 0 } }
  @keyframes rise { from { transform: translateX(-16px); opacity: 0 } }
  @keyframes out { to { transform: translateY(30px); opacity: 0 } }
</style>
<div class="lt"><img class="logo" src="{{logo}}" alt=""><div class="text"><div class="name">{{name}}</div><div class="role">{{role}}</div></div></div>"#,
            vec![
                text("name", "Name", "Alan Turing"),
                text("role", "Role", "Computer Scientist"),
                image("logo", "Logo"),
                color("box", "Box", WHITE),
                color("ink", "Text", INK),
                color("accent", "Accent", CORAL),
            ],
        ),
        // ---- titles
        html(
            "title_card",
            "Title Card",
            "Titles",
            "Cinematic centered title in wide-spaced capitals that slowly tighten, with a lined subtitle.",
            r#"<style>
  .wrap { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center;
          justify-content: center; color: var(--color); text-align: center;
          font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
          text-shadow: 0 4px 30px rgba(0, 0, 0, .35);
          animation: out .9s ease-in forwards; animation-delay: calc(var(--duration) * 1s - .9s); }
  h1 { margin: 0; font-size: 104px; font-weight: 300; text-transform: uppercase; line-height: 1.1;
       animation: track calc(var(--duration) * 1s) cubic-bezier(.2, .6, .3, 1) both, reveal 1.6s ease-out both; }
  .sub { display: flex; align-items: center; gap: 32px; margin-top: 40px; font-size: 28px; font-weight: 500;
         letter-spacing: .42em; text-transform: uppercase; opacity: .85;
         animation: fade 1.4s .8s ease-out both; }
  .sub span { margin-right: -.42em; }
  .sub::before, .sub::after { content: ""; width: 120px; height: 1px; background: currentColor;
                              animation: line 1.4s .7s cubic-bezier(.16, 1, .3, 1) both; }
  @keyframes track { from { letter-spacing: .6em; margin-right: -.6em } to { letter-spacing: .3em; margin-right: -.3em } }
  @keyframes reveal { from { opacity: 0; filter: blur(16px) } }
  @keyframes fade { from { opacity: 0 } }
  @keyframes line { from { transform: scaleX(0) } }
  @keyframes out { to { opacity: 0; filter: blur(10px) } }
</style>
<div class="wrap"><h1>{{title}}</h1><div class="sub"><span>{{subtitle}}</span></div></div>"#,
            vec![
                text("title", "Title", "Chapter One"),
                text("subtitle", "Subtitle", "The beginning"),
                color("color", "Color", WHITE),
            ],
        ),
        html(
            "title_kinetic",
            "Kinetic Title",
            "Titles",
            "Heavy capitals that slam in word by word over an accent tag, then fly out.",
            r#"<style>
  .wrap { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center;
          justify-content: center; text-align: center;
          font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  .tag { display: inline-block; padding: 12px 30px; margin-bottom: 30px; background: var(--accent); color: var(--tag_text);
         font-size: 34px; font-weight: 800; letter-spacing: .14em; text-transform: uppercase; transform: skewX(-10deg);
         animation: tag .55s cubic-bezier(.16, 1, .3, 1) both, tag-out .4s ease-in forwards;
         animation-delay: 0s, calc(var(--duration) * 1s - .4s); }
  .tag span { display: inline-block; transform: skewX(10deg); }
  .tag:empty { display: none; }
  .words { max-width: 1600px; margin: 0; color: var(--color); font-size: 150px; font-weight: 900; font-style: italic;
           line-height: .98; letter-spacing: -.02em; text-transform: uppercase;
           text-shadow: 0 10px 40px rgba(0, 0, 0, .35); }
  .words span { display: inline-block; margin: 0 .12em;
                animation: slam .5s cubic-bezier(.2, 1.4, .4, 1) both, fly .45s cubic-bezier(.7, 0, .84, 0) forwards; }
  @keyframes tag { from { transform: skewX(-10deg) translateY(30px); clip-path: inset(0 100% 0 0) }
                   to { transform: skewX(-10deg); clip-path: inset(0 0 0 0) } }
  @keyframes tag-out { to { transform: skewX(-10deg) translateY(-20px); opacity: 0 } }
  @keyframes slam { from { transform: scale(1.8); opacity: 0; filter: blur(12px) } }
  @keyframes fly { to { transform: translateY(-80px) rotate(-4deg); opacity: 0 } }
</style>
<div class="wrap"><div class="tag"><span>{{tag}}</span></div><h1 class="words">{{title}}</h1></div>
<script>
  // One span per word, staggered in and out.
  const h = document.querySelector('.words');
  const words = h.textContent.trim().split(/\s+/);
  h.textContent = '';
  words.forEach((w, i) => {
    const s = document.createElement('span');
    s.textContent = w;
    s.style.animationDelay = `${0.25 + i * 0.11}s, calc(var(--duration) * 1s - ${0.5 - Math.min(i, 6) * 0.04}s)`;
    h.append(s, ' ');
  });
  const tag = document.querySelector('.tag');
  if (!tag.textContent.trim()) tag.textContent = '';
</script>"#,
            vec![
                text("title", "Title", "Make it loud"),
                text("tag", "Tag", "New episode"),
                color("color", "Color", WHITE),
                color("accent", "Accent", CORAL),
                color("tag_text", "Tag text", WHITE),
            ],
        ),
        html(
            "chapter_title",
            "Chapter Title",
            "Titles",
            "Chapter number and title revealed on the left over a soft shade.",
            r#"<style>
  .shade { position: absolute; inset: 0; opacity: var(--shade);
           background: linear-gradient(90deg, rgba(0, 0, 0, .85) 0%, rgba(0, 0, 0, .4) 45%, transparent 75%);
           animation: fade .8s ease-out both, fade-out .7s ease-in forwards;
           animation-delay: 0s, calc(var(--duration) * 1s - .7s); }
  .ch { position: absolute; left: 150px; top: 50%; transform: translateY(-50%); color: var(--color);
        font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
        animation: out .6s cubic-bezier(.7, 0, .84, 0) forwards; animation-delay: calc(var(--duration) * 1s - .6s); }
  .kicker { display: flex; align-items: center; gap: 22px; color: var(--accent); font-size: 30px; font-weight: 700;
            letter-spacing: .32em; text-transform: uppercase; }
  .kicker::before { content: ""; width: 72px; height: 4px; border-radius: 2px; background: currentColor; transform-origin: left;
                    animation: draw .8s .2s cubic-bezier(.65, 0, .35, 1) both; }
  .clip { overflow: hidden; padding-bottom: 6px; }
  .kicker span { display: inline-block; animation: up .8s .35s cubic-bezier(.16, 1, .3, 1) both; }
  h1 { margin: 18px 0 0; font-size: 112px; font-weight: 800; letter-spacing: -.025em; line-height: 1.02; max-width: 1300px;
       animation: up .9s .5s cubic-bezier(.16, 1, .3, 1) both; }
  @keyframes draw { from { transform: scaleX(0) } }
  @keyframes up { from { transform: translateY(105%) } }
  @keyframes fade { from { opacity: 0 } }
  @keyframes fade-out { to { opacity: 0 } }
  @keyframes out { to { opacity: 0; transform: translate(-40px, -50%) } }
</style>
<div class="shade"></div>
<div class="ch"><div class="kicker clip"><span>Chapter {{number}}</span></div><div class="clip"><h1>{{title}}</h1></div></div>"#,
            vec![
                text("number", "Number", "02"),
                text("title", "Title", "Into the Field"),
                color("color", "Text", WHITE),
                color("accent", "Accent", GOLD),
                float("shade", "Shade", 0.0, 1.0, 0.6),
            ],
        ),
        html(
            "quote_card",
            "Quote Card",
            "Titles",
            "Full-screen card with a large quote revealed word by word and its author.",
            r#"<style>
  html, body { background: var(--background); }
  .card { position: absolute; inset: 0; display: flex; flex-direction: column; justify-content: center;
          padding: 0 260px; color: var(--color);
          animation: out .7s ease-in forwards; animation-delay: calc(var(--duration) * 1s - .7s); }
  .mark { height: 150px; margin-bottom: 10px; color: var(--accent); font: 300px/1 Georgia, "Times New Roman", serif;
          animation: pop .7s cubic-bezier(.34, 1.56, .64, 1) both; }
  blockquote { margin: 0; font: italic 400 72px/1.25 Georgia, "Times New Roman", serif; letter-spacing: -.005em; }
  blockquote span { display: inline-block; animation: word .6s cubic-bezier(.16, 1, .3, 1) both; }
  .author { display: flex; align-items: center; gap: 24px; margin-top: 56px; font-size: 32px; font-weight: 600;
            letter-spacing: .14em; text-transform: uppercase; opacity: .8;
            font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            animation: fade .8s both; }
  .author::before { content: ""; width: 64px; height: 3px; background: var(--accent); }
  @keyframes pop { from { transform: scale(.4) rotate(-12deg); opacity: 0 } }
  @keyframes word { from { transform: translateY(26px); opacity: 0; filter: blur(6px) } }
  @keyframes fade { from { opacity: 0; transform: translateY(12px) } }
  @keyframes out { to { opacity: 0; transform: translateY(-24px) } }
</style>
<div class="card"><div class="mark">&ldquo;</div><blockquote>{{quote}}</blockquote><div class="author">{{author}}</div></div>
<script>
  // Words appear in reading order across about 1.6 s.
  const q = document.querySelector('blockquote');
  const words = q.textContent.trim().split(/\s+/);
  const step = Math.min(0.12, 1.6 / Math.max(1, words.length));
  q.textContent = '';
  words.forEach((w, i) => {
    const s = document.createElement('span');
    s.textContent = w;
    s.style.animationDelay = `${0.35 + i * step}s`;
    q.append(s, ' ');
  });
  document.querySelector('.author').style.animationDelay = `${0.6 + words.length * step}s`;
</script>"#,
            vec![
                text(
                    "quote",
                    "Quote",
                    "The best way to predict the future is to invent it.",
                ),
                text("author", "Author", "Alan Kay"),
                color("background", "Background", [0.06, 0.07, 0.11, 1.0]),
                color("color", "Text", WHITE),
                color("accent", "Accent", GOLD),
            ],
        ),
        // ---- callouts and social
        html(
            "callout",
            "Callout",
            "Callouts",
            "Pulsing ring at a point with a drawn arrow and a label.",
            r#"<style>
  .anchor { position: absolute; left: calc(var(--target-x) * 100%); top: calc(var(--target-y) * 100%); width: 0; height: 0;
            font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            animation: out .45s ease-in forwards; animation-delay: calc(var(--duration) * 1s - .45s); }
  .ring, .pulse { position: absolute; left: calc(var(--size) * -.5px); top: calc(var(--size) * -.5px);
                  width: calc(var(--size) * 1px); height: calc(var(--size) * 1px); box-sizing: border-box;
                  border-radius: 50%; border: 6px solid var(--accent); }
  .ring { box-shadow: 0 0 0 2px rgba(0, 0, 0, .15), 0 6px 24px rgba(0, 0, 0, .3);
          animation: ring .6s cubic-bezier(.34, 1.56, .64, 1) both; }
  .pulse { animation: pulse 1.6s .6s ease-out infinite both; }
  svg { position: absolute; left: -500px; top: -500px; overflow: visible; }
  [data-dir="Up right"] svg { transform: scaleX(-1); }
  [data-dir="Down left"] svg { transform: scaleY(-1); }
  [data-dir="Down right"] svg { transform: scale(-1, -1); }
  .stroke { fill: none; stroke: var(--accent); stroke-width: 7; stroke-linecap: round; stroke-linejoin: round;
            filter: drop-shadow(0 3px 6px rgba(0, 0, 0, .3)); }
  .path { stroke-dasharray: 400; animation: draw .6s .35s cubic-bezier(.65, 0, .35, 1) both; }
  .head { animation: fade .15s .9s both; }
  .label { position: absolute; padding: 16px 30px; border-radius: 999px; white-space: nowrap; background: var(--accent);
           color: var(--ink); font-size: 40px; font-weight: 750; box-shadow: 0 10px 30px rgba(0, 0, 0, .3);
           animation: pop .5s .15s cubic-bezier(.34, 1.56, .64, 1) both; }
  [data-dir="Up left"] .label { right: 300px; bottom: 210px; }
  [data-dir="Up right"] .label { left: 300px; bottom: 210px; }
  [data-dir="Down left"] .label { right: 300px; top: 210px; }
  [data-dir="Down right"] .label { left: 300px; top: 210px; }
  @keyframes ring { from { transform: scale(0); opacity: 0 } }
  @keyframes pulse { from { transform: scale(1); opacity: .8 } to { transform: scale(1.7); opacity: 0 } }
  @keyframes draw { from { stroke-dashoffset: 400 } to { stroke-dashoffset: 0 } }
  @keyframes fade { from { opacity: 0 } }
  @keyframes pop { from { transform: scale(.5); opacity: 0 } }
  @keyframes out { to { opacity: 0; transform: scale(.92) } }
</style>
<div class="anchor" data-dir="{{direction}}">
  <div class="pulse"></div><div class="ring"></div>
  <svg width="1000" height="1000" viewBox="-500 -500 1000 1000">
    <path class="stroke path" d="M -290 -215 C -200 -215 -110 -170 -78 -82" />
    <path class="stroke head" d="M -104 -100 L -76 -76 L -60 -110" />
  </svg>
  <div class="label">{{label}}</div>
</div>"#,
            vec![
                text("label", "Label", "Look here"),
                point("target", "Target", [0.62, 0.45]),
                choice(
                    "direction",
                    "Label",
                    &["Up left", "Up right", "Down left", "Down right"],
                    "Up left",
                ),
                float("size", "Ring size", 60.0, 600.0, 150.0),
                color("accent", "Color", GOLD),
                color("ink", "Text", INK),
            ],
        ),
        html(
            "subscribe",
            "Subscribe Button",
            "Callouts",
            "Call-to-action button that a cursor clicks, turning it to its done state.",
            r#"<style>
  .spot { position: absolute; font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
          animation: out .45s cubic-bezier(.7, 0, .84, 0) forwards; animation-delay: calc(var(--duration) * 1s - .45s); }
  [data-at="Bottom center"] { left: 50%; bottom: 140px; transform: translateX(-50%); }
  [data-at="Bottom left"] { left: 120px; bottom: 140px; }
  [data-at="Bottom right"] { right: 120px; bottom: 140px; }
  [data-at="Center"] { left: 50%; top: 50%; transform: translate(-50%, -50%); }
  .btn { position: relative; display: grid; border-radius: 999px; color: #fff; font-size: 46px; font-weight: 800;
         letter-spacing: .06em; text-transform: uppercase; box-shadow: 0 14px 40px rgba(0, 0, 0, .35);
         background: var(--color);
         animation: pop .6s .1s cubic-bezier(.34, 1.56, .64, 1) both, press .3s 1.3s ease-in-out both, done .2s 1.45s forwards; }
  .face { grid-area: 1 / 1; display: flex; align-items: center; justify-content: center; gap: 22px; padding: 26px 56px; white-space: nowrap; }
  .face svg { width: 50px; height: 50px; fill: currentColor; }
  .before { animation: hide .2s 1.45s forwards; }
  .after { opacity: 0; animation: show .2s 1.45s forwards; }
  .after svg { transform-origin: 50% 8%; animation: ring .9s 1.75s ease-in-out both; }
  .cursor { position: absolute; right: 18%; bottom: -70px; width: 64px; height: 64px;
            filter: drop-shadow(0 4px 8px rgba(0, 0, 0, .45));
            animation: glide .8s .45s cubic-bezier(.16, 1, .3, 1) both, click .3s 1.3s ease-in-out both, leave .5s 2.1s ease-in forwards; }
  @keyframes pop { from { transform: scale(.3); opacity: 0 } }
  @keyframes press { 50% { transform: scale(.92) } }
  @keyframes done { to { background: #3b3b3f } }
  @keyframes hide { to { opacity: 0 } }
  @keyframes show { to { opacity: 1 } }
  @keyframes ring { 0%, 100% { transform: rotate(0) } 15% { transform: rotate(22deg) } 30% { transform: rotate(-18deg) }
                    45% { transform: rotate(12deg) } 60% { transform: rotate(-8deg) } 75% { transform: rotate(4deg) } }
  @keyframes glide { from { transform: translate(220px, 160px); opacity: 0 } }
  @keyframes click { 50% { transform: scale(.82) } }
  @keyframes leave { to { transform: translate(140px, 140px); opacity: 0 } }
  @keyframes out { to { opacity: 0; filter: blur(6px) } }
</style>
<div class="spot" data-at="{{position}}">
  <div class="btn">
    <div class="face before"><svg viewBox="0 0 24 24"><path d="M21.6 7.2a2.6 2.6 0 0 0-1.8-1.8C18.2 5 12 5 12 5s-6.2 0-7.8.4A2.6 2.6 0 0 0 2.4 7.2 27 27 0 0 0 2 12a27 27 0 0 0 .4 4.8 2.6 2.6 0 0 0 1.8 1.8C5.8 19 12 19 12 19s6.2 0 7.8-.4a2.6 2.6 0 0 0 1.8-1.8A27 27 0 0 0 22 12a27 27 0 0 0-.4-4.8zM10 15V9l5.2 3z"/></svg>{{label}}</div>
    <div class="face after"><svg viewBox="0 0 24 24"><path d="M12 22a2.5 2.5 0 0 0 2.5-2.5h-5A2.5 2.5 0 0 0 12 22zm7-6V11c0-3.1-1.6-5.6-4.5-6.3V4a2.5 2.5 0 0 0-5 0v.7C6.6 5.4 5 7.9 5 11v5l-2 2v1h18v-1z"/></svg>{{done_label}}</div>
  </div>
  <svg class="cursor" viewBox="0 0 24 24"><path d="M5 2.5v17.2l4.6-4.3 2.9 6.6 3-1.3-2.9-6.5H19z" fill="white" stroke="rgb(17, 17, 17)" stroke-width="1.3" stroke-linejoin="round"/></svg>
</div>"#,
            vec![
                text("label", "Label", "Subscribe"),
                text("done_label", "Done", "Subscribed"),
                color("color", "Color", RED),
                choice(
                    "position",
                    "Position",
                    &["Bottom center", "Bottom left", "Bottom right", "Center"],
                    "Bottom center",
                ),
            ],
        ),
        html(
            "social_handle",
            "Social Handle",
            "Callouts",
            "Platform icon and handle in a pill that slides in from a corner.",
            r#"<style>
  .badge { position: absolute; display: flex; align-items: center; gap: 22px; padding: 14px 40px 14px 14px;
           border-radius: 999px; background: var(--box); color: var(--color); box-shadow: 0 12px 36px rgba(0, 0, 0, .3);
           font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
           animation: slide .7s cubic-bezier(.16, 1, .3, 1) both, out .45s cubic-bezier(.7, 0, .84, 0) forwards;
           animation-delay: 0s, calc(var(--duration) * 1s - .45s); }
  [data-at="Bottom left"] { left: 100px; bottom: 100px; --dx: -140px; }
  [data-at="Bottom right"] { right: 100px; bottom: 100px; --dx: 140px; }
  [data-at="Top left"] { left: 100px; top: 100px; --dx: -140px; }
  [data-at="Top right"] { right: 100px; top: 100px; --dx: 140px; }
  .icon { display: grid; place-items: center; width: 84px; height: 84px; border-radius: 50%; background: var(--accent);
          animation: spin .8s .15s cubic-bezier(.34, 1.56, .64, 1) both; }
  .icon svg { display: none; width: 48px; height: 48px; }
  [data-platform="YouTube"] .yt, [data-platform="Instagram"] .ig, [data-platform="X"] .x,
  [data-platform="Website"] .web, [data-platform="Email"] .mail { display: block; }
  .handle { font-size: 44px; font-weight: 700; letter-spacing: -.005em; white-space: nowrap;
            animation: fade .6s .3s ease-out both; }
  @keyframes slide { from { transform: translateX(var(--dx)); opacity: 0 } }
  @keyframes spin { from { transform: rotate(-180deg) scale(.3) } }
  @keyframes fade { from { opacity: 0; transform: translateX(-12px) } }
  @keyframes out { to { transform: translateX(var(--dx)); opacity: 0 } }
</style>
<div class="badge" data-at="{{position}}" data-platform="{{platform}}">
  <div class="icon">
    <svg class="yt" viewBox="0 0 24 24" fill="white"><path d="M21.6 7.2a2.6 2.6 0 0 0-1.8-1.8C18.2 5 12 5 12 5s-6.2 0-7.8.4A2.6 2.6 0 0 0 2.4 7.2 27 27 0 0 0 2 12a27 27 0 0 0 .4 4.8 2.6 2.6 0 0 0 1.8 1.8C5.8 19 12 19 12 19s6.2 0 7.8-.4a2.6 2.6 0 0 0 1.8-1.8A27 27 0 0 0 22 12a27 27 0 0 0-.4-4.8zM10 15V9l5.2 3z"/></svg>
    <svg class="ig" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2"><rect x="3.5" y="3.5" width="17" height="17" rx="5"/><circle cx="12" cy="12" r="4"/><circle cx="17.3" cy="6.7" r="1" fill="white" stroke="none"/></svg>
    <svg class="x" viewBox="0 0 24 24" fill="white"><path d="M17.8 3h3.1l-6.8 7.7 8 10.3h-6.2l-4.9-6.3L5.4 21H2.3l7.2-8.3L1.9 3h6.4l4.4 5.8zm-1.1 16.2h1.7L7.4 4.7H5.6z"/></svg>
    <svg class="web" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.8"><circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18M4.5 7.5h15M4.5 16.5h15"/></svg>
    <svg class="mail" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="2.5"/><path d="M3.5 6.5 12 13l8.5-6.5"/></svg>
  </div>
  <div class="handle">{{handle}}</div>
</div>"#,
            vec![
                text("handle", "Handle", "@kooeditor"),
                choice(
                    "platform",
                    "Platform",
                    &["YouTube", "Instagram", "X", "Website", "Email"],
                    "YouTube",
                ),
                choice("position", "Position", CORNERS, "Bottom left"),
                color("accent", "Icon", RED),
                color("box", "Background", [0.08, 0.09, 0.12, 0.88]),
                color("color", "Text", WHITE),
            ],
        ),
        // ---- data
        html(
            "progress_bar",
            "Progress Bar",
            "Data",
            "Bar along the top or bottom edge that fills over the clip.",
            r#"<style>
  .track { position: absolute; left: var(--inset); right: var(--inset); height: calc(var(--thickness) * 1px);
           border-radius: calc(var(--thickness) * 1px); background: var(--track); overflow: hidden;
           animation: in .4s ease-out both, out .4s ease-in forwards; animation-delay: 0s, calc(var(--duration) * 1s - .4s); }
  [data-at="Bottom"] { bottom: var(--inset); }
  [data-at="Top"] { top: var(--inset); }
  .fill { height: 100%; width: 100%; border-radius: inherit; background: var(--fill); transform-origin: left;
          animation: grow calc(var(--duration) * 1s) linear both; }
  @keyframes grow { from { transform: scaleX(0) } to { transform: scaleX(1) } }
  @keyframes in { from { opacity: 0 } }
  @keyframes out { to { opacity: 0 } }
  :root { --inset: calc(var(--margin) * 1px); }
</style>
<div class="track" data-at="{{position}}"><div class="fill"></div></div>"#,
            vec![
                choice("position", "Position", &["Bottom", "Top"], "Bottom"),
                float("thickness", "Thickness", 2.0, 60.0, 10.0),
                float("margin", "Margin", 0.0, 200.0, 0.0),
                color("fill", "Fill", CORAL),
                color("track", "Track", [1.0, 1.0, 1.0, 0.25]),
            ],
        ),
        html(
            "number_counter",
            "Number Counter",
            "Data",
            "Big number that counts from one value to another, with a label.",
            r#"<style>
  .wrap { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center;
          color: var(--color); text-align: center; text-shadow: 0 6px 30px rgba(0, 0, 0, .35);
          font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
          animation: out .5s ease-in forwards; animation-delay: calc(var(--duration) * 1s - .5s); }
  .num { font-size: 220px; font-weight: 850; letter-spacing: -.03em; line-height: 1; font-variant-numeric: tabular-nums;
         animation: pop .6s cubic-bezier(.34, 1.56, .64, 1) both; }
  .label { display: flex; align-items: center; gap: 24px; margin-top: 26px; font-size: 40px; font-weight: 600;
           letter-spacing: .16em; text-transform: uppercase; opacity: .85; animation: rise .7s .25s cubic-bezier(.16, 1, .3, 1) both; }
  .label::before, .label::after { content: ""; width: 56px; height: 4px; border-radius: 2px; background: var(--accent); }
  .label:empty { display: none; }
  @keyframes pop { from { transform: scale(.6); opacity: 0 } }
  @keyframes rise { from { transform: translateY(20px); opacity: 0 } }
  @keyframes out { to { opacity: 0; transform: scale(.96) } }
</style>
<div class="wrap"><div class="num"></div><div class="label">{{label}}</div></div>
<script>
  const num = document.querySelector('.num');
  const format = (v, decimals) => {
    const [whole, frac] = Math.abs(v).toFixed(decimals).split('.');
    const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return (v < 0 && Number(whole + (frac || '')) !== 0 ? '-' : '') + grouped + (frac ? '.' + frac : '');
  };
  window.kooRender = (t) => {
    const p = koo.params;
    const u = Math.min(1, Math.max(0, (t - 0.15) / Math.max(0.01, p.count_time)));
    const e = 1 - Math.pow(1 - u, 4);
    num.textContent = p.prefix + format(p.from + (p.to - p.from) * e, p.decimals) + p.suffix;
  };
</script>"#,
            vec![
                float("from", "From", -1e12, 1e12, 0.0),
                float("to", "To", -1e12, 1e12, 25000.0),
                int("decimals", "Decimals", 0, 4, 0),
                float("count_time", "Count time", 0.1, 60.0, 2.0),
                text("prefix", "Prefix", ""),
                text("suffix", "Suffix", "+"),
                text("label", "Label", "Happy customers"),
                color("color", "Color", WHITE),
                color("accent", "Accent", CORAL),
            ],
        ),
        html(
            "countdown",
            "Countdown",
            "Data",
            "Seconds counting down inside a depleting ring, then a final word.",
            r#"<style>
  .wrap { position: absolute; left: 50%; top: 50%; width: 560px; height: 560px; transform: translate(-50%, -50%);
          display: grid; place-items: center; color: var(--color);
          font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
          animation: in .5s cubic-bezier(.34, 1.56, .64, 1) both, out .4s ease-in forwards;
          animation-delay: 0s, calc(var(--duration) * 1s - .4s); }
  svg { position: absolute; inset: 0; transform: rotate(-90deg); filter: drop-shadow(0 6px 20px rgba(0, 0, 0, .35)); }
  circle { fill: none; stroke-width: 22; }
  .back { stroke: var(--track); }
  .arc { stroke: var(--accent); stroke-linecap: round; }
  .num { font-size: 230px; font-weight: 850; letter-spacing: -.04em; font-variant-numeric: tabular-nums;
         text-shadow: 0 6px 30px rgba(0, 0, 0, .35); }
  .num.long { font-size: 150px; }
  .num.end { font-size: 130px; letter-spacing: 0; text-transform: uppercase; }
  @keyframes in { from { transform: translate(-50%, -50%) scale(.5); opacity: 0 } }
  @keyframes out { to { transform: translate(-50%, -50%) scale(1.1); opacity: 0 } }
</style>
<div class="wrap">
  <svg viewBox="0 0 560 560"><circle class="back" cx="280" cy="280" r="260"/><circle class="arc" cx="280" cy="280" r="260"/></svg>
  <div class="num"></div>
</div>
<script>
  const num = document.querySelector('.num');
  const arc = document.querySelector('.arc');
  const C = 2 * Math.PI * 260;
  arc.style.strokeDasharray = C;
  window.kooRender = (t) => {
    const p = koo.params;
    const left = p.from - t;
    const whole = Math.ceil(left - 1e-6);
    if (left <= 0) {
      num.textContent = p.end_text;
      num.className = 'num end';
      arc.style.strokeDashoffset = C;
      const k = Math.min(1, -left / 0.35);
      num.style.transform = `scale(${0.6 + 0.4 * (1 - Math.pow(1 - k, 3))})`;
      return;
    }
    const m = Math.floor(whole / 60), s = whole % 60;
    num.textContent = m > 0 ? `${m}:${String(s).padStart(2, '0')}` : String(s);
    num.className = m > 0 ? 'num long' : 'num';
    arc.style.strokeDashoffset = C * (1 - (left - Math.floor(left - 1e-6)));
    const frac = whole - left; // 0 at the tick, rising to 1
    num.style.transform = `scale(${1 + 0.18 * Math.max(0, 1 - frac / 0.25) ** 2})`;
  };
</script>"#,
            vec![
                int("from", "From", 1, 3600, 5),
                text("end_text", "End", "Go!"),
                color("color", "Text", WHITE),
                color("accent", "Ring", CORAL),
                color("track", "Track", [1.0, 1.0, 1.0, 0.18]),
            ],
        ),
        html(
            "news_ticker",
            "News Ticker",
            "Data",
            "Bottom bar with a label and headlines scrolling across; one headline per line.",
            r#"<style>
  .bar { position: absolute; left: 0; right: 0; bottom: 70px; height: 92px; display: flex; color: var(--color);
         font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
         box-shadow: 0 10px 30px rgba(0, 0, 0, .3);
         animation: in .6s cubic-bezier(.16, 1, .3, 1) both, out .45s cubic-bezier(.7, 0, .84, 0) forwards;
         animation-delay: 0s, calc(var(--duration) * 1s - .45s); }
  .label { position: relative; z-index: 1; display: flex; align-items: center; padding: 0 44px; background: var(--accent);
           color: var(--label_text); font-size: 36px; font-weight: 850; letter-spacing: .12em; text-transform: uppercase;
           white-space: nowrap; clip-path: polygon(0 0, 100% 0, calc(100% - 26px) 100%, 0 100%); padding-right: 64px;
           animation: wipe .6s .2s cubic-bezier(.16, 1, .3, 1) both; }
  .feed { flex: 1; margin-left: -26px; overflow: hidden; background: var(--bar); display: flex; align-items: center; }
  .run { display: flex; white-space: nowrap; font-size: 40px; font-weight: 600; will-change: transform; }
  .run span::after { content: "\25C6"; margin: 0 40px; color: var(--accent); font-size: 26px; vertical-align: 4px; }
  @keyframes in { from { transform: translateY(170px) } }
  @keyframes wipe { from { clip-path: polygon(0 0, 0 0, 0 100%, 0 100%) } }
  @keyframes out { to { transform: translateY(170px) } }
</style>
<div class="bar"><div class="label">{{label}}</div><div class="feed"><div class="run"></div></div></div>
<script>
  const run = document.querySelector('.run');
  let shown = null;
  let width = 0;
  const build = (headlines) => {
    const items = String(headlines).split('\n').map((s) => s.trim()).filter(Boolean);
    const group = () => {
      const g = document.createElement('div');
      g.style.display = 'flex';
      for (const text of items) {
        const s = document.createElement('span');
        s.textContent = text;
        g.append(s);
      }
      return g;
    };
    run.textContent = '';
    const first = group();
    run.append(first);
    width = first.getBoundingClientRect().width || 1;
    const copies = Math.ceil(document.body.clientWidth / width) + 1;
    for (let i = 0; i < copies; i++) run.append(group());
    shown = headlines;
  };
  window.kooRender = (t) => {
    if (koo.params.headlines !== shown) build(koo.params.headlines);
    const x = (Math.max(0, t - 0.4) * koo.params.speed) % width;
    run.style.transform = `translateX(${-x}px)`;
  };
</script>"#,
            vec![
                text("label", "Label", "Breaking"),
                text(
                    "headlines",
                    "Headlines",
                    "Markets rally as tech stocks hit record highs\nCity council approves new riverside park\nLocal team clinches the championship in overtime",
                ),
                float("speed", "Speed", 20.0, 800.0, 180.0),
                color("accent", "Accent", RED),
                color("label_text", "Label text", WHITE),
                color("bar", "Bar", [0.06, 0.07, 0.1, 0.9]),
                color("color", "Text", WHITE),
            ],
        ),
        // ---- images
        html(
            "image_showcase",
            "Image Showcase",
            "Images",
            "Full-frame picture with a slow Ken Burns zoom toward a focus point and an optional caption.",
            r#"<style>
  .stage { position: absolute; inset: 0; overflow: hidden;
           background: radial-gradient(120% 120% at 30% 20%, #3a4a6b 0%, #1b2233 55%, #0d1018 100%);
           animation: in .7s ease-out both, out .7s ease-in forwards; animation-delay: 0s, calc(var(--duration) * 1s - .7s); }
  .stage img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover;
               transform-origin: calc(var(--focus-x) * 100%) calc(var(--focus-y) * 100%);
               animation: kb calc(var(--duration) * 1s) cubic-bezier(.3, 0, .7, 1) both; }
  .stage img[src=""] { display: none; }
  [data-fit="Fit"] img { object-fit: contain; }
  [data-move="Zoom out"] img { animation-direction: reverse; }
  [data-move="Still"] img { animation: none; }
  .caption { position: absolute; left: 0; right: 0; bottom: 0; padding: 140px 120px 90px; color: #fff;
             background: linear-gradient(transparent, rgba(0, 0, 0, .7));
             font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
             font-size: 54px; font-weight: 650; letter-spacing: -.01em; text-shadow: 0 2px 16px rgba(0, 0, 0, .4);
             animation: rise .9s .5s cubic-bezier(.16, 1, .3, 1) both; }
  .caption:empty { display: none; }
  @keyframes kb { from { transform: scale(1) } to { transform: scale(var(--zoom)) } }
  @keyframes in { from { opacity: 0 } }
  @keyframes out { to { opacity: 0 } }
  @keyframes rise { from { transform: translateY(30px); opacity: 0 } }
</style>
<div class="stage" data-move="{{move}}" data-fit="{{fit}}"><img src="{{image}}" alt=""><div class="caption">{{caption}}</div></div>"#,
            vec![
                image("image", "Image"),
                text("caption", "Caption", ""),
                choice("move", "Move", &["Zoom in", "Zoom out", "Still"], "Zoom in"),
                float("zoom", "Zoom", 1.0, 2.0, 1.2),
                point("focus", "Focus", [0.5, 0.5]),
                choice("fit", "Fit", &["Fill", "Fit"], "Fill"),
            ],
        ),
    ]
}
