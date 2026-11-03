//! Pictures of audio for the agent's `view_audio`: a time ruler, then a
//! spectrogram lane per track (ffmpeg's `showspectrumpic`, log frequency,
//! low notes at the bottom), then rows marking words, sounds and beats.
//! Downbeats also run as faint lines through the lanes.

use crate::canvas::Canvas;
use crate::{FfmpegMedia, err, secs_arg, stderr_tail};
use flikk_core::services::{
    AudioMarkKind, AudioPictureRequest, AudioSegment, Frame, ServiceError, ServiceResult,
};
use flikk_core::time::Time;

const RULER: u32 = 22;
const LANE: u32 = 150;
const ROW: u32 = 18;
const GAP: u32 = 4;
const BACKGROUND: [u8; 3] = [24, 24, 24];
const MISSING: [u8; 3] = [48, 16, 16];
const RULE: [u8; 3] = [150, 150, 150];
const WORD: [u8; 3] = [235, 235, 235];
const SOUND: [u8; 3] = [255, 176, 64];
const BEAT: [u8; 3] = [64, 200, 230];

pub(crate) fn audio_picture(
    media: &FfmpegMedia,
    request: &AudioPictureRequest,
) -> ServiceResult<Frame> {
    let range = request.range;
    if range.is_empty() {
        return Err(ServiceError("audio picture of an empty range".into()));
    }
    let width = request.width.clamp(400, 2400);
    let span = range.duration().as_secs_f64();
    let x_of = |t: Time| {
        ((t - range.start).as_secs_f64() / span * width as f64)
            .round()
            .clamp(0.0, width as f64) as u32
    };
    let has = |k: AudioMarkKind| request.marks.iter().any(|m| m.kind == k);
    let rows: Vec<AudioMarkKind> = [AudioMarkKind::Word, AudioMarkKind::Sound]
        .into_iter()
        .filter(|k| has(*k))
        .chain(
            (has(AudioMarkKind::Beat) || has(AudioMarkKind::Downbeat))
                .then_some(AudioMarkKind::Beat),
        )
        .collect();
    let lanes = request.lanes.len().max(1) as u32;
    let lanes_bottom = RULER + lanes * (LANE + GAP);
    let height = lanes_bottom + rows.len() as u32 * ROW + GAP;
    let mut canvas = Canvas::new(width, height, BACKGROUND);

    // Ruler: a tick every `step`, a time every few.
    let step = [0.25, 0.5, 1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .find(|s| span / s <= 40.0)
        .unwrap_or(30.0);
    let every = if span / step > 20.0 { 4 } else { 2 };
    let first = (range.start.as_secs_f64() / step).ceil() as i64;
    for k in first.. {
        let t = Time::from_secs_f64(k as f64 * step);
        if t > range.end {
            break;
        }
        let x = x_of(t).min(width - 1);
        let tall = k % every == 0;
        canvas.fill(
            x,
            RULER - if tall { 8 } else { 4 },
            1,
            if tall { 8 } else { 4 },
            RULE,
        );
        if tall {
            canvas.text(&stamp(t, step), x + 3, 3, 2, RULE);
        }
    }

    for (i, lane) in request.lanes.iter().enumerate() {
        let y = RULER + i as u32 * (LANE + GAP);
        for seg in &lane.segments {
            let (x0, x1) = (x_of(seg.timeline.start), x_of(seg.timeline.end));
            if x1 < x0 + 4 {
                continue;
            }
            match spectrum(media, seg, x1 - x0, LANE) {
                Ok(frame) => canvas.fit(&frame, x0, y, x1 - x0, LANE),
                Err(_) => canvas.fill(x0, y, x1 - x0, LANE, MISSING),
            }
        }
        canvas.label(&[&lane.name], 0, y, 2, false);
    }

    for mark in request
        .marks
        .iter()
        .filter(|m| m.kind == AudioMarkKind::Downbeat)
    {
        let x = x_of(mark.at.start).min(width - 1);
        canvas.tint(x, RULER, 1, lanes_bottom - RULER - GAP, BEAT, 0.8);
    }

    for (r, row) in rows.iter().enumerate() {
        let y = lanes_bottom + r as u32 * ROW;
        // Where the last label ends, so labels never overlap.
        let mut free = 0;
        for mark in &request.marks {
            let x = x_of(mark.at.start).min(width - 1);
            let x1 = x_of(mark.at.end).max(x + 1);
            match (row, mark.kind) {
                (AudioMarkKind::Word, AudioMarkKind::Word) => {
                    canvas.fill(x, y + 2, 1, 4, WORD);
                    if x >= free {
                        let text: String = mark.label.chars().take(24).collect();
                        canvas.text(&text, x + 2, y + 6, 1, WORD);
                        free = x + 2 + text.chars().count() as u32 * 6 + 4;
                    }
                }
                (AudioMarkKind::Sound, AudioMarkKind::Sound) => {
                    canvas.fill(x, y + 2, (x1 - x).max(2), 4, SOUND);
                    if x >= free {
                        let text: String = mark.label.chars().take(24).collect();
                        canvas.text(&text, x + 1, y + 8, 1, SOUND);
                        free = x + 1 + text.chars().count() as u32 * 6 + 4;
                    }
                }
                (AudioMarkKind::Beat, AudioMarkKind::Beat) => canvas.fill(x, y + 8, 1, 6, BEAT),
                (AudioMarkKind::Beat, AudioMarkKind::Downbeat) => {
                    canvas.fill(x.saturating_sub(1), y + 2, 3, 12, BEAT)
                }
                _ => {}
            }
        }
    }
    Ok(canvas.frame())
}

/// `M:SS`, with tenths when ticks are closer than a second.
fn stamp(t: Time, step: f64) -> String {
    let s = t.as_secs_f64();
    let whole = s as i64;
    if step < 1.0 {
        format!(
            "{}:{:02}.{}",
            whole / 60,
            whole % 60,
            ((s.fract()) * 10.0).round() as i64 % 10
        )
    } else {
        format!("{}:{:02}", whole / 60, whole % 60)
    }
}

/// The spectrogram of one segment, `w` × `h`.
fn spectrum(media: &FfmpegMedia, seg: &AudioSegment, w: u32, h: u32) -> ServiceResult<Frame> {
    let mut cmd = media.ffmpeg();
    cmd.args(["-ss", &secs_arg(seg.source.start)])
        .args(["-t", &secs_arg(seg.source.duration())])
        .arg("-i")
        .arg(&seg.path)
        .args([
            "-filter_complex",
            &format!(
                "[0:a:{}]aformat=channel_layouts=mono,showspectrumpic=s={w}x{h}:legend=0:fscale=log:scale=log:color=intensity[v]",
                seg.stream
            ),
        ])
        .args(["-map", "[v]", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    let out = cmd.output().map_err(|e| err("spectrogram", e))?;
    let need = (w * h * 4) as usize;
    if !out.status.success() || out.stdout.len() < need {
        return Err(ServiceError(format!(
            "spectrogram of {}: {}",
            seg.path.display(),
            stderr_tail(&out.stderr)
        )));
    }
    Ok(Frame {
        width: w,
        height: h,
        rgba: out.stdout[..need].into(),
    })
}
