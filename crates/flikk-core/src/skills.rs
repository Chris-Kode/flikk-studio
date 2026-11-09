//! Skills and styles: the user's own instructions for the agent, shared by
//! every project. A skill is run from the chat as `/name`; a style is picked
//! in the composer and rides along with every message until changed.
//!
//! Each one is a Markdown file, `paths::skills()/<name>.md` or
//! `paths::styles()/<slug>.md`: a short front matter (`name`, a skill's
//! `description`, `author: agent` when the agent made it) over the
//! instructions. A file without front matter is all instructions, named
//! after the file, so a hand-written note works too.
//!
//! Errors are messages for the user.

use crate::paths;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Skill,
    Style,
}

impl Kind {
    /// Lowercase noun for messages: `skill`, `style`.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Style => "style",
        }
    }

    fn dir(self) -> PathBuf {
        match self {
            Kind::Skill => paths::skills(),
            Kind::Style => paths::styles(),
        }
    }
}

/// A saved skill or style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The file's stem: a skill's slash name, a style's slug. Changes when
    /// the entry is renamed.
    pub id: String,
    /// A skill's slash name without the slash (`tighten-intro`), or a
    /// style's name as written (`Calm interview`).
    pub name: String,
    /// One line, for skills; empty for styles.
    pub description: String,
    pub instructions: String,
    /// Made by the agent rather than the user.
    pub by_agent: bool,
    /// Unix seconds of the last save; 0 when unknown.
    pub edited: i64,
}

/// What [`save`] writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub by_agent: bool,
}

impl From<&Entry> for Draft {
    fn from(entry: &Entry) -> Self {
        Self {
            name: entry.name.clone(),
            description: entry.description.clone(),
            instructions: entry.instructions.clone(),
            by_agent: entry.by_agent,
        }
    }
}

/// Every entry of `kind`, by name. Unreadable files are left out.
pub fn list(kind: Kind) -> Vec<Entry> {
    let Ok(dir) = std::fs::read_dir(kind.dir()) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = dir
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter_map(|p| read(&p))
        .collect();
    entries.sort_by_key(|e| e.name.to_lowercase());
    entries
}

/// The entry with this id, or `None`. A skill's id is its slash name, so
/// `get(Kind::Skill, "tighten-intro")` finds `/tighten-intro`.
pub fn get(kind: Kind, id: &str) -> Option<Entry> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    read(&kind.dir().join(format!("{id}.md")))
}

/// The id an entry named `name` has, or would have once saved; `None` when
/// the name is not valid for `kind`.
pub fn id_of(kind: Kind, name: &str) -> Option<String> {
    let name = valid_name(kind, name).ok()?;
    id_for(kind, &name).ok()
}

/// Writes `draft`, as a new entry when `previous` is `None`, or over the
/// entry with that id (moving it when the name changes). Fails when the
/// name is not valid for `kind` or another entry already has it.
pub fn save(kind: Kind, previous: Option<&str>, draft: &Draft) -> Result<Entry> {
    let name = valid_name(kind, &draft.name)?;
    let id = id_for(kind, &name)?;
    let dir = kind.dir();
    let path = dir.join(format!("{id}.md"));
    let own = previous == Some(id.as_str());
    let claimed = if own {
        false
    } else {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(match kind {
                    Kind::Skill => format!("A skill named /{name} already exists"),
                    Kind::Style => format!("A style named {name} already exists"),
                });
            }
            Err(e) => return Err(format!("Can't save {name}: {e}")),
        }
    };
    let text = render(kind, &name, draft);
    if let Err(e) = flikk_os::fs::write_durable(&path, text.as_bytes()) {
        if claimed {
            let _ = std::fs::remove_file(&path);
        }
        return Err(format!("Can't save {name}: {e}"));
    }
    if let Some(old) = previous.filter(|old| *old != id) {
        delete(kind, old);
    }
    read(&path).ok_or_else(|| format!("Can't read {name} back"))
}

/// Removes the entry; a missing one is already gone.
pub fn delete(kind: Kind, id: &str) {
    if get(kind, id).is_some() {
        let _ = std::fs::remove_file(kind.dir().join(format!("{id}.md")));
    }
}

/// A chat message that runs a skill: `/name` and what follows it.
pub struct Invocation {
    /// The skill's slash name, without the slash.
    pub name: String,
    /// The skill's instructions, then the text after the name as the
    /// user's input.
    pub prompt: String,
}

/// The skill `message` runs, or `None` when it does not start with the
/// slash name of a saved skill.
pub fn invocation(message: &str) -> Option<Invocation> {
    let rest = message.trim_start().strip_prefix('/')?;
    let (name, input) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(name, input)| (name, input.trim()));
    let skill = get(Kind::Skill, &name.to_lowercase())?;
    let mut prompt = skill.instructions.trim().to_owned();
    if !input.is_empty() {
        prompt.push_str("\n\nUser input: ");
        prompt.push_str(input);
    }
    Some(Invocation {
        name: skill.name,
        prompt,
    })
}

// ------------------------------------------------------------- the files

fn valid_name(kind: Kind, name: &str) -> Result<String> {
    let name = name.trim();
    match kind {
        Kind::Skill => {
            let name = name.trim_start_matches('/').to_lowercase();
            let fits = !name.is_empty()
                && name.len() <= 40
                && !name.starts_with('-')
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if fits {
                Ok(name)
            } else {
                Err("Skill names are one word, like tighten-intro".into())
            }
        }
        Kind::Style => {
            let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                Err("Name the style".into())
            } else if name.chars().count() > 60 {
                Err("Style names are 60 characters at most".into())
            } else {
                Ok(name)
            }
        }
    }
}

/// The file stem for a valid `name`.
fn id_for(kind: Kind, name: &str) -> Result<String> {
    if kind == Kind::Skill {
        return Ok(name.to_owned());
    }
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        Err("Style names need a letter or a number".into())
    } else {
        Ok(slug)
    }
}

fn render(kind: Kind, name: &str, draft: &Draft) -> String {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = format!("---\nname: {name}\n");
    let description = one_line(&draft.description);
    if kind == Kind::Skill && !description.is_empty() {
        out.push_str(&format!("description: {description}\n"));
    }
    if draft.by_agent {
        out.push_str("author: agent\n");
    }
    out.push_str("---\n");
    out.push_str(draft.instructions.trim());
    out.push('\n');
    out
}

fn read(path: &Path) -> Option<Entry> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let text = std::fs::read_to_string(path).ok()?;
    // An empty file is a name claimed by a save still in progress.
    if text.trim().is_empty() {
        return None;
    }
    let edited = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let mut entry = Entry {
        name: id.clone(),
        id,
        description: String::new(),
        instructions: text.trim().to_owned(),
        by_agent: false,
        edited,
    };
    let text = text.replace("\r\n", "\n");
    let Some((head, body)) = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
    else {
        return Some(entry);
    };
    for line in head.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim() {
            "name" if !value.is_empty() => entry.name = value,
            "description" => entry.description = value,
            "author" => entry.by_agent = value == "agent",
            _ => {}
        }
    }
    entry.instructions = body.trim_start_matches('-').trim().to_owned();
    Some(entry)
}
