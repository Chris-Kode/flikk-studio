//! Built-in HTML generators: titles, lower thirds and other motion graphics.
//! See the module docs of [`super`] for how time and parameters reach the
//! page.
//!
//! Every template lays out on a page with a 1080 CSS px short side (1920 ×
//! 1080 landscape, 1080 × 1920 portrait; bottom-anchored ones rise above
//! the controls vertical apps draw over the bottom), with system fonts only,
//! keeps its background transparent (full-screen cards take a `background`
//! color), and animates with GSAP in `kooTimeline`, timing its exit against
//! `koo.duration`, so it ends cleanly however long the clip is.

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
        origin: None,
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
        font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  .bar { width: 10px; border-radius: 5px; background: var(--accent); transform-origin: 50% 100%; }
  .panel { margin-left: 14px; padding: 20px 40px 22px 30px; border-radius: 6px;
           background: linear-gradient(90deg, rgba(12, 14, 20, .88), rgba(12, 14, 20, .72));
           box-shadow: 0 12px 40px rgba(0, 0, 0, .25); }
  .name { color: #fff; font-size: 64px; font-weight: 750; letter-spacing: -.01em; line-height: 1.05; }
  .role { color: rgba(255, 255, 255, .78); font-size: 30px; font-weight: 600; letter-spacing: .1em;
          text-transform: uppercase; margin-top: 12px; }
  @media (orientation: portrait) { .lt { bottom: 440px; } }
</style>
<div class="lt"><div class="bar"></div><div class="panel"><div class="name">{{name}}</div><div class="role">{{role}}</div></div></div>
<script>
  window.kooTimeline = (koo) => {
    gsap.timeline({ defaults: { ease: 'expo.out' } })
      .from('.bar', { scaleY: 0, duration: .5 })
      .fromTo('.panel', { clipPath: 'inset(0 100% 0 0)' }, { clipPath: 'inset(0 0% 0 0)', duration: .75 }, .12)
      .from('.name, .role', { y: 24, opacity: 0, duration: .7, stagger: .12 }, .3)
      .to('.lt', { x: -48, opacity: 0, duration: .5, ease: 'expo.in' }, koo.duration - .5);
  };
</script>"#,
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
        text-shadow: 0 2px 16px rgba(0, 0, 0, .45); }
  [data-side="Left"] { left: 120px; }
  [data-side="Right"] { right: 120px; text-align: right; }
  .clip { overflow: hidden; padding: 0 4px; }
  .name { display: inline-block; font-size: 60px; font-weight: 300; letter-spacing: .01em; }
  .line { height: 3px; margin: 14px 4px 16px; border-radius: 2px; background: var(--accent);
          box-shadow: 0 1px 8px rgba(0, 0, 0, .25); transform-origin: left; }
  [data-side="Right"] .line { transform-origin: right; }
  .role { display: inline-block; font-size: 26px; font-weight: 600; letter-spacing: .3em;
          text-transform: uppercase; opacity: .85; }
  @media (orientation: portrait) { .lt { bottom: 440px; } }
</style>
<div class="lt" data-side="{{side}}">
  <div class="clip"><span class="name">{{name}}</span></div>
  <div class="line"></div>
  <div class="clip"><span class="role">{{role}}</span></div>
</div>
<script>
  window.kooTimeline = (koo) => {
    gsap.timeline({ defaults: { duration: .9, ease: 'expo.out' } })
      .from('.line', { scaleX: 0, ease: 'power2.inOut' })
      .from('.name', { yPercent: 110 }, .25)
      .from('.role', { yPercent: -120 }, .35)
      .to('.lt', { y: 8, opacity: 0, filter: 'blur(8px)', duration: .6, ease: 'power1.in' }, koo.duration - .6);
  };
</script>"#,
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
        transform-origin: 0 50%; }
  .stripe { position: absolute; left: 0; right: 0; bottom: 0; height: 6px; background: var(--accent); transform-origin: left; }
  .logo { width: 96px; height: 96px; object-fit: contain; border-radius: 10px; }
  .logo[src=""] { display: none; }
  .name { font-size: 54px; font-weight: 800; letter-spacing: -.015em; line-height: 1.05; }
  .role { font-size: 28px; font-weight: 500; opacity: .7; margin-top: 8px; }
  @media (orientation: portrait) { .lt { bottom: 440px; } }
</style>
<div class="lt"><img class="logo" src="{{logo}}" alt=""><div class="text"><div class="name">{{name}}</div><div class="role">{{role}}</div></div><div class="stripe"></div></div>
<script>
  window.kooTimeline = (koo) => {
    gsap.timeline()
      .from('.lt', { scaleX: .2, opacity: 0, duration: .7, ease: 'back.out(1.2)' })
      .from('.logo', { scale: .4, opacity: 0, duration: .6, ease: 'back.out(1.7)' }, .3)
      .from('.stripe', { scaleX: 0, duration: .8, ease: 'power2.inOut' }, .35)
      .from('.text', { x: -16, opacity: 0, duration: .6, ease: 'expo.out' }, .38)
      .to('.lt', { y: 30, opacity: 0, duration: .45, ease: 'expo.in' }, koo.duration - .45);
  };
</script>"#,
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
          text-shadow: 0 4px 30px rgba(0, 0, 0, .35); }
  h1 { margin: 0 -.3em 0 0; font-size: 104px; font-weight: 300; letter-spacing: .3em; text-transform: uppercase;
       line-height: 1.1; }
  .sub { display: flex; align-items: center; gap: 32px; margin-top: 40px; font-size: 28px; font-weight: 500;
         letter-spacing: .42em; text-transform: uppercase; opacity: .85; }
  .sub span { margin-right: -.42em; }
  .sub i { width: 120px; height: 1px; background: currentColor; }
</style>
<div class="wrap"><h1>{{title}}</h1><div class="sub"><i></i><span>{{subtitle}}</span><i></i></div></div>
<script>
  window.kooTimeline = (koo) => {
    gsap.timeline()
      .fromTo('h1', { letterSpacing: '0.6em', marginRight: '-0.6em' },
              { letterSpacing: '0.3em', marginRight: '-0.3em', duration: koo.duration, ease: 'power2.out' })
      .from('h1', { opacity: 0, filter: 'blur(16px)', duration: 1.6, ease: 'power1.out' }, 0)
      .from('.sub i', { scaleX: 0, duration: 1.4, ease: 'expo.out' }, .7)
      .from('.sub', { opacity: 0, duration: 1.4, ease: 'power1.out' }, .8)
      .to('.wrap', { opacity: 0, filter: 'blur(10px)', duration: .9, ease: 'power1.in' }, koo.duration - .9);
  };
</script>"#,
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
         font-size: 34px; font-weight: 800; letter-spacing: .14em; text-transform: uppercase; }
  .tag span { display: inline-block; transform: skewX(10deg); }
  .tag:empty { display: none; }
  .words { max-width: 1600px; margin: 0; color: var(--color); font-size: 150px; font-weight: 900; font-style: italic;
           line-height: .98; letter-spacing: -.02em; text-transform: uppercase;
           text-shadow: 0 10px 40px rgba(0, 0, 0, .35); }
  .word { margin: 0 .12em; }
</style>
<div class="wrap"><div class="tag"><span>{{tag}}</span></div><h1 class="words">{{title}}</h1></div>
<script>
  const tag = document.querySelector('.tag');
  if (!tag.textContent.trim()) tag.textContent = '';
  window.kooTimeline = (koo) => {
    // One element per word, staggered in and out.
    const words = SplitText.create('.words', { type: 'words', wordsClass: 'word' }).words;
    gsap.timeline()
      .fromTo('.tag', { skewX: -10, y: 30, clipPath: 'inset(0 100% 0 0)' },
              { skewX: -10, y: 0, clipPath: 'inset(0 0% 0 0)', duration: .55, ease: 'expo.out' })
      .from(words, { scale: 1.8, duration: .5, ease: 'back.out(1.2)', stagger: .11 }, .25)
      .from(words, { opacity: 0, filter: 'blur(12px)', duration: .5, ease: 'power3.out', stagger: .11 }, .25)
      .to('.tag', { y: -20, opacity: 0, duration: .4, ease: 'power1.in' }, koo.duration - .4)
      .to(words, { y: -80, rotation: -4, opacity: 0, duration: .45, ease: 'expo.in',
                   stagger: (i) => Math.min(i, 6) * .04 }, koo.duration - .5);
  };
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
           background: linear-gradient(90deg, rgba(0, 0, 0, .85) 0%, rgba(0, 0, 0, .4) 45%, transparent 75%); }
  .ch { position: absolute; left: 150px; top: 0; bottom: 0; display: flex; flex-direction: column; justify-content: center;
        color: var(--color); font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  .kicker { display: flex; align-items: center; gap: 22px; color: var(--accent); font-size: 30px; font-weight: 700;
            letter-spacing: .32em; text-transform: uppercase; }
  .kicker i { width: 72px; height: 4px; border-radius: 2px; background: currentColor; transform-origin: left; }
  .clip { overflow: hidden; padding-bottom: 6px; }
  .kicker span { display: inline-block; }
  h1 { margin: 18px 0 0; font-size: 112px; font-weight: 800; letter-spacing: -.025em; line-height: 1.02; max-width: 1300px; }
</style>
<div class="shade"></div>
<div class="ch"><div class="kicker clip"><i></i><span>Chapter {{number}}</span></div><div class="clip"><h1>{{title}}</h1></div></div>
<script>
  window.kooTimeline = (koo) => {
    gsap.timeline()
      .from('.shade', { opacity: 0, duration: .8, ease: 'power1.out' })
      .from('.kicker i', { scaleX: 0, duration: .8, ease: 'power2.inOut' }, .2)
      .from('.kicker span', { yPercent: 105, duration: .8, ease: 'expo.out' }, .35)
      .from('h1', { yPercent: 105, duration: .9, ease: 'expo.out' }, .5)
      .to('.ch', { x: -40, opacity: 0, duration: .6, ease: 'expo.in' }, koo.duration - .6)
      .to('.shade', { opacity: 0, duration: .7, ease: 'power1.in' }, koo.duration - .7);
  };
</script>"#,
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
          padding: 0 260px; color: var(--color); }
  .mark { height: 150px; margin-bottom: 10px; color: var(--accent); font: 300px/1 Georgia, "Times New Roman", serif; }
  blockquote { margin: 0; font: italic 400 72px/1.25 Georgia, "Times New Roman", serif; letter-spacing: -.005em; }
  .author { display: flex; align-items: center; gap: 24px; margin-top: 56px; font-size: 32px; font-weight: 600;
            letter-spacing: .14em; text-transform: uppercase; opacity: .8;
            font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  .author::before { content: ""; width: 64px; height: 3px; background: var(--accent); }
</style>
<div class="card"><div class="mark">&ldquo;</div><blockquote>{{quote}}</blockquote><div class="author">{{author}}</div></div>
<script>
  window.kooTimeline = (koo) => {
    // Words appear in reading order across about 1.6 s.
    const words = SplitText.create('blockquote', { type: 'words' }).words;
    const step = Math.min(.12, 1.6 / Math.max(1, words.length));
    gsap.timeline()
      .from('.mark', { scale: .4, rotation: -12, opacity: 0, duration: .7, ease: 'back.out(1.7)' })
      .from(words, { y: 26, opacity: 0, filter: 'blur(6px)', duration: .6, ease: 'expo.out', stagger: step }, .35)
      .from('.author', { y: 12, opacity: 0, duration: .8, ease: 'power1.out' }, .6 + words.length * step)
      .to('.card', { y: -24, opacity: 0, duration: .7, ease: 'power1.in' }, koo.duration - .7);
  };
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
            font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  .ring, .pulse { position: absolute; left: calc(var(--size) * -.5px); top: calc(var(--size) * -.5px);
                  width: calc(var(--size) * 1px); height: calc(var(--size) * 1px); box-sizing: border-box;
                  border-radius: 50%; border: 6px solid var(--accent); }
  .ring { box-shadow: 0 0 0 2px rgba(0, 0, 0, .15), 0 6px 24px rgba(0, 0, 0, .3); }
  .pulse { opacity: 0; }
  svg { position: absolute; left: -500px; top: -500px; overflow: visible; }
  [data-dir="Up right"] svg { transform: scaleX(-1); }
  [data-dir="Down left"] svg { transform: scaleY(-1); }
  [data-dir="Down right"] svg { transform: scale(-1, -1); }
  .stroke { fill: none; stroke: var(--accent); stroke-width: 7; stroke-linecap: round; stroke-linejoin: round;
            filter: drop-shadow(0 3px 6px rgba(0, 0, 0, .3)); }
  .label { position: absolute; padding: 16px 30px; border-radius: 999px; white-space: nowrap; background: var(--accent);
           color: var(--ink); font-size: 40px; font-weight: 750; box-shadow: 0 10px 30px rgba(0, 0, 0, .3); }
  [data-dir="Up left"] .label { right: 300px; bottom: 210px; }
  [data-dir="Up right"] .label { left: 300px; bottom: 210px; }
  [data-dir="Down left"] .label { right: 300px; top: 210px; }
  [data-dir="Down right"] .label { left: 300px; top: 210px; }
</style>
<div class="anchor" data-dir="{{direction}}">
  <div class="pulse"></div><div class="ring"></div>
  <svg width="1000" height="1000" viewBox="-500 -500 1000 1000">
    <path class="stroke path" d="M -290 -215 C -200 -215 -110 -170 -78 -82" />
    <path class="stroke head" d="M -104 -100 L -76 -76 L -60 -110" />
  </svg>
  <div class="label">{{label}}</div>
</div>
<script>
  window.kooTimeline = (koo) => {
    gsap.timeline({ defaults: { ease: 'back.out(1.7)' } })
      .from('.ring', { scale: 0, opacity: 0, duration: .6 })
      .from('.label', { scale: .5, opacity: 0, duration: .5 }, .15)
      .from('.path', { drawSVG: 0, duration: .6, ease: 'power2.inOut' }, .35)
      .from('.head', { opacity: 0, duration: .15, ease: 'none' }, .9)
      .fromTo('.pulse', { scale: 1, opacity: .8 },
              { scale: 1.7, opacity: 0, duration: 1.6, ease: 'power1.out', repeat: -1, immediateRender: false }, .6)
      .to('.anchor', { scale: .92, opacity: 0, duration: .45, ease: 'power1.in' }, koo.duration - .45);
  };
</script>"#,
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
  .spot { position: absolute; font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  [data-at="Bottom center"] { left: 50%; bottom: 140px; transform: translateX(-50%); }
  [data-at="Bottom left"] { left: 120px; bottom: 140px; }
  [data-at="Bottom right"] { right: 120px; bottom: 140px; }
  [data-at="Center"] { left: 50%; top: 50%; transform: translate(-50%, -50%); }
  .btn { position: relative; display: grid; border-radius: 999px; color: #fff; font-size: 46px; font-weight: 800;
         letter-spacing: .06em; text-transform: uppercase; box-shadow: 0 14px 40px rgba(0, 0, 0, .35);
         background: var(--color); }
  .face { grid-area: 1 / 1; display: flex; align-items: center; justify-content: center; gap: 22px; padding: 26px 56px; white-space: nowrap; }
  .face svg { width: 50px; height: 50px; fill: currentColor; }
  .after { opacity: 0; }
  .after svg { transform-origin: 50% 8%; }
  .cursor { position: absolute; right: 18%; bottom: -70px; width: 64px; height: 64px;
            filter: drop-shadow(0 4px 8px rgba(0, 0, 0, .45)); }
  @media (orientation: portrait) {
    [data-at^="Bottom"] { bottom: 440px; }
    [data-at="Bottom right"] { right: 170px; }
  }
</style>
<div class="spot" data-at="{{position}}">
  <div class="btn">
    <div class="face before"><svg viewBox="0 0 24 24"><path d="M21.6 7.2a2.6 2.6 0 0 0-1.8-1.8C18.2 5 12 5 12 5s-6.2 0-7.8.4A2.6 2.6 0 0 0 2.4 7.2 27 27 0 0 0 2 12a27 27 0 0 0 .4 4.8 2.6 2.6 0 0 0 1.8 1.8C5.8 19 12 19 12 19s6.2 0 7.8-.4a2.6 2.6 0 0 0 1.8-1.8A27 27 0 0 0 22 12a27 27 0 0 0-.4-4.8zM10 15V9l5.2 3z"/></svg>{{label}}</div>
    <div class="face after"><svg viewBox="0 0 24 24"><path d="M12 22a2.5 2.5 0 0 0 2.5-2.5h-5A2.5 2.5 0 0 0 12 22zm7-6V11c0-3.1-1.6-5.6-4.5-6.3V4a2.5 2.5 0 0 0-5 0v.7C6.6 5.4 5 7.9 5 11v5l-2 2v1h18v-1z"/></svg>{{done_label}}</div>
  </div>
  <svg class="cursor" viewBox="0 0 24 24"><path d="M5 2.5v17.2l4.6-4.3 2.9 6.6 3-1.3-2.9-6.5H19z" fill="white" stroke="rgb(17, 17, 17)" stroke-width="1.3" stroke-linejoin="round"/></svg>
</div>
<script>
  window.kooTimeline = (koo) => {
    const press = { duration: .15, ease: 'power1.inOut', yoyo: true, repeat: 1 };
    gsap.timeline()
      .from('.btn', { scale: .3, opacity: 0, duration: .6, ease: 'back.out(1.7)' }, .1)
      .from('.cursor', { x: 220, y: 160, opacity: 0, duration: .8, ease: 'expo.out' }, .45)
      .to('.btn', { scale: .92, ...press }, 1.3)
      .to('.cursor', { scale: .82, ...press }, 1.3)
      .to('.btn', { backgroundColor: '#3b3b3f', duration: .2 }, 1.45)
      .to('.before', { opacity: 0, duration: .2 }, 1.45)
      .to('.after', { opacity: 1, duration: .2 }, 1.45)
      .to('.after svg', {
        keyframes: { '15%': { rotation: 22 }, '30%': { rotation: -18 }, '45%': { rotation: 12 },
                     '60%': { rotation: -8 }, '75%': { rotation: 4 }, '100%': { rotation: 0 }, easeEach: 'power1.inOut' },
        duration: .9, ease: 'none' }, 1.75)
      .to('.cursor', { x: 140, y: 140, opacity: 0, duration: .5, ease: 'power1.in' }, 2.1)
      .to('.spot', { opacity: 0, filter: 'blur(6px)', duration: .45, ease: 'expo.in' }, koo.duration - .45);
  };
</script>"#,
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
           font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  [data-at="Bottom left"] { left: 100px; bottom: 100px; }
  [data-at="Bottom right"] { right: 100px; bottom: 100px; }
  [data-at="Top left"] { left: 100px; top: 100px; }
  [data-at="Top right"] { right: 100px; top: 100px; }
  .icon { display: grid; place-items: center; width: 84px; height: 84px; border-radius: 50%; background: var(--accent); }
  .icon svg { display: none; width: 48px; height: 48px; }
  [data-platform="YouTube"] .yt, [data-platform="Instagram"] .ig, [data-platform="X"] .x,
  [data-platform="Website"] .web, [data-platform="Email"] .mail { display: block; }
  .handle { font-size: 44px; font-weight: 700; letter-spacing: -.005em; white-space: nowrap; }
  @media (orientation: portrait) {
    [data-at^="Bottom"] { bottom: 440px; }
    [data-at="Bottom right"] { right: 170px; }
  }
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
</div>
<script>
  window.kooTimeline = (koo) => {
    const dx = koo.params.position.endsWith('left') ? -140 : 140;
    gsap.timeline()
      .from('.badge', { x: dx, opacity: 0, duration: .7, ease: 'expo.out' })
      .from('.icon', { rotation: -180, scale: .3, duration: .8, ease: 'back.out(1.7)' }, .15)
      .from('.handle', { x: -12, opacity: 0, duration: .6, ease: 'power1.out' }, .3)
      .to('.badge', { x: dx, opacity: 0, duration: .45, ease: 'expo.in' }, koo.duration - .45);
  };
</script>"#,
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
           border-radius: calc(var(--thickness) * 1px); background: var(--track); overflow: hidden; }
  [data-at="Bottom"] { bottom: var(--inset); }
  [data-at="Top"] { top: var(--inset); }
  .fill { height: 100%; width: 100%; border-radius: inherit; background: var(--fill); transform-origin: left; }
  :root { --inset: calc(var(--margin) * 1px); }
</style>
<div class="track" data-at="{{position}}"><div class="fill"></div></div>
<script>
  window.kooTimeline = (koo) => {
    gsap.timeline()
      .from('.track', { opacity: 0, duration: .4, ease: 'power1.out' })
      .fromTo('.fill', { scaleX: 0 }, { scaleX: 1, duration: koo.duration, ease: 'none' }, 0)
      .to('.track', { opacity: 0, duration: .4, ease: 'power1.in' }, koo.duration - .4);
  };
</script>"#,
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
          font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  .num { font-size: 220px; font-weight: 850; letter-spacing: -.03em; line-height: 1; font-variant-numeric: tabular-nums; }
  .label { display: flex; align-items: center; gap: 24px; margin-top: 26px; font-size: 40px; font-weight: 600;
           letter-spacing: .16em; text-transform: uppercase; opacity: .85; }
  .label::before, .label::after { content: ""; width: 56px; height: 4px; border-radius: 2px; background: var(--accent); }
  .label:empty { display: none; }
</style>
<div class="wrap"><div class="num"></div><div class="label">{{label}}</div></div>
<script>
  const num = document.querySelector('.num');
  const format = (v, decimals) => {
    const [whole, frac] = Math.abs(v).toFixed(decimals).split('.');
    const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return (v < 0 && Number(whole + (frac || '')) !== 0 ? '-' : '') + grouped + (frac ? '.' + frac : '');
  };
  window.kooTimeline = (koo) => {
    const p = koo.params;
    const count = { value: p.from };
    const show = () => { num.textContent = p.prefix + format(count.value, p.decimals) + p.suffix; };
    show();
    gsap.timeline()
      .from('.num', { scale: .6, opacity: 0, duration: .6, ease: 'back.out(1.7)' })
      .to(count, { value: p.to, duration: p.count_time, ease: 'power3.out', onUpdate: show }, .15)
      .from('.label', { y: 20, opacity: 0, duration: .7, ease: 'expo.out' }, .25)
      .to('.wrap', { scale: .96, opacity: 0, duration: .5, ease: 'power1.in' }, koo.duration - .5);
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
  .wrap { position: absolute; inset: 0; width: 560px; height: 560px; margin: auto;
          display: grid; place-items: center; color: var(--color);
          font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
  svg { position: absolute; inset: 0; transform: rotate(-90deg); filter: drop-shadow(0 6px 20px rgba(0, 0, 0, .35)); }
  circle { fill: none; stroke-width: 22; }
  .back { stroke: var(--track); }
  .arc { stroke: var(--accent); stroke-linecap: round; }
  .num { font-size: 230px; font-weight: 850; letter-spacing: -.04em; font-variant-numeric: tabular-nums;
         text-shadow: 0 6px 30px rgba(0, 0, 0, .35); }
  .num.long { font-size: 150px; }
  .num.end { font-size: 130px; letter-spacing: 0; text-transform: uppercase; }
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
  const label = (s) => {
    const m = Math.floor(s / 60);
    return m > 0 ? `${m}:${String(s % 60).padStart(2, '0')}` : String(s);
  };
  window.kooTimeline = (koo) => {
    const p = koo.params;
    const set = gsap.quickSetter(num, 'css');
    // One clock for the seconds: each starts with the ring full and the number popping.
    const clock = { left: p.from };
    const draw = () => {
      const left = clock.left;
      if (left <= 0) {
        num.textContent = p.end_text;
        num.className = 'num end';
        arc.style.strokeDashoffset = C;
        return;
      }
      const whole = Math.ceil(left - 1e-6);
      num.textContent = label(whole);
      num.className = whole >= 60 ? 'num long' : 'num';
      arc.style.strokeDashoffset = C * (1 - (left - Math.floor(left - 1e-6)));
      set({ scale: 1 + .18 * Math.max(0, 1 - (whole - left) / .25) ** 2 });
    };
    gsap.timeline()
      .from('.wrap', { scale: .5, opacity: 0, duration: .5, ease: 'back.out(1.7)' })
      .to(clock, { left: 0, duration: p.from, ease: 'none', onUpdate: draw }, 0)
      .fromTo(num, { scale: .6 }, { scale: 1, duration: .35, ease: 'power2.out', immediateRender: false }, p.from)
      .to('.wrap', { scale: 1.1, opacity: 0, duration: .4, ease: 'power1.in' }, koo.duration - .4);
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
         box-shadow: 0 10px 30px rgba(0, 0, 0, .3); }
  .tab { position: relative; z-index: 1; display: flex; }
  .label { display: flex; align-items: center; padding: 0 64px 0 44px; background: var(--accent);
           color: var(--label_text); font-size: 36px; font-weight: 850; letter-spacing: .12em; text-transform: uppercase;
           white-space: nowrap; clip-path: polygon(0 0, 100% 0, calc(100% - 26px) 100%, 0 100%); }
  .feed { flex: 1; margin-left: -26px; overflow: hidden; background: var(--bar); display: flex; align-items: center; }
  .run { display: flex; white-space: nowrap; font-size: 40px; font-weight: 600; }
  .run span::after { content: "\25C6"; margin: 0 40px; color: var(--accent); font-size: 26px; vertical-align: 4px; }
  @media (orientation: portrait) { .bar { bottom: 440px; } }
</style>
<div class="bar"><div class="tab"><div class="label">{{label}}</div></div><div class="feed"><div class="run"></div></div></div>
<script>
  const run = document.querySelector('.run');
  window.kooTimeline = (koo) => {
    const items = String(koo.params.headlines).split('\n').map((s) => s.trim()).filter(Boolean);
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
    const width = first.getBoundingClientRect().width || 1;
    const copies = Math.ceil(document.body.clientWidth / width) + 1;
    for (let i = 0; i < copies; i++) run.append(group());
    gsap.timeline()
      .from('.bar', { y: 170, duration: .6, ease: 'expo.out' })
      .from('.tab', { clipPath: 'inset(0 100% 0 0)', duration: .6, ease: 'expo.out' }, .2)
      .to(run, { x: -width, duration: width / koo.params.speed, ease: 'none', repeat: -1 }, .4)
      .to('.bar', { y: 170, duration: .45, ease: 'expo.in' }, koo.duration - .45);
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
           background: radial-gradient(120% 120% at 30% 20%, #3a4a6b 0%, #1b2233 55%, #0d1018 100%); }
  .stage img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover;
               transform-origin: calc(var(--focus-x) * 100%) calc(var(--focus-y) * 100%); }
  .stage img[src=""] { display: none; }
  [data-fit="Fit"] img { object-fit: contain; }
  .caption { position: absolute; left: 0; right: 0; bottom: 0; padding: 140px 120px 90px; color: #fff;
             background: linear-gradient(transparent, rgba(0, 0, 0, .7));
             font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
             font-size: 54px; font-weight: 650; letter-spacing: -.01em; text-shadow: 0 2px 16px rgba(0, 0, 0, .4); }
  .caption:empty { display: none; }
</style>
<div class="stage" data-fit="{{fit}}"><img src="{{image}}" alt=""><div class="caption">{{caption}}</div></div>
<script>
  window.kooTimeline = (koo) => {
    const { move, zoom } = koo.params;
    const tl = gsap.timeline()
      .from('.stage', { opacity: 0, duration: .7, ease: 'power1.out' })
      .from('.caption', { y: 30, opacity: 0, duration: .9, ease: 'expo.out' }, .5)
      .to('.stage', { opacity: 0, duration: .7, ease: 'power1.in' }, koo.duration - .7);
    if (move !== 'Still') {
      const [from, to] = move === 'Zoom out' ? [zoom, 1] : [1, zoom];
      tl.fromTo('.stage img', { scale: from }, { scale: to, duration: koo.duration, ease: 'sine.inOut' }, 0);
    }
  };
</script>"#,
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
