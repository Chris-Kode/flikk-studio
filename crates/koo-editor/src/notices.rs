//! Warnings and errors listed under the top bar's notice button, kept until
//! the user dismisses them or what they say stops being true.

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Warning,
    Error,
}

/// What a notice's button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    /// Open Settings on the Models tab.
    OpenModels,
    /// Pick a folder to relink offline media from.
    Relink,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    /// What it is about; a newer notice with the same key takes its place.
    pub key: String,
    pub level: Level,
    pub title: String,
    /// Listed under the title.
    pub items: Vec<String>,
    /// A line after them.
    pub detail: Option<String>,
    pub action: Option<NoticeAction>,
}

#[derive(Default)]
pub struct Notices {
    list: Vec<Notice>,
    /// Keys the user dismissed; hidden until their notice is cleared.
    dismissed: HashSet<String>,
}

impl Notices {
    pub fn iter(&self) -> impl Iterator<Item = &Notice> {
        self.list.iter()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The most serious level shown, `None` without notices.
    pub fn level(&self) -> Option<Level> {
        self.list.iter().map(|n| n.level).max()
    }

    /// Show `notice` in place of the one with its key, unless the user
    /// dismissed that key. Whether anything changed.
    pub fn set(&mut self, notice: Notice) -> bool {
        if self.dismissed.contains(&notice.key) {
            return false;
        }
        match self.list.iter_mut().find(|n| n.key == notice.key) {
            Some(n) if *n == notice => false,
            Some(n) => {
                *n = notice;
                true
            }
            None => {
                self.list.push(notice);
                true
            }
        }
    }

    /// What `key`'s notice said is no longer true: take it away, and show
    /// it again should it come back. Whether anything changed.
    pub fn clear(&mut self, key: &str) -> bool {
        self.dismissed.remove(key);
        let before = self.list.len();
        self.list.retain(|n| n.key != key);
        self.list.len() != before
    }

    /// The user closed `key`'s notice; it stays hidden until cleared.
    pub fn dismiss(&mut self, key: &str) {
        self.list.retain(|n| n.key != key);
        self.dismissed.insert(key.to_owned());
    }
}
