//! `FLIKK_DEMO_CHECK=1`: mistakes for the timeline check to find. The
//! project gets a flash of black, a two-frame clip, a title out of
//! title-safe and a caption past the end, which Deliver lists; the pending
//! change set gets a cut through a word and a new flash, which its card
//! counts and its Review list, opened, shows. The set also moves the
//! project's own flash, which it does not count.

use flikk_core::bus::{Author, CommandBus};
use flikk_core::command::{Command, Edge};
use flikk_core::cuts::FADE;
use flikk_core::model::*;
use flikk_core::{Time, TimeRange, TrackId};

pub fn enabled() -> bool {
    super::enabled() && std::env::var("FLIKK_DEMO_CHECK").is_ok_and(|v| v == "1")
}

/// Mistakes in the project itself: `main` is the interview's picture
/// track, `titles` the title track.
pub fn seed_project(project: &mut Project, main: TrackId, titles: TrackId) {
    if !enabled() {
        return;
    }
    let frame = project.timeline.settings.rate.frame_duration();
    let secs = Time::from_secs_f64;
    let tracks = &mut project.timeline.tracks;
    // Three frames of black before the fourth interview shot.
    if let Some(shot) = tracks
        .iter_mut()
        .find(|t| t.id == main)
        .and_then(|t| t.clips.get_mut(2))
    {
        shot.duration = shot.duration - Time(frame.0 * 3);
    }
    // A cutaway two frames long.
    let city = project.media.get(1).map(|m| m.id);
    if let (Some(city), Some(broll)) = (
        city,
        tracks
            .iter_mut()
            .find(|t| t.kind == TrackKind::Video && t.id != main),
    ) {
        let flash = Clip::new(
            secs(30.0),
            Time(frame.0 * 2),
            ClipContent::Media {
                media: city,
                source_in: secs(9.0),
                audio_stream: None,
            },
        );
        let at = broll.clips.partition_point(|c| c.start <= flash.start);
        broll.clips.insert(at, flash);
    }
    // A chapter title pushed into the top corner.
    if let Some(track) = tracks.iter_mut().find(|t| t.id == titles) {
        let mut title = Clip::new(
            secs(19.0),
            secs(3.0),
            ClipContent::Title {
                template: "center".into(),
                text: "Chapter one · North".into(),
            },
        );
        title.transform.position = [0.36, -0.44];
        let at = track.clips.partition_point(|c| c.start <= title.start);
        track.clips.insert(at, title);
    }
    // A closing caption left on after the picture ends.
    if let Some(track) = tracks.iter_mut().find(|t| t.kind == TrackKind::Caption) {
        track.clips.push(Clip::new(
            secs(66.0),
            secs(2.5),
            ClipContent::Caption {
                text: "Thank you for watching.".into(),
                speaker: None,
                words: Vec::new(),
            },
        ));
    }
}

/// Two more edits for the agent's open set, which bring mistakes in: a
/// cut through the middle of "We drove north", and a trim that leaves two
/// frames of black before the last interview shot.
pub fn seed_set(bus: &mut CommandBus, main: TrackId) {
    if !enabled() {
        return;
    }
    let agent = Author::Agent;
    let frame = bus.project().timeline.settings.rate.frame_duration();
    let last = bus
        .project()
        .timeline
        .track(main)
        .and_then(|t| t.clips.last())
        .map(|c| c.id);
    let mut edits = vec![(
        Command::RippleDelete {
            range: TimeRange::new(Time::from_secs_f64(38.6), Time::from_secs_f64(39.3)),
            tracks: Default::default(),
            fade: FADE,
        },
        "Tighten “Things he had never told anyone”",
    )];
    if let Some(clip) = last {
        edits.push((
            Command::Trim {
                clip,
                edge: Edge::Start,
                delta: Time(frame.0 * 2),
                ripple: false,
            },
            "Start the last shot on the look up",
        ));
    }
    for (command, reason) in edits {
        if let Err(e) = bus.execute(command, agent.clone(), reason) {
            eprintln!("demo check: {reason}: {e}");
        }
    }
}
