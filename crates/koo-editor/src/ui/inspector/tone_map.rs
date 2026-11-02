//! HDR tone mapping of the selected clips' media (`koo_core::hdr`): Auto or
//! Off, exposure and the source peak, through `Command::SetToneMap`.

use super::{InspectorPanel, section_with};
use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::MediaId;
use koo_core::command::Command;
use koo_core::hdr::{self, Mapping, ToneMap, ToneMapMode};
use koo_core::model::{Clip, ClipContent, MediaItem};

/// Sliders of the tone map, kept across renders.
pub(super) struct ToneMapUi {
    state: Entity<EditorState>,
    /// The HDR media of the selected clips; the first one is shown.
    media: Vec<MediaId>,
    exposure: Entity<SliderState>,
    peak: Entity<SliderState>,
    dragging: bool,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
enum Knob {
    Exposure,
    Peak,
}

impl ToneMapUi {
    fn new(state: Entity<EditorState>, cx: &mut Context<Self>) -> Self {
        let exposure = cx.new(|_| {
            SliderState::new()
                .min(hdr::EXPOSURE.0)
                .max(hdr::EXPOSURE.1)
                .step(0.1)
                .default_value(0.0)
        });
        let peak = cx.new(|_| {
            SliderState::new()
                .max(hdr::PEAK.1)
                .min(hdr::PEAK.0)
                .scale(SliderScale::Logarithmic)
                .step(10.0)
                .default_value(hdr::DEFAULT_PEAK)
        });
        let _subscriptions = vec![
            cx.subscribe(&exposure, |this, _, ev: &SliderEvent, cx| {
                this.slid(Knob::Exposure, ev, cx)
            }),
            cx.subscribe(&peak, |this, _, ev: &SliderEvent, cx| {
                this.slid(Knob::Peak, ev, cx)
            }),
        ];
        Self {
            state,
            media: Vec::new(),
            exposure,
            peak,
            dragging: false,
            _subscriptions,
        }
    }

    fn slid(&mut self, knob: Knob, ev: &SliderEvent, cx: &mut Context<Self>) {
        let (v, release) = match ev {
            SliderEvent::Change(v) => (v.start(), false),
            SliderEvent::Release(v) => (v.start(), true),
        };
        self.dragging = !release;
        if release {
            self.commit(
                |tm| match knob {
                    Knob::Exposure => tm.exposure = (v * 10.0).round() / 10.0,
                    Knob::Peak => tm.peak = Some((v / 10.0).round() * 10.0),
                },
                cx,
            );
        }
        cx.notify();
    }

    /// Change every shown item's tone map with `f`, as one undo step.
    fn commit(&self, f: impl Fn(&mut ToneMap), cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let commands: Vec<Command> = self
                .media
                .iter()
                .filter_map(|id| s.bus.project().media(*id))
                .filter_map(|item| {
                    let mut tone_map = item.tone_map.clone();
                    f(&mut tone_map);
                    (tone_map != item.tone_map).then_some(Command::SetToneMap {
                        media: item.id,
                        tone_map,
                    })
                })
                .collect();
            match commands.len() {
                0 => {}
                1 => {
                    s.execute(commands.into_iter().next().unwrap(), cx);
                }
                _ => {
                    s.execute_batch(commands, "set_tone_map", cx);
                }
            }
        });
    }

    /// Show `item`'s values unless a slider is held.
    fn sync(&mut self, media: Vec<MediaId>, item: &MediaItem, window: &mut Window, cx: &mut App) {
        self.media = media;
        if self.dragging {
            return;
        }
        let exposure = item.tone_map.exposure;
        let peak = shown_peak(item);
        self.exposure.update(cx, |s, cx| {
            if (s.value().start() - exposure).abs() > 1e-4 {
                s.set_value(exposure, window, cx);
            }
        });
        self.peak.update(cx, |s, cx| {
            if (s.value().start() - peak).abs() > 0.5 {
                s.set_value(peak, window, cx);
            }
        });
    }
}

/// The peak the mapping uses: the item's own, else its metadata's.
fn shown_peak(item: &MediaItem) -> f32 {
    let auto = MediaItem {
        tone_map: ToneMap {
            mode: ToneMapMode::Auto,
            ..item.tone_map.clone()
        },
        ..item.clone()
    };
    Mapping::of(&auto).map_or(hdr::DEFAULT_PEAK, |m| m.peak)
}

impl InspectorPanel {
    /// The tone map of the selected clips' HDR media; `None` when none is.
    pub(super) fn tone_map_section(
        &mut self,
        clips: &[Clip],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project(cx);
        let mut items: Vec<MediaItem> = Vec::new();
        for clip in clips {
            if let ClipContent::Media { media, .. } = &clip.content
                && let Some(item) = project.media(*media)
                && hdr::hdr_of(item).is_some()
                && !items.iter().any(|i| i.id == item.id)
            {
                items.push(item.clone());
            }
        }
        let item = items.first()?.clone();
        let ids: Vec<MediaId> = items.iter().map(|i| i.id).collect();
        let state = self.state.clone();
        let ui = window.use_keyed_state("tone-map-ui", cx, move |_, cx| ToneMapUi::new(state, cx));
        ui.update(cx, |ui, cx| ui.sync(ids, &item, window, cx));
        let tm = item.tone_map.clone();
        let auto = tm.mode == ToneMapMode::Auto;
        let muted = cx.theme().muted_foreground;
        let row = |label: &'static str, control: AnyElement, value: Option<String>| {
            h_flex()
                .h(px(28.))
                .gap_2()
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(control))
                .when_some(value, |row, value| {
                    row.child(
                        div()
                            .w(px(48.))
                            .flex_none()
                            .text_xs()
                            .text_right()
                            .text_color(muted)
                            .child(value),
                    )
                })
        };
        let (mode_ui, reset_ui) = (ui.clone(), ui.clone());
        let mode = TabBar::new("tone-map-mode")
            .segmented()
            .xsmall()
            .selected_index(if auto { 0 } else { 1 })
            .child(Tab::new().label("Auto"))
            .child(Tab::new().label("Off"))
            .on_click(move |ix, _, cx| {
                let mode = if *ix == 0 {
                    ToneMapMode::Auto
                } else {
                    ToneMapMode::Off
                };
                mode_ui.update(cx, |ui, cx| ui.commit(|tm| tm.mode = mode, cx));
            });
        let reset = Button::new("tone-map-reset")
            .ghost()
            .xsmall()
            .icon(IconName::Undo2)
            .tooltip("Reset")
            .disabled(tm.is_default())
            .on_click(move |_, _, cx| {
                reset_ui.update(cx, |ui, cx| ui.commit(|tm| *tm = ToneMap::default(), cx))
            });
        let (exposure, peak) = {
            let ui = ui.read(cx);
            (ui.exposure.clone(), ui.peak.clone())
        };
        let shown_exposure = exposure.read(cx).value().start();
        let shown_peak = peak.read(cx).value().start();
        let title = hdr::hdr_of(&item).map_or("HDR".to_string(), |h| h.describe());
        Some(
            section_with("HDR", reset, cx)
                .child(
                    div()
                        .id("tone-map-source")
                        .child(row("Tone map", mode.into_any_element(), None))
                        .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx)),
                )
                .when(auto, |s| {
                    s.child(row(
                        "Exposure",
                        Slider::new(&exposure).w_full().into_any_element(),
                        Some(format!("{shown_exposure:+.1}")),
                    ))
                    .child(row(
                        "Peak",
                        Slider::new(&peak).w_full().into_any_element(),
                        Some(format!("{shown_peak:.0}")),
                    ))
                })
                .into_any_element(),
        )
    }
}
