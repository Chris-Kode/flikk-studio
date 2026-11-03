//! The new words as a recording that fits where the old ones were: cut
//! out of the provider's take of the whole sentence, brought to the
//! speaker's level and tone in that sentence, over the recording's own room
//! tone, and padded to the pauses the old words had.

use super::Fix;
use crate::cloud::{self, Generated, GenerationInfo, GenerationTask, Reading};
use crate::model::MediaItem;
use crate::services::{MediaService, Services};
use crate::time::{Time, TimeRange};
use std::f32::consts::PI;
use std::path::{Path, PathBuf};

/// Everything is worked on as mono at this rate, and written so.
const RATE: u32 = 48_000;
/// Kept of the take before the first new word and after the last, at most
/// (half the pause to the word next to it, at most this).
const PRE: f64 = 0.08;
const POST: f64 = 0.15;
/// Fades on the ends of what is cut out of the take.
const EDGE_FADE: f64 = 0.008;
/// Crossfade where the room tone loops.
const TONE_FADE: f64 = 0.05;
/// The most room tone looped.
const TONE_MAX: f64 = 3.0;
/// Speech frames: 20 ms, counted as speech within this of the loudest.
const FRAME: usize = RATE as usize / 50;
const SPEECH_DB: f32 = 25.0;
/// Where the tone is split into low and high, and how far either is moved.
const SPLIT_HZ: f32 = 1500.0;
const TILT_MAX: f32 = 4.0;
const GAIN_MAX_DB: f32 = 24.0;

/// Have the provider say `fix`'s sentence in `reading`'s voice, and make
/// the recording of its new words to cut in, `source` being the item they
/// replace. A voiceover item, not in the pool yet. Slow: a request, then
/// decoding and the aligner.
pub fn make(
    fix: &Fix,
    source: &MediaItem,
    reading: &Reading,
    services: &Services,
    task: &GenerationTask,
) -> Result<MediaItem, String> {
    let generation = services
        .generation
        .clone()
        .ok_or("Generation is not available in this build.")?;
    let media = services
        .media
        .clone()
        .ok_or("Decoding is not available in this build.")?;
    let analysis = services
        .analysis
        .clone()
        .ok_or("Analysis is not available in this build.")?;
    let dir = crate::paths::cache().join("respeak");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let request = super::request(fix, reading);
    let take = generation
        .generate(&request, &dir, task)
        .map_err(|e| e.to_string())?;
    let made = splice(
        fix,
        source,
        reading,
        &take,
        media.as_ref(),
        |path, script| {
            analysis
                .time_script(path, script)
                .map(|w| w.into_iter().map(|w| w.range).collect())
                .map_err(|e| e.0)
        },
    );
    if made.is_ok() {
        let _ = std::fs::remove_file(&take.path);
        let _ = std::fs::remove_file(sidecar(&take.path));
    }
    made
}

/// The new words cut out of `take` and fitted in; `time` says where each
/// word of a script is said in a file.
fn splice(
    fix: &Fix,
    source: &MediaItem,
    reading: &Reading,
    take: &Generated,
    media: &dyn MediaService,
    time: impl Fn(&Path, &str) -> Result<Vec<TimeRange>, String>,
) -> Result<MediaItem, String> {
    let script = fix.script();
    let words = time(&take.path, &script)?;
    let new = fix.new_words();
    if new.is_empty() || words.len() < new.end {
        return Err("The provider's take could not be followed.".into());
    }
    let take_item = cloud::sample_item(take.path.clone(), Some(media));
    let take_len = take_item
        .info
        .as_ref()
        .map(|i| i.duration)
        .unwrap_or(Time::ZERO);
    let take_stream = take_item
        .info
        .as_ref()
        .and_then(|i| i.audio.first())
        .map_or(0, |a| a.index);
    let audio = decode(
        media,
        &take_item,
        take_stream,
        TimeRange::new(Time::ZERO, take_len),
    )?;
    let secs = |t: Time| t.as_secs_f64();
    let (s, e) = (secs(words[new.start].start), secs(words[new.end - 1].end));
    if e <= s {
        return Err("The provider's take could not be followed.".into());
    }
    let pre = match new.start {
        0 => s,
        k => (s - secs(words[k - 1].end)) / 2.0,
    }
    .clamp(0.0, PRE);
    let post = match words.get(new.end) {
        Some(w) => (secs(w.start) - e) / 2.0,
        None => secs(take_len) - e,
    }
    .clamp(0.0, POST);

    // Level and tone of the speaker in this sentence, and of the take.
    let reference = decode(media, source, fix.stream, fix.sentence)?;
    let whole = TimeRange::new(words[0].start, words[words.len() - 1].end);
    let theirs = Voice::of(&reference);
    let read = Voice::of(&slice(&audio, whole.start.as_secs_f64(), secs(whole.end)));
    let gain = 10f32.powf(((theirs.db - read.db).clamp(-GAIN_MAX_DB, GAIN_MAX_DB)) / 20.0);
    let tilt = if read.tilt > 0.0 && theirs.tilt > 0.0 {
        (theirs.tilt / read.tilt)
            .sqrt()
            .clamp(1.0 / TILT_MAX, TILT_MAX)
    } else {
        1.0
    };
    let mut said = slice(&audio, s - pre, e + post);
    shape(&mut said, tilt, gain);
    fade_ends(&mut said, samples(EDGE_FADE));

    // Laid over the room tone, after the pause the old words had before
    // them, and followed by the one they had after.
    let lead = secs(fix.words.start - fix.source.start).max(0.0);
    let tail = secs(fix.source.end - fix.words.end).max(0.0);
    let offset = samples((lead - pre).max(0.0));
    let total = (offset + said.len()).max(offset + samples(pre + (e - s) + tail));
    let tone = fix
        .tone
        .iter()
        .max_by_key(|r| r.duration())
        .map(|r| {
            let end = r.end.min(r.start + Time::from_secs_f64(TONE_MAX));
            decode(media, source, fix.stream, TimeRange::new(r.start, end))
        })
        .transpose()?
        .unwrap_or_default();
    let mut out = looped(&tone, total);
    for (o, x) in out[offset..].iter_mut().zip(&said) {
        *o += x;
    }
    let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
    if peak > 0.99 {
        out.iter_mut().for_each(|x| *x *= 0.99 / peak);
    }

    let path = free_path(&crate::paths::generated(), &fix.text);
    write_wav(&path, &out).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let info = GenerationInfo {
        prompt: fix.text.clone(),
        reading: Some(reading.clone()),
        ..take.info.clone()
    };
    let json = serde_json::json!({
        "fix": fix,
        "made_by": &info,
        "credits": take.cost,
    });
    if let Ok(text) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(sidecar(&path), text);
    }
    Ok(cloud::media_item(
        &Generated {
            path,
            info,
            cost: take.cost,
        },
        Some(media),
    ))
}

/// How a voice sounds: its speech level, and how bright it is (the high
/// band's energy over the low band's).
struct Voice {
    db: f32,
    tilt: f32,
}

impl Voice {
    fn of(audio: &[f32]) -> Self {
        let (low, high) = split(audio);
        let frames: Vec<(f32, f32, f32)> = (0..audio.len() / FRAME)
            .map(|k| {
                let r = k * FRAME..(k + 1) * FRAME;
                let energy = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>() / FRAME as f32;
                (
                    energy(&audio[r.clone()]),
                    energy(&low[r.clone()]),
                    energy(&high[r]),
                )
            })
            .collect();
        let loudest = frames.iter().map(|f| f.0).fold(0f32, f32::max);
        if loudest <= 0.0 {
            return Self {
                db: -100.0,
                tilt: 0.0,
            };
        }
        let floor = loudest * 10f32.powf(-SPEECH_DB / 10.0);
        let speech: Vec<&(f32, f32, f32)> = frames.iter().filter(|f| f.0 >= floor).collect();
        let n = speech.len().max(1) as f32;
        let mean = speech.iter().map(|f| f.0).sum::<f32>() / n;
        let low: f32 = speech.iter().map(|f| f.1).sum();
        let high: f32 = speech.iter().map(|f| f.2).sum();
        Self {
            db: 10.0 * mean.max(1e-12).log10(),
            tilt: if low > 0.0 { high / low } else { 0.0 },
        }
    }
}

/// `audio` split at [`SPLIT_HZ`] by a one-pole low-pass: low, high.
fn split(audio: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let a = (-2.0 * PI * SPLIT_HZ / RATE as f32).exp();
    let mut state = 0.0;
    let low: Vec<f32> = audio
        .iter()
        .map(|&x| {
            state = (1.0 - a) * x + a * state;
            state
        })
        .collect();
    let high = audio.iter().zip(&low).map(|(x, l)| x - l).collect();
    (low, high)
}

/// The high band times `tilt` (a shelf), all of it times `gain`.
fn shape(audio: &mut [f32], tilt: f32, gain: f32) {
    let (low, high) = split(audio);
    for (i, x) in audio.iter_mut().enumerate() {
        *x = (low[i] + tilt * high[i]) * gain;
    }
}

fn fade_ends(audio: &mut [f32], n: usize) {
    let n = n.min(audio.len() / 2);
    let len = audio.len();
    for i in 0..n {
        let w = (i as f32 / n as f32 * PI / 2.0).sin();
        audio[i] *= w;
        audio[len - 1 - i] *= w;
    }
}

/// `tone` repeated to `n` samples, crossfaded at equal power where it
/// starts again; silence without any.
fn looped(tone: &[f32], n: usize) -> Vec<f32> {
    let fade = samples(TONE_FADE);
    if tone.len() <= 2 * fade + 1 {
        return vec![0.0; n];
    }
    let hop = tone.len() - fade;
    let mut out = vec![0.0; n + tone.len()];
    let mut at = 0;
    while at < n {
        for (i, &x) in tone.iter().enumerate() {
            let w = if i < fade && at > 0 {
                (i as f32 / fade as f32 * PI / 2.0).sin()
            } else if i >= hop {
                ((tone.len() - 1 - i) as f32 / fade as f32 * PI / 2.0).sin()
            } else {
                1.0
            };
            out[at + i] += x * w;
        }
        at += hop;
    }
    out.truncate(n);
    out
}

fn samples(secs: f64) -> usize {
    (secs.max(0.0) * RATE as f64).round() as usize
}

fn slice(audio: &[f32], from: f64, to: f64) -> Vec<f32> {
    let a = samples(from).min(audio.len());
    let b = samples(to).clamp(a, audio.len());
    audio[a..b].to_vec()
}

/// `range` of `item`'s `stream` as mono at [`RATE`].
fn decode(
    media: &dyn MediaService,
    item: &MediaItem,
    stream: u32,
    range: TimeRange,
) -> Result<Vec<f32>, String> {
    let from = range.start.max(Time::ZERO);
    let want = samples((range.end - from).as_secs_f64());
    let mut pcm = media.audio(item, stream, from, RATE).map_err(|e| e.0)?;
    let mut out = Vec::with_capacity(want);
    let mut buf = vec![0f32; 16_384];
    while out.len() < want {
        let n = pcm.read(&mut buf).map_err(|e| e.0)?;
        if n == 0 {
            break;
        }
        out.extend(
            buf[..n]
                .chunks(2)
                .map(|f| f.iter().sum::<f32>() / f.len() as f32),
        );
    }
    out.truncate(want);
    Ok(out)
}

/// `<dir>/fix-<a few words>.wav`, numbered past any file already there.
fn free_path(dir: &Path, text: &str) -> PathBuf {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(str::to_lowercase)
        .collect();
    let base = if words.is_empty() {
        "fix".to_owned()
    } else {
        format!("fix-{}", words.join("-"))
    };
    let mut path = dir.join(format!("{base}.wav"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.wav"));
        n += 1;
    }
    path
}

fn sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".json");
    PathBuf::from(name)
}

/// Mono 32-bit float WAV at [`RATE`], through a `.part` file.
fn write_wav(path: &Path, audio: &[f32]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let data = (audio.len() * 4) as u32;
    let mut bytes = Vec::with_capacity(44 + audio.len() * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for x in audio {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    let part = path.with_extension("wav.part");
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path)
}
