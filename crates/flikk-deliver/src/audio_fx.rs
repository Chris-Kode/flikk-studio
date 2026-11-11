//! Audio effects ([`AudioFx`]) as FFmpeg filters, tuned to match the
//! viewer's real-time processors in `flikk_core::audiofx::dsp`:
//!
//! | Effect | Filters | Match |
//! | --- | --- | --- |
//! | Equalizer | `lowshelf`, `equalizer`, `highshelf` (slope 1, Q) | same biquads |
//! | High/Low pass | 2-pole `highpass`/`lowpass`, Q 0.707 | same biquads |
//! | Compressor | `acompressor`, RMS, 9 dB knee | the preview is a port |
//! | Gate | `agate`, ratio 10, -60 dB range | the preview is a port |
//! | Limiter | `alimiter`, 5 ms look-ahead, compensated | both brick wall at the ceiling |
//! | De-esser | `highpass` band, `sidechaincompress`d by itself, summed back as `x − hp + g·hp` | same structure and curve |
//! | Echo | `aecho` taps from the feedback line | same down to -60 dB |
//! | Reverb | `afir` with the Freeverb impulse response | exact (Freeverb is linear) |
//! | Noise reduction | spectral `afftdn`, made into a copy of the source | both read the copy (`flikk_core::cleanup`) |

use crate::ffmpeg::escape_path;
use crate::keyframes::num;
use anyhow::{Context as _, Result};
use flikk_core::audiofx::dsp::{self, PASS_Q, SHELF_SLOPE, db};
use flikk_core::audiofx::{AudioFx, AudioFxKind};
use flikk_os::process::{command, ffmpeg};
use std::path::Path;
use std::sync::OnceLock;

/// Where a graph builder takes labels and chains.
pub(crate) trait Sink {
    fn label(&mut self, prefix: &str) -> String;
    fn push(&mut self, chain: String);
}

/// Apply `effects` in order to stream `input` at `sr` Hz; returns the
/// output label (`input` itself when nothing applies). Side files (reverb
/// impulse responses) go to `work_dir`.
pub(crate) fn apply(
    g: &mut dyn Sink,
    input: String,
    effects: &[AudioFx],
    sr: u32,
    work_dir: &Path,
) -> Result<String> {
    let mut cur = input;
    let mut pending: Vec<String> = Vec::new();
    for fx in effects.iter().filter(|fx| fx.is_active()) {
        if let Some(filters) = linear(fx, sr) {
            pending.extend(filters);
            continue;
        }
        cur = flush(g, cur, &mut pending);
        cur = match fx.kind {
            AudioFxKind::DeEsser => de_esser(g, cur, fx, sr),
            _ => reverb(g, cur, fx, sr, work_dir)?,
        };
    }
    Ok(flush(g, cur, &mut pending))
}

fn flush(g: &mut dyn Sink, cur: String, pending: &mut Vec<String>) -> String {
    if pending.is_empty() {
        return cur;
    }
    let out = g.label("afx");
    g.push(format!("[{cur}]{}[{out}]", pending.join(",")));
    pending.clear();
    out
}

fn freq(v: f64, sr: u32) -> String {
    num(v.clamp(10.0, dsp::max_freq(sr as f64)).round())
}

/// Single-chain filters; `None` for effects that need a sub-graph.
fn linear(fx: &AudioFx, sr: u32) -> Option<Vec<String>> {
    let v = |name: &str| fx.value(name);
    Some(match fx.kind {
        AudioFxKind::Equalizer => {
            let mut f = Vec::new();
            for (filter, freq_name, gain_name) in [
                ("lowshelf", "low_freq", "low_gain_db"),
                ("equalizer", "mid_freq", "mid_gain_db"),
                ("highshelf", "high_freq", "high_gain_db"),
            ] {
                let gain = v(gain_name);
                if gain.abs() < 1e-3 {
                    continue;
                }
                let width = if filter == "equalizer" {
                    format!("t=q:w={}", num(v("mid_q")))
                } else {
                    format!("t=s:w={}", num(SHELF_SLOPE))
                };
                f.push(format!(
                    "{filter}=f={}:g={}:{width}",
                    freq(v(freq_name), sr),
                    num(gain)
                ));
            }
            f
        }
        AudioFxKind::HighPass | AudioFxKind::LowPass => {
            let name = if fx.kind == AudioFxKind::HighPass {
                "highpass"
            } else {
                "lowpass"
            };
            vec![format!(
                "{name}=f={}:p=2:t=q:w={}",
                freq(v("freq"), sr),
                num(PASS_Q)
            )]
        }
        AudioFxKind::Compressor => vec![format!(
            "acompressor=threshold={}:ratio={}:attack={}:release={}:makeup={}:knee=2.828427:detection=rms:link=average",
            num(db(v("threshold_db"))),
            num(v("ratio")),
            num(v("attack_ms")),
            num(v("release_ms")),
            num(db(v("makeup_db")))
        )],
        AudioFxKind::Limiter => vec![format!(
            "alimiter=limit={}:attack={}:release={}:level=0:latency=1",
            num(db(v("ceiling_db")).clamp(0.0625, 1.0)),
            num(dsp::LIMITER_LOOKAHEAD_MS),
            num(v("release_ms"))
        )],
        AudioFxKind::Gate => vec![format!(
            "agate=threshold={}:ratio={}:range={}:attack={}:release={}:knee=2.828427:detection=rms:link=average",
            num(db(v("threshold_db"))),
            num(dsp::GATE_RATIO),
            num(dsp::GATE_RANGE),
            num(v("attack_ms")),
            num(v("release_ms"))
        )],
        // Read from a cleaned copy of the source instead (`flikk_core::cleanup`).
        AudioFxKind::NoiseReduction => vec![],
        AudioFxKind::Echo => {
            let taps = dsp::echo_taps(v("delay_ms"), v("feedback"), v("mix"));
            let join = |f: &dyn Fn(&(f64, f64)) -> String| {
                taps.iter().map(f).collect::<Vec<_>>().join("|")
            };
            vec![format!(
                "aecho=in_gain=1:out_gain=1:delays={}:decays={}",
                join(&|t| num(t.0)),
                join(&|t| num(t.1))
            )]
        }
        AudioFxKind::DeEsser | AudioFxKind::Reverb => return None,
    })
}

/// `x − hp(x) + g·hp(x)`, `g` from compressing the band by itself.
fn de_esser(g: &mut dyn Sink, input: String, fx: &AudioFx, sr: u32) -> String {
    let (threshold, ratio, attack, release) = dsp::de_esser_settings(fx.value("amount"));
    let [x, y, ha, hb, hk, hc, out] =
        ["dsx", "dsy", "dsa", "dsb", "dsk", "dsc", "dess"].map(|p| g.label(p));
    g.push(format!("[{input}]asplit=2[{x}][{y}]"));
    g.push(format!(
        "[{y}]highpass=f={}:p=2:t=q:w={},asplit=3[{ha}][{hb}][{hk}]",
        freq(fx.value("freq"), sr),
        num(PASS_Q)
    ));
    g.push(format!(
        "[{hb}][{hk}]sidechaincompress=threshold={}:ratio={}:attack={}:release={}:makeup=1:knee=2.828427:detection=rms:link=average[{hc}]",
        num(threshold),
        num(ratio),
        num(attack),
        num(release)
    ));
    // amerge keeps the three sample-aligned (amix drops negative weights).
    g.push(format!(
        "[{x}][{ha}][{hc}]amerge=inputs=3,pan=stereo|c0=c0-c2+c4|c1=c1-c3+c5[{out}]"
    ));
    out
}

/// Dry signal plus `L + R` convolved with the Freeverb impulse response.
fn reverb(
    g: &mut dyn Sink,
    input: String,
    fx: &AudioFx,
    sr: u32,
    work_dir: &Path,
) -> Result<String> {
    let (room, damping, mix) = (fx.value("room"), fx.value("damping"), fx.value("mix"));
    let path = work_dir.join(format!(
        "reverb-{}-{}-{}-{sr}.wav",
        (room * 1000.0).round(),
        (damping * 1000.0).round(),
        (mix * 1000.0).round()
    ));
    if !path.exists() {
        let ir = dsp::reverb_impulse(room, damping, mix, sr);
        write_wav(&path, &ir, sr).context("could not write the reverb impulse response")?;
    }
    let (dry, _) = dsp::reverb_gains(mix);
    let [d, w, ws, ir, wet, out] =
        ["rvd", "rvw", "rvs", "rvi", "rvwet", "verb"].map(|p| g.label(p));
    g.push(format!("[{input}]asplit=2[{d}][{w}]"));
    // Padding lets afir flush the convolution up to the last input sample;
    // amerge stops with the dry input (amix would drop its last frame).
    g.push(format!(
        "[{w}]pan=stereo|c0=c0+c1|c1=c0+c1,apad=pad_dur=0.5[{ws}]"
    ));
    g.push(format!(
        "amovie={},aformat=sample_fmts=fltp:sample_rates={sr}:channel_layouts=stereo[{ir}]",
        escape_path(&path)
    ));
    g.push(format!(
        "[{ws}][{ir}]afir=dry=1:wet=1:{}:maxir=10[{wet}]",
        no_ir_normalization()
    ));
    g.push(format!(
        "[{d}][{wet}]amerge=inputs=2,pan=stereo|c0={dry}*c0+c2|c1={dry}*c1+c3[{out}]",
        dry = num(dry)
    ));
    Ok(out)
}

/// `afir` normalizes the impulse response by default; the option that
/// turns it off was renamed in FFmpeg 7 (`gtype` → `irnorm`).
fn no_ir_normalization() -> &'static str {
    static OPTION: OnceLock<&'static str> = OnceLock::new();
    OPTION.get_or_init(|| {
        let help = command(ffmpeg())
            .args(["-hide_banner", "-h", "filter=afir"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if help.contains("irnorm") {
            "irnorm=-1"
        } else {
            "gtype=none"
        }
    })
}

/// 32-bit float WAV, channels interleaved.
fn write_wav(path: &Path, channels: &[Vec<f32>], sr: u32) -> std::io::Result<()> {
    let n = channels.iter().map(Vec::len).min().unwrap_or(0);
    let ch = channels.len() as u32;
    let data = n as u32 * ch * 4;
    let mut b: Vec<u8> = Vec::with_capacity(44 + data as usize);
    b.extend(b"RIFF");
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(3u16.to_le_bytes()); // IEEE float
    b.extend((ch as u16).to_le_bytes());
    b.extend(sr.to_le_bytes());
    b.extend((sr * ch * 4).to_le_bytes());
    b.extend(((ch * 4) as u16).to_le_bytes());
    b.extend(32u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    for i in 0..n {
        for c in channels {
            b.extend(c[i].to_le_bytes());
        }
    }
    std::fs::write(path, b)
}
