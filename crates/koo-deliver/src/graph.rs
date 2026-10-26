//! Project → one FFmpeg `filter_complex` graph.
//!
//! Video: each visible track becomes one stream, the `concat` of transparent
//! gaps and clip segments at sequence size and rate; tracks are overlaid
//! bottom-up on black. Each clip segment trims its source (`movie` with a
//! seek, `trim`/`setpts`), grades it, scales/rotates/fades it (keyframes as
//! expressions over `t`) and places it on a transparent canvas. Concat keeps
//! only the current segment's decoder busy, so long timelines stay cheap.
//!
//! Audio: each audible track is a `concat` of silence and clip segments
//! (`atrim`, gain, fades), then track volume and pan, optional ducking via
//! `sidechaincompress` keyed by the dialogue tracks, the mix, and optional
//! `loudnorm`.
//!
//! Sources are opened with `movie`/`amovie` inside the graph, so the whole
//! render is one script file and one input-less ffmpeg invocation.

use crate::captions;
use crate::ffmpeg::{escape_path, probe_size};
use crate::keyframes::{Anim, anim, num};
use crate::preset::{LoudnessTarget, RenderSettings};
use anyhow::{Context as _, Result, bail};
use koo_core::memory::CaptionStyle;
use koo_core::model::{Clip, ClipContent, Grade, MediaKind, Project, Property, Track, TrackKind};
use koo_core::time::FLICKS_PER_SECOND;
use koo_core::{Rate, Time, TimeRange, TrackId};
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
}

impl GraphSpec {
    pub fn new(project: &Project, settings: &RenderSettings, styles: &[CaptionStyle]) -> Self {
        let seq = &project.timeline.settings;
        Self {
            width: settings.width.unwrap_or(seq.width),
            height: settings.height.unwrap_or(seq.height),
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
    };
    let frames = b.frames(spec.range.end).max(1);
    let video = if spec.video {
        Some(b.video(frames, work_dir)?)
    } else {
        None
    };
    let audio = if spec.audio { Some(b.audio()?) } else { None };
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
}

/// A picture source for a clip segment.
struct Picture {
    path: PathBuf,
    still: bool,
    size: (u32, u32),
}

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
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
        self.chains.push(format!(
            "color=c=black:s={w}x{h}:r={r},trim=end_frame={frames},setsar=1,format=yuv420p[{cur}]"
        ));
        let tracks: Vec<&Track> = self
            .project
            .timeline
            .tracks
            .iter()
            .rev()
            .filter(|t| matches!(t.kind, TrackKind::Video | TrackKind::Overlay) && !t.mix.mute)
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
        if self.spec.burn_captions && self.has_text() {
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

    fn has_text(&self) -> bool {
        self.project.timeline.tracks.iter().any(|t| {
            !t.mix.mute
                && matches!(t.kind, TrackKind::Caption | TrackKind::Title)
                && t.clips.iter().any(|c| c.range().overlaps(&self.spec.range))
        })
    }

    fn video_track(&mut self, track: &Track) -> Result<Option<String>> {
        let mut segments = Vec::new();
        let mut cursor = 0i64;
        for clip in &track.clips {
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
            segments.push(self.clip_segment(clip, &picture, start, f1 - f0));
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
        Ok(Some(Picture { path, still, size }))
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
    fn clip_segment(&mut self, clip: &Clip, pic: &Picture, start: Time, frames: i64) -> String {
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
        }
        f.extend(grade_filters(&clip.grade));
        if matches!(clip.content, ClipContent::Media { .. }) {
            f.extend(grade_filters(&self.project.timeline.grade));
        }
        f.push("format=yuva420p".into());

        let (sw, sh) = (pic.size.0.max(1) as f64, pic.size.1.max(1) as f64);
        let fit = (w as f64 / sw).min(h as f64 / sh);
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
        out
    }

    // ---------------------------------------------------------------- audio

    fn audio(&mut self) -> Result<String> {
        let sr = self.spec.sample_rate;
        let total = self.samples(self.spec.range.end).max(1);
        let tracks: Vec<&Track> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .collect();
        let solo = tracks.iter().any(|t| t.mix.solo);
        let mut streams: Vec<(TrackId, String)> = Vec::new();
        for track in tracks {
            if track.mix.mute || (solo && !track.mix.solo) {
                continue;
            }
            if let Some(s) = self.audio_track(track)? {
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
                self.chains
                    .push(format!("[{key_src}]asplit=2[{key}][{voice}]"));
                // sidechaincompress has no range control: pick the ratio so
                // dialogue around -20 dBFS (16.5 dB over the threshold)
                // pulls the music down by about `reduction_db`.
                let reduction = d.reduction_db.abs() as f64;
                let ratio = if reduction >= 16.0 {
                    20.0
                } else {
                    (1.0 / (1.0 - reduction / 16.5)).clamp(1.0, 20.0)
                };
                let attack = (d.attack.as_secs_f64() * 1000.0).clamp(0.01, 2000.0);
                let release = (d.release.as_secs_f64() * 1000.0).clamp(0.01, 9000.0);
                let ducked = self.label("ducked");
                self.chains.push(format!(
                    "[{music}][{key}]sidechaincompress=threshold=0.015:ratio={}:attack={}:release={}:makeup=1[{ducked}]",
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

    fn audio_track(&mut self, track: &Track) -> Result<Option<String>> {
        let mut segments = Vec::new();
        let mut cursor = 0i64;
        for clip in &track.clips {
            let Some(visible) = clip.range().intersect(&self.spec.range) else {
                continue;
            };
            let s0 = self.samples(visible.start).max(cursor);
            let s1 = self.samples(visible.end);
            if s1 <= s0 {
                continue;
            }
            let Some(segment) = self.audio_segment(clip, visible, s1 - s0)? else {
                continue;
            };
            if s0 > cursor {
                let gap = self.label("silence");
                self.chains.push(format!(
                    "anullsrc=r={}:cl=stereo,atrim=end_sample={}[{gap}]",
                    self.spec.sample_rate,
                    s0 - cursor
                ));
                segments.push(gap);
            }
            segments.push(segment);
            cursor = s1;
        }
        let Some(stream) = self.concat(segments, false) else {
            return Ok(None);
        };
        let mut f: Vec<String> = Vec::new();
        if track.mix.volume_db.abs() > 1e-3 {
            f.push(format!("volume={}dB", num(track.mix.volume_db as f64)));
        }
        let pan = track.mix.pan.clamp(-1.0, 1.0) as f64;
        if pan.abs() > 1e-3 {
            let (l, r) = ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));
            f.push(format!("pan=stereo|c0={}*c0|c1={}*c1", num(l), num(r)));
        }
        if f.is_empty() {
            return Ok(Some(stream));
        }
        let out = self.label("trackmix");
        self.chains
            .push(format!("[{stream}]{}[{out}]", f.join(",")));
        Ok(Some(out))
    }

    fn audio_segment(
        &mut self,
        clip: &Clip,
        visible: TimeRange,
        samples: i64,
    ) -> Result<Option<String>> {
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
        let sr = self.spec.sample_rate;
        let dur = clip.duration.as_secs_f64();
        let offset = (visible.start - clip.start).as_secs_f64();
        let length = visible.duration().as_secs_f64();
        let fade_in = clip.audio.fade_in.as_secs_f64().clamp(0.0, dur);
        let fade_out = clip.audio.fade_out.as_secs_f64().clamp(0.0, dur);

        // Fades are in clip time. When the range starts inside the fade-in,
        // decode from the clip start so the curve is the same, then cut.
        let from = if offset > 0.0 && offset < fade_in {
            0.0
        } else {
            offset
        };
        let s0 = source_in.as_secs_f64() + from;
        let s1 = source_in.as_secs_f64() + offset + length;
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
        if clip.audio.gain_db.abs() > 1e-3 {
            f.push(format!("volume={}dB", num(clip.audio.gain_db as f64)));
        }
        if fade_in > 0.0 && from < fade_in {
            f.push(format!(
                "afade=t=in:st=0:d={}:curve=tri",
                num(fade_in - from)
            ));
        }
        if fade_out > 0.0 {
            let st = dur - fade_out - from;
            if st >= 0.0 {
                f.push(format!(
                    "afade=t=out:st={}:d={}:curve=tri",
                    num(st),
                    num(fade_out)
                ));
            } else {
                f.push(format!(
                    "afade=t=out:st=0:d={}:curve=tri",
                    num((fade_out + st).max(0.001))
                ));
            }
        }
        if from < offset {
            f.push(format!("atrim=start={}", num(offset - from)));
            f.push("asetpts=PTS-STARTPTS".into());
        }
        f.push(format!("apad=whole_len={samples}"));
        f.push(format!("atrim=end_sample={samples}"));
        let out = self.label("aclip");
        self.chains.push(format!("{}[{out}]", f.join(",")));
        Ok(Some(out))
    }
}

/// Primary grade (PRD K1) as filters; nothing for a neutral grade.
fn grade_filters(g: &Grade) -> Vec<String> {
    let mut f = Vec::new();
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
    // Exposure in stops and per-channel gain: a multiply in RGB.
    let k = 2f32.powf(g.exposure);
    let gain = [g.gain[0] * k, g.gain[1] * k, g.gain[2] * k];
    if gain.iter().any(|v| !near(*v, 1.0)) {
        f.push(format!(
            "colorchannelmixer=rr={}:gg={}:bb={}",
            num(gain[0] as f64),
            num(gain[1] as f64),
            num(gain[2] as f64)
        ));
    }
    if !near(g.contrast, 1.0) || !near(g.saturation, 1.0) || !near(g.gamma, 1.0) {
        f.push(format!(
            "eq=contrast={}:saturation={}:gamma={}",
            num(g.contrast.clamp(-1000.0, 1000.0) as f64),
            num(g.saturation.clamp(0.0, 3.0) as f64),
            num(g.gamma.clamp(0.1, 10.0) as f64)
        ));
    }
    // Temperature/tint: -1..1 (values beyond that are read as percent).
    let unit = |v: f32| if v.abs() > 1.0 { v / 100.0 } else { v }.clamp(-1.0, 1.0);
    let (temp, tint) = (unit(g.temperature), unit(g.tint));
    let lift = g.lift.map(|v| v.clamp(-1.0, 1.0));
    if !near(temp, 0.0) || !near(tint, 0.0) || lift.iter().any(|v| !near(*v, 0.0)) {
        f.push(format!(
            "colorbalance=rs={}:gs={}:bs={}:rm={}:gm={}:bm={}",
            num(lift[0] as f64),
            num(lift[1] as f64),
            num(lift[2] as f64),
            num((temp * 0.3) as f64),
            num((-tint * 0.3) as f64),
            num((-temp * 0.3) as f64)
        ));
    }
    if g.gamma_rgb.iter().any(|v| !near(*v, 1.0)) {
        let p = g.gamma_rgb.map(|v| 1.0 / v.clamp(0.1, 10.0) as f64);
        f.push(format!(
            "lutrgb=r='gammaval({})':g='gammaval({})':b='gammaval({})'",
            num(p[0]),
            num(p[1]),
            num(p[2])
        ));
    }
    if let Some(lut) = &g.lut {
        f.push(format!("lut3d=file={}", escape_path(lut)));
    }
    f
}
