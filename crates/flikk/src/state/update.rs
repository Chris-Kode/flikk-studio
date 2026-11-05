//! The update notice: the latest release is looked for at launch and every
//! few hours; once one is found it stays under the bell, which can't
//! dismiss it, until the user updates and restarts. The work is
//! `crate::update`'s.

use super::EditorState;
use crate::notices::{Level, Notice, NoticeAction};
use crate::update::{self, Release};
use gpui_kit::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const NOTICE: &str = "update";
/// How often GitHub is asked for a newer release.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How often a download's progress is shown.
const PROGRESS_POLL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub enum Updater {
    /// None found yet.
    #[default]
    Idle,
    Available(Release),
    /// Thousandths downloaded.
    Installing(Release, Arc<AtomicU32>),
    /// Installed; the new version starts on restart.
    Ready(Release),
    Failed(Release, String),
}

impl EditorState {
    /// Look for a newer release now and every [`CHECK_EVERY`].
    pub(super) fn watch_for_updates(cx: &mut Context<Self>) -> Task<()> {
        if !update::enabled() {
            return Task::ready(());
        }
        cx.spawn(async move |this, cx| {
            loop {
                let found = cx.background_executor().spawn(async { update::latest() });
                // Offline, rate limited or nothing new: ask again later.
                if let Ok(Some(release)) = found.await
                    && this
                        .update(cx, |this, cx| this.offer_update(release, cx))
                        .is_err()
                {
                    break;
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        })
    }

    fn offer_update(&mut self, release: Release, cx: &mut Context<Self>) {
        match &self.updater {
            Updater::Installing(..) | Updater::Ready(_) => return,
            Updater::Available(shown) | Updater::Failed(shown, _) if *shown == release => return,
            _ => {}
        }
        self.updater = Updater::Available(release);
        if self.update_notice() {
            cx.notify();
        }
    }

    /// The notice's Update button: download and install, showing progress.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let release = match std::mem::take(&mut self.updater) {
            Updater::Available(release) | Updater::Failed(release, _) => release,
            other => {
                self.updater = other;
                return;
            }
        };
        let progress = Arc::new(AtomicU32::new(0));
        self.updater = Updater::Installing(release.clone(), progress.clone());
        self.update_notice();
        cx.notify();

        let work = {
            let release = release.clone();
            cx.background_executor()
                .spawn(async move { update::install(&release, &progress) })
        };
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.updater = match result {
                    Ok(()) => Updater::Ready(release),
                    Err(e) => Updater::Failed(release, format!("{e:#}")),
                };
                this.update_notice();
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_POLL).await;
                let installing = this.update(cx, |this, cx| {
                    let installing = matches!(this.updater, Updater::Installing(..));
                    if installing && this.update_notice() {
                        cx.notify();
                    }
                    installing
                });
                if !matches!(installing, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Show the updater's state under the bell. Whether it changed.
    fn update_notice(&mut self) -> bool {
        let (title, detail, action) = match &self.updater {
            Updater::Idle => return self.notices.clear(NOTICE),
            Updater::Available(r) => (
                format!("Flikk {} is available", r.version),
                None,
                Some(NoticeAction::Update),
            ),
            Updater::Installing(r, progress) => {
                let percent = progress.load(Ordering::Relaxed) / 10;
                (
                    format!("Downloading Flikk {}… {percent} %", r.version),
                    None,
                    None,
                )
            }
            Updater::Ready(r) => (
                format!("Flikk {} is ready", r.version),
                None,
                Some(NoticeAction::Restart),
            ),
            Updater::Failed(r, message) => (
                format!("Could not update to Flikk {}", r.version),
                Some(message.clone()),
                Some(NoticeAction::Update),
            ),
        };
        self.notices.set(Notice {
            key: NOTICE.into(),
            level: Level::Update,
            title,
            items: Vec::new(),
            detail,
            action,
        })
    }
}
