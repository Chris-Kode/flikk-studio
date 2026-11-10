//! A blockout on screen: [`BlockoutPlayer`] draws a scene as it plays, in
//! a tile of a video request's card and large in [`editor`], the dialog
//! where the user changes the scene by asking.

pub mod editor;

use crate::ui::kit::theme as t;
use crate::ui::viewer::latest::Latest;
use flikk_core::blockout::{FPS, Scene};
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Drawing a frame slower than this makes the next ones narrower.
const SLOW: Duration = Duration::from_millis(45);
/// Drawing a frame faster than this makes the next ones wider again.
const FAST: Duration = Duration::from_millis(16);
/// The narrowest a picture is drawn while it can't keep up.
const NARROWEST: u32 = 192;

/// A scene playing in a loop, drawn off the UI thread at the width it can
/// keep up with. It fills the box it is given.
pub struct BlockoutPlayer {
    scene: Arc<Scene>,
    /// Counts the scenes shown, so a frame of an earlier one is dropped.
    version: u64,
    /// The picture's shape: width to height.
    shape: (u32, u32),
    /// The widest it is drawn, and how wide it is drawn now.
    full_width: u32,
    width: u32,
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

    /// Seconds into the scene, and how long it runs.
    pub fn time(&self) -> (f64, f64) {
        (self.at, self.scene.duration)
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        match self.playing.take() {
            Some(_) => {
                self.clock = None;
                cx.notify();
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
            self.start_drawing(at, cx);
        }
        cx.notify();
    }

    fn start_drawing(&mut self, at: f64, cx: &mut Context<Self>) {
        self.drawing.start();
        let (scene, version) = (self.scene.clone(), self.version);
        let width = self.width;
        let height = (width as u64 * self.shape.1 as u64 / self.shape.0 as u64).max(1) as u32;
        let drawn = cx.background_executor().spawn(async move {
            let began = Instant::now();
            let frame = scene.render(at, width, height);
            // Greys read the same as RGBA and as the BGRA the GPU takes.
            let picture = image::RgbaImage::from_raw(width, height, frame.rgba.to_vec())
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
                this.pace(took);
                if let Some(next) = this.drawing.finish() {
                    this.start_drawing(next, cx);
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
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        div().size_full().bg(t::SCOPE).children(
            self.picture
                .clone()
                .map(|picture| img(picture).size_full().object_fit(ObjectFit::Contain)),
        )
    }
}
