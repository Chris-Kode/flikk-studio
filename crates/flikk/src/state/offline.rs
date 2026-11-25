//! Media whose source file is missing: worked out when the media list
//! changes and looked at again on disk every few seconds, so a file that
//! goes missing or comes back mid-session is noticed. One notice lists
//! them and leads to Relink.

use super::EditorState;
use crate::notices::{Level, Notice, NoticeAction, Notices};
use flikk_core::MediaId;
use flikk_core::model::Project;
use gpui_kit::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

/// How often every media file is looked for on disk.
const POLL: Duration = Duration::from_secs(2);
/// Key of the notice listing the offline media.
const NOTICE: &str = "offline-media";

/// Which media of the open project are offline, and the media paths that
/// was worked out for.
#[derive(Default)]
pub(super) struct OfflineMedia {
    offline: HashSet<MediaId>,
    paths: Vec<(MediaId, PathBuf)>,
}

impl OfflineMedia {
    /// Whether `media`'s file was missing when last looked at.
    pub(super) fn contains(&self, media: MediaId) -> bool {
        self.offline.contains(&media)
    }

    /// Work out which of `project`'s sources are missing when its media
    /// list changed since the last look, and keep the notice about them in
    /// `notices` current.
    pub(super) fn check(&mut self, project: &Project, notices: &mut Notices) {
        let paths: Vec<_> = project
            .media
            .iter()
            .map(|m| (m.id, m.path.clone()))
            .collect();
        if paths == self.paths {
            return;
        }
        self.offline = missing_media(&paths);
        self.paths = paths;
        self.notice(project, notices);
    }

    /// Take a look at `paths` on disk made off the UI thread, which found
    /// `missing`: sources that went missing, or came back when a drive was
    /// plugged in again. A look taken before the media list changed is
    /// dropped. Whether what is offline changed.
    pub(super) fn recheck(
        &mut self,
        paths: &[(MediaId, PathBuf)],
        missing: HashSet<MediaId>,
        project: &Project,
        notices: &mut Notices,
    ) -> bool {
        if paths != self.paths.as_slice() || missing == self.offline {
            return false;
        }
        self.offline = missing;
        self.notice(project, notices);
        true
    }

    fn notice(&self, project: &Project, notices: &mut Notices) {
        if self.offline.is_empty() {
            notices.clear(NOTICE);
            return;
        }
        let items: Vec<String> = project
            .media
            .iter()
            .filter(|m| self.offline.contains(&m.id))
            .map(crate::ui::format::item_name)
            .collect();
        let title = match items.len() {
            1 => "1 media file is offline".to_owned(),
            n => format!("{n} media files are offline"),
        };
        notices.set(Notice {
            key: NOTICE.into(),
            level: Level::Warning,
            title,
            items,
            detail: None,
            action: Some(NoticeAction::Relink),
        });
    }
}

/// The media in `paths` whose file is not on disk.
fn missing_media(paths: &[(MediaId, PathBuf)]) -> HashSet<MediaId> {
    paths
        .iter()
        .filter(|(_, p)| !p.exists())
        .map(|(id, _)| *id)
        .collect()
}

impl EditorState {
    /// Whether `media`'s source file is missing.
    pub fn is_offline(&self, media: MediaId) -> bool {
        self.offline.contains(media)
    }

    /// Work out which sources are missing when the media list changed, and
    /// keep the notice about them current.
    pub(super) fn check_offline(&mut self) {
        self.offline.check(self.bus.project(), &mut self.notices);
    }

    /// Look for every media file on disk every [`POLL`], off the UI thread:
    /// an unplugged or network drive can be slow to answer.
    pub(super) fn poll_offline(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(paths) = this.update(cx, |this, _| this.offline.paths.clone()) else {
                    break;
                };
                let (paths, missing) = cx
                    .background_executor()
                    .spawn(async move {
                        let missing = missing_media(&paths);
                        (paths, missing)
                    })
                    .await;
                let update = this.update(cx, |this, cx| {
                    let project = this.bus.project();
                    if this
                        .offline
                        .recheck(&paths, missing, project, &mut this.notices)
                    {
                        cx.notify();
                    }
                });
                if update.is_err() {
                    break;
                }
            }
        })
    }
}
