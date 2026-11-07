//! Project → one FFmpeg `filter_complex` graph.
//!
//! Video: each visible track becomes one stream, the `concat` of transparent
//! gaps and clip segments at sequence size and rate; tracks are overlaid
//! bottom-up on black. Each clip segment trims its source (`movie` with a
//! seek, `trim`/`setpts`), grades it, scales/rotates/fades it (keyframes as
//! expressions over `t`) and places it on a transparent canvas. Concat keeps
//! only the current segment's decoder busy, so long timelines stay cheap.
//! Disabled clips are skipped. Ranges that use effects, generators,
//! transitions, blend modes, crop, flip or speed are composited by
//! `flikk_core::compose` instead and piped in ([`GraphSpec::piped`]).
//!
//! Audio: each audible track is a `concat` of silence and clip spans (two
//! lanes where crossfades overlap), mixed, then track volume, pan and track
//! effects; optional ducking via `sidechaincompress` keyed by the dialogue
//! tracks, the mix, and optional `loudnorm`. A span trims its source with
//! handles, retimes it with `atempo` (under a speed ramp, stages whose
//! tempo an `asendcmd` script moves every 50 ms), runs the clip effects
//! ([`crate::audio_fx`]) and one `volume` expression for gain or volume
//! keyframes, fades and equal-power crossfade ramps, following
//! `flikk_core::audiofx` like the viewer's mixer.
//!
//! Sources are opened with `movie`/`amovie` inside the graph, so the whole
//! render is one script file and one input-less ffmpeg invocation.

use crate::ffmpeg::{escape_path, probe_size};
use crate::keyframes::{Anim, anim, num};
use crate::preset::{LoudnessTarget, RenderSettings};
use crate::{audio_fx, captions};
use anyhow::{Context as _, Result, bail};
use flikk_core::audiofx::{self, Span};
use flikk_core::memory::CaptionStyle;
use flikk_core::model::{Clip, ClipContent, Grade, MediaKind, Project, Property, Track, TrackKind};
use flikk_core::speed::RampAudio;
use flikk_core::time::FLICKS_PER_SECOND;
use flikk_core::{Rate, Time, TimeRange, TrackId};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What to build: output geometry, the timeline range, and which streams.
#[derive(Clone, Debug)]
pub struct GraphSpec {
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub sample_rate: u32,
    pub range: TimeRange,
    pub video: bool,
    pub audio: bool,
    pub burn_captions: bool,
    pub caption_styles: Vec<CaptionStyle>,
    pub loudnorm: Option<Loudnorm>,
    /// The picture arrives composited on input 0 (RGBA frames from
    /// `flikk_core::compose`, see [`crate::composed`]); the graph only burns
    /// in captions and builds the audio. Set with `lettered` by
    /// [`crate::composed::route`].
    pub piped: bool,
    /// The piped frames already have captions and titles lettered on them
    /// (see [`crate::composed::route`]); the graph leaves them out.
    pub lettered: bool,
}

impl GraphSpec {
    pub fn new(project: &Project, settings: &RenderSettings, styles: &[CaptionStyle]) -> Self {
        let seq = &project.timeline.settings;
        let (width, height) = settings.size(seq);
        Self {
            width,
            height,
            rate: settings.rate.unwrap_or(seq.rate),
            sample_rate: seq.sample_rate.max(8000),
            range: settings
                .range
                .unwrap_or(TimeRange::new(Time::ZERO, project.timeline.duration())),
            video: settings.video.is_some(),
            audio: true,
            burn_captions: settings.burn_captions,
            caption_styles: styles.to_vec(),
            loudnorm: None,
            piped: false,
            lettered: false,
        }
    }
}

/// Two-pass loudness normalization: measure, then apply linearly.
#[derive(Clone, Debug)]
pub enum Loudnorm {
    Measure(LoudnessTarget),
    Apply(LoudnessTarget, Measured),
}

/// The first pass's `loudnorm` JSON, as printed.
#[derive(Clone, Debug)]
pub struct Measured {
    pub input_i: String,
    pub input_tp: String,
    pub input_lra: String,
    pub input_thresh: String,
    pub target_offset: String,
}

impl Measured {
    /// Parse the JSON block `loudnorm` prints at the end of stderr.
    pub fn parse(stderr: &str) -> Option<Self> {
        let start = stderr
            .rfind("\"input_i\"")
            .and_then(|i| stderr[..i].rfind('{'))?;
        let end = start + stderr[start..].find('}')?;
        let v: serde_json::Value = serde_json::from_str(&stderr[start..=end]).ok()?;
        let field = |k: &str| -> Option<String> {
            let s = v.get(k)?.as_str()?.trim().to_string();
            s.parse::<f64>().ok().filter(|x| x.is_finite())?;
            Some(s)
        };
        Some(Self {
            input_i: field("input_i")?,
            input_tp: field("input_tp")?,
            input_lra: field("input_lra")?,
            input_thresh: field("input_thresh")?,
            target_offset: field("target_offset")?,
        })
    }
}

/// A built graph. Map `[vout]`/`[aout]` when present.
#[derive(Clone, Debug)]
pub struct Graph {
    pub script: String,
    pub video: Option<String>,
    pub audio: Option<String>,
    pub frames: i64,
    pub seconds: f64,
}

/// Build the graph for `spec`. Side files (the burned-in ASS) are written
/// to `work_dir`.
pub fn build(project: &Project, spec: &GraphSpec, work_dir: &Path) -> Result<Graph> {
    if spec.range.is_empty() {
        bail!("nothing to render: the timeline is empty");
    }
    if spec.width < 2 || spec.height < 2 || spec.rate.num == 0 || spec.rate.den == 0 {
        bail!("invalid output size or frame rate");
    }
    let mut b = Builder {
        project,
        spec,
        chains: Vec::new(),
        next: 0,
        sizes: HashMap::new(),
        work_dir,
    };
    let frames = b.frames(spec.range.end).max(1);
    let video = if spec.video {
        Some(b.video(frames, work_dir)?)
    } else {
        None
    };
    let audio = if spec.audio {
        Some(b.audio(work_dir)?)
    } else {
        None
    };
    Ok(Graph {
        script: b.chains.join(";\n") + "\n",
        video,
        audio,
        frames,
        seconds: spec.range.duration().as_secs_f64(),
    })
}

struct Builder<'a> {
    project: &'a Project,
    spec: &'a GraphSpec,
    chains: Vec<String>,
    next: usize,
    sizes: HashMap<PathBuf, (u32, u32)>,
    /// Side files: captions, grade LUTs.
    work_dir: &'a Path,
}

/// A picture source for a clip segment.
struct Picture {
    path: PathBuf,
    still: bool,
    size: (u32, u32),
    /// HDR → SDR filters, as the media engine decodes the file.
    tone: Option<String>,
}

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
}

/// Whether a caption or title shows in `range`.
pub(crate) fn has_text(project: &Project, range: TimeRange) -> bool {
    project.timeline.tracks.iter().any(|t| {
        !t.mix.mute
            && matches!(t.kind, TrackKind::Caption | TrackKind::Title)
            && t.clips.iter().any(|c| c.range().overlaps(&range))
    })
}

fn is_still_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "tga"
    )
}

impl Builder<'_> {
    fn label(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn rate(&self) -> String {
        format!("{}/{}", self.spec.rate.num, self.spec.rate.den)
    }

    /// Frame index of timeline time `t`, relative to the range start.
    fn frames(&self, t: Time) -> i64 {
        let rel = (t - self.spec.range.start).0 as i128;
        let num = self.spec.rate.num as i128;
        let den = self.spec.rate.den as i128 * FLICKS_PER_SECOND as i128;
        ((rel * num * 2 + den) / (den * 2)) as i64
    }

    /// Timeline time of frame `f` (relative to the range start).
    fn frame_time(&self, f: i64) -> Time {
        let flicks = f as i128 * self.spec.rate.den as i128 * FLICKS_PER_SECOND as i128
            / self.spec.rate.num as i128;
        self.spec.range.start + Time(flicks as i64)
    }

    fn samples(&self, t: Time) -> i64 {
        let rel = (t - self.spec.range.start).0 as i128;
        let sr = self.spec.sample_rate as i128;
        let f = FLICKS_PER_SECOND as i128;
        ((rel * sr * 2 + f) / (f * 2)) as i64
    }

    fn size_of(&mut self, path: &Path, known: Option<(u32, u32)>) -> Result<(u32, u32)> {
        if let Some(s) = known.filter(|s| s.0 > 0 && s.1 > 0) {
            return Ok(s);
        }
        if let Some(s) = self.sizes.get(path) {
            return Ok(*s);
        }
        let s = probe_size(path)
            .with_context(|| format!("could not read the picture size of {}", path.display()))?;
        self.sizes.insert(path.to_path_buf(), s);
        Ok(s)
    }

    // ---------------------------------------------------------------- video

    fn video(&mut self, frames: i64, work_dir: &Path) -> Result<String> {
        let (w, h, r) = (self.spec.width, self.spec.height, self.rate());
        let mut cur = self.label("base");
        if self.spec.piped {
            self.chains.push(format!(
                "[0:v]trim=end_frame={frames},setsar=1,format=yuv420p[{cur}]"
            ));
        } else {
            self.chains.push(format!(
                "color=c=black:s={w}x{h}:r={r},trim=end_frame={frames},setsar=1,format=yuv420p[{cur}]"
            ));
        }
        let tracks: Vec<&Track> = self
            .project
            .timeline
            .tracks
            .iter()
            .rev()
            .filter(|t| matches!(t.kind, TrackKind::Video | TrackKind::Overlay) && !t.mix.mute)
            .filter(|_| !self.spec.piped)
            .collect();
        for track in tracks {
            if let Some(stream) = self.video_track(track)? {
                let out = self.label("comp");
                self.chains.push(format!(
                    "[{cur}][{stream}]overlay=eof_action=pass:format=auto[{out}]"
                ));
                cur = out;
            }
        }
        if self.spec.burn_captions && !self.spec.lettered && has_text(self.project, self.spec.range)
        {
            let ass = captions::to_ass(
                self.project,
                &self.spec.caption_styles,
                w,
                h,
                self.spec.range,
                true,
            );
            let path = work_dir.join("captions.ass");
            std::fs::write(&path, ass).context("could not write the caption file")?;
            let out = self.label("subs");
            self.chains
                .push(format!("[{cur}]ass=filename={}[{out}]", escape_path(&path)));
            cur = out;
        }
        self.chains.push(format!("[{cur}]setsar=1[vout]"));
        Ok("vout".into())
    }

    fn video_track(&mut self, track: &Track) -> Result<Option<String>> {
        let mut segments = Vec::new();
        let mut cursor = 0i64;
        for clip in track.clips.iter().filter(|c| c.enabled) {
            let Some(visible) = clip.range().intersect(&self.spec.range) else {
                continue;
            };
            let Some(picture) = self.picture(clip)? else {
                continue;
            };
            let f0 = self.frames(visible.start).max(cursor);
            let f1 = self.frames(visible.end);
            if f1 <= f0 {
                continue;
            }
            if f0 > cursor {
                segments.push(self.gap(f0 - cursor));
            }
            let start = self.frame_time(f0);
            segments.push(self.clip_segment(clip, &picture, start, f1 - f0)?);
            cursor = f1;
        }
        Ok(self.concat(segments, true))
    }

    fn concat(&mut self, segments: Vec<String>, video: bool) -> Option<String> {
        match segments.len() {
            0 => None,
            1 => segments.into_iter().next(),
            n => {
                let out = self.label(if video { "vtrack" } else { "atrack" });
                let mut chain: String = segments.iter().map(|s| format!("[{s}]")).collect();
                let (v, a) = if video { (1, 0) } else { (0, 1) };
                let _ = write!(chain, "concat=n={n}:v={v}:a={a}[{out}]");
                self.chains.push(chain);
                Some(out)
            }
        }
    }

    fn picture(&mut self, clip: &Clip) -> Result<Option<Picture>> {
        let mut tone = None;
        let (path, still, known) = match &clip.content {
            ClipContent::Media { media, .. } => {
                let item = self
                    .project
                    .media(*media)
                    .with_context(|| format!("media {media} is missing from the project"))?;
                if item.kind == MediaKind::Audio {
                    return Ok(None);
                }
                let known = item
                    .info
                    .as_ref()
                    .and_then(|i| i.video.as_ref())
                    .map(|v| (v.width, v.height));
                if item.kind == MediaKind::Video {
                    tone = crate::ffmpeg::tone_filters(item)?;
                }
                (item.path.clone(), item.kind == MediaKind::Image, known)
            }
            ClipContent::Overlay { asset, .. } => (asset.clone(), is_still_path(asset), None),
            ClipContent::Title { .. }
            | ClipContent::Caption { .. }
            | ClipContent::Generator { .. } => return Ok(None),
        };
        if !path.exists() {
            bail!("media offline: {}", path.display());
        }
        let size = self.size_of(&path, known)?;
        Ok(Some(Picture {
            path,
            still,
            size,
            tone,
        }))
    }

    fn gap(&mut self, frames: i64) -> String {
        let (w, h, r) = (self.spec.width, self.spec.height, self.rate());
        let out = self.label("gap");
        self.chains.push(format!(
            "color=c=black@0:s={w}x{h}:r={r},format=yuva420p,trim=end_frame={frames},setsar=1[{out}]"
        ));
        out
    }

    /// `frames` frames of `clip` starting at timeline time `start`, on a
    /// transparent canvas.
    fn clip_segment(
        &mut self,
        clip: &Clip,
        pic: &Picture,
        start: Time,
        frames: i64,
    ) -> Result<String> {
        let (w, h, r) = (self.spec.width, self.spec.height, self.rate());
        let fd = 1.0 / self.spec.rate.fps();
        let offset = (start - clip.start).max(Time::ZERO).as_secs_f64();
        let t = format!("(t+{})", num(offset));
        let tr = &clip.transform;
        let keys = &clip.keyframes;
        let scale = anim(tr, keys, Property::Scale, &t);
        let px = anim(tr, keys, Property::PositionX, &t);
        let py = anim(tr, keys, Property::PositionY, &t);
        let rot = anim(tr, keys, Property::Rotation, &t);
        let opacity = anim(tr, keys, Property::Opacity, &format!("(T+{})", num(offset)));

        let mut f: Vec<String> = Vec::new();
        let path = escape_path(&pic.path);
        if pic.still {
            f.push(format!("movie={path}"));
            f.push("loop=loop=-1:size=1".into());
            f.push(format!("setpts=N/({r})/TB"));
            f.push(format!("trim=end_frame={frames}"));
        } else {
            let source_in = match &clip.content {
                ClipContent::Media { source_in, .. } => source_in.as_secs_f64(),
                _ => 0.0,
            };
            let s0 = source_in + offset;
            let s1 = s0 + (frames as f64 + 1.0) * fd;
            f.push(format!("movie={path}:seek_point={}", num(s0)));
            f.push(format!("trim=start={}:end={}", num(s0), num(s1)));
            f.push("setpts=PTS-STARTPTS".into());
            f.push(format!("fps={r}"));
            f.extend(pic.tone.clone());
        }
        let timeline = matches!(clip.content, ClipContent::Media { .. })
            .then_some(&self.project.timeline.grade);
        f.extend(grade_filters(&clip.grade, timeline, self.work_dir)?);
        f.push("format=yuva420p".into());

        let (sw, sh) = (pic.size.0.max(1) as f64, pic.size.1.max(1) as f64);
        let fit = tr.fit.scale(pic.size, (w, h));
        let (mut cw, mut ch) = (sw * fit, sh * fit);
        let rotated = !matches!(rot, Anim::Const(v) if v.abs() < 1e-6);
        let unscaled = matches!(scale, Anim::Const(v) if (v - 1.0).abs() < 1e-6);
        let frame_eval = |a: &Anim| if a.is_const() { "init" } else { "frame" };
        if rotated {
            f.push(format!("scale={}:{}", even(cw), even(ch)));
            let (ow, oh) = match rot.constant() {
                Some(deg) => {
                    let (s, c) = deg.to_radians().sin_cos();
                    (
                        even(cw * c.abs() + ch * s.abs()),
                        even(cw * s.abs() + ch * c.abs()),
                    )
                }
                None => {
                    let d = even(cw.hypot(ch));
                    (d, d)
                }
            };
            f.push(format!(
                "rotate=a='{}*PI/180':c=none:ow={ow}:oh={oh}",
                rot.term()
            ));
            (cw, ch) = (ow as f64, oh as f64);
        }
        if !(rotated && unscaled) {
            f.push(format!(
                "scale=w='max(2,trunc({}*{}/2)*2)':h='max(2,trunc({}*{}/2)*2)':eval={}",
                num(cw),
                scale.term(),
                num(ch),
                scale.term(),
                frame_eval(&scale)
            ));
        }
        match &opacity {
            Anim::Const(o) if *o >= 0.999 => {}
            Anim::Const(o) => f.push(format!("colorchannelmixer=aa={}", num(o.clamp(0.0, 1.0)))),
            Anim::Expr(_) => f.push(format!(
                "geq=lum='lum(X,Y)':cb='cb(X,Y)':cr='cr(X,Y)':a='alpha(X,Y)*clip({},0,1)'",
                opacity.term()
            )),
        }
        let src = self.label("clip");
        self.chains.push(format!("{}[{src}]", f.join(",")));

        let canvas = self.label("canvas");
        self.chains.push(format!(
            "color=c=black@0:s={w}x{h}:r={r},format=yuva420p,trim=end_frame={frames}[{canvas}]"
        ));
        let x = format!("({w}-{}*{})/2+{}*{w}", num(cw), scale.term(), px.term());
        let y = format!("({h}-{}*{})/2+{}*{h}", num(ch), scale.term(), py.term());
        let animated = !(scale.is_const() && px.is_const() && py.is_const());
        let out = self.label("seg");
        self.chains.push(format!(
            "[{canvas}][{src}]overlay=x='{x}':y='{y}':eval={}:eof_action=pass:repeatlast=0:format=auto,setsar=1,format=yuva420p[{out}]",
            if animated { "frame" } else { "init" }
        ));
        Ok(out)
    }

    // ---------------------------------------------------------------- audio

    fn audio(&mut self, work_dir: &Path) -> Result<String> {
        let sr = self.spec.sample_rate;
        let total = self.samples(self.spec.range.end).max(1);
        let project = self.project;
        let tl = &project.timeline;
        let mut streams: Vec<(TrackId, String)> = Vec::new();
        for track in tl.tracks.iter().filter(|t| tl.audible(t)) {
            if let Some(s) = self.audio_track(track, work_dir)? {
                streams.push((track.id, s));
            }
        }

        let ducking = self.project.timeline.ducking.as_ref();
        let music = ducking.and_then(|d| streams.iter().position(|(id, _)| *id == d.music_track));
        let mut mix: Vec<String> = Vec::new();
        match (ducking, music) {
            (Some(d), Some(mi)) if streams.len() > 1 => {
                let (_, music) = streams.remove(mi);
                let dialogue: Vec<String> = streams.into_iter().map(|(_, s)| s).collect();
                let key_src = self.mix(dialogue);
                let (key, voice) = (self.label("key"), self.label("voice"));
                // Padded: sidechaincompress stops when its key ends, which
                // would cut the music where the dialogue stops.
                self.chains.push(format!(
                    "[{key_src}]apad=whole_len={total},asplit=2[{key}][{voice}]"
                ));
                let (threshold, ratio) = audiofx::dsp::ducking(d.reduction_db as f64);
                let attack = (d.attack.as_secs_f64() * 1000.0).clamp(0.01, 2000.0);
                let release = (d.release.as_secs_f64() * 1000.0).clamp(0.01, 9000.0);
                let ducked = self.label("ducked");
                self.chains.push(format!(
                    "[{music}][{key}]sidechaincompress=threshold={}:ratio={}:attack={}:release={}:makeup=1:knee=2.828427:detection=rms:link=average[{ducked}]",
                    num(threshold),
                    num(ratio),
                    num(attack),
                    num(release)
                ));
                mix.push(voice);
                mix.push(ducked);
            }
            _ => mix.extend(streams.into_iter().map(|(_, s)| s)),
        }

        let mut chain = if mix.is_empty() {
            format!("anullsrc=r={sr}:cl=stereo")
        } else {
            let m = self.mix(mix);
            format!("[{m}]anull")
        };
        let _ = write!(chain, ",apad=whole_len={total},atrim=end_sample={total}");
        match &self.spec.loudnorm {
            None => {}
            Some(Loudnorm::Measure(t)) => {
                let _ = write!(
                    chain,
                    ",loudnorm=I={}:TP={}:LRA=11:print_format=json",
                    num(t.lufs as f64),
                    num(t.true_peak_db as f64)
                );
            }
            Some(Loudnorm::Apply(t, m)) => {
                let _ = write!(
                    chain,
                    ",loudnorm=I={}:TP={}:LRA=11:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true:print_format=summary,aresample={sr}",
                    num(t.lufs as f64),
                    num(t.true_peak_db as f64),
                    m.input_i,
                    m.input_tp,
                    m.input_lra,
                    m.input_thresh,
                    m.target_offset
                );
            }
        }
        chain.push_str("[aout]");
        self.chains.push(chain);
        Ok("aout".into())
    }

    /// Mix streams without level normalization.
    fn mix(&mut self, streams: Vec<String>) -> String {
        if streams.len() == 1 {
            return streams.into_iter().next().expect("one");
        }
        let out = self.label("mix");
        let inputs: String = streams.iter().map(|s| format!("[{s}]")).collect();
        self.chains.push(format!(
            "{inputs}amix=inputs={}:normalize=0:duration=longest:dropout_transition=0[{out}]",
            streams.len()
        ));
        out
    }

    /// One track: its clip spans laid out in lanes (a crossfade overlaps two
    /// spans, so they go to different lanes, each a `concat` of silence and
    /// segments), mixed, then track volume, pan and track effects.
    fn audio_track(&mut self, track: &Track, work_dir: &Path) -> Result<Option<String>> {
        // Per lane: the sample its content reaches, and its segments.
        let mut lanes: Vec<(i64, Vec<String>)> = Vec::new();
        for span in audiofx::spans(track) {
            let Some(visible) = span.range.intersect(&self.spec.range) else {
                continue;
            };
            let s0 = self.samples(visible.start);
            let s1 = self.samples(visible.end);
            let lane = match lanes.iter().position(|(cursor, _)| *cursor <= s0) {
                Some(i) => i,
                None if lanes.len() < 2 => {
                    lanes.push((0, Vec::new()));
                    lanes.len() - 1
                }
                // A third overlap (a transition longer than the clip
                // between): play it from where the lane is free.
                None => (0..lanes.len()).min_by_key(|i| lanes[*i].0).unwrap_or(0),
            };
            let s0 = s0.max(lanes[lane].0);
            if s1 <= s0 {
                continue;
            }
            let start = self.sample_time(s0);
            let Some(segment) = self.audio_segment(&span, start, s1 - s0, work_dir)? else {
                continue;
            };
            let cursor = lanes[lane].0;
            if s0 > cursor {
                let gap = self.silence(s0 - cursor);
                lanes[lane].1.push(gap);
            }
            lanes[lane].1.push(segment);
            lanes[lane].0 = s1;
        }
        let streams: Vec<String> = lanes
            .into_iter()
            .filter_map(|(_, segments)| self.concat(segments, false))
            .collect();
        if streams.is_empty() {
            return Ok(None);
        }
        let stream = self.mix(streams);
        let mut f: Vec<String> = Vec::new();
        if track.mix.volume_db.abs() > 1e-3 {
            f.push(format!("volume={}dB", num(track.mix.volume_db as f64)));
        }
        let pan = track.mix.pan.clamp(-1.0, 1.0) as f64;
        if pan.abs() > 1e-3 {
            let (l, r) = ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));
            f.push(format!("pan=stereo|c0={}*c0|c1={}*c1", num(l), num(r)));
        }
        let stream = if f.is_empty() {
            stream
        } else {
            let out = self.label("trackmix");
            self.chains
                .push(format!("[{stream}]{}[{out}]", f.join(",")));
            out
        };
        let sr = self.spec.sample_rate;
        let out = audio_fx::apply(self, stream, &track.mix.effects, sr, work_dir)?;
        Ok(Some(out))
    }

    fn silence(&mut self, samples: i64) -> String {
        let gap = self.label("silence");
        self.chains.push(format!(
            "anullsrc=r={}:cl=stereo,atrim=end_sample={samples}[{gap}]",
            self.spec.sample_rate
        ));
        gap
    }

    /// Timeline time of sample `s` (relative to the range start).
    fn sample_time(&self, s: i64) -> Time {
        let flicks = s as i128 * FLICKS_PER_SECOND as i128 / self.spec.sample_rate as i128;
        self.spec.range.start + Time(flicks as i64)
    }

    /// `samples` samples of `span` from timeline time `start`: the source
    /// (with handles; silence before the media starts), retimed with
    /// `atempo`, through the clip's effects, then one `volume` expression
    /// for gain or volume keyframes, fades and crossfade ramps.
    fn audio_segment(
        &mut self,
        span: &Span,
        start: Time,
        samples: i64,
        work_dir: &Path,
    ) -> Result<Option<String>> {
        let clip = span.clip;
        let ClipContent::Media {
            media,
            source_in,
            audio_stream,
        } = &clip.content
        else {
            return Ok(None);
        };
        let item = self
            .project
            .media(*media)
            .with_context(|| format!("media {media} is missing from the project"))?;
        if item.kind == MediaKind::Image || item.info.as_ref().is_some_and(|i| i.audio.is_empty()) {
            return Ok(None);
        }
        if !item.path.exists() {
            bail!("media offline: {}", item.path.display());
        }
        if clip.is_ramped() && clip.retime.audio == RampAudio::Mute {
            return Ok(None);
        }
        let sr = self.spec.sample_rate;
        let speed = clip.speed.clamp(0.01, 100.0);
        let length = samples as f64 / sr as f64;
        // Source time at the segment start; before 0 is silence.
        let src = clip.to_source(start).unwrap_or(*source_in).as_secs_f64();
        let mut lead = (-src / speed).max(0.0);
        let s0 = src.max(0.0);
        // A little extra source so atempo's output never runs short.
        let mut s1 = s0 + (length - lead).max(0.0) * speed + 0.1 * speed;
        let ramp = clip.is_ramped().then(|| {
            let at = |t: Time| clip.to_source(t).map_or(0.0, |s| s.as_secs_f64());
            let zero = clip.start + clip.timeline_offset(-*source_in);
            lead = (zero - start).as_secs_f64().clamp(0.0, length);
            let end = start + Time::from_secs_f64(length);
            s1 = at(end).max(s0) + 0.1 * clip.speed_at(end - clip.start);
            ramp_tempo(clip, start + Time::from_secs_f64(lead), end, s0)
        });
        let vary = !clip.is_ramped() && flikk_core::sync::varispeed::is_varispeed(speed);
        let mut f = if vary {
            // Drift correction: resampled as the preview does it.
            let file = work_dir.join(format!("{}.flac", self.label("vary")));
            crate::varispeed::render(
                item,
                *audio_stream,
                s0,
                (length - lead).max(0.0),
                speed,
                sr,
                &file,
            )?;
            vec![
                format!("amovie={}", escape_path(&file)),
                format!("aformat=sample_fmts=fltp:sample_rates={sr}:channel_layouts=stereo"),
            ]
        } else {
            let mut f = vec![format!(
                "amovie={}:seek_point={}{}",
                escape_path(&item.path),
                num(s0),
                audio_stream.map(|i| format!(":si={i}")).unwrap_or_default()
            )];
            f.push(format!("atrim=start={}:end={}", num(s0), num(s1)));
            f.push("asetpts=PTS-STARTPTS".into());
            f.push(format!(
                "aformat=sample_fmts=fltp:sample_rates={sr}:channel_layouts=stereo"
            ));
            f
        };
        match ramp {
            Some(steps) => f.extend(self.ramp_atempo(&steps, work_dir)?),
            None if vary => {}
            None => f.extend(atempo(speed)),
        }
        let lead_samples = (lead * sr as f64).round() as i64;
        if lead_samples > 0 {
            f.push(format!("adelay=delays={lead_samples}S:all=1"));
        }
        let src = self.label("asrc");
        self.chains.push(format!("{}[{src}]", f.join(",")));
        let src = audio_fx::apply(self, src, &clip.audio.effects, sr, work_dir)?;

        let mut f = Vec::new();
        let gain = segment_gain(span, start);
        match &gain {
            Anim::Const(g) if (g - 1.0).abs() < 1e-6 => {}
            Anim::Const(g) => f.push(format!("volume={}", num(*g))),
            Anim::Expr(_) => {
                f.push("asetnsamples=n=256:p=0".into());
                f.push(format!("volume='{}':eval=frame", gain.term()));
            }
        }
        f.push(format!("apad=whole_len={samples}"));
        f.push(format!("atrim=end_sample={samples}"));
        let out = self.label("aclip");
        self.chains.push(format!("[{src}]{}[{out}]", f.join(",")));
        Ok(Some(out))
    }
}

impl audio_fx::Sink for Builder<'_> {
    fn label(&mut self, prefix: &str) -> String {
        Builder::label(self, prefix)
    }

    fn push(&mut self, chain: String) {
        self.chains.push(chain);
    }
}

impl Builder<'_> {
    /// `atempo` stages whose tempo follows `steps` (source seconds from
    /// the segment's start, speed), driven by an `asendcmd` script in
    /// `work_dir`. Enough stages that each stays at 0.5 or more.
    fn ramp_atempo(&mut self, steps: &[(f64, f64)], work_dir: &Path) -> Result<Vec<String>> {
        let slowest = steps.iter().map(|s| s.1).fold(1.0f64, f64::min);
        let stages = if slowest < 0.5 {
            (slowest.ln() / 0.5f64.ln()).ceil() as i32
        } else {
            1
        }
        .max(1);
        let tempo = |speed: f64| speed.clamp(0.01, 100.0).powf(1.0 / stages as f64);
        let names: Vec<String> = (0..stages).map(|_| self.label("ramp")).collect();
        let mut script = String::new();
        for (at, speed) in steps {
            let v = num(tempo(*speed));
            let cmds: Vec<String> = names
                .iter()
                .map(|n| format!("atempo@{n} tempo {v}"))
                .collect();
            let _ = writeln!(script, "{} {};", num(*at), cmds.join(", "));
        }
        let path = work_dir.join(format!("{}.cmd", names[0]));
        std::fs::write(&path, script).context("could not write the speed ramp's tempo script")?;
        let first = steps.first().map_or(1.0, |s| s.1);
        let mut f = vec![format!("asendcmd=f={}", escape_path(&path))];
        f.extend(
            names
                .iter()
                .map(|n| format!("atempo@{n}=tempo={}", num(tempo(first)))),
        );
        Ok(f)
    }
}

/// Every 50 ms of timeline from `from` to `to`, where the ramped `clip`'s
/// source is then (seconds from `s0`) and its speed.
fn ramp_tempo(clip: &Clip, from: Time, to: Time, s0: f64) -> Vec<(f64, f64)> {
    let step = Time::from_millis(50);
    let mut out = Vec::new();
    let mut t = from;
    loop {
        let at = clip.to_source(t).map_or(0.0, |s| s.as_secs_f64()) - s0;
        out.push((at.max(0.0), clip.speed_at(t - clip.start)));
        if t >= to {
            break;
        }
        t = (t + step).min(to);
    }
    out
}

/// `atempo` stages for `speed` (each stage takes 0.5..100).
fn atempo(speed: f64) -> Vec<String> {
    if (speed - 1.0).abs() < 1e-6 {
        return Vec::new();
    }
    let mut f = Vec::new();
    let mut rest = speed;
    while rest < 0.5 {
        f.push("atempo=0.5".to_string());
        rest /= 0.5;
    }
    f.push(format!("atempo={}", num(rest.min(100.0))));
    f
}

/// Linear gain of a segment of `span` starting at timeline time `start`,
/// over the segment's `t`: volume (gain or keyframes), fades from the
/// span's edges and equal-power ramps, as in `Span::envelope` and
/// `Span::volume_db`.
fn segment_gain(span: &Span, start: Time) -> Anim {
    let clip = span.clip;
    let secs = |t: Time| t.as_secs_f64();
    let mut terms: Vec<String> = Vec::new();
    let mut constant = 1.0f64;
    if clip
        .keyframes
        .iter()
        .any(|k| k.property == Property::Volume)
    {
        let ct = format!("(t+{})", num(secs(start - clip.start)));
        match anim(&clip.transform, &clip.keyframes, Property::Volume, &ct) {
            Anim::Const(v) => constant *= 10f64.powf(v / 20.0),
            e => terms.push(format!("pow(10,{}/20)", e.term())),
        }
    } else {
        constant *= 10f64.powf(clip.audio.gain_db as f64 / 20.0);
    }
    // Span-relative time.
    let st = format!("(t+{})", num(secs(start - span.range.start)));
    let fade_in = secs(clip.audio.fade_in);
    if fade_in > 0.0 {
        terms.push(format!("clip({st}/{},0,1)", num(fade_in)));
    }
    let fade_out = secs(clip.audio.fade_out);
    if fade_out > 0.0 {
        terms.push(format!(
            "clip(({}-{st})/{},0,1)",
            num(secs(span.range.duration())),
            num(fade_out)
        ));
    }
    for ramp in &span.ramps {
        let p = format!(
            "clip((t+{})/{},0,1)",
            num(secs(start - ramp.range.start)),
            num(secs(ramp.range.duration()).max(1e-6))
        );
        let f = if ramp.rising { "sin" } else { "cos" };
        terms.push(format!("{f}({p}*PI/2)"));
    }
    if terms.is_empty() {
        return Anim::Const(constant);
    }
    if (constant - 1.0).abs() > 1e-9 {
        terms.insert(0, num(constant));
    }
    Anim::Expr(terms.join("*"))
}

/// A clip's grade, then the timeline grade over it, as filters (one baked
/// LUT, see [`crate::grade`]); nothing when both are neutral.
fn grade_filters(clip: &Grade, timeline: Option<&Grade>, work_dir: &Path) -> Result<Vec<String>> {
    let mut grades = vec![clip];
    grades.extend(timeline);
    crate::grade::filters(&grades, work_dir)
}
