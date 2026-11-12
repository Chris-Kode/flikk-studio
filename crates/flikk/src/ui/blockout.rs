//! A blockout on screen: [`BlockoutPlayer`] draws a scene as it plays, in
//! a tile of a video request's card and large in [`editor`], the dialog
//! where the user changes the scene by asking. There it can also show the
//! set from outside the shot, turned by dragging, and the shot as its
//! depth, its lines or its things; a click on a thing in the shot names
//! it.

pub mod editor;

use crate::ui::kit::theme as t;
use crate::ui::viewer::latest::Latest;
use flikk_core::blockout::{Detail, FPS, Outside, Pass, Scene};
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Drawing a frame slower than this makes the next ones narrower.
const SLOW: Duration = Duration::from_millis(45);
/// Drawing a frame faster than this makes the next ones wider again.
const FAST: Duration = Duration::from_millis(16);
/// The narrowest a picture is drawn while it can't keep up.
const NARROWEST: u32 = 192;
/// Degrees the set turns for each pixel it is dragged.
const TURN: f64 = 0.4;
/// The lowest the set is looked at from, in degrees above the horizon.
const LOWEST: f64 = 3.0;

/// What a player shows of its scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Looking {
    /// The shot: what a video model is given to follow.
    Shot,
    /// The set from outside the shot, with its camera drawn in.
    Set(Outside),
    /// The shot as one of its passes.
    Pass(Pass),
}

/// The user pointed at a thing in the shot: its name.
pub struct Picked(pub String);

impl EventEmitter<Picked> for BlockoutPlayer {}

/// A scene playing in a loop, drawn off the UI thread at the width it can
/// keep up with; a frame it stops on is drawn again in full. It fills the
/// box it is given.
pub struct BlockoutPlayer {
    scene: Arc<Scene>,
    /// Counts the scenes shown, so a frame of an earlier one is dropped.
    version: u64,
    /// The picture's shape: width to height.
    shape: (u32, u32),
    /// The widest it is drawn, and how wide it is drawn now.
    full_width: u32,
    width: u32,
    looking: Looking,
    /// Whether lines mark the thirds of the shot.
    thirds: bool,
    /// Where the picture is on screen.
    drawn_in: Rc<Cell<Bounds<Pixels>>>,
    /// While the set is dragged round: where the drag began, and how
    /// the set was looked at then.
    turning: Option<(Point<Pixels>, Outside)>,
    /// Seconds into the scene on screen.
    at: f64,
    /// While it plays: a moment, and where the scene was then.
    playing: Option<(Instant, f64)>,
    drawing: Latest<f64>,
    picture: Option<Arc<RenderImage>>,
    /// Pictures replaced since the last paint, to release from the GPU.
    retired: Vec<Arc<RenderImage>>,
    clock: Option<Task<()>>,
}

impl BlockoutPlayer {
    /// `scene`, playing, in a picture `shape` (width to height) at most
    /// `full_width` pixels wide.
    pub fn new(
        scene: Scene,
        shape: (u32, u32),
        full_width: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release_in(window, |this, window, _| {
            for image in this.retired.drain(..).chain(this.picture.take()) {
                let _ = window.drop_image(image);
            }
        })
        .detach();
        let mut this = Self {
            scene: Arc::new(scene),
            version: 0,
            shape: (shape.0.max(1), shape.1.max(1)),
            full_width: full_width.max(NARROWEST),
            width: full_width.max(NARROWEST),
            looking: Looking::Shot,
            thirds: false,
            drawn_in: Rc::new(Cell::new(Bounds::default())),
            turning: None,
            at: 0.0,
            playing: None,
            drawing: Latest::Idle,
            picture: None,
            retired: Vec::new(),
            clock: None,
        };
        this.play(cx);
        this
    }

    /// Show `scene` in place of the one playing, from the same moment.
    pub fn set_scene(&mut self, scene: Scene, cx: &mut Context<Self>) {
        self.scene = Arc::new(scene);
        self.version += 1;
        self.at = self.at.min(self.scene.duration);
        if let Some(playing) = self.playing.as_mut() {
            *playing = (Instant::now(), self.at);
        }
        self.draw(cx);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    pub fn looking(&self) -> Looking {
        self.looking
    }

    /// Show the scene as `looking` has it.
    pub fn look(&mut self, looking: Looking, cx: &mut Context<Self>) {
        if !matches!(looking, Looking::Set(_)) {
            self.turning = None;
        }
        self.looking = looking;
        // A picture of how it was looked at before is not shown.
        self.version += 1;
        self.draw(cx);
    }

    pub fn has_thirds(&self) -> bool {
        self.thirds
    }

    pub fn toggle_thirds(&mut self, cx: &mut Context<Self>) {
        self.thirds = !self.thirds;
        cx.notify();
    }

    /// The left button went down on the picture: start turning the set,
    /// or name the thing of the shot under the pointer.
    fn pressed(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        match self.looking {
            Looking::Set(from) => self.turning = Some((position, from)),
            Looking::Shot => {
                let picture = self.drawn_in.get();
                if !picture.contains(&position) {
                    return;
                }
                let across = (position.x - picture.origin.x) / picture.size.width;
                let down = (position.y - picture.origin.y) / picture.size.height;
                if let Some(name) = self.scene.thing_at(self.at, self.shape, (across, down)) {
                    cx.emit(Picked(name.to_owned()));
                }
            }
            Looking::Pass(_) => {}
        }
    }

    /// The pointer moved: turn the set with it while it is dragged. A
    /// move with the button up ends the drag.
    fn moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((from, was)) = self.turning else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.turning = None;
            return;
        }
        let dragged = event.position - from;
        let turned = Outside {
            around: was.around - f64::from(f32::from(dragged.x)) * TURN,
            above: (was.above + f64::from(f32::from(dragged.y)) * TURN).clamp(LOWEST, 90.0),
            ..was
        };
        // The drag goes on from where it began.
        self.look(Looking::Set(turned), cx);
        self.turning = Some((from, was));
    }

    /// The wheel turned over the set: go nearer or further off.
    fn scrolled(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Looking::Set(from) = self.looking else {
            return;
        };
        let lines = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 20.0,
        };
        let closer = (from.closer * f64::from(lines * 0.12).exp()).clamp(0.3, 12.0);
        self.look(Looking::Set(Outside { closer, ..from }), cx);
    }

    /// Seconds into the scene, and how long it runs.
    pub fn time(&self) -> (f64, f64) {
        (self.at, self.scene.duration)
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        match self.playing.take() {
            Some(_) => {
                self.clock = None;
                self.draw(cx);
            }
            None => self.play(cx),
        }
    }

    /// Stop on the frame `fraction` (0..1) of the way through.
    pub fn seek(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.playing = None;
        self.clock = None;
        self.at = fraction.clamp(0.0, 1.0) as f64 * self.scene.duration;
        self.draw(cx);
    }

    fn play(&mut self, cx: &mut Context<Self>) {
        self.playing = Some((Instant::now(), self.at));
        let frame = Duration::from_secs_f64(1.0 / FPS as f64);
        self.clock = Some(cx.spawn(async move |this, cx| {
            loop {
                let playing = this.update(cx, |this, cx| {
                    let Some((since, from)) = this.playing else {
                        return false;
                    };
                    let length = this.scene.duration.max(1e-3);
                    this.at = (from + since.elapsed().as_secs_f64()) % length;
                    this.draw(cx);
                    true
                });
                if !matches!(playing, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(frame).await;
            }
        }));
        cx.notify();
    }

    /// Draw the frame on screen now, or after the one being drawn.
    fn draw(&mut self, cx: &mut Context<Self>) {
        if let Some(at) = self.drawing.queue(self.at) {
            self.start_drawing(at, Detail::Draft, cx);
        }
        cx.notify();
    }

    fn start_drawing(&mut self, at: f64, detail: Detail, cx: &mut Context<Self>) {
        self.drawing.start();
        let (scene, version, looking) = (self.scene.clone(), self.version, self.looking);
        let width = match detail {
            Detail::Draft => self.width,
            Detail::Full => self.full_width,
        };
        let height = (width as u64 * self.shape.1 as u64 / self.shape.0 as u64).max(1) as u32;
        let drawn = cx.background_executor().spawn(async move {
            let began = Instant::now();
            let frame = match looking {
                Looking::Shot => scene.render(at, width, height, detail),
                Looking::Set(from) => scene.render_outside(at, (width, height), &from, detail),
                Looking::Pass(pass) => scene.render_pass(at, width, height, pass),
            };
            // The GPU takes blue first; greys read the same either way.
            let mut bgra = frame.rgba.to_vec();
            if matches!(looking, Looking::Pass(_)) {
                bgra.as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .for_each(|pixel| pixel.swap(0, 2));
            }
            let picture = image::RgbaImage::from_raw(width, height, bgra)
                .map(|buffer| Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])));
            (picture, began.elapsed())
        });
        cx.spawn(async move |this, cx| {
            let (picture, took) = drawn.await;
            let _ = this.update(cx, |this, cx| {
                if version == this.version
                    && let Some(picture) = picture
                {
                    this.retired.extend(this.picture.replace(picture));
                }
                if detail == Detail::Draft {
                    this.pace(took);
                }
                match this.drawing.finish() {
                    Some(next) => this.start_drawing(next, Detail::Draft, cx),
                    // Stopped on this frame: draw it once more, in full.
                    None if this.playing.is_none() && detail == Detail::Draft => {
                        this.start_drawing(at, Detail::Full, cx);
                    }
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep the motion fluid: draw narrower after a slow frame, wider
    /// again after a fast one.
    fn pace(&mut self, took: Duration) {
        if took > SLOW {
            self.width = (self.width * 3 / 4).max(NARROWEST.min(self.full_width));
        } else if took < FAST {
            self.width = (self.width * 5 / 4).min(self.full_width);
        }
    }
}

impl Render for BlockoutPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        let (picture, shape) = (self.picture.clone(), self.shape);
        let drawn_in = self.drawn_in.clone();
        let thirds = self.thirds && matches!(self.looking, Looking::Shot | Looking::Pass(_));
        // Painted by hand: an image element takes its height from its
        // width and its own shape, so an upright picture overflowed a
        // wide box instead of fitting inside it.
        let paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, _: &mut App| {
            let inside = fitted(bounds, shape);
            drawn_in.set(inside);
            if let Some(picture) = picture.clone() {
                let _ = window.paint_image(inside, inside, Corners::default(), picture, 0, false);
            }
            if thirds {
                let line = t::alpha(t::ON_AC, 0.3);
                for third in [1. / 3., 2. / 3.] {
                    let across =
                        point(inside.origin.x + inside.size.width * third, inside.origin.y);
                    let down = point(
                        inside.origin.x,
                        inside.origin.y + inside.size.height * third,
                    );
                    window.paint_quad(fill(
                        Bounds::new(across, size(px(1.), inside.size.height)),
                        line,
                    ));
                    window.paint_quad(fill(
                        Bounds::new(down, size(inside.size.width, px(1.))),
                        line,
                    ));
                }
            }
        };
        let end_turning = cx.listener(|this, _: &MouseUpEvent, _, _| this.turning = None);
        div()
            .relative()
            .size_full()
            .bg(t::SCOPE)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| this.pressed(event.position, cx)),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.moved(event, cx)))
            .on_mouse_up(MouseButton::Left, end_turning)
            .on_scroll_wheel(
                cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.scrolled(event, cx)),
            )
            .child(canvas(|_, _, _| (), paint).absolute().inset_0())
    }
}

/// The largest box of `shape` (width to height) inside `bounds`, centred.
fn fitted(bounds: Bounds<Pixels>, shape: (u32, u32)) -> Bounds<Pixels> {
    let (wide, high) = (shape.0 as f32, shape.1 as f32);
    let scale = (bounds.size.width / wide).min(bounds.size.height / high);
    let inside = size(scale * wide, scale * high);
    Bounds {
        origin: point(
            bounds.origin.x + (bounds.size.width - inside.width) / 2.,
            bounds.origin.y + (bounds.size.height - inside.height) / 2.,
        ),
        size: inside,
    }
}
