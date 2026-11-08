//! Text for names, times and frame rates the panels show. Each clock
//! formats differently on purpose (rounded, truncated, zero-padded, with
//! tenths), and the name says which.

use flikk_core::model::MediaItem;
use flikk_core::{Rate, Time};
use std::path::Path;

/// The last part of `path`, or the whole path when it has none (`/`, `..`).
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// What the pool calls `item`: its given name, else its file name.
pub(crate) fn item_name(item: &MediaItem) -> String {
    match &item.name {
        Some(name) => name.clone(),
        None => file_name(&item.path),
    }
}

/// `M:SS`, or `H:MM:SS` past an hour, rounded to the nearest second.
/// Negative times read as `0:00`.
pub(crate) fn clock_rounded(t: Time) -> String {
    hours_minutes_seconds(t.as_secs_f64().max(0.0).round() as i64)
}

/// `M:SS`, or `H:MM:SS` past an hour, cutting off the part of a second.
/// Negative times read as `0:00`.
pub(crate) fn clock_truncated(t: Time) -> String {
    hours_minutes_seconds(t.as_secs_f64().max(0.0) as i64)
}

/// `M:SS` or `H:MM:SS` for whole seconds.
fn hours_minutes_seconds(s: i64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// [`clock_truncated`] with tenths of a second, rounded to the nearest
/// tenth: `0:01.5`, `1:02:03.4`.
pub(crate) fn clock_tenths(t: Time) -> String {
    let tenths = (t.as_secs_f64().max(0.0) * 10.0).round() as i64;
    format!(
        "{}.{}",
        clock_truncated(Time::from_secs_f64((tenths / 10) as f64)),
        tenths % 10
    )
}

/// `M:SS.s`, rounded to the nearest tenth, with minutes that keep counting
/// past an hour (`61:02.5`).
pub(crate) fn minutes_tenths(t: Time) -> String {
    let tenths = (t.as_secs_f64().max(0.) * 10.).round() as i64;
    let (m, s, f) = (tenths / 600, tenths / 10 % 60, tenths % 10);
    format!("{m}:{s:02}.{f}")
}

/// `MM:SS` with both zero-padded (`01:05`), or `H:MM:SS` past an hour,
/// cutting off the part of a second.
pub(crate) fn clock_padded(t: Time) -> String {
    let s = t.as_secs_f64().max(0.) as u64;
    match s / 3600 {
        0 => format!("{:02}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// `25`, `29.97`, `23.976`: a whole rate as is, else up to three decimals
/// without trailing zeros.
pub(crate) fn fps(rate: Rate) -> String {
    if rate.den == 1 {
        return rate.num.to_string();
    }
    let text = format!("{:.3}", rate.fps());
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A sequence rate by the name the rate list gives it (`23.976`), else
/// up to three decimals without trailing zeros.
pub(crate) fn sequence_fps(rate: Rate) -> String {
    Rate::SEQUENCE
        .iter()
        .find(|(r, _)| *r == rate)
        .map(|(_, name)| (*name).to_owned())
        .unwrap_or_else(|| {
            let fps = format!("{:.3}", rate.fps());
            fps.trim_end_matches('0').trim_end_matches('.').to_owned()
        })
}

/// `25`, `29.97`: frames per second to two decimals at most, without
/// trailing zeros (`23.976` reads `23.98`).
pub(crate) fn fps_two_places(fps: f64) -> String {
    let s = format!("{fps:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}
