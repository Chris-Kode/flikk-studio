//! Deliver workspace (PRD D1–D3, C6): preset, output file, render queue,
//! and caption/timeline/still exports.

use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Selectable as _, Sizable as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::Rate;
use koo_core::memory::{CaptionStyle, Presets};
use koo_core::model::Project;
use koo_deliver::{
    Encoder, LoudnessTarget, RenderJob, RenderQueue, RenderRequest, RenderSettings, RenderState,
    VideoCodec,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

type PresetSelect = SelectState<SearchableVec<SharedString>>;

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
    const ALL: [Export; 5] = [
        Export::Srt,
        Export::Ass,
        Export::Otio,
        Export::Fcpxml,
        Export::Still,
    ];

    fn label(self) -> &'static str {
        match self {
            Export::Srt => "SRT",
            Export::Ass => "ASS",
            Export::Otio => "OTIO",
            Export::Fcpxml => "FCPXML",
            Export::Still => "PNG",
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

pub struct DeliverPanel {
    state: Entity<EditorState>,
    queue: RenderQueue,
    presets: Vec<RenderSettings>,
    preset: usize,
    preset_select: Entity<PresetSelect>,
    /// Custom preset: width, height, fps, CRF.
    custom: [Entity<InputState>; 4],
    /// Sequence size the custom width and height were last filled from.
    sequence_size: (u32, u32),
    output: Option<PathBuf>,
    loudness: bool,
    burn_captions: bool,
    /// For H.264 and HEVC; saved for the user.
    encoder: Encoder,
    polling: bool,
    _subscriptions: Vec<Subscription>,
}

impl DeliverPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let presets = RenderSettings::presets();
        let names: Vec<SharedString> = presets.iter().map(|p| p.name.clone().into()).collect();
        let preset_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(names),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let seq = state.read(cx).bus.project().timeline.settings.clone();
        let input =
            |placeholder: &'static str, value: String, window: &mut Window, cx: &mut App| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        let custom = [
            input("Width", seq.width.to_string(), window, cx),
            input("Height", seq.height.to_string(), window, cx),
            input("FPS", format_fps(seq.rate), window, cx),
            input("CRF", "18".into(), window, cx),
        ];
        let _subscriptions = vec![
            cx.observe_in(&state, window, |this: &mut Self, state, window, cx| {
                let seq = &state.read(cx).bus.project().timeline.settings;
                let size = (seq.width, seq.height);
                if size != this.sequence_size {
                    this.sequence_size = size;
                    for (input, v) in this.custom.iter().zip([size.0, size.1]) {
                        input.update(cx, |i, cx| i.set_value(v.to_string(), window, cx));
                    }
                }
                cx.notify();
            }),
            cx.subscribe(
                &preset_select,
                |this, _, event: &SelectEvent<SearchableVec<SharedString>>, cx| {
                    let SelectEvent::Confirm(Some(name)) = event else {
                        return;
                    };
                    if let Some(ix) = this.presets.iter().position(|p| p.name == name.as_ref()) {
                        this.preset = ix;
                        let ext = this.presets[ix].extension();
                        if let Some(out) = &mut this.output {
                            out.set_extension(ext);
                        }
                        cx.notify();
                    }
                },
            ),
        ];
        // Which hardware encoders work, found off the UI thread.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async { koo_deliver::hw::probe() })
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
            preset: 0,
            preset_select,
            custom,
            sequence_size: (seq.width, seq.height),
            output: None,
            loudness: false,
            burn_captions: true,
            encoder: saved_encoder(),
            polling: false,
            _subscriptions,
        }
    }

    fn project(&self, cx: &App) -> Project {
        self.state.read(cx).bus.project().clone()
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

    /// The chosen output, or a free name in the default folder.
    fn output(&self, cx: &App) -> PathBuf {
        self.output.clone().unwrap_or_else(|| {
            let name = format!(
                "{}.{}",
                self.file_stem(cx),
                self.presets[self.preset].extension()
            );
            koo_deliver::free_name(&default_dir().join(name))
        })
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
                // The next render gets a fresh name.
                self.output = Some(koo_deliver::free_name(&output));
                self.poll(cx);
                cx.notify();
            }
            Err(e) => {
                if output.exists() {
                    self.output = Some(koo_deliver::free_name(&output));
                }
                self.set_status(format!("{e:#}"), cx);
            }
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
        let suggested = koo_deliver::free_name(&dir.join(&name));
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
                        Export::Srt => koo_deliver::export_srt(&project, None, &styles, &path),
                        Export::Ass => koo_deliver::export_ass(&project, &styles, &path),
                        Export::Otio => koo_deliver::export_otio(&project, &path),
                        Export::Fcpxml => koo_deliver::export_fcpxml(&project, &path),
                        Export::Still => {
                            koo_deliver::export_frame(&project, playhead, &styles, &services, &path)
                        }
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

    /// Auto, Hardware, Software, for presets a hardware encoder can take.
    fn render_encoder(&self, preset: &RenderSettings, cx: &mut Context<Self>) -> ButtonGroup {
        let codec = preset.hardware_codec().map(|(codec, _)| codec);
        let probe = koo_deliver::hw::probed();
        let found = probe
            .zip(codec)
            .and_then(|(p, codec)| p.best(codec))
            .map(|e| e.api.label());
        // Known to be missing only once the probe is done.
        let missing = probe.is_some() && found.is_none();
        ButtonGroup::new("encoder")
            .outline()
            .small()
            .children(Encoder::ALL.iter().map(|&e| {
                Button::new(e.key())
                    .label(e.label())
                    .selected(e == self.encoder)
                    .when(e == Encoder::Hardware && missing, |b| b.disabled(true))
                    .when_some(found.filter(|_| e != Encoder::Software), |b, api| {
                        b.tooltip(api)
                    })
            }))
            .on_click(cx.listener(|this, clicked: &Vec<usize>, _, cx| {
                if let Some(&ix) = clicked.first() {
                    this.set_encoder(Encoder::ALL[ix], cx);
                }
            }))
    }

    fn render_job(&self, job: &RenderJob, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, red, green) = (theme.muted_foreground, theme.red, theme.green);
        let id = job.id;
        let name: SharedString = job
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        let finished = job.state.is_finished();
        let status: AnyElement = match &job.state {
            RenderState::Queued => Progress::new(("queued", id.0.as_u128() as u64))
                .value(0.)
                .into_any_element(),
            RenderState::Running { fraction } => Progress::new(("progress", id.0.as_u128() as u64))
                .value(fraction * 100.)
                .into_any_element(),
            RenderState::Done => Icon::new(IconName::Check)
                .small()
                .text_color(green)
                .into_any_element(),
            RenderState::Failed { .. } => Icon::new(IconName::CircleAlert)
                .small()
                .text_color(red)
                .into_any_element(),
            RenderState::Canceled => Icon::new(IconName::Ban)
                .small()
                .text_color(muted)
                .into_any_element(),
        };
        let output = job.output.clone();
        let queue = self.queue.clone();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .h(px(28.))
                    .gap_2()
                    .child(
                        div()
                            .id(("job", id.0.as_u128() as u64))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(job.state == RenderState::Done, |d| {
                                d.cursor_pointer()
                                    .on_click(move |_, _, cx| cx.reveal_path(&output))
                            })
                            .child(name),
                    )
                    .child(div().w(px(96.)).flex().justify_end().child(status))
                    .child(
                        Button::new(("job-close", id.0.as_u128() as u64))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if finished {
                                    queue.remove(id);
                                } else {
                                    this.queue.cancel(id);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when_some(
                match &job.state {
                    RenderState::Failed { message } => Some(message.clone()),
                    _ => None,
                },
                |el, message| {
                    el.child(div().text_xs().text_color(red).line_clamp(3).child(message))
                },
            )
    }
}

impl Render for DeliverPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let preset = &self.presets[self.preset];
        let custom = preset.name == RenderSettings::custom().name;
        let has_video = preset.video.is_some();
        let encoder = preset
            .hardware_codec()
            .is_some()
            .then(|| self.render_encoder(preset, cx));
        let output = self.output(cx);
        let output_name: SharedString = output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into();
        let jobs = self.queue.jobs();
        let job_rows: Vec<AnyElement> = jobs
            .iter()
            .rev()
            .map(|j| self.render_job(j, cx).into_any_element())
            .collect();

        v_flex()
            .id("deliver")
            .size_full()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(
                div()
                    .flex_none()
                    .child(Select::new(&self.preset_select).small()),
            )
            .when(custom, |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .children(self.custom.iter().map(|i| Input::new(i).small())),
                )
            })
            .when_some(encoder, |el, group| el.child(group))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        Checkbox::new("loudness")
                            .label(format!("Normalize to {} LUFS", self.loudness_target().lufs))
                            .checked(self.loudness)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.loudness = *checked;
                                cx.notify();
                            })),
                    )
                    .when(has_video, |el| {
                        el.child(
                            Checkbox::new("burn-captions")
                                .label("Burn in captions")
                                .checked(self.burn_captions)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.burn_captions = *checked;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                Button::new("output")
                    .outline()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label(output_name)
                    .tooltip(output.to_string_lossy().into_owned())
                    .on_click(cx.listener(|this, _, _, cx| this.choose_output(cx))),
            )
            .child(
                Button::new("render")
                    .primary()
                    .label("Render")
                    .on_click(cx.listener(|this, _, _, cx| this.start_render(cx))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(Export::ALL.iter().map(|kind| {
                        let kind = *kind;
                        Button::new(kind.label())
                            .ghost()
                            .xsmall()
                            .label(kind.label())
                            .when(matches!(kind, Export::Still), |b| b.icon(IconName::Frame))
                            .on_click(cx.listener(move |this, _, _, cx| this.export(kind, cx)))
                    })),
            )
            .when(!job_rows.is_empty(), |el| {
                el.child(div().h(px(1.)).bg(border))
                    .child(v_flex().gap_1().children(job_rows))
            })
    }
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
