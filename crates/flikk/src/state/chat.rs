//! The project's chats with the agent: the sessions saved beside the
//! project, the messages of the open one as the agent streams into them,
//! the question it waits on and its last error. Every rule about how a
//! turn's text, tool calls and questions become messages lives here.

use super::EditorState;
use crate::persistence::{self, ChatSession, Chats};
use flikk_core::agent::{ActivityEntry, Attachment, ChatMessage, ChatRole, EditorContext};
use flikk_core::ids::AttachmentId;
use flikk_core::model::Project;
use flikk_core::{ChangeSetId, ProjectId};
use gpui_kit::*;
use std::path::PathBuf;

/// The chat sessions of the open project and the open one's messages.
pub(super) struct ChatLog {
    /// Messages of the active chat session.
    messages: Vec<ChatMessage>,
    /// Every chat session of the project. The active one's messages are
    /// edited in `messages` and copied back on save.
    chats: Chats,
    /// The last message is the one the agent is streaming into now: text
    /// and tool calls may go on in it. Off once a turn ends, so an external
    /// client's calls after it start their own message.
    stream_open: bool,
    /// The agent's question still waiting on an answer. Only its choices
    /// can be picked. Never saved: a reopened chat owes no answer.
    question: Option<OpenQuestion>,
    /// The agent's last error, for the chat; cleared by the user's next
    /// message or another chat.
    agent_error: Option<String>,
}

/// A question the agent asked with `ask_user` that no message has answered
/// yet.
#[derive(Clone, Debug)]
struct OpenQuestion {
    /// The chat it was asked in.
    chat: String,
    /// Its message's index in that chat.
    message: usize,
}

impl ChatLog {
    /// `chats` as loaded for a project, with its active session open.
    pub(super) fn load(chats: Chats) -> Self {
        let messages = chats
            .sessions
            .iter()
            .find(|s| s.id == chats.active)
            .map(|s| s.messages.clone())
            .unwrap_or_default();
        Self {
            messages,
            chats,
            stream_open: false,
            question: None,
            agent_error: None,
        }
    }

    /// Messages of the active chat, oldest first.
    pub(super) fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Id of the active chat.
    pub(super) fn active(&self) -> &str {
        &self.chats.active
    }

    /// The agent's last error, until the user's next message or another
    /// chat.
    pub(super) fn agent_error(&self) -> Option<&str> {
        self.agent_error.as_deref()
    }

    /// Index in [`Self::messages`] of the agent's question still waiting
    /// on an answer.
    pub(super) fn open_question(&self) -> Option<usize> {
        self.question
            .as_ref()
            .filter(|q| q.chat == self.chats.active && q.message < self.messages.len())
            .map(|q| q.message)
    }

    /// Files and folders attached to the active chat.
    pub(super) fn attachments(&self) -> &[Attachment] {
        self.active_session()
            .map_or(&[], |s| s.attachments.as_slice())
    }

    /// The messages so far as the agent is reminded of them when its
    /// session is new: what the user and the agent wrote, without tool rows
    /// and cards.
    pub(super) fn history(&self) -> Vec<(ChatRole, String)> {
        self.messages
            .iter()
            .filter(|m| m.change_set.is_none() && !m.text.trim().is_empty())
            .map(|m| (m.role, m.text.clone()))
            .collect()
    }

    /// The user wrote `text`. Picked or typed, it answers the open
    /// question, which closes with it, and the agent's error goes.
    pub(super) fn push_user(&mut self, text: String) {
        self.messages
            .push(message(ChatRole::User, text, vec![], vec![]));
        self.agent_error = None;
        self.question = None;
    }

    /// The agent's turn failed with `message`.
    pub(super) fn agent_failed(&mut self, message: String) {
        self.agent_error = Some(message);
    }

    /// The agent wrote `text`: it goes on in the message it is streaming
    /// into, unless that holds a question, a card or tool calls.
    pub(super) fn append_text(&mut self, text: String) {
        match self.messages.last_mut() {
            Some(m)
                if self.stream_open
                    && m.role == ChatRole::Agent
                    && m.change_set.is_none()
                    && m.choices.is_empty()
                    && m.tools.is_empty() =>
            {
                m.text.push_str(&text)
            }
            _ => self.push_agent(text, vec![]),
        }
    }

    /// The agent asked `text`, offering `choices`. A later question in the
    /// turn replaces this one as the open question.
    pub(super) fn ask(&mut self, text: String, choices: Vec<String>) {
        self.push_agent(text, choices);
        self.question = Some(OpenQuestion {
            chat: self.chats.active.clone(),
            message: self.messages.len() - 1,
        });
    }

    /// Take out the rows of `change_set`, whose card shows above the
    /// composer instead. The open question moves up by the rows removed
    /// before it.
    pub(super) fn remove_change_set(&mut self, change_set: ChangeSetId) {
        if let Some(q) = &mut self.question {
            let before = self.messages[..q.message.min(self.messages.len())]
                .iter()
                .filter(|m| m.change_set == Some(change_set))
                .count();
            q.message -= before;
        }
        self.messages.retain(|m| m.change_set != Some(change_set));
    }

    /// Show tool call `entry`. Calls in a row stack into one message;
    /// anything else in between starts a new stack.
    pub(super) fn push_tool(&mut self, entry: ActivityEntry) {
        let open = std::mem::replace(&mut self.stream_open, true);
        match self.messages.last_mut() {
            Some(m) if open && m.role == ChatRole::Agent && !m.tools.is_empty() => {
                m.tools.push(entry)
            }
            _ => self
                .messages
                .push(message(ChatRole::Agent, String::new(), vec![], vec![entry])),
        }
    }

    /// Put `entry` where call `call` was shown running. A call no longer
    /// shown (its turn was stopped, or another chat is open) is dropped.
    pub(super) fn finish_tool(&mut self, call: u64, entry: ActivityEntry) {
        let row = self
            .messages
            .iter_mut()
            .rev()
            .flat_map(|m| m.tools.iter_mut())
            .find(|t| t.running == Some(call));
        if let Some(row) = row {
            *row = entry;
        }
    }

    /// The agent's turn ended: calls still shown running were stopped with
    /// it, and what it writes next starts its own message.
    pub(super) fn end_turn(&mut self) {
        stop_running(&mut self.messages);
        self.stream_open = false;
    }

    /// What showed for the chat that was open goes with it: the open
    /// stream, question and error.
    pub(super) fn reset_view(&mut self) {
        self.stream_open = false;
        self.agent_error = None;
        self.question = None;
    }

    /// Copy the open chat's messages back into its session, titled from
    /// the user's first message as plain text in `project`, and save the
    /// chats.
    pub(super) fn save_active(&mut self, project: &Project) {
        let title = self
            .messages
            .iter()
            .find(|m| m.role == ChatRole::User)
            .map(|m| chat_title(&crate::ui::chat::plain_text(&m.text, project)));
        let active = self.chats.active.clone();
        let Some(session) = self.chats.sessions.iter_mut().find(|s| s.id == active) else {
            return;
        };
        if session.title.is_empty()
            && let Some(title) = title
        {
            session.title = title;
        }
        if let Some(last) = self.messages.last() {
            session.at = last.at;
        }
        session.messages = self.messages.clone();
        // The running mark is not saved, and the call would not go on after
        // a reopen.
        stop_running(&mut session.messages);
        self.save(project.id);
    }

    /// Save the chats as they are, for `project`.
    pub(super) fn save(&self, project: ProjectId) {
        persistence::save_chats(project, &self.chats);
    }

    /// Chat sessions, most recent first, as `(id, title)`; the titles of
    /// those not titled yet as plain text in `project`.
    pub(super) fn sessions(&self, project: &Project) -> Vec<(String, String)> {
        let mut sessions: Vec<_> = self.chats.sessions.iter().collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.at));
        sessions
            .into_iter()
            .map(|s| {
                // The active chat may have messages not saved into it yet.
                let first = (s.id == self.chats.active)
                    .then(|| self.messages.iter().find(|m| m.role == ChatRole::User))
                    .flatten();
                let title = match first {
                    _ if !s.title.is_empty() => s.title.clone(),
                    Some(m) => chat_title(&crate::ui::chat::plain_text(&m.text, project)),
                    None => "Untitled".to_owned(),
                };
                (s.id.clone(), title)
            })
            .collect()
    }

    /// Whether there is a chat `id`.
    pub(super) fn has(&self, id: &str) -> bool {
        self.chats.sessions.iter().any(|s| s.id == id)
    }

    /// Add an empty chat session, not opened. Its id.
    pub(super) fn add_session(&mut self) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.chats
            .sessions
            .push(ChatSession::new(id.clone(), now()));
        id
    }

    /// Open chat `id`. The chats left without a message or attachment go:
    /// they have nothing to come back to.
    pub(super) fn open(&mut self, id: &str) {
        self.chats.active = id.to_owned();
        self.chats
            .sessions
            .retain(|s| s.id == id || !s.messages.is_empty() || !s.attachments.is_empty());
        self.messages = self
            .active_session()
            .map(|s| s.messages.clone())
            .unwrap_or_default();
    }

    /// Open a new empty chat in place of the open one, which is left as
    /// last saved. Its id.
    pub(super) fn open_fresh(&mut self) -> String {
        let fresh = self.add_session();
        self.messages.clear();
        self.chats.active = fresh.clone();
        fresh
    }

    /// Take chat `id` out of the sessions.
    pub(super) fn remove(&mut self, id: &str) {
        self.chats.sessions.retain(|s| s.id != id);
    }

    /// Attach `paths` to the active chat: those already attached are
    /// skipped. Why the others that were not attached were refused
    /// (unreadable, or inside Flikk's own data); `None` without an active
    /// session.
    pub(super) fn attach(&mut self, paths: Vec<PathBuf>) -> Option<Vec<String>> {
        let own = std::fs::canonicalize(flikk_core::paths::home()).ok();
        let session = self.active_session_mut()?;
        let mut refused = Vec::new();
        for path in paths {
            match Attachment::of(&path) {
                Ok(a) if own.as_ref().is_some_and(|own| a.path.starts_with(own)) => {
                    refused.push(format!("{} holds Flikk's own data", a.path.display()));
                }
                Ok(a) if session.attachments.iter().any(|b| b.path == a.path) => {}
                Ok(a) => session.attachments.push(a),
                Err(e) => refused.push(e),
            }
        }
        Some(refused)
    }

    /// Take attachment `id` off the active chat. Whether it was there.
    pub(super) fn detach(&mut self, id: AttachmentId) -> bool {
        let Some(session) = self.active_session_mut() else {
            return false;
        };
        let before = session.attachments.len();
        session.attachments.retain(|a| a.id != id);
        session.attachments.len() != before
    }

    /// Replace the open chat's messages with `messages` as they are, past
    /// the streaming rules: for the demo, which seeds a conversation.
    pub(super) fn seed(&mut self, messages: Vec<ChatMessage>) {
        self.messages = messages;
    }

    fn push_agent(&mut self, text: String, choices: Vec<String>) {
        self.stream_open = true;
        self.messages
            .push(message(ChatRole::Agent, text, choices, vec![]));
    }

    fn active_session(&self) -> Option<&ChatSession> {
        self.chats
            .sessions
            .iter()
            .find(|s| s.id == self.chats.active)
    }

    fn active_session_mut(&mut self) -> Option<&mut ChatSession> {
        let active = &self.chats.active;
        self.chats.sessions.iter_mut().find(|s| &s.id == active)
    }
}

/// A message written now, linked to no change set.
fn message(
    role: ChatRole,
    text: String,
    choices: Vec<String>,
    tools: Vec<ActivityEntry>,
) -> ChatMessage {
    ChatMessage {
        role,
        text,
        choices,
        change_set: None,
        tools,
        at: now(),
    }
}

/// Tool calls still shown running in `messages` were stopped: a call that
/// outlives its turn, or its chat's save, never finishes.
fn stop_running(messages: &mut [ChatMessage]) {
    for row in messages.iter_mut().flat_map(|m| m.tools.iter_mut()) {
        if row.running.take().is_some() {
            row.ok = false;
            row.summary = "Stopped".into();
        }
    }
}

/// A session title from its first message: the first line, shortened.
fn chat_title(text: &str) -> String {
    const MAX: usize = 28;
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= MAX {
        line.to_owned()
    } else {
        let short: String = line.chars().take(MAX - 1).collect();
        format!("{}…", short.trim_end())
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- chat
impl EditorState {
    /// Messages of the active chat session, oldest first.
    pub fn chat(&self) -> &[ChatMessage] {
        self.chat.messages()
    }

    /// The agent's last error, for the chat; cleared by the user's next
    /// message or another chat.
    pub fn agent_error(&self) -> Option<&str> {
        self.chat.agent_error()
    }

    /// Replace the active chat's messages as they are: for the demo, which
    /// seeds a conversation.
    pub fn seed_chat(&mut self, messages: Vec<ChatMessage>) {
        self.chat.seed(messages);
    }

    /// Index in [`Self::chat`] of the agent's question still waiting on an
    /// answer: only its choices can be picked.
    pub fn open_question(&self) -> Option<usize> {
        self.chat.open_question()
    }

    /// A message is queued or its turn under way: another is refused.
    pub fn agent_busy(&self) -> bool {
        self.agent.status().is_busy()
    }

    /// Send `text` as the user's message. Does nothing while the agent is
    /// busy: it would refuse the message, which must then not show, nor
    /// clear the settled cards.
    pub fn send_chat(&mut self, text: String, cx: &mut Context<Self>) {
        self.send_chat_with_context(text, None, cx);
    }

    /// Send `text` as the user's message, like [`Self::send_chat`] (nothing
    /// while the agent is busy); the agent also reads `context` after it (the resolved `@` references), which
    /// the chat neither shows nor stores.
    pub fn send_chat_with_context(
        &mut self,
        text: String,
        context: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.agent_busy() {
            return;
        }
        let history = self.chat.history();
        self.chat.push_user(text.clone());
        let message = match context {
            Some(context) => format!("{text}\n\n{context}"),
            None => text,
        };
        let editor = EditorContext {
            history,
            ..self.context()
        };
        self.agent.send(message, editor, &self.settings);
        self.clear_settled();
        self.save_chat();
        cx.notify();
    }

    /// Save the open chat with its messages so far.
    pub(super) fn save_chat(&mut self) {
        self.chat.save_active(self.bus.project());
    }

    /// Files and folders attached to the active chat, which the agent may
    /// read.
    pub fn attachments(&self) -> &[Attachment] {
        self.chat.attachments()
    }

    /// Attach `paths` to `chat` of `project`, the chat open when the user
    /// picked or dropped them; dropped if that is no longer the open one.
    /// Paths already attached, unreadable or inside Flikk's own data are
    /// left out. Refused while the agent replies, so what it may read stays
    /// fixed for the whole turn.
    pub fn attach(
        &mut self,
        project: ProjectId,
        chat: &str,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if project != self.bus.project().id || chat != self.chat.active() {
            return;
        }
        if self.agent_busy() {
            self.status = Some("Attach once the agent has replied".into());
            cx.notify();
            return;
        }
        let Some(refused) = self.chat.attach(paths) else {
            return;
        };
        if !refused.is_empty() {
            self.status = Some(refused.join("; "));
        }
        self.chat.save(project);
        cx.notify();
    }

    /// Stop the agent reading an attachment of the active chat. Refused
    /// while the agent replies, like [`Self::attach`].
    pub fn detach(&mut self, id: AttachmentId, cx: &mut Context<Self>) {
        if self.agent_busy() {
            return;
        }
        if self.chat.detach(id) {
            self.chat.save(self.bus.project().id);
            cx.notify();
        }
    }

    // ---------------------------------------------------------- chat sessions

    /// Chat sessions, most recent first, as `(id, title)`.
    pub fn chat_sessions(&self) -> Vec<(String, String)> {
        self.chat.sessions(self.bus.project())
    }

    pub fn active_chat(&self) -> &str {
        self.chat.active()
    }

    /// Chats can't be opened, started or deleted now: a turn is under way,
    /// or requests in the open chat wait on the user.
    pub fn chat_locked(&self) -> bool {
        self.agent_busy() || !self.agent.pending_approvals().is_empty()
    }

    /// Start a chat with a fresh agent conversation. An empty active chat
    /// is reused instead of piling up blank ones.
    pub fn new_chat(&mut self, cx: &mut Context<Self>) {
        if self.chat_locked() || self.chat.messages().is_empty() {
            return;
        }
        let id = self.chat.add_session();
        self.switch_chat(&id, cx);
    }

    pub fn switch_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.chat_locked() || id == self.chat.active() || !self.chat.has(id) {
            return;
        }
        self.save_chat();
        self.chat.open(id);
        self.reset_chat_view();
        self.agent.set_chat(id);
        self.bus.set_chat(id);
        self.chat.save(self.bus.project().id);
        cx.notify();
    }

    /// Delete a chat and forget its agent conversation. Deleting the active
    /// chat opens the most recent other one, or a new empty chat.
    pub fn delete_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.chat_locked() {
            return;
        }
        let project = self.bus.project().id;
        if id == self.chat.active() {
            let next = self
                .chat_sessions()
                .into_iter()
                .map(|(id, _)| id)
                .find(|other| other != id);
            match next {
                Some(next) => self.switch_chat(&next, cx),
                None => {
                    let fresh = self.chat.open_fresh();
                    self.reset_chat_view();
                    self.agent.set_chat(&fresh);
                    self.bus.set_chat(&fresh);
                }
            }
        }
        self.chat.remove(id);
        flikk_agent::session::forget_chat(project, id);
        self.chat.save(project);
        cx.notify();
    }

    /// What showed for the chat that was open goes with it.
    pub(super) fn reset_chat_view(&mut self) {
        self.settled.clear();
        self.focused_change = None;
        self.preview = None;
        self.chat.reset_view();
    }
}
