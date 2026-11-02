//! Renders through the compositor: every output frame is composed by
//! [`koo_core::compose::program`] at render size and piped into FFmpeg as
//! raw RGBA, where the graph ([`GraphSpec::piped`]) burns in captions and
//! titles (so the compositor leaves them out, unless FFmpeg cannot burn them
//! in, see [`letter`]) and builds the audio in the same process.

use crate::Letterer;
use crate::ffmpeg::{self, command};
use crate::graph::{self, Graph, GraphSpec};
use anyhow::{Context as _, Result, anyhow, bail};
use koo_core::compose::{self, Pictures, Streams};
use koo_core::depth;
use koo_core::model::{MediaItem, Project};
use koo_core::services::{Frame, LetteringService, PersonRef, Services};
use koo_core::speed::Waiting;
use koo_core::stabilize;
use koo_core::time::FLICKS_PER_SECOND;
use koo_core::{ClipId, Time, TimeRange};
use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Make every matte the subject masks over `range` show (tracking them
/// when they track), and the person masks' frames in it, before frames
/// are composed; `progress` gets 0..1. Whether there was any to make.
/// Fails when a subject or the people cannot be found (the model is
/// missing).
pub(crate) fn find_subjects(
    project: &Project,
    range: TimeRange,
    services: &Services,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<bool> {
    let Some(service) = &services.subjects else {
        return Ok(false);
    };
    let subjects: Vec<_> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| !t.mix.mute)
        .flat_map(|t| &t.clips)
        .filter(|c| c.enabled && c.range().overlaps(&range))
        .flat_map(|c| {
            c.masks
                .iter()
                .filter(|m| m.enabled)
                .filter_map(move |m| compose::subject_ref(project, c, m))
        })
        .filter(|r| {
            let track = r.subject.track;
            !service.status(*r).complete(track)
        })
        .collect();
    // Person masks: the frames of their clips the range shows.
    let people: Vec<PersonRef> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| !t.mix.mute)
        .flat_map(|t| &t.clips)
        .filter(|c| c.enabled && c.range().overlaps(&range))
        .flat_map(|c| {
            c.masks.iter().filter(|m| m.enabled).filter_map(move |m| {
                let whole = compose::person_ref(project, c, m)?;
                let (from, to) = (range.start.max(c.start), range.end.min(c.end()));
                let (a, b) = (c.to_source(from)?, c.to_source(to)?);
                Some(PersonRef {
                    range: TimeRange::new(a.min(b), a.max(b)),
                    ..whole
                })
            })
        })
        .filter(|r| {
            let s = service.person_status(*r);
            s.error.is_some() || s.tracked < s.frames
        })
        .collect();
    let n = (subjects.len() + people.len()) as f32;
    for (i, r) in subjects.iter().enumerate() {
        let part = |p: f32| progress((i as f32 + p) / n);
        service
            .complete(*r, &part, cancel)
            .map_err(|e| anyhow!("subject mask {}: {e}", short(r.mask)))?;
    }
    for (i, r) in people.iter().enumerate() {
        let i = subjects.len() + i;
        let part = |p: f32| progress((i as f32 + p) / n);
        service
            .complete_person(*r, &part, cancel)
            .map_err(|e| anyhow!("person mask {}: {e}", short(r.mask)))?;
    }
    Ok(!subjects.is_empty() || !people.is_empty())
}

/// Measure the camera path of every stabilized clip over `range` that is
/// not measured yet, before frames are composed; `progress` gets 0..1.
/// Whether there was any to measure. A path that cannot be measured fails
/// the render.
pub(crate) fn measure_motion(
    project: &Project,
    range: TimeRange,
    services: &Services,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<bool> {
    let Some(stabilizer) = &services.stabilizer else {
        return Ok(false);
    };
    let mut items: Vec<&MediaItem> = Vec::new();
    for clip in project
        .timeline
        .tracks
        .iter()
        .filter(|t| !t.mix.mute)
        .flat_map(|t| &t.clips)
        .filter(|c| c.enabled && c.range().overlaps(&range))
    {
        if let Some((item, _)) = stabilize::source(project, clip)
            && stabilizer.path(item).is_none()
            && !items.iter().any(|i| i.id == item.id)
        {
            items.push(item);
        }
    }
    let n = items.len() as f32;
    for (i, item) in items.iter().enumerate() {
        let part = |p: f32| progress((i as f32 + p) / n);
        stabilizer
            .complete(item, &part, cancel)
            .map_err(|e| anyhow!("stabilizing {}: {e}", item.path.display()))?;
    }
    Ok(!items.is_empty())
}

/// Make every depth map the clips over `range` read (their depth effects
/// and masks), before frames are composed; `progress` gets 0..1. Whether
/// there was any to make. Fails when one cannot be made (the model is
/// missing).
pub(crate) fn find_depth(
    project: &Project,
    range: TimeRange,
    services: &Services,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<bool> {
    let Some(service) = &services.depth else {
        return Ok(false);
    };
    let wanted: Vec<_> = project
        .timeline
        .tracks
        .iter()
        .filter(|t| !t.mix.mute)
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .filter(|(_, c)| c.enabled && depth::clip_needs_depth(project, c))
        .filter_map(|(t, c)| Some((c.id, depth::source(project, t, c, range)?)))
        .filter(|(_, (item, r))| !service.status(item, *r).complete())
        .collect();
    let n = wanted.len() as f32;
    for (i, (clip, (item, r))) in wanted.iter().enumerate() {
        let part = |p: f32| progress((i as f32 + p) / n);
        service
            .complete(item, *r, &part, cancel)
            .map_err(|e| anyhow!("depth of clip {}: {e}", short(clip)))?;
    }
    Ok(!wanted.is_empty())
}

fn short(id: impl std::fmt::Display) -> String {
    id.to_string().chars().take(8).collect()
}

/// Whether `range` must be composited rather than drawn by the graph, and
/// can be (the media engine decodes the sources).
pub(crate) fn wanted(project: &Project, range: TimeRange, services: &Services) -> bool {
    services.media.is_some() && compose::needs_compositor(project, range)
}

/// Pipe `spec` through the compositor to letter its captions and titles
/// there, when FFmpeg has no `ass` filter to burn them in with.
pub(crate) fn letter(project: &Project, spec: &mut GraphSpec, services: &Services) {
    spec.lettered = spec.video
        && spec.burn_captions
        && services.media.is_some()
        && graph::has_text(project, spec.range)
        && !ffmpeg::has_ass();
    spec.piped |= spec.lettered;
}

/// Source pictures for a render: decoded in step with the frames from the
/// original files, never the playback proxies.
struct Originals(Streams);

impl Pictures for Originals {
    fn frame(&mut self, clip: ClipId, item: &MediaItem, t: Time) -> Option<Frame> {
        // The media engine finds proxies by path or fingerprint.
        let item = MediaItem {
            proxy: None,
            generated: None,
            fingerprint: None,
            ..item.clone()
        };
        self.0.frame(clip, &item, t)
    }

    fn finish(&mut self) {
        self.0.finish();
    }
}

/// Composes the frames of `spec`'s range in order.
pub(crate) struct Frames<'a> {
    project: &'a Project,
    /// Lettering only when [`GraphSpec::lettered`].
    services: Services,
    spec: &'a GraphSpec,
    pictures: Originals,
}

impl<'a> Frames<'a> {
    pub(crate) fn new(project: &'a Project, services: &'a Services, spec: &'a GraphSpec) -> Self {
        let pictures = Originals(Streams::new(services.media.clone(), spec.width, spec.rate));
        let lettering = spec.lettered.then(|| {
            Arc::new(Letterer::new(spec.caption_styles.clone())) as Arc<dyn LetteringService>
        });
        Self {
            project,
            services: Services {
                lettering,
                // Every frame between source frames is made, not blended.
                interpolation: services.interpolation.clone().map(|s| {
                    Arc::new(Waiting(s)) as Arc<dyn koo_core::services::InterpolationService>
                }),
                ..services.clone()
            },
            spec,
            pictures,
        }
    }

    /// Output frame `f` (from the range start).
    pub(crate) fn frame(&mut self, f: i64) -> Frame {
        let flicks = f as i128 * self.spec.rate.den as i128 * FLICKS_PER_SECOND as i128
            / self.spec.rate.num.max(1) as i128;
        let t = self.spec.range.start + Time(flicks as i64);
        compose::program(
            self.project,
            t,
            (self.spec.width, self.spec.height),
            &self.services,
            &mut self.pictures,
        )
    }
}

/// The input arguments for `spec`'s frames on stdin.
pub(crate) fn input_args(spec: &GraphSpec) -> Vec<OsString> {
    [
        "-f".to_string(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-video_size".into(),
        format!("{}x{}", spec.width, spec.height),
        "-framerate".into(),
        format!("{}/{}", spec.rate.num, spec.rate.den),
        "-i".into(),
        "pipe:0".into(),
    ]
    .map(OsString::from)
    .to_vec()
}

/// Run FFmpeg with `args` (which read `pipe:0`, see [`input_args`]),
/// writing `graph.frames` frames from `frames`. `progress` gets the
/// fraction written; `cancel` kills FFmpeg between frames.
pub(crate) fn run(
    args: &[OsString],
    graph: &Graph,
    frames: &mut Frames<'_>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<()> {
    let mut child = command(ffmpeg::ffmpeg())
        .arg("-hide_banner")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {}", ffmpeg::ffmpeg().display()))?;
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let mut stdin = child.stdin.take().expect("piped");
    let total = graph.frames.max(1);
    let mut write_failed = false;
    for f in 0..total {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("canceled");
        }
        let frame = frames.frame(f);
        if stdin.write_all(&frame.rgba).is_err() {
            write_failed = true;
            break;
        }
        progress((f + 1) as f32 / total as f32);
    }
    drop(stdin);
    let status = child.wait()?;
    let stderr = errors.join().unwrap_or_default();
    if !status.success() || write_failed {
        return Err(anyhow!(ffmpeg::last_error(&stderr)));
    }
    Ok(())
}
