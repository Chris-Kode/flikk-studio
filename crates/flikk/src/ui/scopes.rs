//! Video scopes for the Color workspace: luma waveform, RGB parade,
//! vectorscope (with the skin-tone line) and histogram of the frame at the
//! playhead. The frame is the topmost visible media clip's, graded with its
//! clip grade and then the timeline grade through `flikk_core::grade`, so the
//! scopes read exactly what the viewer shows and the export renders. They
//! are drawn off the UI thread; the latest request wins, and playback
//! refreshes them a few times a second.

use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{EmptyState, IconName, Panel, PanelHeader, Segmented, h_flex, v_flex};
use flikk_core::grade::{self, LUMA};
use flikk_core::model::{ClipContent, Grade, MediaItem, MediaKind, Project, TrackKind};
use flikk_core::services::Frame;
use flikk_core::{ClipId, Time};
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Width of the frames the scopes read.
const FRAME_WIDTH: u32 = 480;
/// Least time between two refreshes during playback.
const PLAYING_EVERY: Duration = Duration::from_millis(200);
/// Height of a scope's title row, and the gap between scopes.
const TITLE: f32 = 22.;
const GAP: f32 = 8.;
/// Narrowest scope whose title row fits its readout.
const NARROW: f32 = 240.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    All,
    Waveform,
    Parade,
    Vector,
    Histogram,
}

const MODES: [(Mode, &str); 5] = [
    (Mode::All, "All"),
    (Mode::Waveform, "Waveform"),
    (Mode::Parade, "Parade"),
    (Mode::Vector, "Vector"),
    (Mode::Histogram, "Histogram"),
];

/// The scopes in drawing order, as indices into `Scopes::images`.
const WAVEFORM: usize = 0;
const PARADE: usize = 1;
const VECTOR: usize = 2;
const HISTOGRAM: usize = 3;

/// What the scopes show: a clip's source frame through two grades.
#[derive(Clone, PartialEq)]
struct Request {
    clip: ClipId,
    item: MediaItem,
    source: Time,
    grade: Grade,
    timeline: Grade,
    /// The vectorscope magnified 2×.
    zoom: bool,
}

/// A level range in percent, the faintest tails left out.
type Range = (u32, u32);

/// The four scopes as images, and the levels the frame spans.
#[derive(Clone)]
struct Scopes {
    images: [Arc<RenderImage>; 4],
    luma: Range,
    rgb: [Range; 3],
    zoom: bool,
}

pub struct ScopesPanel {
    state: Entity<EditorState>,
    mode: Mode,
    zoom: bool,
    scopes: Option<Scopes>,
    requested: Option<Request>,
    generation: u64,
    busy: bool,
    last: Option<Instant>,
    timer: bool,
    /// The body's size at the last paint, which "All" lays out to.
    body: Size<Pixels>,
    /// Images replaced since the last paint, to free from the sprite atlas.
    retired: Vec<Arc<RenderImage>>,
    _observe: Subscription,
}

impl ScopesPanel {
    pub fn new(state: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            mode: Mode::All,
            zoom: false,
            scopes: None,
            requested: None,
            generation: 0,
            busy: false,
            last: None,
            timer: false,
            body: Size::default(),
            retired: Vec::new(),
            _observe,
        }
    }

    fn show(&mut self, scopes: Option<Scopes>) {
        if let Some(old) = std::mem::replace(&mut self.scopes, scopes) {
            self.retired.extend(old.images);
        }
    }

    /// The topmost visible media clip at the playhead, as the viewer shows
    /// it (a dragged control included).
    fn current(&self, cx: &App) -> Option<Request> {
        let state = self.state.read(cx);
        let project: &Project = state.display_project();
        let t = state.playhead;
        project
            .timeline
            .tracks
            .iter()
            .filter(|tr| matches!(tr.kind, TrackKind::Video | TrackKind::Overlay) && !tr.mix.mute)
            .filter_map(|tr| tr.clip_at(t))
            .find_map(|clip| {
                let ClipContent::Media { media, .. } = &clip.content else {
                    return None;
                };
                let item = project
                    .media(*media)
                    .filter(|m| m.kind != MediaKind::Audio)?;
                let rate = item
                    .info
                    .as_ref()
                    .and_then(|i| i.video.as_ref())
                    .map(|v| v.rate)
                    .unwrap_or(project.timeline.settings.rate);
                Some(Request {
                    clip: clip.id,
                    item: item.clone(),
                    source: clip.to_source(t)?.floor_to_frame(rate),
                    grade: clip.grade.clone(),
                    timeline: project.timeline.grade.clone(),
                    zoom: self.zoom,
                })
            })
    }

    fn request(&mut self, cx: &mut Context<Self>) {
        let request = self.current(cx);
        if request == self.requested || self.busy {
            return;
        }
        let Some(request) = request else {
            self.requested = None;
            self.show(None);
            return;
        };
        let Some(media) = self.state.read(cx).services.media.clone() else {
            return;
        };
        // During playback, refresh a few times a second.
        if self.state.read(cx).playing
            && let Some(wait) = self
                .last
                .map(|l| PLAYING_EVERY.saturating_sub(l.elapsed()))
                .filter(|w| !w.is_zero())
        {
            if !self.timer {
                self.timer = true;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |this, cx| {
                        this.timer = false;
                        cx.notify();
                    });
                })
                .detach();
            }
            return;
        }
        self.requested = Some(request.clone());
        self.generation += 1;
        self.busy = true;
        self.last = Some(Instant::now());
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let scopes = cx
                .background_executor()
                .spawn(async move {
                    let frame = media
                        .frame(&request.item, request.source, FRAME_WIDTH)
                        .ok()?;
                    let frame = grade::graded(&frame, &[&request.grade, &request.timeline]);
                    draw(&frame, request.zoom)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.generation == generation {
                    this.show(scopes);
                }
                // A newer request may be waiting.
                cx.notify();
            });
        })
        .detach();
    }

    /// The scopes `mode` shows, as rows of indices: "All" takes the grid
    /// (one, two or four columns) that shows them largest. Also whether
    /// the cells are too narrow for the titles' readouts.
    fn grid(&self) -> (Vec<Vec<usize>>, bool) {
        let single = |i| (vec![vec![i]], false);
        match self.mode {
            Mode::Waveform => single(WAVEFORM),
            Mode::Parade => single(PARADE),
            Mode::Vector => single(VECTOR),
            Mode::Histogram => single(HISTOGRAM),
            Mode::All => {
                let (w, h) = (f32::from(self.body.width), f32::from(self.body.height));
                let width = |cols: usize| (w - GAP * (cols - 1) as f32) / cols as f32;
                let scale = |cols: usize| {
                    let rows = 4 / cols;
                    let ch = (h - GAP * (rows - 1) as f32) / rows as f32 - TITLE;
                    SIZES
                        .iter()
                        .map(|(iw, ih)| (width(cols) / *iw as f32).min(ch / *ih as f32))
                        .fold(f32::INFINITY, f32::min)
                };
                let cols = [1, 2, 4]
                    .into_iter()
                    .max_by(|a, b| scale(*a).total_cmp(&scale(*b)))
                    .unwrap_or(1);
                let rows = (0..4)
                    .collect::<Vec<_>>()
                    .chunks(cols)
                    .map(<[usize]>::to_vec)
                    .collect();
                (rows, width(cols) < NARROW)
            }
        }
    }

    /// One scope: its title row over its well. A narrow one leaves out the
    /// title's readout.
    fn scope(&self, kind: usize, scopes: &Scopes, narrow: bool, cx: &mut Context<Self>) -> Div {
        let levels = |(lo, hi): Range| mono(format!("{lo} \u{2013} {hi}"));
        let (name, note) = match kind {
            WAVEFORM => ("Waveform", Some("Luma")),
            PARADE => ("Parade", Some("RGB")),
            VECTOR => ("Vectorscope", None),
            _ => ("Histogram", Some("RGB")),
        };
        let readout = match kind {
            WAVEFORM => levels(scopes.luma).into_any_element(),
            PARADE => h_flex()
                .gap(px(6.))
                .children(
                    CHANNELS
                        .iter()
                        .zip(scopes.rgb)
                        .map(|((letter, color), range)| {
                            h_flex()
                                .gap(px(4.))
                                .child(mono(*letter).text_color(*color))
                                .child(levels(range))
                        }),
                )
                .into_any_element(),
            VECTOR => h_flex()
                .gap(px(4.))
                .text_size(px(10.))
                .font_weight(FontWeight::NORMAL)
                .text_color(t::T3)
                .child(div().w(px(10.)).h(px(1.)).bg(t::WARN))
                .child("Skin line")
                .into_any_element(),
            _ => h_flex()
                .gap(px(6.))
                .children(CHANNELS.iter().map(|(letter, color)| {
                    h_flex()
                        .gap(px(3.))
                        .child(div().size(px(6.)).rounded(px(1.)).bg(*color))
                        .child(mono(*letter))
                }))
                .into_any_element(),
        };
        let zoom = (kind == VECTOR).then(|| {
            Segmented::new("vector-zoom")
                .compact()
                .items(["1×", "2×"])
                .selected(self.zoom as usize)
                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                    this.zoom = *ix == 1;
                    cx.notify();
                }))
        });
        let title = h_flex()
            .flex_none()
            .h(px(TITLE))
            .gap(px(6.))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .child(div().text_color(t::T2).child(name))
            .children(note.map(|n| {
                div()
                    .text_color(t::T3)
                    .font_weight(FontWeight::NORMAL)
                    .child(n)
            }))
            .child(div().flex_1())
            .children((!narrow).then_some(readout))
            .children(zoom);
        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .child(title)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded(t::RADIUS_SM)
                    .border_1()
                    .border_color(t::LN)
                    .bg(t::SCOPE)
                    .overflow_hidden()
                    .child(well(kind, scopes.images[kind].clone(), scopes.zoom)),
            )
    }
}

/// The parade's and histogram's channels: letter and colour.
const CHANNELS: [(&str, Rgba); 3] = [("R", t::CH_R), ("G", t::CH_G), ("B", t::CH_B)];

fn mono(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(t::mono())
        .text_size(px(10.))
        .font_weight(FontWeight::NORMAL)
        .text_color(t::T3)
        .child(text.into())
}

impl Render for ScopesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        self.request(cx);
        let (playhead, rate) = {
            let state = self.state.read(cx);
            (state.playhead, state.bus.project().timeline.settings.rate)
        };
        let mode = self.mode;
        let tabs = Segmented::new("scope-mode")
            .compact()
            .fill()
            .items(MODES.iter().map(|(_, l)| *l))
            .selected(MODES.iter().position(|(m, _)| *m == mode).unwrap_or(0))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.mode = MODES[*ix].0;
                cx.notify();
            }));
        let content =
            match self.scopes.clone() {
                None => EmptyState::new(IconName::ImageOff, "No picture").into_any_element(),
                Some(scopes) => {
                    let (rows, narrow) = self.grid();
                    v_flex()
                        .size_full()
                        .gap(px(GAP))
                        .children(rows.into_iter().map(|row| {
                            h_flex().flex_1().min_h_0().gap(px(GAP)).children(
                                row.into_iter().map(|k| self.scope(k, &scopes, narrow, cx)),
                            )
                        }))
                        .into_any_element()
                }
            };
        let panel = cx.entity().downgrade();
        // Measures the body, so "All" can pick its grid.
        let measure = canvas(
            move |bounds, _, cx| {
                let _ = panel.update(cx, |this, cx| {
                    if this.body != bounds.size {
                        this.body = bounds.size;
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        Panel::new()
            .child(
                PanelHeader::new()
                    .title("Scopes")
                    .child(mono(playhead.timecode(rate)).mr(px(4.))),
            )
            .child(div().flex_none().px(px(10.)).pt(px(8.)).child(tabs))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .px(px(10.))
                    .pt(px(4.))
                    .pb(px(10.))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(measure)
                            .child(content),
                    ),
            )
    }
}

/// A scope's well: the image as large as it fits, keeping its aspect, with
/// the graticule's figures around it.
fn well(kind: usize, image: Arc<RenderImage>, zoom: bool) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            // Room for the level figures left of the traces, or the
            // histogram's below.
            let (left, bottom) = match kind {
                WAVEFORM | PARADE => (px(24.), px(4.)),
                HISTOGRAM => (px(8.), px(16.)),
                _ => (px(4.), px(4.)),
            };
            let room = Bounds::new(
                point(bounds.origin.x + left, bounds.origin.y + px(6.)),
                size(
                    bounds.size.width - left - px(8.),
                    bounds.size.height - bottom - px(6.),
                ),
            );
            if room.size.width <= px(0.) || room.size.height <= px(0.) {
                return;
            }
            let (iw, ih) = SIZES[kind];
            let k = (room.size.width / px(iw as f32)).min(room.size.height / px(ih as f32));
            let s = size(px(iw as f32) * k, px(ih as f32) * k);
            let rect = Bounds::new(
                point(
                    room.origin.x + (room.size.width - s.width) / 2.,
                    room.origin.y + (room.size.height - s.height) / 2.,
                ),
                s,
            );
            let _ = window.paint_image(rect, rect, Corners::default(), image.clone(), 0, false);
            let at = |x: f32, y: f32| point(rect.origin.x + px(x) * k, rect.origin.y + px(y) * k);
            match kind {
                WAVEFORM | PARADE => {
                    for q in 0..=4 {
                        let y = at(0., (PAD + 255 * q / 4) as f32).y;
                        let text = format!("{}", 100 - 25 * q);
                        figure(
                            &text,
                            point(rect.origin.x - px(6.), y),
                            Align::Right,
                            window,
                            cx,
                        );
                    }
                }
                HISTOGRAM => {
                    for q in 0..=4 {
                        let x = rect.origin.x + rect.size.width * (q as f32 / 4.);
                        let align = match q {
                            0 => Align::Left,
                            4 => Align::Right,
                            _ => Align::Centre,
                        };
                        let y = rect.origin.y + rect.size.height + px(8.);
                        figure(&format!("{}", 25 * q), point(x, y), align, window, cx);
                    }
                }
                _ => {
                    for (name, c) in TARGETS {
                        let (cb, cr) = chroma(c);
                        let (x, y) = vector_at(cb * 1.22, cr * 1.22, zoom);
                        let (n, _) = SIZES[VECTOR];
                        if (6.0..n as f32 - 6.0).contains(&x) && (6.0..n as f32 - 6.0).contains(&y)
                        {
                            figure(name, at(x, y), Align::Centre, window, cx);
                        }
                    }
                }
            }
        },
    )
    .size_full()
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Centre,
    Right,
}

/// A graticule figure centred vertically on `at`, aligned on it.
fn figure(text: &str, at: Point<Pixels>, align: Align, window: &mut Window, cx: &mut App) {
    let run = TextRun {
        len: text.len(),
        font: font(t::mono()),
        color: t::T3.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line =
        window
            .text_system()
            .shape_line(SharedString::from(text.to_string()), px(8.), &[run], None);
    let x = match align {
        Align::Left => at.x,
        Align::Centre => at.x - line.width / 2.,
        Align::Right => at.x - line.width,
    };
    let _ = line.paint(
        point(x, at.y - px(5.)),
        px(10.),
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

// ----------------------------------------------------------------- drawing

/// Image sizes: waveform, parade, vectorscope, histogram.
/// Traces have one row per 8-bit level plus `PAD` above and below.
const SIZES: [(usize, usize); 4] = [(320, 268), (390, 268), (220, 220), (320, 200)];
const PAD: usize = 6;

/// The 75 % colour targets on the vectorscope, by name.
const TARGETS: [(&str, [f32; 3]); 6] = [
    ("R", [0.75, 0.0, 0.0]),
    ("YL", [0.75, 0.75, 0.0]),
    ("G", [0.0, 0.75, 0.0]),
    ("CY", [0.0, 0.75, 0.75]),
    ("B", [0.0, 0.0, 0.75]),
    ("MG", [0.75, 0.0, 0.75]),
];

fn rgb(c: Rgba) -> [f32; 3] {
    [c.r, c.g, c.b]
}

/// What drawing `c` adds to the well's ground: `c` itself where nothing
/// else is.
fn over_ground(c: Rgba) -> [f32; 3] {
    let g = rgb(t::SCOPE);
    let c = rgb(c);
    [c[0] - g[0], c[1] - g[1], c[2] - g[2]]
}

fn scaled(c: [f32; 3], k: f32) -> [f32; 3] {
    c.map(|v| v * k)
}

/// A float RGB image drawn additively.
struct Plot {
    w: usize,
    h: usize,
    px: Vec<[f32; 3]>,
}

impl Plot {
    fn new((w, h): (usize, usize)) -> Self {
        Self {
            w,
            h,
            px: vec![rgb(t::SCOPE); w * h],
        }
    }

    fn add(&mut self, x: usize, y: usize, c: [f32; 3]) {
        if x < self.w && y < self.h {
            let p = &mut self.px[y * self.w + x];
            for i in 0..3 {
                p[i] += c[i];
            }
        }
    }

    fn hline(&mut self, y: usize, x0: usize, x1: usize, c: [f32; 3]) {
        for x in x0..x1.min(self.w) {
            self.add(x, y, c);
        }
    }

    /// A line from `a` to `b` in pixel coordinates, dashed every `dash`
    /// pixels when given.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [f32; 3], dash: Option<usize>) {
        let n = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize + 1;
        for i in 0..=n {
            if dash.is_some_and(|d| (i / d) % 2 == 1) {
                continue;
            }
            let t = i as f32 / n as f32;
            let x = a.0 + (b.0 - a.0) * t;
            let y = a.1 + (b.1 - a.1) * t;
            if x >= 0.0 && y >= 0.0 {
                self.add(x as usize, y as usize, c);
            }
        }
    }

    fn image(self) -> Arc<RenderImage> {
        let mut bgra = Vec::with_capacity(self.w * self.h * 4);
        for p in &self.px {
            let [r, g, b] = p.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            bgra.extend_from_slice(&[b, g, r, 255]);
        }
        let buffer = image::RgbaImage::from_raw(self.w as u32, self.h as u32, bgra)
            .expect("buffer matches its size");
        Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
    }
}

/// Brightness of a density count, `per` being the average count per cell.
fn glow(count: u32, per: f32) -> f32 {
    if count == 0 {
        0.0
    } else {
        (count as f32 / per).sqrt().mul_add(0.5, 0.18).min(1.0)
    }
}

fn luma(px: &[u8; 4]) -> f32 {
    LUMA[0] * px[0] as f32 + LUMA[1] * px[1] as f32 + LUMA[2] * px[2] as f32
}

/// The levels `bins` spans in percent, its faintest 0.2 % at each end left
/// out.
fn span(bins: &[u32; 256]) -> Range {
    let total: u32 = bins.iter().sum();
    let tail = total / 500;
    let percent = |level: usize| (level as u32 * 100 + 127) / 255;
    let mut seen = 0;
    let lo = bins
        .iter()
        .position(|n| {
            seen += n;
            seen > tail
        })
        .unwrap_or(0);
    seen = 0;
    let hi = 255
        - bins
            .iter()
            .rev()
            .position(|n| {
                seen += n;
                seen > tail
            })
            .unwrap_or(0);
    (percent(lo), percent(hi))
}

fn draw(frame: &Frame, zoom: bool) -> Option<Scopes> {
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    if fw == 0 || fh == 0 || frame.rgba.len() < fw * fh * 4 {
        return None;
    }
    let pixels = frame.rgba.as_chunks::<4>().0;
    let mut luma_bins = [0u32; 256];
    let mut bins = [[0u32; 256]; 3];
    for px in pixels.iter().filter(|px| px[3] >= 8) {
        luma_bins[luma(px).round().clamp(0.0, 255.0) as usize] += 1;
        for c in 0..3 {
            bins[c][px[c] as usize] += 1;
        }
    }
    Some(Scopes {
        images: [
            waveform(pixels, fw).image(),
            parade(pixels, fw).image(),
            vectorscope(pixels, zoom).image(),
            histogram(&bins).image(),
        ],
        luma: span(&luma_bins),
        rgb: bins.each_ref().map(span),
        zoom,
    })
}

/// Graticule at 0, 25, 50, 75 and 100 %, the top one dashed in the
/// clipping colour.
fn levels(p: &mut Plot) {
    let w = p.w;
    for k in 0..4 {
        p.hline(PAD + 255 * (k + 1) / 4, 0, w, over_ground(t::LN));
    }
    let clip = scaled(rgb(t::ERR), 0.33);
    p.line((0., PAD as f32), (w as f32, PAD as f32), clip, Some(3));
}

/// Column-binned trace of one 0..255 value per pixel into `p` at columns
/// `x0..x0 + cols`, one row per level. Each column is normalized by the
/// pixels it holds, so uneven binning doesn't stripe the trace.
fn trace(
    p: &mut Plot,
    pixels: &[[u8; 4]],
    fw: usize,
    (x0, cols): (usize, usize),
    value: impl Fn(&[u8; 4]) -> f32,
    color: [f32; 3],
) {
    let mut counts = vec![0u32; cols * 256];
    let mut per_col = vec![0u32; cols];
    for (i, px) in pixels.iter().enumerate() {
        if px[3] < 8 {
            continue;
        }
        let col = (i % fw) * cols / fw;
        let row = 255 - value(px).round().clamp(0.0, 255.0) as usize;
        counts[row * cols + col] += 1;
        per_col[col] += 1;
    }
    for row in 0..256 {
        for col in 0..cols {
            let per = per_col[col] as f32 / 256.0 * 4.0;
            let g = glow(counts[row * cols + col], per.max(1e-3));
            if g > 0.0 {
                p.add(x0 + col, PAD + row, scaled(color, g));
            }
        }
    }
}

fn waveform(pixels: &[[u8; 4]], fw: usize) -> Plot {
    let mut p = Plot::new(SIZES[WAVEFORM]);
    levels(&mut p);
    let (t1, ok) = (rgb(t::T1), rgb(t::OK));
    let color = [0, 1, 2].map(|i| t1[i] * 0.8 + ok[i] * 0.2);
    let w = p.w;
    trace(&mut p, pixels, fw, (0, w), luma, scaled(color, 0.9));
    p
}

fn parade(pixels: &[[u8; 4]], fw: usize) -> Plot {
    let mut p = Plot::new(SIZES[PARADE]);
    levels(&mut p);
    let gap = 6;
    let cols = (p.w - 2 * gap) / 3;
    for (c, (_, color)) in CHANNELS.into_iter().enumerate() {
        let x0 = c * (cols + gap);
        trace(
            &mut p,
            pixels,
            fw,
            (x0, cols),
            |px| px[c] as f32,
            rgb(color),
        );
    }
    p
}

/// Cb and Cr (Rec. 709) of a 0..1 color, each -0.5..0.5.
fn chroma(c: [f32; 3]) -> (f32, f32) {
    let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    ((c[2] - y) / 1.8556, (c[0] - y) / 1.5748)
}

/// Where chroma `(cb, cr)` lands on the vectorscope image: ±0.5 reaches the
/// rim at 1×.
fn vector_at(cb: f32, cr: f32, zoom: bool) -> (f32, f32) {
    let mid = SIZES[VECTOR].0 as f32 / 2.0;
    let scale = mid * 0.92 / 0.5 * if zoom { 2.0 } else { 1.0 };
    (mid + cb * scale, mid - cr * scale)
}

fn vectorscope(pixels: &[[u8; 4]], zoom: bool) -> Plot {
    let mut p = Plot::new(SIZES[VECTOR]);
    let n = p.w;
    let at = |cb: f32, cr: f32| vector_at(cb, cr, zoom);
    // Rim and half circle, axes, 75 % targets and the skin-tone line.
    let rim = if zoom { 0.25 } else { 0.5 };
    let steps = 360;
    for (radius, color) in [(rim, t::LN_HV), (rim / 2., t::LN)] {
        for k in 0..steps {
            let a0 = k as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = (k + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            p.line(
                at(a0.cos() * radius, a0.sin() * radius),
                at(a1.cos() * radius, a1.sin() * radius),
                over_ground(color),
                None,
            );
        }
    }
    let axis = over_ground(t::LN);
    p.line(at(-rim, 0.0), at(rim, 0.0), axis, None);
    p.line(at(0.0, -rim), at(0.0, rim), axis, None);
    let skin = 123f32.to_radians();
    p.line(
        at(0.0, 0.0),
        at(skin.cos() * rim, skin.sin() * rim),
        scaled(rgb(t::WARN), 0.55),
        Some(3),
    );
    let target = over_ground(t::LN_CTL);
    for (_, c) in TARGETS {
        let (cb, cr) = chroma(c);
        let (x, y) = at(cb, cr);
        let r = 4.0;
        p.line((x - r, y - r), (x + r, y - r), target, None);
        p.line((x - r, y + r), (x + r, y + r), target, None);
        p.line((x - r, y - r), (x - r, y + r), target, None);
        p.line((x + r, y - r), (x + r, y + r), target, None);
    }
    let mut counts = vec![0u32; n * n];
    let mut colors = vec![[0f32; 3]; n * n];
    let mut total = 0usize;
    for px in pixels.iter().filter(|px| px[3] >= 8) {
        let c = [px[0], px[1], px[2]].map(|v| v as f32 / 255.0);
        let (cb, cr) = chroma(c);
        let (x, y) = at(cb, cr);
        if x < 0.0 || y < 0.0 {
            continue;
        }
        let (x, y) = (x as usize, y as usize);
        if x < n && y < n {
            counts[y * n + x] += 1;
            for i in 0..3 {
                colors[y * n + x][i] += c[i];
            }
            total += 1;
        }
    }
    let per = (total as f32 / (n * n) as f32 * 8.0).max(1e-3);
    for i in 0..n * n {
        let g = glow(counts[i], per);
        if g > 0.0 {
            // The pixels' own hue, brightened.
            let avg = colors[i].map(|v| v / counts[i] as f32);
            let max = avg[0].max(avg[1]).max(avg[2]).max(0.2);
            let c = avg.map(|v| (v / max * 0.7 + 0.3) * g);
            p.add(i % n, i / n, c);
        }
    }
    p
}

fn histogram(bins: &[[u32; 256]; 3]) -> Plot {
    let mut p = Plot::new(SIZES[HISTOGRAM]);
    let (w, h) = (p.w, p.h);
    let pad = PAD;
    let rows = h - 2 * pad;
    // Scale to the 99.5th percentile of bin counts, so a spike of pure
    // black or white doesn't flatten the rest.
    let mut all: Vec<u32> = bins.iter().flatten().copied().collect();
    all.sort_unstable();
    let top = all[(all.len() * 995 / 1000).min(all.len() - 1)].max(1) as f32;
    let grid = over_ground(t::LN);
    for x in (0..w).step_by(w / 4).skip(1) {
        for y in pad..h - pad {
            p.add(x, y, grid);
        }
    }
    for (c, (_, color)) in CHANNELS.into_iter().enumerate() {
        let (fill, edge) = (scaled(rgb(color), 0.28), scaled(rgb(color), 0.7));
        for x in 0..w {
            let v = bins[c][x * 256 / w] as f32 / top;
            let height = (v.min(1.0) * rows as f32) as usize;
            for y in 0..height {
                p.add(
                    x,
                    h - pad - 1 - y,
                    if y + 1 == height { edge } else { fill },
                );
            }
        }
    }
    p
}
