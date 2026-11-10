//! The blockout dialog: the scene playing large, and beside it a thread
//! where the user says what should be different. Each ask is one short
//! run of the agent's backend that answers with the scene changed
//! (`flikk_agent::reblock`); nothing here reaches a video provider. Save
//! hands the scene back to whoever opened the dialog.

use super::BlockoutPlayer;
use crate::state::EditorState;
use crate::ui::kit::play::{play_button, seek_bar};
use crate::ui::kit::{
    Button, DialogExt as _, IconName, Input, ScrollableElement as _, Spinner, h_flex, theme as t,
    v_flex,
};
use flikk_agent::reblock::{Exchange, Reblock, Reblocked};
use flikk_core::blockout::Scene;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;
use std::time::Duration;

/// How often a change being worked out is looked at.
const POLL: Duration = Duration::from_millis(120);
/// The widest the scene is drawn here.
const WIDTH: u32 = 960;
/// How tall the scene's box is; the picture fits inside whatever its shape.
const STAGE: Pixels = px(340.);
/// How wide the thread beside it is.
const THREAD: Pixels = px(296.);

/// Called with the scene the user saved.
pub type OnSave = Rc<dyn Fn(Scene, &mut Window, &mut App)>;

/// What came of one ask.
#[derive(Clone, Debug, PartialEq)]
enum Answer {
    Working,
    Said(String),
    Failed(String),
    /// The user stopped it.
    Stopped,
}

struct Turn {
    ask: String,
    answer: Answer,
}

pub struct BlockoutEditor {
    state: Entity<EditorState>,
    player: Entity<BlockoutPlayer>,
    /// The scene as it stands, and as it was opened.
    scene: Scene,
    opened: Scene,
    /// The picture's shape, width to height.
    shape: (u32, u32),
    /// The scene before each change made here, newest last.
    before: Vec<Scene>,
    thread: Vec<Turn>,
    /// The change being worked out; its turn is the thread's last.
    asking: Option<Reblock>,
    ask: Entity<InputState>,
    scroll: ScrollHandle,
    poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl BlockoutEditor {
    fn new(
        state: Entity<EditorState>,
        scene: Scene,
        shape: (u32, u32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let player = cx.new(|cx| BlockoutPlayer::new(scene.clone(), shape, WIDTH, window, cx));
        let ask = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Lower the camera, have her wave, cut closer at 3 s…")
        });
        let _subscriptions = vec![
            cx.observe(&player, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &ask,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.send(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
        ];
        Self {
            state,
            player,
            opened: scene.clone(),
            scene,
            shape,
            before: Vec::new(),
            thread: demo_thread(),
            asking: None,
            ask,
            scroll: ScrollHandle::new(),
            poll: None,
            _subscriptions,
        }
    }

    /// Whether Save has something to save.
    fn changed(&self) -> bool {
        self.scene != self.opened
    }

    /// Whether Enter belongs to the ask being typed, not to Save.
    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.ask.read(cx).focus_handle(cx).is_focused(window)
            && !self.ask.read(cx).value().trim().is_empty()
    }

    /// Ask for what is typed.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ask = self.ask.read(cx).value().trim().to_owned();
        if ask.is_empty() || self.asking.is_some() {
            return;
        }
        let earlier: Vec<Exchange> = (self.thread.iter())
            .filter_map(|turn| match &turn.answer {
                Answer::Said(answer) => Some(Exchange {
                    ask: turn.ask.clone(),
                    answer: answer.clone(),
                }),
                _ => None,
            })
            .collect();
        let settings = self.state.read(cx).settings.clone();
        let answer = match Reblock::start(&self.scene, self.shape, &earlier, &ask, &settings) {
            Ok(asking) => {
                self.asking = Some(asking);
                self.follow(cx);
                Answer::Working
            }
            Err(e) => Answer::Failed(e.to_string()),
        };
        self.thread.push(Turn { ask, answer });
        self.ask
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Look at the change being worked out until it is over.
    fn follow(&mut self, cx: &mut Context<Self>) {
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let working = this.update(cx, |this, cx| this.look(cx));
                if !matches!(working, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Take what the change being worked out came to; whether it is still
    /// working.
    fn look(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(asking) = self.asking.as_mut() else {
            return false;
        };
        let answer = match asking.poll() {
            Reblocked::Working => return true,
            Reblocked::Done { scene, answer } => {
                if let Some(scene) = scene.filter(|scene| *scene != self.scene) {
                    self.before
                        .push(std::mem::replace(&mut self.scene, scene.clone()));
                    self.player.update(cx, |p, cx| p.set_scene(scene, cx));
                }
                Answer::Said(answer)
            }
            Reblocked::Failed(e) => Answer::Failed(e),
        };
        self.settle(answer, cx);
        false
    }

    /// The ask being worked out is over, with `answer`.
    fn settle(&mut self, answer: Answer, cx: &mut Context<Self>) {
        self.asking = None;
        if let Some(turn) = self.thread.last_mut() {
            turn.answer = answer;
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.asking.is_some() {
            self.settle(Answer::Stopped, cx);
        }
    }

    /// Take the last change back.
    fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(scene) = self.before.pop() {
            self.scene = scene.clone();
            self.player.update(cx, |p, cx| p.set_scene(scene, cx));
            cx.notify();
        }
    }

    fn render_stage(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.player.read(cx);
        let (at, length) = player.time();
        let playing = player.is_playing();
        let seek = self.player.clone();
        v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(8.))
            .child(
                div()
                    .w_full()
                    .h(STAGE)
                    .overflow_hidden()
                    .rounded(t::RADIUS_SM)
                    .border_1()
                    .border_color(t::LN)
                    .child(self.player.clone()),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(play_button("blockout-play", playing).on_click(
                        cx.listener(|this, _, _, cx| this.player.update(cx, |p, cx| p.toggle(cx))),
                    ))
                    .child(seek_bar(
                        (at / length.max(1e-3)) as f32,
                        None,
                        move |fraction, cx| seek.update(cx, |p, cx| p.seek(fraction, cx)),
                        cx,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .font_family(t::mono())
                            .text_size(px(11.))
                            .text_color(t::T2)
                            .child(format!("{at:04.1} / {length:04.1} s")),
                    )
                    .child(
                        Button::new("blockout-undo")
                            .ghost()
                            .small()
                            .icon(IconName::Undo2)
                            .tooltip("Undo change")
                            .disabled(self.before.is_empty() || self.asking.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
                    ),
            )
    }

    fn render_thread(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let turns = self.thread.iter().map(|turn| {
            let answer = match &turn.answer {
                Answer::Working => h_flex()
                    .h(px(18.))
                    .child(Spinner::new().size(px(10.)).color(t::T2)),
                Answer::Said(text) => div().text_color(t::T1).child(text.clone()),
                Answer::Failed(text) => div().text_color(t::ERR).child(text.clone()),
                Answer::Stopped => div().text_color(t::T3).child("Stopped"),
            };
            v_flex()
                .w_full()
                .gap(px(6.))
                .child(
                    h_flex().w_full().justify_end().child(
                        div()
                            .max_w(relative(0.9))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(t::RADIUS_LG)
                            .border_1()
                            .border_color(t::LN)
                            .bg(t::R1)
                            .child(turn.ask.clone()),
                    ),
                )
                .child(answer.w_full().line_height(px(18.)))
        });
        let working = self.asking.is_some();
        let empty = self.ask.read(cx).value().trim().is_empty();
        let send = if working {
            Button::new("blockout-stop")
                .ghost()
                .xsmall()
                .icon(IconName::Square)
                .tooltip("Stop")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("blockout-send")
                .ghost()
                .xsmall()
                .icon(IconName::ArrowUp)
                .tooltip("Ask")
                .disabled(empty)
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
        };
        v_flex()
            .flex_none()
            .w(THREAD)
            .gap(px(8.))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .when(self.thread.is_empty(), |el| {
                        el.child(
                            h_flex()
                                .size_full()
                                .justify_center()
                                .text_color(t::T3)
                                .child("Ask for a change"),
                        )
                    })
                    .when(!self.thread.is_empty(), |el| {
                        el.child(
                            v_flex()
                                .id("blockout-thread")
                                .size_full()
                                .gap(px(12.))
                                .children(turns)
                                .track_scroll(&self.scroll)
                                .overflow_y_scroll(),
                        )
                        .vertical_scrollbar(&self.scroll)
                    }),
            )
            .child(Input::new(&self.ask).suffix(send))
    }
}

impl Render for BlockoutEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_stretch()
            .gap(px(16.))
            .child(self.render_stage(cx))
            .child(self.render_thread(cx))
    }
}

/// Demo only (`FLIKK_DEMO_SHOT=blockout`): a thread with two asks answered.
fn demo_thread() -> Vec<Turn> {
    if crate::demo::shot::asked().as_deref() != Some("blockout") {
        return Vec::new();
    }
    let turn = |ask: &str, answer: &str| Turn {
        ask: ask.into(),
        answer: Answer::Said(answer.into()),
    };
    vec![
        turn(
            "Start the camera lower, at knee height",
            "The camera now starts 0.5 m above the ground and rises as it swings round.",
        ),
        turn(
            "Park the car closer to her path",
            "Moved the car 3 m toward the runner's line, so she passes it at 4 s.",
        ),
    ]
}

/// Open the dialog on `scene`, drawn in a picture `shape` (width to
/// height). `on_save` gets the scene when the user saves a changed one.
pub fn open(
    state: Entity<EditorState>,
    scene: Scene,
    shape: (u32, u32),
    on_save: OnSave,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let editor = cx.new(|cx| BlockoutEditor::new(state, scene, shape, window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let (saving, on_save) = (editor.clone(), on_save.clone());
        let changed = editor.read(cx).changed();
        dialog
            .title("Blockout")
            .w(px(940.))
            .child(editor.clone())
            .confirm("Save", move |window, cx| {
                // Enter in the ask field asks; it doesn't save.
                if saving.read(cx).typing(window, cx) {
                    saving.update(cx, |editor, cx| editor.send(window, cx));
                    return false;
                }
                on_save(saving.read(cx).scene.clone(), window, cx);
                true
            })
            .confirm_disabled(!changed)
    });
}
