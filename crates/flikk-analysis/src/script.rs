//! A script timed in a recording of it: the words of a take a provider
//! read, found the way a voiceover's are (the aligner follows the script,
//! or the edges are measured on the audio levels without it), so a few of
//! them can be cut out of it.

use crate::{Shared, align, ffmpeg, follow_script, vad};
use flikk_core::analysis::Word;
use flikk_core::cloud;
use flikk_core::services::{ServiceError, ServiceResult};
use flikk_core::time::{Time, TimeRange};
use std::path::Path;

/// `script`'s words with where each is said in the file at `path`.
pub(crate) fn time(shared: &Shared, path: &Path, script: &str) -> ServiceResult<Vec<Word>> {
    let audio = ffmpeg::extract_audio(path, 0, None, &|_| {})
        .map_err(|e| ServiceError(format!("{e:#}")))?;
    if audio.is_empty() {
        return Err(ServiceError("The take has no sound.".into()));
    }
    let (levels, sound) = align::measure(&audio);
    // A take is clean speech: the energy VAD hears it well.
    let speech = vad::speech_ranges(&audio);
    let end = Time::from_secs_f64(audio.len() as f64 / ffmpeg::SAMPLE_RATE as f64);
    let mut words: Vec<Word> = cloud::spoken_words(script)
        .into_iter()
        .map(|text| Word {
            range: TimeRange::new(Time::ZERO, Time::ZERO),
            text,
            confidence: 1.0,
            speaker: None,
        })
        .collect();
    align::spread(&mut words, &speech, Time::ZERO, end);
    match follow_script(shared, &audio, &speech, &sound, &mut words, &|_| {}) {
        Ok(true) => {}
        _ => align::refine(&mut words, &levels, &sound),
    }
    Ok(words)
}
