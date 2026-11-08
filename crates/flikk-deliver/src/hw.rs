//! Hardware video encoders: which ones the installed FFmpeg can really use
//! (listed by `ffmpeg -encoders`, then confirmed with a tiny trial encode),
//! and the arguments that keep their output comparable to libx264/libx265
//! at the preset's CRF.
//!
//! The probe runs once per session on a background thread ([`warm_up`]);
//! the render worker waits for it ([`probe_until`]), the UI only peeks
//! ([`probed`]).

use flikk_os::process::{ChildGuard, command, ffmpeg};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::Read as _;
use std::process::Stdio;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Which encoder a render uses for H.264 and HEVC; the other codecs are
/// always software.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoder {
    /// Hardware when the machine has one, else software; a hardware
    /// encoder that fails falls back to software.
    #[default]
    Auto,
    /// Hardware only; fails when there is none.
    Hardware,
    /// libx264 / libx265.
    Software,
}

impl Encoder {
    pub const ALL: [Encoder; 3] = [Encoder::Auto, Encoder::Hardware, Encoder::Software];

    pub fn label(self) -> &'static str {
        match self {
            Encoder::Auto => "Auto",
            Encoder::Hardware => "Hardware",
            Encoder::Software => "Software",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Encoder::Auto => "auto",
            Encoder::Hardware => "hardware",
            Encoder::Software => "software",
        }
    }

    /// `auto`, `hardware`/`hw`/`gpu`, `software`/`sw`/`cpu`, any case.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Encoder::Auto),
            "hardware" | "hw" | "gpu" => Some(Encoder::Hardware),
            "software" | "sw" | "cpu" => Some(Encoder::Software),
            _ => None,
        }
    }
}

/// The video codecs hardware can encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

/// A hardware encoder family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    VideoToolbox,
    Nvenc,
    Qsv,
    Amf,
    Vaapi,
}

impl Api {
    /// In order of preference on this platform.
    fn candidates() -> &'static [Api] {
        if cfg!(target_os = "macos") {
            &[Api::VideoToolbox]
        } else if cfg!(windows) {
            &[Api::Nvenc, Api::Qsv, Api::Amf]
        } else {
            &[Api::Nvenc, Api::Qsv, Api::Vaapi, Api::Amf]
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Api::VideoToolbox => "VideoToolbox",
            Api::Nvenc => "NVENC",
            Api::Qsv => "Quick Sync",
            Api::Amf => "AMF",
            Api::Vaapi => "VA-API",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Api::VideoToolbox => "videotoolbox",
            Api::Nvenc => "nvenc",
            Api::Qsv => "qsv",
            Api::Amf => "amf",
            Api::Vaapi => "vaapi",
        }
    }
}

/// One usable hardware encoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwEncoder {
    pub api: Api,
    pub codec: Codec,
    /// VA-API render node.
    device: Option<String>,
}

impl HwEncoder {
    /// FFmpeg's name, e.g. `h264_nvenc`.
    pub fn name(&self) -> String {
        let codec = match self.codec {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
        };
        format!("{codec}_{}", self.api.suffix())
    }

    /// Arguments before the inputs (hardware device for VA-API).
    pub fn global_args(&self) -> Vec<OsString> {
        match (&self.api, &self.device) {
            (Api::Vaapi, Some(device)) => [
                "-init_hw_device".to_string(),
                format!("vaapi=flikk:{device}"),
                "-filter_hw_device".into(),
                "flikk".into(),
            ]
            .map(OsString::from)
            .to_vec(),
            _ => Vec::new(),
        }
    }

    /// Filters appended to the video chain before the encoder: VA-API
    /// takes frames in GPU memory. The conversion to 4:2:0 stays in
    /// swscale, as for the software encoders, so the matrix (and the
    /// absent colour tags) match.
    pub fn upload_filter(&self) -> Option<&'static str> {
        (self.api == Api::Vaapi).then_some("format=nv12,hwupload")
    }

    /// Video encoder arguments for `crf` (libx264's scale for H.264,
    /// libx265's for HEVC) at `width`×`height`, `fps`.
    pub fn codec_args(&self, crf: u8, width: u32, height: u32, fps: f64) -> Vec<String> {
        // Hardware encoders spend fewer bits per quantizer step than x264's
        // CRF with psy tuning; one step lower keeps the look comparable.
        let q = crf.saturating_sub(1).clamp(1, 51).to_string();
        let mut args: Vec<String> = vec!["-c:v".into(), self.name()];
        let mut push = |a: &[&str]| args.extend(a.iter().map(|s| s.to_string()));
        match self.api {
            Api::Nvenc => {
                push(&["-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", &q]);
                push(&["-b:v", "0", "-spatial-aq", "1", "-pix_fmt", "yuv420p"]);
            }
            Api::Qsv => {
                push(&[
                    "-preset",
                    "medium",
                    "-global_quality",
                    &q,
                    "-pix_fmt",
                    "nv12",
                ]);
            }
            Api::Amf => {
                push(&["-quality", "quality", "-rc", "cqp"]);
                push(&["-qp_i", &q, "-qp_p", &q, "-pix_fmt", "nv12"]);
            }
            Api::Vaapi => push(&["-rc_mode", "CQP", "-qp", &q]),
            Api::VideoToolbox => {
                // Constant quality is Apple silicon only; an average bitrate
                // works on every Mac.
                let kbps = bitrate_kbps(self.codec, crf, width, height, fps).to_string();
                push(&["-b:v", &format!("{kbps}k"), "-pix_fmt", "yuv420p"]);
                push(&["-allow_sw", "0", "-realtime", "0"]);
            }
        }
        if self.codec == Codec::Hevc {
            push(&["-tag:v", "hvc1"]);
        }
        args
    }
}

/// The average bitrate x264/x265 land near at `crf` for ordinary footage:
/// 0.13 bits per pixel for H.264 at CRF 18, halving every 6 steps, HEVC
/// 40 % less (its CRF scale sits ~4 steps above x264's).
fn bitrate_kbps(codec: Codec, crf: u8, width: u32, height: u32, fps: f64) -> u64 {
    let (base, at) = match codec {
        Codec::H264 => (0.13, 18.0),
        Codec::Hevc => (0.078, 22.0),
    };
    let bpp = base * 2f64.powf((at - crf as f64) / 6.0);
    let bits = bpp * width as f64 * height as f64 * fps.max(1.0);
    ((bits / 1000.0) as u64).max(500)
}

/// What the probe found.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub encoders: Vec<HwEncoder>,
}

impl Probe {
    /// The preferred usable encoder for `codec`.
    pub fn best(&self, codec: Codec) -> Option<&HwEncoder> {
        self.encoders.iter().find(|e| e.codec == codec)
    }
}

static PROBE: OnceLock<Probe> = OnceLock::new();
/// Set once [`warm_up`] has started the probe thread.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Start probing on a background thread, if not started already. False
/// when no thread could be started.
pub fn warm_up() -> bool {
    if PROBE.get().is_some() || STARTED.swap(true, Ordering::AcqRel) {
        return true;
    }
    let spawned = std::thread::Builder::new()
        .name("flikk-hw-probe".into())
        .spawn(|| {
            // A probe that panics finds nothing, so waiters never hang.
            if std::panic::catch_unwind(probe).is_err() {
                let _ = PROBE.set(Probe::default());
            }
        })
        .is_ok();
    if !spawned {
        STARTED.store(false, Ordering::Release);
    }
    spawned
}

/// The probe's result, without waiting: `None` until it is done (not
/// started, or still probing).
pub fn probed() -> Option<&'static Probe> {
    PROBE.get()
}

/// The probe's result, probing now if needed. Blocks: background threads
/// only.
pub fn probe() -> &'static Probe {
    PROBE.get_or_init(run_probe)
}

/// [`probe`], probing on the background thread so the wait can stop:
/// `None` once `cancel` is set.
pub fn probe_until(cancel: &AtomicBool) -> Option<&'static Probe> {
    if !warm_up() {
        return Some(probe());
    }
    loop {
        if let Some(found) = PROBE.get() {
            return Some(found);
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_probe() -> Probe {
    let listed = listed_encoders();
    let device = render_node();
    let candidates: Vec<HwEncoder> = Api::candidates()
        .iter()
        .flat_map(|&api| {
            [Codec::H264, Codec::Hevc].map(|codec| HwEncoder {
                api,
                codec,
                device: (api == Api::Vaapi).then(|| device.clone()).flatten(),
            })
        })
        .filter(|e| listed.iter().any(|n| *n == e.name()))
        .filter(|e| e.api != Api::Vaapi || e.device.is_some())
        .collect();
    // Trials in parallel: the probe costs one trial, not one per encoder.
    let trials: Vec<_> = candidates
        .into_iter()
        .map(|e| std::thread::spawn(move || trial(&e).then_some(e)))
        .collect();
    Probe {
        encoders: trials
            .into_iter()
            .filter_map(|t| t.join().ok().flatten())
            .collect(),
    }
}

/// Encoder names from `ffmpeg -encoders`.
fn listed_encoders() -> Vec<String> {
    let Ok(out) = command(ffmpeg())
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let flags = it.next()?;
            (flags.starts_with('V') && flags.len() == 6).then(|| it.next().map(str::to_owned))?
        })
        .collect()
}

/// The first DRM render node (Linux).
fn render_node() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("renderD"))
        .collect();
    nodes.sort();
    nodes.first().map(|n| format!("/dev/dri/{n}"))
}

/// Encode a few frames of a 256×256 colour source with the render's own
/// arguments; whether it worked within a few seconds.
fn trial(encoder: &HwEncoder) -> bool {
    let (w, h, fps) = (256, 256, 30.0);
    let mut chain = format!("color=c=gray:s={w}x{h}:r=30,trim=end_frame=5,format=rgba");
    if let Some(upload) = encoder.upload_filter() {
        chain = format!("{chain},{upload}");
    }
    let mut args: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error"]
        .map(OsString::from)
        .to_vec();
    args.extend(encoder.global_args());
    args.extend(["-filter_complex", &format!("{chain}[v]"), "-map", "[v]"].map(OsString::from));
    args.extend(
        encoder
            .codec_args(18, w, h, fps)
            .into_iter()
            .map(OsString::from),
    );
    args.extend(["-f", "null", "-"].map(OsString::from));
    let Ok(mut child) = ChildGuard::spawn(
        command(ffmpeg())
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped()),
    ) else {
        return false;
    };
    // Drain stderr so a chatty failure never blocks the child.
    let mut stderr = child.stderr.take();
    let drain = std::thread::spawn(move || {
        let mut sink = Vec::new();
        if let Some(s) = &mut stderr {
            let _ = s.read_to_end(&mut sink);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => break false,
        }
    };
    // Killed (when still running) before the drain is joined: its pipe
    // closes only once the child is gone.
    drop(child);
    let _ = drain.join();
    ok
}
