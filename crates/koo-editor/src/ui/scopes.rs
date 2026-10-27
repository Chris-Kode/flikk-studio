//! Video scopes for the Color workspace: luma waveform, RGB parade,
//! vectorscope (with the skin-tone line) and histogram of the frame at the
//! playhead. The frame is the topmost visible media clip's, graded with its
//! clip grade and then the timeline grade through `koo_core::grade`, so the
//! scopes read exactly what the viewer shows and the export renders. They
//! are drawn off the UI thread; the latest request wins, and playback
//! refreshes them a few times a second.

use crate::state::EditorState;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, v_flex};
use gpui_kit::*;
use koo_core::grade::{self, LUMA};
use koo_core::model::{ClipContent, Grade, MediaItem, MediaKind, Project, TrackKind};
use koo_core::services::Frame;
use koo_core::{ClipId, Time};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Width of the frames the scopes read.
const FRAME_WIDTH: u32 = 480;
/// Least time between two refreshes during playback.
const PLAYING_EVERY: Duration = Duration::from_millis(200);

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

/// What the scopes show: a clip's source frame through two grades.
#[derive(Clone, PartialEq)]
struct Request {
    clip: ClipId,
    item: MediaItem,
    source: Time,
    grade: Grade,
    timeline: Grade,
}

/// The four scopes as images, in `MODES` order after `All`.
#[derive(Clone)]
struct Images([Arc<RenderImage>; 4]);

pub struct ScopesPanel {
    state: Entity<EditorState>,
    mode: Mode,
    images: Option<Images>,
    requested: Option<Request>,
    generation: u64,
    busy: bool,
    last: Option<Instant>,
    timer: bool,
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
            images: None,
            requested: None,
            generation: 0,
            busy: false,
            last: None,
            timer: false,
            retired: Vec::new(),
            _observe,
        }
    }

    fn show(&mut self, images: Option<Images>) {
        if let Some(old) = std::mem::replace(&mut self.images, images) {
            self.retired.extend(old.0);
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
            let images = cx
                .background_executor()
                .spawn(async move {
                    let frame = media
                        .frame(&request.item, request.source, FRAME_WIDTH)
                        .ok()?;
                    let frame = grade::graded(&frame, &[&request.grade, &request.timeline]);
                    draw(&frame)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if this.generation == generation {
                    this.show(images);
                }
                // A newer request may be waiting.
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ScopesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        self.request(cx);
        let theme = cx.theme();
        let (bg, border, muted) = (theme.background, theme.border, theme.muted_foreground);
        let mode = self.mode;
        let tabs = TabBar::new("scope-mode")
            .segmented()
            .xsmall()
            .selected_index(MODES.iter().position(|(m, _)| *m == mode).unwrap_or(0))
            .children(MODES.iter().map(|(_, l)| Tab::new().label(*l)))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.mode = MODES[*ix].0;
                cx.notify();
            }));
        let body = match &self.images {
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(muted)
                .child("No picture")
                .into_any_element(),
            Some(images) => {
                let shown: Vec<(usize, Arc<RenderImage>)> = match mode {
                    Mode::All => images.0.iter().cloned().enumerate().collect(),
                    m => {
                        let i = MODES.iter().position(|(x, _)| *x == m).unwrap_or(1) - 1;
                        vec![(i, images.0[i].clone())]
                    }
                };
                scopes_canvas(shown, border).into_any_element()
            }
        };
        v_flex()
            .size_full()
            .bg(bg)
            .p_2()
            .gap_2()
            .child(div().flex().justify_center().child(tabs))
            .child(body)
    }
}

/// The scopes on one canvas: one fills it; four take the grid (one, two or
/// four columns) that shows them largest. Each keeps its aspect.
fn scopes_canvas(shown: Vec<(usize, Arc<RenderImage>)>, border: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let gap = px(6.);
            let n = shown.len();
            let grid = |cols: usize| {
                let rows = n.div_ceil(cols);
                let w = (bounds.size.width - gap * (cols - 1) as f32) / cols as f32;
                let h = (bounds.size.height - gap * (rows - 1) as f32) / rows as f32;
                let cells: Vec<Bounds<Pixels>> = (0..n)
                    .map(|i| {
                        let x = bounds.origin.x + (w + gap) * (i % cols) as f32;
                        let y = bounds.origin.y + (h + gap) * (i / cols) as f32;
                        Bounds::new(point(x, y), size(w, h))
                    })
                    .collect();
                let scale = shown
                    .iter()
                    .map(|(k, _)| {
                        let (iw, ih) = SIZES[*k];
                        (w / px(iw as f32)).min(h / px(ih as f32))
                    })
                    .fold(f32::INFINITY, f32::min);
                (scale, cells)
            };
            let cells = [1, 2, 4]
                .into_iter()
                .filter(|c| *c <= n)
                .map(grid)
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, cells)| cells)
                .unwrap_or_default();
            for ((kind, image), cell) in shown.into_iter().zip(cells) {
                let (iw, ih) = SIZES[kind];
                let k = (cell.size.width / px(iw as f32)).min(cell.size.height / px(ih as f32));
                let s = size(px(iw as f32) * k, px(ih as f32) * k);
                let rect = Bounds::new(
                    point(
                        cell.origin.x + (cell.size.width - s.width) / 2.,
                        cell.origin.y + (cell.size.height - s.height) / 2.,
                    ),
                    s,
                );
                let _ = window.paint_image(rect, rect, Corners::all(px(3.)), image, 0, false);
                window.paint_quad(quad(
                    rect,
                    px(3.),
                    transparent_black(),
                    px(1.),
                    border,
                    BorderStyle::default(),
                ));
            }
        },
    )
    .flex_1()
    .min_h_0()
    .w_full()
}

// ----------------------------------------------------------------- drawing

/// Image sizes: waveform, parade, vectorscope, histogram.
/// Traces have one row per 8-bit level plus `PAD` above and below.
const SIZES: [(usize, usize); 4] = [(320, 268), (390, 268), (220, 220), (320, 200)];
const PAD: usize = 6;

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
            px: vec![[0.035, 0.035, 0.04]; w * h],
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

    /// A line from `a` to `b` in pixel coordinates.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [f32; 3]) {
        let n = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize + 1;
        for i in 0..=n {
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

const GRID: [f32; 3] = [0.09, 0.09, 0.1];

/// Brightness of a density count, `per` being the average count per cell.
fn glow(count: u32, per: f32) -> f32 {
    if count == 0 {
        0.0
    } else {
        (count as f32 / per).sqrt().mul_add(0.5, 0.18).min(1.0)
    }
}

fn draw(frame: &Frame) -> Option<Images> {
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    if fw == 0 || fh == 0 || frame.rgba.len() < fw * fh * 4 {
        return None;
    }
    let pixels = frame.rgba.as_chunks::<4>().0;
    Some(Images([
        waveform(pixels, fw, fh).image(),
        parade(pixels, fw, fh).image(),
        vectorscope(pixels).image(),
        histogram(pixels).image(),
    ]))
}

/// Graticule at 0, 25, 50, 75 and 100 %.
fn levels(p: &mut Plot, pad: usize, x0: usize, x1: usize) {
    for k in 0..=4 {
        p.hline(pad + 255 * k / 4, x0, x1, GRID);
    }
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
                p.add(x0 + col, PAD + row, color.map(|c| c * g));
            }
        }
    }
}

fn waveform(pixels: &[[u8; 4]], fw: usize, _fh: usize) -> Plot {
    let mut p = Plot::new(SIZES[0]);
    let w = p.w;
    levels(&mut p, PAD, 0, w);
    let luma =
        |px: &[u8; 4]| LUMA[0] * px[0] as f32 + LUMA[1] * px[1] as f32 + LUMA[2] * px[2] as f32;
    trace(&mut p, pixels, fw, (0, w), luma, [0.75, 0.95, 0.8]);
    p
}

fn parade(pixels: &[[u8; 4]], fw: usize, _fh: usize) -> Plot {
    let mut p = Plot::new(SIZES[1]);
    let w = p.w;
    levels(&mut p, PAD, 0, w);
    let gap = 6;
    let cols = (w - 2 * gap) / 3;
    let colors = [[1.0, 0.3, 0.25], [0.3, 1.0, 0.35], [0.35, 0.5, 1.0]];
    for (c, color) in colors.into_iter().enumerate() {
        let x0 = c * (cols + gap);
        trace(&mut p, pixels, fw, (x0, cols), |px| px[c] as f32, color);
    }
    p
}

/// Cb and Cr (Rec. 709) of a 0..1 color, each -0.5..0.5.
fn chroma(c: [f32; 3]) -> (f32, f32) {
    let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    ((c[2] - y) / 1.8556, (c[0] - y) / 1.5748)
}

fn vectorscope(pixels: &[[u8; 4]]) -> Plot {
    let mut p = Plot::new(SIZES[2]);
    let n = p.w;
    let mid = n as f32 / 2.0;
    // ±0.5 chroma reaches the rim.
    let scale = mid * 0.92 / 0.5;
    let at = |cb: f32, cr: f32| (mid + cb * scale, mid - cr * scale);
    // Rim, axes, 75 % targets and the skin-tone line.
    let steps = 360;
    for k in 0..steps {
        let a0 = k as f32 / steps as f32 * std::f32::consts::TAU;
        let a1 = (k + 1) as f32 / steps as f32 * std::f32::consts::TAU;
        p.line(
            at(a0.cos() * 0.5, a0.sin() * 0.5),
            at(a1.cos() * 0.5, a1.sin() * 0.5),
            GRID,
        );
    }
    p.line(at(-0.5, 0.0), at(0.5, 0.0), GRID);
    p.line(at(0.0, -0.5), at(0.0, 0.5), GRID);
    let skin = 123f32.to_radians();
    p.line(
        at(0.0, 0.0),
        at(skin.cos() * 0.5, skin.sin() * 0.5),
        [0.22, 0.16, 0.1],
    );
    for c in [
        [0.75, 0.0, 0.0],
        [0.75, 0.75, 0.0],
        [0.0, 0.75, 0.0],
        [0.0, 0.75, 0.75],
        [0.0, 0.0, 0.75],
        [0.75, 0.0, 0.75],
    ] {
        let (cb, cr) = chroma(c);
        let (x, y) = at(cb, cr);
        let tint = c.map(|v| v * 0.3 + 0.05);
        let r = 4.0;
        p.line((x - r, y - r), (x + r, y - r), tint);
        p.line((x - r, y + r), (x + r, y + r), tint);
        p.line((x - r, y - r), (x - r, y + r), tint);
        p.line((x + r, y - r), (x + r, y + r), tint);
    }
    let mut counts = vec![0u32; n * n];
    let mut colors = vec![[0f32; 3]; n * n];
    let mut total = 0usize;
    for px in pixels.iter().filter(|px| px[3] >= 8) {
        let c = [px[0], px[1], px[2]].map(|v| v as f32 / 255.0);
        let (cb, cr) = chroma(c);
        let (x, y) = at(cb, cr);
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

fn histogram(pixels: &[[u8; 4]]) -> Plot {
    let mut p = Plot::new(SIZES[3]);
    let (w, h) = (p.w, p.h);
    let pad = PAD;
    let rows = h - 2 * pad;
    let mut bins = [[0u32; 256]; 3];
    for px in pixels.iter().filter(|px| px[3] >= 8) {
        for c in 0..3 {
            bins[c][px[c] as usize] += 1;
        }
    }
    // Scale to the 99.5th percentile of bin counts, so a spike of pure
    // black or white doesn't flatten the rest.
    let mut all: Vec<u32> = bins.iter().flatten().copied().collect();
    all.sort_unstable();
    let top = all[(all.len() * 995 / 1000).min(all.len() - 1)].max(1) as f32;
    for x in (0..w).step_by(w / 4) {
        for y in pad..h - pad {
            p.add(x, y, GRID);
        }
    }
    let colors = [[0.55, 0.12, 0.1], [0.1, 0.5, 0.15], [0.12, 0.2, 0.6]];
    for (c, color) in colors.into_iter().enumerate() {
        for x in 0..w {
            let v = bins[c][x * 256 / w] as f32 / top;
            let height = (v.min(1.0) * rows as f32) as usize;
            for y in 0..height {
                p.add(x, h - pad - 1 - y, color);
            }
        }
    }
    p
}
