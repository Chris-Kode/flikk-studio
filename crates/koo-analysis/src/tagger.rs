//! Sound tagging with EfficientAT mn10_as (MIT, Florian Schmid): the 527
//! AudioSet classes for 2 s windows every second of 32 kHz audio, turned
//! into the sounds and music of the sound script.
//!
//! The model was trained on 10 s clips and answers nonsense for 2 s alone;
//! followed by 4 s of silence, a window gets nine in ten of the answers it
//! would get padded to 10 s, at half the cost. A window is tagged with a
//! sound wherever in it the sound plays, so a sound's run of windows
//! starts up to 2 s before it and ends up to 1 s before its end; the run
//! is shrunk by that much, and a short sound is then moved onto the
//! loudest stretch of the audio levels around it.

use crate::download::ModelFile;
use crate::ffmpeg::SOUND_RATE;
use anyhow::{Result, anyhow, bail};
use koo_core::analysis::{Levels, Music, Sound};
use koo_core::time::{FLICKS_PER_SECOND, Time, TimeRange};
use ort::session::Session;
use ort::value::Tensor;

pub const MODEL: ModelFile = ModelFile {
    name: "efficientat-mn10-as.onnx",
    url: "https://huggingface.co/Chris-Kode/efficientat-mn10-as-onnx/resolve/81f0e383972c52b354de924778fec25b2a4a29d3/efficientat-mn10-as.onnx",
    env: "KOOEDITOR_TAGGER_MODEL",
    what: "sound tagger",
};

/// Samples the tagger hears at once, and between the starts of windows.
pub const WINDOW: usize = 2 * SOUND_RATE as usize;
pub const STEP: usize = SOUND_RATE as usize;
/// What the model is given: a window and then silence.
const INPUT: usize = 6 * SOUND_RATE as usize;
/// Windows run at once.
pub const BATCH: usize = 32;
const CLASSES: usize = 527;

/// A sound starts at this probability and lasts while it stays above
/// `SOUND_OFF`. Speech alone keeps every other class below 0.05.
const SOUND_ON: f32 = 0.25;
const SOUND_OFF: f32 = 0.12;
/// Music needs more, and longer: silence alone scores 0.25 as music, and a
/// chord struck once is not music.
const MUSIC_ON: f32 = 0.5;
const MUSIC_OFF: f32 = 0.3;
const MUSIC_MIN: Time = Time(FLICKS_PER_SECOND * 3);
/// Runs of one label closer than this are one sound (in windows).
const SOUND_GAP: usize = 1;
const MUSIC_GAP: usize = 2;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Sound,
    /// A sound that also plays inside music (bells, claps); dropped where
    /// it does.
    Musical,
    Music,
    Genre,
    Mood,
    Vocals,
    Instrument,
}

/// A name in the sound script and the AudioSet classes (indices into the
/// model's 527) it stands for. Classes left out are speech (the
/// transcript has it), too general to help an editor ("Animal",
/// "Vehicle"), or describe the room rather than a sound.
struct Group {
    label: &'static str,
    kind: Kind,
    classes: &'static [u16],
}

const fn g(label: &'static str, kind: Kind, classes: &'static [u16]) -> Group {
    Group {
        label,
        kind,
        classes,
    }
}

const fn sound(label: &'static str, classes: &'static [u16]) -> Group {
    g(label, Kind::Sound, classes)
}

const fn musical(label: &'static str, classes: &'static [u16]) -> Group {
    g(label, Kind::Musical, classes)
}

const fn genre(label: &'static str, classes: &'static [u16]) -> Group {
    g(label, Kind::Genre, classes)
}

const fn mood(label: &'static str, classes: &'static [u16]) -> Group {
    g(label, Kind::Mood, classes)
}

const fn instrument(label: &'static str, classes: &'static [u16]) -> Group {
    g(label, Kind::Instrument, classes)
}

const GROUPS: &[Group] = &[
    // People
    sound("shouting", &[8, 9, 11, 12, 13]),
    sound("whoop", &[10]),
    sound("screaming", &[14]),
    sound("laughter", &[16, 17, 18, 19, 20, 21]),
    sound("crying", &[22, 23, 24, 25]),
    sound("sigh", &[26]),
    musical("humming", &[37]),
    sound("groan", &[38, 39]),
    musical("whistling", &[40]),
    sound("snoring", &[43]),
    sound("gasp", &[44, 45]),
    sound("cough", &[47, 48]),
    sound("sneeze", &[49]),
    sound("sniff", &[46, 50]),
    sound("footsteps", &[51, 52, 53]),
    sound("chewing", &[54, 55]),
    sound("burp", &[58]),
    sound("hiccup", &[59]),
    sound("fart", &[60]),
    musical("finger snap", &[62]),
    musical("clapping", &[63]),
    sound("heartbeat", &[64, 65]),
    sound("cheering", &[66]),
    sound("applause", &[67]),
    sound("crowd", &[68, 69, 70]),
    sound("children playing", &[71]),
    // Animals
    sound("dog barking", &[74, 75, 76, 78]),
    sound("dog howling", &[77]),
    sound("growling", &[79]),
    sound("dog whimpering", &[80]),
    sound("cat meowing", &[81, 83, 85]),
    sound("purring", &[82]),
    sound("hissing", &[84]),
    sound("horse", &[87, 88, 89]),
    sound("cow", &[90, 91]),
    sound("pig", &[93, 94]),
    sound("goat", &[95, 96]),
    sound("sheep", &[97]),
    sound("rooster", &[99, 100, 101]),
    sound("turkey", &[102, 103]),
    sound("duck", &[104, 105]),
    sound("goose", &[106, 107]),
    sound("roar", &[109, 110]),
    sound("birdsong", &[111, 112, 113]),
    sound("squawk", &[114]),
    sound("pigeon", &[115, 116]),
    sound("crow", &[117, 118]),
    sound("owl", &[119, 120]),
    sound("wings flapping", &[121]),
    sound("insects", &[126, 127]),
    sound("mosquito", &[128]),
    sound("buzzing", &[129, 130, 131]),
    sound("frog", &[132, 133]),
    sound("whale", &[136]),
    // Bells and chimes
    musical("bell", &[200, 201, 205]),
    musical("jingle bells", &[202]),
    sound("bicycle bell", &[203]),
    musical("wind chime", &[206]),
    // Nature
    sound("wind", &[283, 284]),
    sound("wind noise", &[285]),
    sound("thunder", &[286, 287]),
    sound("water", &[288]),
    sound("rain", &[289, 290, 291]),
    sound("stream", &[292]),
    sound("waterfall", &[293]),
    sound("waves", &[294, 295]),
    sound("steam", &[296]),
    sound("gurgling", &[297]),
    sound("fire", &[298, 299]),
    // Vehicles and engines
    sound("boat", &[301, 302, 303, 304, 305]),
    sound("car", &[307, 314]),
    sound("car horn", &[308, 309, 318]),
    sound("car alarm", &[310]),
    sound("tire squeal", &[312, 313]),
    sound("race car", &[315]),
    sound("truck", &[316, 317]),
    sound("reversing beeps", &[319]),
    sound("bus", &[321]),
    sound("siren", &[322, 323, 324, 325, 396, 397]),
    sound("motorcycle", &[326]),
    sound("traffic", &[327]),
    sound("train", &[328, 329, 332, 333, 334]),
    sound("train horn", &[330, 331]),
    sound("aircraft", &[335, 336, 337, 338, 340]),
    sound("helicopter", &[339]),
    sound("bicycle", &[341]),
    sound("skateboard", &[342]),
    sound("engine", &[343, 344, 348, 349, 350, 352]),
    sound("engine starting", &[351]),
    sound("engine revving", &[353]),
    sound("dental drill", &[345]),
    sound("lawn mower", &[346]),
    sound("chainsaw", &[347]),
    // Home
    sound("door", &[354, 357, 362]),
    sound("doorbell", &[355, 356]),
    sound("slam", &[358]),
    sound("knock", &[359]),
    sound("tap", &[360]),
    sound("squeak", &[361]),
    sound("drawer", &[363]),
    sound("dishes", &[364, 365]),
    sound("chopping", &[366]),
    sound("frying", &[367, 490]),
    sound("microwave", &[368]),
    sound("blender", &[369]),
    sound("running water", &[370, 371, 372]),
    sound("hair dryer", &[373]),
    sound("toilet flush", &[374]),
    sound("toothbrush", &[375, 376]),
    sound("vacuum cleaner", &[377]),
    sound("zipper", &[378]),
    sound("keys jangling", &[379]),
    sound("coin", &[380]),
    sound("scissors", &[381]),
    sound("electric shaver", &[382]),
    sound("shuffling cards", &[383]),
    sound("typing", &[384, 385, 386]),
    sound("writing", &[387]),
    // Signals and machines
    sound("alarm", &[388, 395, 399, 400]),
    sound("phone ringing", &[389, 390, 391]),
    sound("dial tone", &[392, 393, 394]),
    sound("buzzer", &[398]),
    sound("foghorn", &[401]),
    sound("whistle", &[402, 403]),
    sound("ticking clock", &[406, 407, 408]),
    sound("sewing machine", &[411]),
    sound("fan", &[412]),
    sound("air conditioning", &[413]),
    sound("cash register", &[414]),
    sound("printer", &[415]),
    sound("camera shutter", &[416, 417]),
    sound("hammering", &[419]),
    sound("jackhammer", &[420]),
    sound("sawing", &[421]),
    sound("sanding", &[422, 423]),
    sound("power tool", &[424, 425]),
    // Impacts and the like
    sound("explosion", &[426, 435, 436]),
    sound("gunshot", &[427, 428, 429, 430, 431]),
    sound("fireworks", &[432, 433]),
    sound("popping", &[434]),
    sound("crack", &[438, 439, 440]),
    sound("glass", &[441]),
    sound("glass clinking", &[442]),
    sound("glass breaking", &[443]),
    sound("splash", &[445, 446]),
    sound("dripping", &[448]),
    sound("pouring", &[449, 450, 451, 452]),
    sound("spray", &[453]),
    sound("boiling", &[456]),
    sound("whoosh", &[459]),
    sound("thud", &[460, 461]),
    sound("bouncing", &[465, 471]),
    sound("bang", &[466]),
    sound("slap", &[467, 468]),
    sound("crash", &[469]),
    sound("breaking", &[470]),
    sound("whip", &[472]),
    sound("scratching", &[474, 475]),
    sound("crumpling", &[478, 479]),
    sound("tearing", &[480]),
    musical("beep", &[481, 482, 483]),
    sound("clang", &[484]),
    sound("squeal", &[485]),
    sound("creak", &[486]),
    sound("rustling", &[487]),
    sound("whir", &[488]),
    sound("clatter", &[489]),
    sound("clicking", &[491, 492]),
    sound("rumble", &[493]),
    musical("jingle", &[495]),
    sound("hum", &[496]),
    sound("boing", &[498]),
    sound("crunch", &[499]),
    musical("tone", &[501]),
    sound("mains hum", &[516]),
    sound("static", &[515, 520, 521]),
    sound("distortion", &[517]),
    sound("television", &[524]),
    sound("radio", &[525]),
    // Music, and what it sounds like
    g("music", Kind::Music, &[137, 267, 268, 269, 270]),
    genre("pop", &[216]),
    genre("hip hop", &[36, 217]),
    genre("rock", &[219, 220, 221, 222, 223, 224, 225]),
    genre("r&b", &[226, 227]),
    genre("reggae", &[228, 263]),
    genre("country", &[229, 231]),
    genre("swing", &[230]),
    genre("funk", &[232]),
    genre("folk", &[233, 264]),
    genre("middle eastern", &[234]),
    genre("jazz", &[235]),
    genre("disco", &[236]),
    genre("classical", &[184, 237]),
    genre("opera", &[238]),
    genre("electronic", &[239, 240, 241, 242, 243, 244, 245, 247]),
    genre("ambient", &[246, 253]),
    genre("latin", &[248, 249, 250]),
    genre("blues", &[251]),
    genre("children's", &[252, 271]),
    genre("a cappella", &[255]),
    genre("african", &[256, 257]),
    genre("gospel", &[258, 259]),
    genre("asian", &[260, 261, 262]),
    genre("video game", &[272]),
    genre("christmas", &[273]),
    genre("dance", &[274]),
    mood("upbeat", &[276]),
    mood("funny", &[277]),
    mood("sad", &[278]),
    mood("tender", &[279]),
    mood("exciting", &[280]),
    mood("angry", &[281]),
    mood("scary", &[282]),
    g("vocals", Kind::Vocals, &[27, 28, 32, 33, 34, 35, 254]),
    instrument("piano", &[153, 154]),
    instrument("guitar", &[140, 141, 142, 143, 144, 145, 146]),
    instrument("drums", &[162, 163, 164, 165]),
    instrument("synth", &[158]),
    instrument("strings", &[189, 190, 191, 193]),
    instrument("brass", &[185, 186, 187, 188]),
    instrument("organ", &[155, 156, 157]),
    instrument("saxophone", &[197]),
    instrument("flute", &[196]),
];

/// The tagger's scores for one window: the highest probability among
/// each group's classes, in [`GROUPS`] order.
pub type Scores = Vec<f32>;

pub struct Tagger {
    session: Session,
}

impl Tagger {
    pub fn load(model: &std::path::Path) -> Result<Self> {
        Ok(Self {
            session: crate::onnx::session(model, MODEL.what)?,
        })
    }

    /// Scores for `windows` of up to [`WINDOW`] samples.
    pub fn run(&mut self, windows: &[Vec<f32>]) -> Result<Vec<Scores>> {
        let n = windows.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut flat = vec![0.0; n * INPUT];
        for (w, slot) in windows.iter().zip(flat.chunks_mut(INPUT)) {
            let w = &w[..w.len().min(WINDOW)];
            slot[..w.len()].copy_from_slice(w);
        }
        let input = Tensor::from_array(([n, INPUT], flat)).map_err(|e| anyhow!("{e}"))?;
        let outputs = self
            .session
            .run(ort::inputs!["samples" => input])
            .map_err(|e| anyhow!("sound tagger failed: {e}"))?;
        let (_, probs) = outputs["probabilities"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        if probs.len() != n * CLASSES {
            bail!("sound tagger gave {} numbers for {n} windows", probs.len());
        }
        Ok(probs
            .chunks(CLASSES)
            .map(|p| {
                GROUPS
                    .iter()
                    .map(|g| g.classes.iter().map(|&c| p[c as usize]).fold(0.0, f32::max))
                    .collect()
            })
            .collect())
    }
}

/// Source time where window `k` starts.
fn start(k: usize) -> Time {
    Time::from_secs_f64((k * STEP) as f64 / SOUND_RATE as f64)
}

const WINDOW_TIME: Time = Time(FLICKS_PER_SECOND * 2);
const STEP_TIME: Time = Time(FLICKS_PER_SECOND);

/// Runs of windows where `score` rises to `on` and stays above `off`,
/// joined across gaps of up to `gap` windows: (first, last, peak).
fn runs(
    scores: &[Scores],
    group: usize,
    on: f32,
    off: f32,
    gap: usize,
) -> Vec<(usize, usize, f32)> {
    let mut out: Vec<(usize, usize, f32)> = Vec::new();
    let mut open: Option<(usize, f32)> = None;
    let mut push = |first: usize, last: usize, peak: f32| {
        if peak < on {
            return;
        }
        match out.last_mut() {
            Some(prev) if first <= prev.1 + gap + 1 => {
                prev.1 = last;
                prev.2 = prev.2.max(peak);
            }
            _ => out.push((first, last, peak)),
        }
    };
    for (k, s) in scores.iter().enumerate() {
        let p = s[group];
        match open {
            Some((first, peak)) if p >= off => open = Some((first, peak.max(p))),
            Some((first, peak)) => {
                push(first, k - 1, peak);
                open = (p >= off).then_some((k, p));
            }
            None if p >= off => open = Some((k, p)),
            None => {}
        }
    }
    if let Some((first, peak)) = open {
        push(first, scores.len() - 1, peak);
    }
    out
}

/// Where a run of windows `first..=last` puts its sound: from half a step
/// before the end of the first window to half a step into the last,
/// reaching the edges of the audio where the run does.
fn heard(first: usize, last: usize, windows: usize, end: Time) -> TimeRange {
    let half = Time(STEP_TIME.0 / 2);
    let a = if first == 0 {
        Time::ZERO
    } else {
        start(first) + WINDOW_TIME - half
    };
    let b = if last + 1 == windows {
        end
    } else {
        start(last) + half
    };
    if b > a {
        return TimeRange::new(a.min(end), b.min(end));
    }
    // Shorter than the windows can tell: about the middle of the overlap.
    let mid = Time((a.0 + b.0) / 2);
    TimeRange::new(
        (mid - Time(STEP_TIME.0 / 2)).max(Time::ZERO).min(end),
        (mid + Time(STEP_TIME.0 / 2)).min(end),
    )
}

/// The sounds and music in `scores` (one entry per window, windows
/// [`STEP`] apart) of audio `end` long; short sounds are moved onto the
/// loudest stretch of `levels` around them.
pub fn read(scores: &[Scores], end: Time, levels: Option<&Levels>) -> (Vec<Sound>, Vec<Music>) {
    let n = scores.len();
    let music_group = GROUPS
        .iter()
        .position(|g| g.kind == Kind::Music)
        .expect("music group");
    let music: Vec<Music> = runs(scores, music_group, MUSIC_ON, MUSIC_OFF, MUSIC_GAP)
        .into_iter()
        .map(|(first, last, _)| (first, last, heard(first, last, n, end)))
        .filter(|(_, _, range)| range.duration() >= MUSIC_MIN)
        .map(|(first, last, range)| Music {
            range,
            tags: tags(&scores[first..=last]),
            beats: Vec::new(),
            downbeats: Vec::new(),
        })
        .collect();

    let mut sounds = Vec::new();
    for (i, group) in GROUPS.iter().enumerate() {
        if !matches!(group.kind, Kind::Sound | Kind::Musical) {
            continue;
        }
        let mut found: Vec<Sound> = Vec::new();
        for (first, last, peak) in runs(scores, i, SOUND_ON, SOUND_OFF, SOUND_GAP) {
            let mut range = heard(first, last, n, end);
            if range.duration() <= STEP_TIME
                && let Some(levels) = levels
            {
                let reach = TimeRange::new(start(first), (start(last) + WINDOW_TIME).min(end));
                range = loudest(levels, range, reach).unwrap_or(range);
            }
            let in_music = |r: &TimeRange| {
                music.iter().any(|m| {
                    m.range
                        .intersect(r)
                        .is_some_and(|x| x.duration().0 * 2 > r.duration().0)
                })
            };
            if group.kind == Kind::Musical && in_music(&range) {
                continue;
            }
            match found.last_mut() {
                Some(prev) if range.start - prev.range.end <= STEP_TIME => {
                    prev.range.end = prev.range.end.max(range.end);
                    prev.confidence = prev.confidence.max(peak);
                }
                _ => found.push(Sound {
                    range,
                    label: group.label.to_string(),
                    confidence: peak,
                }),
            }
        }
        sounds.extend(found);
    }
    sounds.sort_by_key(|s| (s.range.start, s.range.end));
    (sounds, music)
}

/// The loudest stretch of `levels` around `near`, within `reach`: from
/// its peak out to where the level falls 20 dB below it (and 6 dB above
/// the floor around it), bridging dips of up to 50 ms.
fn loudest(levels: &Levels, near: TimeRange, reach: TimeRange) -> Option<TimeRange> {
    let frame = |t: Time| (t.0.max(0) / Levels::FRAME.0) as usize;
    let len = levels.len();
    let lo = frame(reach.start.max(near.start - STEP_TIME)).min(len);
    let hi = frame(reach.end.min(near.end + STEP_TIME)).min(len);
    if hi <= lo {
        return None;
    }
    let peak = (lo..hi).max_by(|&a, &b| levels.db(a).total_cmp(&levels.db(b)))?;
    let around = frame(near.start).saturating_sub(300)..(frame(near.end) + 300).min(len);
    let mut db: Vec<f32> = around.map(|i| levels.db(i)).collect();
    if db.is_empty() {
        return None;
    }
    let k = db.len() / 10;
    let floor = *db.select_nth_unstable_by(k, f32::total_cmp).1;
    let threshold = (levels.db(peak) - 20.0).max(floor + 6.0);
    if levels.db(peak) < threshold {
        return None;
    }
    let edge = |step: isize| {
        let (mut at, mut quiet) = (peak as isize, 0);
        loop {
            let next = at + step;
            if next < lo as isize || next >= hi as isize {
                return at;
            }
            if levels.db(next as usize) >= threshold {
                quiet = 0;
            } else {
                quiet += 1;
                if quiet > 5 {
                    return at;
                }
            }
            at = next;
        }
    };
    let (a, b) = (edge(-1) as usize, edge(1) as usize);
    let at = |i: usize| Time(i as i64 * Levels::FRAME.0);
    // Back off a dip the walk ended on.
    let (mut a, mut b) = (a, b);
    while a < peak && levels.db(a) < threshold {
        a += 1;
    }
    while b > peak && levels.db(b) < threshold {
        b -= 1;
    }
    Some(TimeRange::new(at(a), at(b + 1)))
}

/// What a stretch of music sounds like: its likeliest genres (two at
/// most), mood, vocals and lead instruments (two at most).
fn tags(scores: &[Scores]) -> Vec<String> {
    let mean = |i: usize| scores.iter().map(|s| s[i]).sum::<f32>() / scores.len().max(1) as f32;
    let pick = |kind: Kind, min: f32, most: usize| {
        let mut found: Vec<(f32, &'static str)> = GROUPS
            .iter()
            .enumerate()
            .filter(|(_, g)| g.kind == kind)
            .map(|(i, g)| (mean(i), g.label))
            .filter(|(m, _)| *m >= min)
            .collect();
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        found.truncate(most);
        found.into_iter().map(|(_, l)| l.to_string())
    };
    pick(Kind::Genre, 0.15, 2)
        .chain(pick(Kind::Mood, 0.1, 1))
        .chain(pick(Kind::Vocals, 0.25, 1))
        .chain(pick(Kind::Instrument, 0.25, 2))
        .collect()
}
