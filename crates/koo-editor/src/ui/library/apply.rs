//! What the library does with an item: apply it to the selection, mention it
//! in the chat, or start, copy and remove project programs. Every change is a
//! `Command`, so the agent's tools reach the same results.

use crate::state::EditorState;
use gpui_kit::*;
use koo_core::audiofx::{AudioFx, AudioFxKind};
use koo_core::command::{Command, Edge};
use koo_core::fx::{
    self, FxAsset, FxInstance, FxKind, FxLanguage, FxRef, ParamKind, ParamSpec, ParamValue,
    Transition,
};
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::{ClipId, FxId, Time, TimeRange, TrackId};

/// Length of a generator dropped at the playhead.
const GENERATOR_LEN: f64 = 5.0;
/// Shortest gap a generator is fitted into.
const MIN_GAP: f64 = 0.5;
const TRANSITION_LEN: f64 = 1.0;

/// Apply a library program to the selection: effects onto the selected
/// picture clips, a transition onto the first one's start, a generator at the
/// playhead.
pub fn apply_fx(s: &mut EditorState, r: &FxRef, cx: &mut Context<EditorState>) {
    let Some(asset) = fx::asset(s.bus.project(), r).cloned() else {
        return;
    };
    match asset.kind {
        FxKind::Effect => {
            let clips = picture_clips(s);
            if clips.is_empty() {
                s.status = Some("Select a picture clip".into());
                cx.notify();
                return;
            }
            s.execute(
                Command::AddEffect {
                    clips,
                    effect: FxInstance::new(r.clone()),
                    index: None,
                },
                cx,
            );
        }
        FxKind::Transition => {
            let Some(clip) = picture_clips(s).first().copied() else {
                s.status = Some("Select a picture clip".into());
                cx.notify();
                return;
            };
            let existing = s
                .bus
                .project()
                .timeline
                .clip(clip)
                .and_then(|(_, c)| c.transition_in.clone());
            let mut transition = Transition::new(r.clone(), Time::from_secs_f64(TRANSITION_LEN));
            if let Some(old) = existing {
                transition.duration = old.duration;
                transition.align = old.align;
            }
            let applied = s.execute(
                Command::SetTransition {
                    clip,
                    edge: Edge::Start,
                    transition: Some(transition),
                },
                cx,
            );
            if applied.is_some() {
                s.select_transition(Some((clip, Edge::Start)), cx);
            }
        }
        FxKind::Generator => add_generator(s, &asset, cx),
    }
}

/// Add an audio effect after the others on the selected clips' sound.
pub fn apply_audio(s: &mut EditorState, kind: AudioFxKind, cx: &mut Context<EditorState>) {
    let tl = &s.bus.project().timeline;
    let mut targets: Vec<ClipId> = Vec::new();
    for id in &s.selection.clips {
        let Some((_, clip)) = tl.clip(*id) else {
            continue;
        };
        if clip.media().is_none() {
            continue;
        }
        let sound = crate::ui::inspector::sound_clip(tl, clip);
        let on_audio = tl
            .clip(sound.id)
            .is_some_and(|(t, _)| t.kind == TrackKind::Audio);
        if on_audio && !targets.contains(&sound.id) {
            targets.push(sound.id);
        }
    }
    if targets.is_empty() {
        s.status = Some("Select a clip with sound".into());
        cx.notify();
        return;
    }
    let commands: Vec<Command> = targets
        .iter()
        .filter_map(|id| tl.clip(*id))
        .map(|(_, c)| {
            let mut effects = c.audio.effects.clone();
            effects.push(AudioFx::new(kind));
            Command::SetAudioEffects {
                clip: c.id,
                effects,
            }
        })
        .collect();
    if commands.len() == 1 {
        s.execute(commands.into_iter().next().unwrap(), cx);
    } else {
        s.execute_batch(commands, "set_audio_effects", cx);
    }
}

/// Selected clips that can take video effects and transitions.
fn picture_clips(s: &EditorState) -> Vec<ClipId> {
    let tl = &s.bus.project().timeline;
    s.selection
        .clips
        .iter()
        .filter(|id| {
            tl.clip(**id).is_some_and(|(t, c)| {
                matches!(
                    t.kind,
                    TrackKind::Video | TrackKind::Overlay | TrackKind::Title
                ) && !matches!(c.content, ClipContent::Caption { .. })
            })
        })
        .copied()
        .collect()
}

/// A generator at the playhead on the first title or overlay track (HTML
/// prefers title tracks) with room for it, else on a new track on top.
fn add_generator(s: &mut EditorState, asset: &FxAsset, cx: &mut Context<EditorState>) {
    let at = s.playhead;
    let kinds: &[TrackKind] = match asset.language {
        FxLanguage::Html => &[TrackKind::Title, TrackKind::Overlay],
        FxLanguage::Glsl => &[TrackKind::Overlay, TrackKind::Title],
    };
    let project = s.bus.project();
    let free = kinds.iter().find_map(|kind| {
        project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == *kind && !t.locked)
            .find_map(|t| gap_at(project, t.id, at).map(|range| (t.id, range)))
    });
    let (track, range) = match free {
        Some(found) => found,
        None => {
            let kind = kinds[0];
            let name = new_track_name(project, kind);
            // A new track's id is made by the planner, so the generator
            // follows as its own step.
            let added = s.execute(
                Command::AddTrack {
                    kind,
                    index: 0,
                    name,
                },
                cx,
            );
            let Some(track) = added.and(s.bus.project().timeline.tracks.first().map(|t| t.id))
            else {
                return;
            };
            let len = Time::from_secs_f64(GENERATOR_LEN);
            (track, TimeRange::new(at, at + len))
        }
    };
    let applied = s.execute(
        Command::AddGenerator {
            track,
            range,
            fx: FxInstance::new(asset.id.clone()),
        },
        cx,
    );
    if applied.is_some()
        && let Some(clip) = s
            .bus
            .project()
            .timeline
            .track(track)
            .and_then(|t| t.clips.iter().find(|c| c.start == range.start))
    {
        let id = clip.id;
        s.select_transition(None, cx);
        s.select_clips(vec![id], cx);
    }
}

/// Free time on `track` from `at`: up to [`GENERATOR_LEN`], at least
/// [`MIN_GAP`].
fn gap_at(project: &Project, track: TrackId, at: Time) -> Option<TimeRange> {
    let t = project.timeline.track(track)?;
    if t.clips.iter().any(|c| c.range().contains(at)) {
        return None;
    }
    let mut end = at + Time::from_secs_f64(GENERATOR_LEN);
    if let Some(next) = t
        .clips
        .iter()
        .filter(|c| c.start >= at)
        .map(|c| c.start)
        .min()
    {
        end = end.min(next);
    }
    (end - at >= Time::from_secs_f64(MIN_GAP)).then(|| TimeRange::new(at, end))
}

fn new_track_name(project: &Project, kind: TrackKind) -> String {
    let prefix = match kind {
        TrackKind::Title => "T",
        TrackKind::Overlay => "O",
        _ => "V",
    };
    let n = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == kind)
        .count();
    format!("{prefix}{}", n + 1)
}

/// The chat reference for a program: `@fx <key or id8>` and its name.
pub fn fx_mention(project: &Project, r: &FxRef) -> (String, SharedString) {
    let name = fx::asset(project, r)
        .map(|a| a.name.clone())
        .unwrap_or_else(|| r.to_string());
    (format!("@fx {r}"), name.into())
}

/// Starter programs for "New program", one per kind and language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Starter {
    Effect,
    Transition,
    Generator,
    Html,
}

impl Starter {
    pub const ALL: [Starter; 4] = [Self::Effect, Self::Transition, Self::Generator, Self::Html];

    pub fn label(self) -> &'static str {
        match self {
            Self::Effect => "Effect",
            Self::Transition => "Transition",
            Self::Generator => "Generator",
            Self::Html => "Title (HTML)",
        }
    }

    fn asset(self, name: String) -> FxAsset {
        let (kind, language, code, params) = match self {
            Self::Effect => (
                FxKind::Effect,
                FxLanguage::Glsl,
                EFFECT,
                vec![number("amount", "Amount", 0.0, 1.0, 1.0)],
            ),
            Self::Transition => (
                FxKind::Transition,
                FxLanguage::Glsl,
                TRANSITION,
                vec![number("softness", "Softness", 0.0, 1.0, 0.2)],
            ),
            Self::Generator => (
                FxKind::Generator,
                FxLanguage::Glsl,
                GENERATOR,
                vec![number("speed", "Speed", 0.0, 4.0, 1.0)],
            ),
            Self::Html => (
                FxKind::Generator,
                FxLanguage::Html,
                HTML,
                vec![
                    ParamSpec {
                        name: "text".into(),
                        label: "Text".into(),
                        kind: ParamKind::Text,
                        default: ParamValue::Text("Title".into()),
                    },
                    ParamSpec {
                        name: "color".into(),
                        label: "Color".into(),
                        kind: ParamKind::Color,
                        default: ParamValue::List(vec![1.0, 1.0, 1.0, 1.0]),
                    },
                ],
            ),
        };
        FxAsset {
            id: FxRef::Project(FxId::new()),
            name,
            kind,
            language,
            category: String::new(),
            description: String::new(),
            code: code.into(),
            params,
        }
    }
}

fn number(name: &str, label: &str, min: f64, max: f64, default: f64) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Float { min, max },
        default: ParamValue::Number(default),
    }
}

/// Create a project program from a starter and open it in the code editor.
pub fn new_program(s: &mut EditorState, starter: Starter, cx: &mut Context<EditorState>) {
    let name = unique_name(s.bus.project(), &format!("New {}", starter.label()));
    let asset = starter.asset(name);
    save_and_open(s, asset, cx);
}

/// Copy a program into the project under a new id and open the copy.
pub fn duplicate(s: &mut EditorState, r: &FxRef, cx: &mut Context<EditorState>) {
    let Some(mut asset) = fx::asset(s.bus.project(), r).cloned() else {
        return;
    };
    asset.id = FxRef::Project(FxId::new());
    asset.name = unique_name(s.bus.project(), &format!("{} copy", asset.name));
    save_and_open(s, asset, cx);
}

fn save_and_open(s: &mut EditorState, asset: FxAsset, cx: &mut Context<EditorState>) {
    let id = asset.id.clone();
    if s.execute(Command::SetFxAsset { asset }, cx).is_some() {
        s.edit_fx(Some(id), cx);
    }
}

/// Remove a project program; the bus refuses while clips use it.
pub fn remove(s: &mut EditorState, r: &FxRef, cx: &mut Context<EditorState>) {
    if let FxRef::Project(id) = r {
        let removed = s.execute(Command::RemoveFxAsset { asset: *id }, cx);
        if removed.is_some() && s.editing_fx.as_ref() == Some(r) {
            s.edit_fx(None, cx);
        }
    }
}

fn unique_name(project: &Project, base: &str) -> String {
    let taken = |n: &str| fx::library(project).any(|a| a.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|i| format!("{base} {i}"))
        .find(|n| !taken(n))
        .unwrap()
}

const EFFECT: &str = "vec4 effect(vec2 uv) {
    vec4 c = getColor(uv);
    float luma = dot(c.rgb, vec3(0.2126, 0.7152, 0.0722));
    return vec4(mix(c.rgb, vec3(luma), amount), c.a);
}
";

const TRANSITION: &str = "vec4 transition(vec2 uv) {
    float edge = progress * (1.0 + softness);
    float m = smoothstep(edge - softness, edge, uv.x);
    return mix(getToColor(uv), getFromColor(uv), m);
}
";

const GENERATOR: &str = "vec4 generate(vec2 uv) {
    float t = iTime * speed;
    vec3 c = 0.5 + 0.5 * cos(t + uv.xyx * 3.0 + vec3(0.0, 2.0, 4.0));
    return vec4(c, 1.0);
}
";

const HTML: &str = r#"<div class="title">{{text}}</div>
<style>
  .title {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font: 700 120px/1.1 system-ui, sans-serif;
    color: var(--color);
    animation: rise 0.8s cubic-bezier(.2, .8, .2, 1) both;
  }
  @keyframes rise {
    from { opacity: 0; transform: translateY(40px); }
  }
</style>
"#;
