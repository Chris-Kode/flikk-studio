//! `KOOEDITOR_DEMO_HDR=1`: the selected clip's media is HDR (HLG, the
//! Dolby Vision 8.4 base layer phones record), so the pool shows its badge
//! and the inspector's Color tab its tone map.

use koo_core::ClipId;
use koo_core::hdr::{Hdr, ToneMap, Transfer};
use koo_core::model::{ClipContent, Project};

pub fn seed(project: &mut Project, selected: ClipId) {
    if !std::env::var("KOOEDITOR_DEMO_HDR").is_ok_and(|v| v == "1") {
        return;
    }
    let Some(ClipContent::Media { media, .. }) = project
        .timeline
        .clip(selected)
        .map(|(_, c)| c.content.clone())
    else {
        return;
    };
    let Some(item) = project.media.iter_mut().find(|m| m.id == media) else {
        return;
    };
    if let Some(video) = item.info.as_mut().and_then(|i| i.video.as_mut()) {
        video.hdr = Some(Box::new(Hdr {
            transfer: Transfer::Hlg,
            primaries: "bt2020".into(),
            matrix: "bt2020nc".into(),
            range: "tv".into(),
            bit_depth: 10,
            peak: None,
            dolby_vision: Some(8),
        }));
    }
    item.tone_map = ToneMap {
        exposure: 0.3,
        ..ToneMap::default()
    };
}
