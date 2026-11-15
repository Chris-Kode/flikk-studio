//! Voiceovers from Pika's own speech model and the other vendors' Pika
//! hosts ([`SPEECH`]): the request in each model's fields
//! (`crate::speech`), made as a media job, and the file it made fetched
//! next to the others. The provider's catalog lists the same ids, in the
//! same order, the default first. Each id is the operation's path after
//! `/v1/media/`. Eleven v4 also reads the account's clones ([`super::voices`]).

use super::{NAME, jobs, voices};
use crate::audio::{self, Fixed};
use crate::speech::{self, HostedSpeech};
use flikk_core::cloud::{
    CloudError, CloudResult, DeliveryMarks, GenerateRequest, GenerationTask, VoiceOps,
    VoiceoverModel,
};
use std::path::{Path, PathBuf};

/// The id of the model that reads clones.
pub const CLONES: &str = "elevenlabs/eleven-v4/text-to-speech";

pub const SPEECH: &[HostedSpeech] = &[
    HostedSpeech {
        id: CLONES,
        spec: VoiceoverModel {
            name: "ElevenLabs v4",
            script_chars: 10_000,
            marks: DeliveryMarks::AudioTags,
            tuned: true,
            voices: VoiceOps {
                clone: true,
                ..VoiceOps::NONE
            },
        },
        text: "text",
        voice: "voice",
        tuning: Some((
            "voice_settings.stability",
            "voice_settings.similarity_boost",
        )),
        // MP3 is all it makes.
        fixed: &[],
        voices: &[
            "darian", "talia", "elara", "baxter", "eldrin", "elowen", "kaelen", "lawrence",
            "alicia", "maisie", "warren", "jade", "caleb", "sawyer", "finley", "florence", "wyatt",
        ],
        clone_id: Some(voices::is_clone),
    },
    HostedSpeech {
        id: "minimax/minimax-speech-2.8-hd/text-to-speech",
        spec: VoiceoverModel {
            name: "MiniMax Speech 2.8 HD",
            // It takes 50,000; a voiceover is kept to the editor's limit.
            script_chars: 10_000,
            marks: DeliveryMarks::Interjections,
            tuned: false,
            voices: VoiceOps::NONE,
        },
        text: "text",
        voice: "voice_id",
        tuning: None,
        fixed: &[
            ("format", Fixed::Str("flac")),
            ("sample_rate", Fixed::Int(44_100)),
        ],
        voices: &[
            "Wise_Woman",
            "Friendly_Person",
            "Inspirational_girl",
            "Deep_Voice_Man",
            "Calm_Woman",
            "Casual_Guy",
            "Lively_Girl",
            "Patient_Man",
            "Young_Knight",
            "Determined_Man",
            "Lovely_Girl",
            "Decent_Boy",
            "Imposing_Manner",
            "Elegant_Man",
            "Abbess",
            "Sweet_Girl_2",
            "Exuberant_Girl",
        ],
        clone_id: None,
    },
    HostedSpeech {
        id: "pika/pika-audio/pika-speech",
        spec: VoiceoverModel {
            name: "Pika Speech",
            // It takes 15,000; a voiceover is kept to the editor's limit.
            script_chars: 10_000,
            marks: DeliveryMarks::Plain,
            tuned: false,
            voices: VoiceOps::NONE,
        },
        text: "script",
        voice: "voice_preset",
        tuning: None,
        // MP3 is all it makes.
        fixed: &[],
        voices: &[
            "calm_documentary_narrator",
            "deep_trailer_bass",
            "warm_honey_drawl",
            "fast_talking_wiseguy",
            "graceful_elder_storyteller",
            "chill_surfer_dude",
            "bubbly_it_girl",
            "roaring_locker_room_coach",
            "velvet_radio_alto",
            "gruff_slow_drawl",
            "brassy_sharp_wit",
            "vintage_radio_showman",
            "gentle_hushed_librarian",
            "rapid_sports_caller",
            "warm_playful_spark",
            "folksy_porch_storyteller",
            "crisp_founder_pitch",
            "smoky_noir_monotone",
            "sunny_morning_host",
            "cozy_grandpa_storyteller",
            "plummy_posh_gentleman",
            "cheeky_bright_charmer",
            "rapid_market_banter",
            "stern_clipped_headmistress",
            "hearty_fireside_rumble",
            "soft_thoughtful_lilt",
            "musical_poetic_lilt",
            "melodic_quick_storyteller",
            "gravelly_deadpan",
            "imperious_grande_dame",
            "dry_cheerful_joker",
            "polished_news_anchor",
            "warm_grounded_friend",
            "commanding_safari_guide",
            "rhythmic_radio_charmer",
            "mellow_wise_elder",
            "lilting_warm_hostess",
            "articulate_tech_lead",
            "booming_epic_showman",
            "cool_minimal_designer",
            "precise_stern_engineer",
            "breathy_chic_charm",
            "slow_heavy_brooder",
            "fiery_passionate_cook",
            "loving_loud_grandma",
            "gentle_careful_sweetness",
            "velvet_worldly_narrator",
            "deep_night_storyteller",
            "bright_peppy_vlogger",
            "poised_documentary_calm",
            "grumpy_soft_heart",
            "zany_kid_inventor",
            "hammy_pirate_captain",
            "theatrical_scheming_villain",
            "slow_goofy_giant",
            "wheezy_batty_wizard",
            "raspy_scrappy_kid",
            "chatty_beauty_bestie",
            "hyped_gym_motivator",
            "soft_cozy_aesthetic",
            "smooth_hype_unboxer",
            "bored_chic_trendsetter",
            "joyful_food_fanatic",
            "deadpan_ironic_fry",
            "mellow_road_tripper",
            "breathy_luxe_diva",
            "fast_nerdy_reviewer",
            "unhinged_arena_hype",
            "abyssal_doom_growl",
            "airhorn_club_mc",
            "barking_drill_fury",
            "smoky_mystic_drama",
            "booming_ring_announcer",
            "feather_whisper_asmr",
            "seismic_boss_god",
            "thunderous_revival_preacher",
        ],
        clone_id: None,
    },
];

pub fn find(id: &str) -> Option<&'static HostedSpeech> {
    SPEECH.iter().find(|m| m.id == id)
}

pub fn run(
    key: &str,
    model: &HostedSpeech,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let input = speech::input(model, request)?;
    let job = jobs::run(key, model.id, &input, task)?;
    let (url, content_type) = audio::made(&job["output"])
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no speech back")))?;
    let path = audio::fetch(url, content_type, stem, task)?;
    Ok((path, None))
}
