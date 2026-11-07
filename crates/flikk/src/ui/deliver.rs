//! Deliver workspace (PRD D1–D3, C6): preset cards, the format, encoder
//! and range, loudness and burn-in, the output file, caption/timeline/still
//! exports and the render queue.

use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, Checkbox, Icon, IconName, Input, Panel, PanelHeader, Progress, ScrollableElement as _,
    Segmented, Select, SelectEvent, SelectState, Spinner, Tooltip, h_flex, v_flex,
};
use flikk_core::Rate;
use flikk_core::memory::{CaptionStyle, Presets};
use flikk_core::model::{Project, SequenceSettings};
use flikk_deliver::{
    AudioCodec, Encoder, LoudnessTarget, RenderJob, RenderQueue, RenderRequest, RenderSettings,
    RenderState, VideoCodec,
};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What an export button writes.
#[derive(Clone, Copy)]
enum Export {
    Srt,
    Ass,
    Otio,
    Fcpxml,
    Still,
}

impl Export {
    /// The export rows: a label and its buttons.
    const GROUPS: [(&'static str, &'static [Export]); 3] = [
        ("Captions", &[Export::Srt, Export::Ass]),
        ("Timeline", &[Export::Otio, Export::Fcpxml]),
        ("Still", &[Export::Still]),
    ];

    fn label(self) -> &'static str {
        match self {
            Export::Srt => "SRT",
            Export::Ass => "ASS",
            Export::Otio => "OTIO",
            Export::Fcpxml => "FCPXML",
            Export::Still => "PNG at playhead",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Export::Srt => "srt",
            Export::Ass => "ass",
            Export::Otio => "otio",
            Export::Fcpxml => "fcpxml",
            Export::Still => "png",
        }
    }
}

/// A preset card. Presets that differ only in codec share one and are
/// told apart in the Format row.
struct Card {
    title: String,
    /// Indices into the presets.
    presets: Vec<usize>,
}

pub struct DeliverPanel {
    state: Entity<EditorState>,
    queue: RenderQueue,
    presets: Vec<RenderSettings>,
    cards: Vec<Card>,
    preset: usize,
    /// The formats of the chosen card, when it has more than one.
    format_select: Entity<SelectState<SharedString>>,
    /// Custom preset: width, height, fps, CRF.
    custom: [Entity<InputState>; 4],
    /// Sequence size the custom width and height were last filled from.
    sequence_size: (u32, u32),
    /// Sample rate the format labels were last written with.
    sample_rate: u32,
    /// The output the user chose; without one it follows the project name
    /// in the default folder. Each render writes the first free name from it.
    output: Option<PathBuf>,
    loudness: bool,
    burn_captions: bool,
    /// Ignore the in and out marks.
    whole_timeline: bool,
    /// For H.264 and HEVC; saved for the user.
    encoder: Encoder,
    polling: bool,
    _subscriptions: Vec<Subscription>,
}

impl DeliverPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let presets = RenderSettings::presets();
        let mut cards: Vec<Card> = Vec::new();
        for (ix, preset) in presets.iter().enumerate() {
            let title = card_title(preset);
            match cards.iter_mut().find(|c| c.title == title) {
                Some(card) => card.presets.push(ix),
                None => cards.push(Card {
                    title,
                    presets: vec![ix],
                }),
            }
        }
        let seq = state.read(cx).bus.project().timeline.settings.clone();
        let formats = cards[0]
            .presets
            .iter()
            .map(|&ix| format_label(&presets[ix], &seq))
            .collect();
        let format_select = cx.new(|cx| SelectState::new(formats, Some(0), window, cx));
        let input = |value: String, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value))
        };
        let custom = [
            input(seq.width.to_string(), window, cx),
            input(seq.height.to_string(), window, cx),
            input(format_fps(seq.rate), window, cx),
            input("18".into(), window, cx),
        ];
        let mut _subscriptions = vec![
            cx.observe_in(&state, window, |this: &mut Self, state, window, cx| {
                let seq = &state.read(cx).bus.project().timeline.settings;
                let (size, rate) = ((seq.width, seq.height), seq.sample_rate);
                if size != this.sequence_size {
                    this.sequence_size = size;
                    for (input, v) in this.custom.iter().zip([size.0, size.1]) {
                        input.update(cx, |i, cx| i.set_value(v.to_string(), window, cx));
                    }
                }
                if rate != this.sample_rate {
                    this.sample_rate = rate;
                    this.relabel_formats(window, cx);
                }
                cx.notify();
            }),
            // By row, not label: the labels name the sample rate, which can
            // change while the list shows.
            cx.subscribe(
                &format_select,
                |this, select, _: &SelectEvent<SharedString>, cx| {
                    let card = this.card_of(this.preset);
                    let picked = select
                        .read(cx)
                        .selected_index()
                        .and_then(|row| this.cards[card].presets.get(row).copied());
                    if let Some(ix) = picked {
                        this.select_preset(ix, cx);
                    }
                },
            ),
        ];
        // Custom fields: the Render button and the summary follow them.
        _subscriptions.extend(
            custom
                .iter()
                .map(|input| cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())),
        );
        // Which hardware encoders work, found off the UI thread.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async { flikk_deliver::hw::probe() })
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        // Shared with the chat, so renders the agent queues show up here.
        let queue = state.read(cx).render_queue.clone();
        Self {
            state,
            queue,
            presets,
            cards,
            preset: 0,
            format_select,
            custom,
            sequence_size: (seq.width, seq.height),
            sample_rate: seq.sample_rate,
            output: None,
            loudness: false,
            burn_captions: true,
            whole_timeline: false,
            encoder: saved_encoder(),
            polling: false,
            _subscriptions,
        }
    }

    fn project(&self, cx: &App) -> Project {
        self.state.read(cx).bus.project().clone()
    }

    fn sequence(&self, cx: &App) -> SequenceSettings {
        self.state.read(cx).bus.project().timeline.settings.clone()
    }

    fn card_of(&self, preset: usize) -> usize {
        self.cards
            .iter()
            .position(|c| c.presets.contains(&preset))
            .unwrap_or(0)
    }

    fn select_preset(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.preset = ix;
        let ext = self.presets[ix].extension();
        if let Some(out) = &mut self.output {
            out.set_extension(ext);
        }
        cx.notify();
    }

    fn select_card(&mut self, card: usize, window: &mut Window, cx: &mut Context<Self>) {
        let members = self.cards[card].presets.clone();
        if members.contains(&self.preset) {
            return;
        }
        let seq = self.sequence(cx);
        let formats = members
            .iter()
            .map(|&ix| format_label(&self.presets[ix], &seq))
            .collect();
        self.format_select.update(cx, |s, cx| {
            s.set_items(formats, window, cx);
            s.set_selected_index(Some(0), window, cx);
        });
        self.select_preset(members[0], cx);
    }

    /// Write the chosen card's format labels anew, keeping the pick.
    fn relabel_formats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.sequence(cx);
        let members = &self.cards[self.card_of(self.preset)].presets;
        let formats = members
            .iter()
            .map(|&ix| format_label(&self.presets[ix], &seq))
            .collect();
        let row = members.iter().position(|&ix| ix == self.preset);
        self.format_select.update(cx, |s, cx| {
            s.set_items(formats, window, cx);
            s.set_selected_index(row, window, cx);
        });
    }

    fn set_status(&self, message: impl Into<String>, cx: &mut Context<Self>) {
        let message = message.into();
        self.state.update(cx, |s, cx| {
            s.status = Some(message);
            cx.notify();
        });
    }

    /// The caption presets edited in the transcript panel.
    fn caption_styles(&self, cx: &App) -> Vec<CaptionStyle> {
        self.state.read(cx).presets.caption_styles.clone()
    }

    fn loudness_target(&self) -> LoudnessTarget {
        let p = Presets::default();
        LoudnessTarget {
            lufs: p.loudness_lufs,
            true_peak_db: p.true_peak_db,
        }
    }

    fn file_stem(&self, cx: &App) -> String {
        let name = self.state.read(cx).bus.project().name.trim().to_string();
        let name: String = name
            .chars()
            .map(|c| if r#"/\:*?"<>|"#.contains(c) { '_' } else { c })
            .collect();
        if name.is_empty() {
            "Untitled".into()
        } else {
            name
        }
    }

    /// What Render writes next: the chosen output or the project's name in
    /// the default folder, made free of files and of queued renders.
    fn output(&self, cx: &App) -> PathBuf {
        let base = self.output.clone().unwrap_or_else(|| {
            let name = format!(
                "{}.{}",
                self.file_stem(cx),
                self.presets[self.preset].extension()
            );
            default_dir().join(name)
        });
        free_output(&base, &self.queue)
    }

    fn settings(&self, cx: &App) -> Result<RenderSettings, String> {
        let mut settings = self.presets[self.preset].clone();
        if settings.name == RenderSettings::custom().name {
            let [w, h, fps, crf] = self.custom.each_ref().map(|i| i.read(cx).value());
            let int = |s: &str| s.trim().parse::<u32>().ok();
            let w = int(&w).filter(|v| (16..=8192).contains(v));
            let h = int(&h).filter(|v| (16..=8192).contains(v));
            let rate = parse_rate(&fps);
            let crf = int(&crf).filter(|v| *v <= 51);
            match (w, h, rate, crf) {
                (Some(w), Some(h), Some(rate), Some(crf)) => {
                    settings.width = Some(w & !1);
                    settings.height = Some(h & !1);
                    settings.rate = Some(rate);
                    settings.video = Some(VideoCodec::H264 { crf: crf as u8 });
                }
                _ => return Err("Check the custom size, frame rate and CRF".into()),
            }
        }
        settings.loudness = self.loudness.then(|| self.loudness_target());
        settings.burn_captions = settings.video.is_some() && self.burn_captions;
        settings.range = self
            .state
            .read(cx)
            .in_out
            .filter(|r| !r.is_empty() && !self.whole_timeline);
        settings.encoder = self.encoder;
        Ok(settings)
    }

    fn choose_output(&mut self, cx: &mut Context<Self>) {
        let current = self.output(cx);
        let dir = current
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(default_dir);
        let name = current
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = self.presets[self.preset].extension();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut path))) = rx.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension(ext);
            }
            let _ = this.update(cx, |this, cx| {
                this.output = Some(path);
                cx.notify();
            });
        })
        .detach();
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        let settings = match self.settings(cx) {
            Ok(s) => s,
            Err(e) => return self.set_status(e, cx),
        };
        let output = self.output(cx);
        let request = RenderRequest {
            project: self.project(cx),
            settings,
            output: output.clone(),
            caption_styles: self.caption_styles(cx),
            services: self.state.read(cx).services.clone(),
        };
        match self.queue.enqueue(request) {
            Ok(_) => {
                // The next render gets a fresh name: the queue holds this one.
                self.poll(cx);
                cx.notify();
            }
            Err(e) => self.set_status(format!("{e:#}"), cx),
        }
    }

    /// Refresh while jobs run.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let busy = this.update(cx, |this, cx| {
                    cx.notify();
                    this.polling = this.queue.is_busy();
                    this.polling
                });
                if !matches!(busy, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn export(&mut self, kind: Export, cx: &mut Context<Self>) {
        let dir = self
            .output
            .as_ref()
            .and_then(|o| o.parent().map(Path::to_path_buf))
            .unwrap_or_else(default_dir);
        let name = format!("{}.{}", self.file_stem(cx), kind.extension());
        let suggested = flikk_deliver::free_name(&dir.join(&name));
        let suggested = suggested
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(name);
        let project = self.project(cx);
        let playhead = self.state.read(cx).playhead;
        let styles = self.caption_styles(cx);
        let services = self.state.read(cx).services.clone();
        let rx = cx.prompt_for_new_path(&dir, Some(&suggested));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut path))) = rx.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension(kind.extension());
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    match kind {
                        Export::Srt => flikk_deliver::export_srt(&project, None, &styles, &path),
                        Export::Ass => flikk_deliver::export_ass(&project, &styles, &path),
                        Export::Otio => flikk_deliver::export_otio(&project, &path),
                        Export::Fcpxml => flikk_deliver::export_fcpxml(&project, &path),
                        Export::Still => flikk_deliver::export_frame(
                            &project, playhead, &styles, &services, &path,
                        ),
                    }
                })
                .await;
            if let Err(e) = result {
                let _ = this.update(cx, |this, cx| this.set_status(format!("{e:#}"), cx));
            }
        })
        .detach();
    }

    fn set_encoder(&mut self, encoder: Encoder, cx: &mut Context<Self>) {
        self.encoder = encoder;
        crate::persistence::with_store(|s| s.save_render_encoder(encoder.key()));
        cx.notify();
    }

    /// The preset cards, three to a row.
    fn render_cards(&self, seq: &SequenceSettings, cx: &mut Context<Self>) -> impl IntoElement {
        let cards: Vec<AnyElement> = self
            .cards
            .iter()
            .enumerate()
            .map(|(ix, card)| {
                let selected = card.presets.contains(&self.preset);
                let shown = &self.presets[if selected {
                    self.preset
                } else {
                    card.presets[0]
                }];
                div()
                    .id(("preset", ix))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(8.))
                    .rounded(t::RADIUS)
                    .border_1()
                    .map(|d| {
                        if selected {
                            d.border_color(t::AC)
                                .bg(t::alpha(t::AC, 0.06))
                                .shadow(t::field_ring())
                        } else {
                            d.border_color(t::LN)
                                .bg(t::R1)
                                .cursor_pointer()
                                .hover(|s| s.bg(t::HV))
                        }
                    })
                    .child(
                        div().mb(px(4.)).child(
                            Icon::new(card_icon(shown, seq))
                                .size(px(15.))
                                .color(if selected { t::AC } else { t::T2 }),
                        ),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(card.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(t::mono())
                            .text_size(px(10.))
                            .text_color(t::T3)
                            .child(card_spec(shown, seq)),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select_card(ix, window, cx)),
                    )
                    .into_any_element()
            })
            .collect();
        let mut grid = v_flex().gap(px(6.));
        let mut cards = cards.into_iter().peekable();
        while cards.peek().is_some() {
            let mut row = h_flex().gap(px(6.)).items_stretch();
            for _ in 0..3 {
                row = row.child(match cards.next() {
                    Some(card) => card,
                    None => div().flex_1().into_any_element(),
                });
            }
            grid = grid.child(row);
        }
        section().child(grid)
    }

    /// Auto, Hardware, Software, for presets a hardware encoder can take.
    fn render_encoder(&self, preset: &RenderSettings, cx: &mut Context<Self>) -> impl IntoElement {
        let codec = preset.hardware_codec().map(|(codec, _)| codec);
        let probe = flikk_deliver::hw::probed();
        let found = probe
            .zip(codec)
            .and_then(|(p, codec)| p.best(codec))
            .map(|e| e.api.label());
        // Known to be missing only once the probe is done.
        let missing = probe.is_some() && found.is_none();
        let position = |e: Encoder| Encoder::ALL.iter().position(|&x| x == e).unwrap_or(0);
        div()
            .id("encoder")
            .when_some(found, |d, api| d.tooltip(Tooltip::text(api)))
            .child(
                Segmented::new("encoder-choice")
                    .compact()
                    .fill()
                    .items(Encoder::ALL.iter().map(|e| e.label()))
                    .selected(position(self.encoder))
                    .disabled_item(position(Encoder::Hardware), missing)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.set_encoder(Encoder::ALL[*ix], cx)
                    })),
            )
    }

    /// Format, custom size and rate, encoder and range.
    fn render_settings(&self, seq: &SequenceSettings, cx: &mut Context<Self>) -> impl IntoElement {
        let preset = &self.presets[self.preset];
        let custom = preset.name == RenderSettings::custom().name;
        let choices = self.cards[self.card_of(self.preset)].presets.len() > 1;
        let format = if choices {
            Select::new(&self.format_select).small().into_any_element()
        } else {
            format_field(format_parts(preset, seq)).into_any_element()
        };
        let in_out = self.state.read(cx).in_out.filter(|r| !r.is_empty());
        let field = |input: Input| div().flex_1().min_w_0().child(input.font_family(t::mono()));
        let pair = |a: Input, b: Input| h_flex().gap(px(6.)).child(field(a)).child(field(b));
        let [w, h, fps, crf] = &self.custom;

        section()
            .gap(px(4.))
            .child(row("Format", format))
            .when(custom, |el| {
                el.child(row(
                    "Size",
                    pair(
                        Input::new(w).small().prefix("W"),
                        Input::new(h).small().prefix("H"),
                    ),
                ))
                .child(row(
                    "Frame rate",
                    pair(
                        Input::new(fps).small().suffix("fps"),
                        Input::new(crf).small().prefix("CRF"),
                    ),
                ))
            })
            .when(preset.hardware_codec().is_some(), |el| {
                el.child(row("Encoder", self.render_encoder(preset, cx)))
            })
            .when(in_out.is_some(), |el| {
                el.child(row(
                    "Range",
                    Segmented::new("range")
                        .compact()
                        .fill()
                        .item("Whole timeline")
                        .item("In → Out")
                        .selected(if self.whole_timeline { 0 } else { 1 })
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.whole_timeline = *ix == 0;
                            cx.notify();
                        })),
                ))
            })
    }
}

impl DeliverPanel {
    /// Loudness normalization and caption burn-in.
    fn render_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target = self.loudness_target();
        let has_video = self.presets[self.preset].video.is_some();
        section()
            .py(px(8.))
            .child(
                h_flex()
                    .h(px(26.))
                    .child(
                        Checkbox::new("loudness")
                            .label(format!("Normalize to {} LUFS", minus(target.lufs)))
                            .checked(self.loudness)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.loudness = *checked;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        meta(format!("{} dBTP", minus(target.true_peak_db))).font_family(t::mono()),
                    ),
            )
            .when(has_video, |el| {
                el.child(
                    h_flex().h(px(26.)).child(
                        Checkbox::new("burn-captions")
                            .label("Burn in captions")
                            .checked(self.burn_captions)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.burn_captions = *checked;
                                cx.notify();
                            })),
                    ),
                )
            })
    }

    /// The output file, Render, and what it will write.
    fn render_output(&self, seq: &SequenceSettings, cx: &mut Context<Self>) -> impl IntoElement {
        let output = self.output(cx);
        let name: SharedString = output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        let settings = self.settings(cx);
        let summary = match &settings {
            Ok(s) => {
                let duration = s
                    .range
                    .map(|r| r.duration())
                    .unwrap_or_else(|| self.state.read(cx).bus.project().timeline.duration());
                let lead = if s.video.is_some() {
                    let (w, h) = s.size(seq);
                    format!("{w}×{h} · {} fps ·", format_fps(s.rate.unwrap_or(seq.rate)))
                } else {
                    format!("{} kHz · {} ·", khz(seq.sample_rate), audio_label(s.audio))
                };
                h_flex()
                    .gap(px(4.))
                    .child(lead)
                    .child(
                        div()
                            .font_family(t::mono())
                            .child(duration.timecode(seq.rate)),
                    )
                    .into_any_element()
            }
            Err(e) => div().text_color(t::ERR).child(e.clone()).into_any_element(),
        };
        section()
            .gap(px(8.))
            .child(
                h_flex()
                    .h(px(40.))
                    .gap(px(8.))
                    .pl(px(10.))
                    .pr(px(8.))
                    .rounded(t::RADIUS)
                    .border_1()
                    .border_color(t::LN)
                    .bg(t::G)
                    .child(Icon::new(IconName::FolderOpen).color(t::T2))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(name))
                            .child(
                                div()
                                    .truncate()
                                    .font_family(t::mono())
                                    .text_size(px(10.))
                                    .text_color(t::T3)
                                    .child(display_dir(&output)),
                            ),
                    )
                    .child(
                        Button::new("output")
                            .ghost()
                            .small()
                            .label("Change")
                            .on_click(cx.listener(|this, _, _, cx| this.choose_output(cx))),
                    ),
            )
            .child(
                Button::new("render")
                    .primary()
                    .icon(IconName::Upload)
                    .label("Render")
                    .disabled(settings.is_err())
                    .w_full()
                    .h(px(32.))
                    .text_size(px(13.))
                    .on_click(cx.listener(|this, _, _, cx| this.start_render(cx))),
            )
            .child(div().text_size(px(11.)).text_color(t::T3).child(summary))
    }

    /// Caption, timeline and still exports.
    fn render_exports(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = Export::GROUPS
            .iter()
            .map(|&(label, kinds)| {
                let buttons: Vec<Button> = kinds
                    .iter()
                    .map(|&kind| {
                        Button::new(kind.label())
                            .small()
                            .label(kind.label())
                            .when(matches!(kind, Export::Still), |b| b.icon(IconName::Frame))
                            .on_click(cx.listener(move |this, _, _, cx| this.export(kind, cx)))
                    })
                    .collect();
                h_flex()
                    .gap(px(10.))
                    .child(
                        div()
                            .w(px(52.))
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(t::T2)
                            .child(label),
                    )
                    .child(h_flex().gap(px(4.)).children(buttons))
                    .into_any_element()
            })
            .collect();
        section().gap(px(8.)).children(rows)
    }

    /// Running and queued jobs in order, then finished ones, newest first.
    fn render_queue(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let jobs = self.queue.jobs();
        if jobs.is_empty() {
            return None;
        }
        let count = jobs.len();
        let (finished, active): (Vec<_>, Vec<_>) =
            jobs.into_iter().partition(|j| j.state.is_finished());
        let any_finished = !finished.is_empty();
        let jobs: Vec<RenderJob> = active
            .into_iter()
            .chain(finished.into_iter().rev())
            .collect();
        let last = jobs.len() - 1;
        let rows: Vec<AnyElement> = jobs
            .iter()
            .enumerate()
            .map(|(ix, job)| self.render_job(job, ix == last, cx))
            .collect();
        Some(
            v_flex()
                .child(
                    PanelHeader::new()
                        .title("Render queue")
                        .context(count.to_string())
                        .child(
                            Button::new("clear-finished")
                                .ghost()
                                .xsmall()
                                .label("Clear finished")
                                .disabled(!any_finished)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.queue.clear_finished();
                                    cx.notify();
                                })),
                        ),
                )
                .children(rows),
        )
    }

    fn render_job(&self, job: &RenderJob, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = job.id;
        let key = id.0.as_u128() as u64;
        let name: SharedString = job
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        let status = |name: IconName, color: Rgba| {
            Icon::new(name)
                .size(px(13.))
                .color(color)
                .into_any_element()
        };
        let (icon, trailing, running) = match &job.state {
            RenderState::Queued => (status(IconName::Clock, t::T3), Some(meta("Queued")), None),
            RenderState::Running { fraction } => (
                Spinner::new().size(px(13.)).color(t::AC).into_any_element(),
                Some(
                    meta(format!("{:.0} %", fraction * 100.))
                        .font_family(t::mono())
                        .text_color(t::T2),
                ),
                Some(*fraction),
            ),
            RenderState::Done => (
                status(IconName::Check, t::OK),
                file_size(&job.output).map(|size| meta(size).font_family(t::mono())),
                None,
            ),
            RenderState::Failed { .. } => (status(IconName::CircleAlert, t::ERR), None, None),
            RenderState::Canceled => (status(IconName::Ban, t::T3), Some(meta("Canceled")), None),
        };
        let action = if job.state == RenderState::Done {
            let output = job.output.clone();
            Button::new(("job-reveal", key))
                .ghost()
                .xsmall()
                .icon(IconName::FolderSearch)
                .tooltip(REVEAL)
                .on_click(move |_, _, cx| cx.reveal_path(&output))
        } else {
            let finished = job.state.is_finished();
            Button::new(("job-close", key))
                .ghost()
                .xsmall()
                .icon(IconName::X)
                .tooltip(if finished { "Remove" } else { "Cancel" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if finished {
                        this.queue.remove(id);
                    } else {
                        this.queue.cancel(id);
                    }
                    cx.notify();
                }))
        };
        let dim = matches!(job.state, RenderState::Queued | RenderState::Canceled);
        let message = match &job.state {
            RenderState::Failed { message } => Some(message.clone()),
            _ => None,
        };
        let note = |text: SharedString| {
            div()
                .pl(px(21.))
                .mt(px(2.))
                .text_size(px(10.))
                .text_color(t::T3)
                .child(text)
        };
        v_flex()
            .px(px(12.))
            .py(px(9.))
            .when(running.is_some(), |d| d.bg(t::alpha(t::AC, 0.04)))
            .when(!last, |d| d.border_b_1().border_color(t::LN))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(icon)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(dim, |d| d.text_color(t::T2))
                            .child(name),
                    )
                    .children(trailing)
                    .child(action),
            )
            .when_some(running, |el, fraction| {
                el.child(Progress::new().value(fraction).mt(px(7.))).child(
                    div()
                        .mt(px(4.))
                        .text_size(px(10.))
                        .text_color(t::T3)
                        .child(job.label.clone()),
                )
            })
            .when(job.state == RenderState::Done, |el| {
                el.child(note(job.label.clone().into()))
            })
            .when_some(message, |el, message| {
                el.child(
                    note(message.into())
                        .mt(px(3.))
                        .text_size(px(11.))
                        .text_color(t::ERR)
                        .line_clamp(3),
                )
            })
            .into_any_element()
    }
}

impl Render for DeliverPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Renders the agent queued keep the progress moving too.
        if self.queue.is_busy() {
            self.poll(cx);
        }
        let seq = self.sequence(cx);
        Panel::new()
            .child(PanelHeader::new().title("Deliver"))
            .child(
                div()
                    .id("deliver")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .child(self.render_cards(&seq, cx))
                            .child(self.render_settings(&seq, cx))
                            .child(self.render_options(cx))
                            .child(self.render_output(&seq, cx))
                            .child(self.render_exports(cx))
                            .children(self.render_queue(cx)),
                    ),
            )
    }
}

/// "Show in Finder" and its equivalents.
pub(crate) const REVEAL: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in folder"
};

/// A block of the panel, ruled off below.
fn section() -> Div {
    v_flex().p(px(12.)).border_b_1().border_color(t::LN)
}

/// A settings row: the label, then its control.
fn row(label: &'static str, control: impl IntoElement) -> Div {
    h_flex()
        .min_h(px(30.))
        .gap(px(8.))
        .child(div().w(px(78.)).flex_none().text_color(t::T2).child(label))
        .child(div().flex_1().min_w_0().child(control))
}

/// Small meta text.
fn meta(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .text_size(px(11.))
        .text_color(t::T3)
        .child(text.into())
}

/// The format of a card with a single preset, read-only.
fn format_field(parts: Vec<String>) -> Div {
    let mut field = h_flex()
        .h(px(22.))
        .px(px(8.))
        .gap(px(6.))
        .rounded(t::RADIUS_SM)
        .border_1()
        .border_color(t::LN)
        .bg(t::G)
        .text_size(px(11.))
        .whitespace_nowrap()
        .overflow_hidden();
    for (ix, part) in parts.into_iter().enumerate() {
        if ix > 0 {
            field = field.child(div().text_color(t::T3).child("·"));
        }
        field = field.child(part);
    }
    field
}

fn is_custom(preset: &RenderSettings) -> bool {
    preset.name == RenderSettings::custom().name
}

/// `1080p60`, `4K30`, `Master`, `Audio`, `Custom`.
fn card_title(preset: &RenderSettings) -> String {
    match (preset.video, preset.short_side, preset.rate) {
        _ if is_custom(preset) => "Custom".into(),
        (None, ..) => "Audio".into(),
        (Some(VideoCodec::ProRes422Hq), ..) => "Master".into(),
        (Some(_), Some(short), Some(rate)) => {
            let size = if short >= 2160 {
                "4K".to_string()
            } else {
                format!("{short}p")
            };
            format!("{size}{}", rate.rounded_fps())
        }
        _ => preset.name.clone(),
    }
}

fn card_icon(preset: &RenderSettings, seq: &SequenceSettings) -> IconName {
    match preset.video {
        _ if is_custom(preset) => IconName::SlidersHorizontal,
        None => IconName::AudioLines,
        Some(VideoCodec::ProRes422Hq) => IconName::Film,
        Some(_) if preset.short_side.is_some_and(|s| s >= 2160) => IconName::Clapperboard,
        Some(_) if seq.height > seq.width => IconName::Smartphone,
        Some(_) => IconName::MonitorPlay,
    }
}

/// The line under a card's title: size and codec, or the audio format.
fn card_spec(preset: &RenderSettings, seq: &SequenceSettings) -> String {
    match preset.video {
        _ if is_custom(preset) => "W × H · CRF".into(),
        None => format!(
            "{} · {} kHz",
            preset.extension().to_uppercase(),
            khz(seq.sample_rate)
        ),
        Some(VideoCodec::ProRes422Hq) => codec_label(VideoCodec::ProRes422Hq).into(),
        Some(codec) => {
            let (w, h) = preset.size(seq);
            format!("{w}×{h} · {}", codec_label(codec))
        }
    }
}

fn codec_label(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 { .. } => "H.264",
        VideoCodec::H265 { .. } => "HEVC",
        VideoCodec::ProRes422Hq => "ProRes 422 HQ",
    }
}

fn audio_label(codec: AudioCodec) -> String {
    match codec {
        AudioCodec::Aac { kbps } => format!("AAC {kbps}"),
        AudioCodec::Pcm24 => "PCM 24-bit".into(),
    }
}

/// Container, video codec, audio codec, and the sample rate for audio only.
fn format_parts(preset: &RenderSettings, seq: &SequenceSettings) -> Vec<String> {
    let mut parts = vec![preset.extension().to_uppercase()];
    parts.extend(preset.video.map(|v| codec_label(v).to_string()));
    parts.push(audio_label(preset.audio));
    if preset.video.is_none() {
        parts.push(format!("{} kHz", khz(seq.sample_rate)));
    }
    parts
}

fn format_label(preset: &RenderSettings, seq: &SequenceSettings) -> SharedString {
    format_parts(preset, seq).join(" · ").into()
}

/// `48`, `44.1`.
fn khz(sample_rate: u32) -> String {
    (sample_rate as f32 / 1000.).to_string()
}

/// A number with a typographic minus.
fn minus(v: f32) -> String {
    v.to_string().replace('-', "\u{2212}")
}

/// `path` if free, else the first free `name (2).ext`, `name (3).ext`, …,
/// like [`flikk_deliver::free_name`]; a render still in `queue` holds its
/// output before the file exists.
fn free_output(path: &Path, queue: &RenderQueue) -> PathBuf {
    let queued: Vec<PathBuf> = queue
        .jobs()
        .into_iter()
        .filter(|j| !j.state.is_finished())
        .map(|j| j.output)
        .collect();
    let taken = |p: &Path| p.exists() || queued.iter().any(|q| q == p);
    if !taken(path) {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    (2..)
        .map(|n| {
            let name = match &ext {
                Some(ext) => format!("{stem} ({n}).{ext}"),
                None => format!("{stem} ({n})"),
            };
            path.with_file_name(name)
        })
        .find(|p| !taken(p))
        .expect("unbounded")
}

/// The output's folder, with the home folder as `~`.
fn display_dir(output: &Path) -> String {
    let dir = output.parent().unwrap_or(output);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => dir.display().to_string(),
    }
}

/// A finished render's size on disk.
fn file_size(path: &Path) -> Option<String> {
    let bytes = std::fs::metadata(path).ok()?.len() as f64;
    Some(if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else if bytes >= 1e6 {
        format!("{:.0} MB", bytes / 1e6)
    } else {
        format!("{:.0} KB", (bytes / 1e3).max(1.))
    })
}

fn format_fps(rate: Rate) -> String {
    if rate.den == 1 {
        rate.num.to_string()
    } else {
        format!("{:.3}", rate.fps())
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// `60`, `29.97`, `30000/1001`.
fn parse_rate(text: &str) -> Option<Rate> {
    let text = text.trim();
    if let Some((n, d)) = text.split_once('/') {
        let (num, den) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
        return (num > 0 && den > 0).then_some(Rate { num, den });
    }
    let fps: f64 = text.parse().ok()?;
    if !(1.0..=240.0).contains(&fps) {
        return None;
    }
    // NTSC rates as exact rationals.
    for base in [24u32, 30, 60, 120] {
        if (fps - base as f64 * 1000.0 / 1001.0).abs() < 0.01 {
            return Some(Rate {
                num: base * 1000,
                den: 1001,
            });
        }
    }
    let rounded = fps.round();
    if (fps - rounded).abs() < 1e-6 {
        return Some(Rate {
            num: rounded as u32,
            den: 1,
        });
    }
    Some(Rate {
        num: (fps * 1000.0).round() as u32,
        den: 1000,
    })
}

/// The encoder the user picked last, else Auto.
pub(crate) fn saved_encoder() -> Encoder {
    crate::persistence::with_store(|s| s.render_encoder())
        .flatten()
        .and_then(|k| Encoder::parse(&k))
        .unwrap_or_default()
}

/// Movies (macOS), Videos (Windows), else the home folder.
pub(crate) fn default_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let videos = if cfg!(target_os = "macos") {
        home.join("Movies")
    } else {
        home.join("Videos")
    };
    if videos.is_dir() { videos } else { home }
}
