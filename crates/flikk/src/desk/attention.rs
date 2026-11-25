//! Telling the user that something in Flikk waits for them while its
//! window is not the one in front: a system notification for what another
//! app asks, and one for each open project whose agent waits for an
//! approval, asked a question or left changes to review.
//!
//! What waits is looked at every [`LOOK_EVERY`]. It is told once it has
//! stood for two looks, so requests that arrive together make one
//! notification, and only when the window is behind: what the user is
//! looking at needs none. A notification is taken back when nothing waits
//! there any more. Clicking one brings Flikk forward, with the project it
//! is about in the editor ([`answer`]).

use super::Desk;
use crate::state::EditorState;
use flikk_core::ProjectId;
use flikk_core::changeset::ChangeSetState;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const LOOK_EVERY: Duration = Duration::from_millis(500);

/// The tag of the notification about what other apps ask.
const APPS: &str = "apps";
/// What the tag of a project's notification starts with, before its id.
const PROJECT: &str = "project-";

/// The most of a question or request a notification shows.
const BODY_MAX: usize = 160;

pub(super) struct Attention {
    /// What waited at the last look, by notification tag.
    waiting: HashMap<SharedString, Waiting>,
    /// The projects looked at before. What already waits in a project when
    /// it is opened is not news.
    seen: HashSet<EntityId>,
    _looking: Task<()>,
}

struct Waiting {
    body: String,
    /// The user knows: they were told, or the window was in front.
    told: bool,
}

/// Something that waits for the user, as its notification says it.
struct Waits {
    tag: SharedString,
    title: String,
    body: String,
    /// Whether it started waiting while Flikk was looking.
    news: bool,
}

impl Attention {
    /// Look at what waits for as long as the desk lives, and answer the
    /// notifications the user clicks.
    pub(super) fn start(window: &mut Window, cx: &mut Context<Desk>) -> Self {
        cx.on_system_notification_response(|response, cx| answer(&response.tag, cx));
        let looking = cx.spawn_in(window, async move |desk, cx| {
            loop {
                cx.background_executor().timer(LOOK_EVERY).await;
                let looked =
                    desk.update_in(cx, |desk, window, cx| desk.tell_what_waits(window, cx));
                if looked.is_err() {
                    break;
                }
            }
        });
        Self {
            waiting: HashMap::new(),
            seen: HashSet::new(),
            _looking: looking,
        }
    }
}

impl Desk {
    /// Notify the user of what started to wait for them, if the window is
    /// not in front, and take back the notifications of what no longer
    /// does.
    fn tell_what_waits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let in_front = window.is_window_active();
        let before = std::mem::take(&mut self.attention.waiting);
        let mut waiting = HashMap::new();
        for waits in self.what_waits(cx) {
            let told = match before.get(&waits.tag) {
                Some(was) if was.body == waits.body => {
                    if !was.told && !in_front {
                        cx.show_system_notification(SystemNotification {
                            tag: waits.tag.clone(),
                            title: waits.title.into(),
                            body: waits.body.clone().into(),
                            actions: Vec::new(),
                        });
                    }
                    true
                }
                // Just changed: told at the next look, if it still stands.
                _ => !waits.news,
            };
            let body = waits.body;
            waiting.insert(waits.tag, Waiting { body, told });
        }
        for tag in before.keys().filter(|tag| !waiting.contains_key(*tag)) {
            cx.dismiss_system_notification(tag);
        }
        self.attention.waiting = waiting;
        self.attention.seen = self.states().map(|state| state.entity_id()).collect();
    }

    /// Everything that waits for the user now.
    fn what_waits(&self, cx: &App) -> Vec<Waits> {
        let mut waits = Vec::new();
        if let [first, more @ ..] = self.asks() {
            let body = match more.len() {
                0 => first.title.clone(),
                more => format!("{} and {more} more", first.title),
            };
            waits.push(Waits {
                tag: APPS.into(),
                title: "Another app asks".into(),
                body,
                news: true,
            });
        }
        for state in self.states() {
            let s = state.read(cx);
            if let Some(body) = waits_in(s) {
                let project = s.bus.project();
                waits.push(Waits {
                    tag: format!("{PROJECT}{}", project.id.0.simple()).into(),
                    title: project.name.clone(),
                    body,
                    news: self.attention.seen.contains(&state.entity_id()),
                });
            }
        }
        waits
    }
}

/// What waits for the user in a project, in one line: the approvals its
/// agent asked for, else its question, else the changes left to review
/// once it is done.
fn waits_in(s: &EditorState) -> Option<String> {
    let approvals = s.pending_approvals();
    let mut asked = approvals.iter().filter(|p| !p.request.allowed);
    if let Some(first) = asked.next() {
        let summary = cut(&first.request.summary);
        return Some(match asked.count() {
            0 => format!("Needs approval: {summary}"),
            more => format!("Needs approval: {summary} and {more} more"),
        });
    }
    if s.agent_busy() {
        return None;
    }
    if let Some(question) = s.open_question().and_then(|ix| s.chat().get(ix)) {
        return Some(cut(&question.text));
    }
    let to_review: usize = (s.bus.change_sets().iter())
        .filter(|set| !set.author.is_user() && set.state == ChangeSetState::Pending)
        .map(|set| set.pending_items().count())
        .sum();
    match to_review {
        0 => None,
        1 => Some("1 change to review".into()),
        n => Some(format!("{n} changes to review")),
    }
}

/// The first line of `text`, cut to what a notification shows.
fn cut(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(BODY_MAX) {
        Some((end, _)) => format!("{}…", line[..end].trim_end()),
        None => line.to_owned(),
    }
}

/// The user clicked the notification tagged `tag`: bring Flikk forward,
/// with the project it is about in the editor when it is still open.
fn answer(tag: &str, cx: &mut App) {
    cx.activate(true);
    let Some(window) = cx.windows().first().copied() else {
        return;
    };
    let named = tag.strip_prefix(PROJECT).and_then(|id| id.parse().ok());
    let _ = window.update(cx, |_, window, cx| {
        window.activate_window();
        let Some(project) = named.map(ProjectId) else {
            return;
        };
        Desk::get(cx).update(cx, |desk, cx| {
            let open = desk
                .states()
                .find(|s| s.read(cx).bus.project().id == project);
            if let Some(state) = open.cloned() {
                desk.enter(&state, cx);
            }
        });
    });
}
