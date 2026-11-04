//! whisper.cpp with its stock decoding (beam 5, temperature fallback,
//! entropy/logprob/no-speech thresholds at their defaults), one line per file:
//! `<file>\t<language>\t<text>`. usage: wcli model.bin lang|auto file.f32...
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    whisper_rs::install_logging_hooks();
    let ctx = WhisperContext::new_with_params(&args[1], WhisperContextParameters::default()).unwrap();
    let mut state = ctx.create_state().unwrap();
    for f in &args[3..] {
        let bytes = std::fs::read(f).unwrap();
        let audio: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        let mut p = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 });
        p.set_language(Some(&args[2]));
        p.set_n_threads(4);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        p.set_print_special(false);
        state.full(p, &audio).unwrap();
        let text: String = state.as_iter().map(|s| s.to_str_lossy().unwrap_or_default().into_owned()).collect();
        let lang = whisper_rs::get_lang_str(state.full_lang_id_from_state()).unwrap_or("?");
        println!("{}\t{}\t{}", f, lang, text.replace(['\t', '\n'], " "));
    }
}
