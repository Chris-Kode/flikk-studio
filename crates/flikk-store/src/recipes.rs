//! Recipes: saved multi-step agent tasks, run as slash commands. The seven
//! built-in recipes from the PRD are seeded on every open (user edits to
//! them are kept).

use crate::{Store, now, parse_uuid, uuid_text};
use anyhow::{Result, bail};
use flikk_core::ids::RecipeId;
use flikk_core::memory::{Recipe, RecipeParam};
use rusqlite::{OptionalExtension, Row, params};
use uuid::Uuid;

const RECIPE_COLUMNS: &str = "id, name, description, params, prompt, builtin";

impl Store {
    /// Built-ins first, then by name.
    pub fn list_recipes(&self) -> Result<Vec<Recipe>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RECIPE_COLUMNS} FROM recipes ORDER BY builtin DESC, name"
        ))?;
        let rows = stmt.query_map([], raw_recipe)?;
        rows.map(|r| recipe_from_raw(r?)).collect()
    }

    pub fn recipe(&self, id: RecipeId) -> Result<Option<Recipe>> {
        self.recipe_where("id = ?1", &uuid_text(&id.0))
    }

    /// By slash name, without the slash (`rough-cut`).
    pub fn recipe_by_name(&self, name: &str) -> Result<Option<Recipe>> {
        self.recipe_where("name = ?1", name.trim_start_matches('/'))
    }

    fn recipe_where(&self, cond: &str, arg: &str) -> Result<Option<Recipe>> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {RECIPE_COLUMNS} FROM recipes WHERE {cond}"),
                [arg],
                raw_recipe,
            )
            .optional()?;
        raw.map(recipe_from_raw).transpose()
    }

    /// Insert or update. Slash names are unique.
    pub fn save_recipe(&self, recipe: &Recipe) -> Result<()> {
        let name = recipe.name.trim().trim_start_matches('/');
        if name.is_empty() || name.contains(char::is_whitespace) {
            bail!("recipe name must be one word, e.g. rough-cut");
        }
        let taken: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM recipes WHERE name = ?1 AND id != ?2",
                params![name, uuid_text(&recipe.id.0)],
                |r| r.get(0),
            )
            .optional()?;
        if taken.is_some() {
            bail!("a recipe named /{name} already exists");
        }
        let builtin = is_builtin(recipe.id);
        self.conn.execute(
            "INSERT INTO recipes (id, name, description, params, prompt, builtin, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name,
                 description = excluded.description, params = excluded.params,
                 prompt = excluded.prompt, updated_at = excluded.updated_at",
            params![
                uuid_text(&recipe.id.0),
                name,
                recipe.description,
                serde_json::to_string(&recipe.params)?,
                recipe.prompt,
                builtin,
                now(),
            ],
        )?;
        Ok(())
    }

    /// Built-in recipes cannot be deleted (they would be re-seeded); use
    /// [`Store::reset_recipe`] or disable them in settings instead.
    pub fn delete_recipe(&self, id: RecipeId) -> Result<()> {
        if is_builtin(id) {
            bail!("built-in recipes can be disabled, not deleted");
        }
        self.conn
            .execute("DELETE FROM recipes WHERE id = ?1", [uuid_text(&id.0)])?;
        Ok(())
    }

    /// Restore a built-in recipe to its shipped text.
    pub fn reset_recipe(&self, id: RecipeId) -> Result<()> {
        let Some(original) = builtin_recipes().into_iter().find(|r| r.id == id) else {
            bail!("not a built-in recipe");
        };
        self.conn.execute(
            "DELETE FROM recipes WHERE id = ?1",
            [uuid_text(&original.id.0)],
        )?;
        seed(&self.conn)
    }
}

pub(crate) fn seed(conn: &rusqlite::Connection) -> Result<()> {
    let t = now();
    for r in builtin_recipes() {
        // A user recipe may already use the slash name; the built-in then
        // waits until that one is renamed.
        conn.execute(
            "INSERT OR IGNORE INTO recipes
                 (id, name, description, params, prompt, builtin, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params![
                uuid_text(&r.id.0),
                r.name,
                r.description,
                serde_json::to_string(&r.params)?,
                r.prompt,
                t,
            ],
        )?;
    }
    Ok(())
}

type RawRecipe = (String, String, String, String, String, bool);

fn raw_recipe(r: &Row) -> rusqlite::Result<RawRecipe> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}

fn recipe_from_raw(raw: RawRecipe) -> Result<Recipe> {
    let (id, name, description, params, prompt, builtin) = raw;
    Ok(Recipe {
        id: RecipeId(parse_uuid(&id)?),
        name,
        description,
        params: serde_json::from_str(&params).unwrap_or_default(),
        prompt,
        builtin,
    })
}

// ------------------------------------------------------------------ built-ins

/// Built-in ids are fixed so settings can refer to them across installs.
const BUILTIN_BASE: u128 = 0x6b6f6f00_0000_4000_8000_000000000000;

fn builtin_id(n: u128) -> RecipeId {
    RecipeId(Uuid::from_u128(BUILTIN_BASE + n))
}

fn is_builtin(id: RecipeId) -> bool {
    builtin_recipes().iter().any(|r| r.id == id)
}

pub(crate) fn builtin_ids() -> Vec<RecipeId> {
    builtin_recipes().into_iter().map(|r| r.id).collect()
}

fn param(name: &str, description: &str, default: Option<&str>) -> RecipeParam {
    RecipeParam {
        name: name.into(),
        description: description.into(),
        default: default.map(Into::into),
    }
}

/// Shared ground rules appended to every built-in prompt.
const GROUND_RULES: &str = "\
Ground rules:
- Read before you edit: get the project outline and the transcript/analysis for the range you work on, in pieces, never the whole thing at once.
- Look before you cut: call view_range on any section you are unsure about and check the collage.
- Every edit is a proposal in one change set; give each item a short reason the user can read.
- Never cut inside a protected marker range; if a step would, skip it and say so.
- Follow the approved style rules and source facts; when a rule and the request disagree, ask with ask_user.
- Finish with a two or three line summary: what changed, total duration before and after, anything you skipped.";

pub fn builtin_recipes() -> Vec<Recipe> {
    let recipe = |n: u128, name: &str, description: &str, params, body: &str| Recipe {
        id: builtin_id(n),
        name: name.into(),
        description: description.into(),
        params,
        prompt: format!("{body}\n\n{GROUND_RULES}"),
        builtin: true,
    };
    vec![
        recipe(
            1,
            "rough-cut",
            "Highlight rough cut",
            vec![
                param("target", "Target length, e.g. 3:00", Some("3:00")),
                param("focus", "What the highlights should be about", Some("the strongest moments")),
            ],
            "Build a highlight rough cut of about {target}, focused on {focus}.
1. Read the project outline, then the transcript with speakers and the shot changes for every source on the timeline.
2. Pick candidate moments: complete thoughts with a clear start and end, strong reactions, key information. Prefer cutting at sentence boundaries and shot changes.
3. Check each candidate with view_range so it is visually usable (no blur, no one walking out of frame, no black).
4. Propose the cut with ripple_delete / delete_clips for everything outside the picks, keeping story order unless a reorder clearly helps; use move_clip for reorders.
5. Add a marker at each kept section with a two to four word label.
6. If the result is more than 10% off {target}, trim the weakest picks or add the next best ones until it fits.",
        ),
        recipe(
            2,
            "dead-time",
            "Dead-time cleanup",
            vec![
                param("min_silence", "Shortest pause to remove, in seconds", Some("0.6")),
                param("padding", "Breath kept at each side of a cut, in seconds", Some("0.12")),
                param("fillers", "Also remove filler words (yes/no)", Some("yes")),
            ],
            "Remove dead time from the timeline.
1. Get the silences from analysis with a minimum length of {min_silence} s.
2. For each silence, keep {padding} s on both sides and ripple_delete the rest, across all unlocked tracks so audio stays in sync.
3. If fillers is yes ({fillers}), find filler words in the transcript (um, uh, er, like / you know used as filler, repeated false starts) and ripple_delete them when cutting them does not break the sentence; leave them when the word carries meaning.
4. Skip pauses that are intentional: a reaction, a laugh, a beat after a punchline, music-only sections. Use view_range when in doubt.
5. Batch the cuts into one change set, grouped from the end of the timeline to the start so earlier cuts do not shift later ranges.",
        ),
        recipe(
            3,
            "teaser",
            "Teaser",
            vec![
                param("length", "Teaser length, e.g. 0:30", Some("0:30")),
                param("placement", "start (prepend to the video) or separate", Some("start")),
            ],
            "Cut a teaser of about {length} from the strongest moments of this video.
1. Read the transcript and pick three to six short moments (2 to 8 s each) that raise a question or show a payoff without giving away the ending.
2. Check every pick with view_range: it must read well without context and look clean.
3. If placement is start ({placement}), insert the picks at the start of the timeline with insert_clip in insert mode; otherwise put them after the end, separated by a marker named Teaser.
4. Order them to build tension; end on the strongest line.
5. Add a short fade (add_fade, 4 to 8 frames) between teaser moments and duck any music under dialogue.",
        ),
        recipe(
            4,
            "captions",
            "Caption pass",
            vec![
                param("style", "Caption style name from the profile", Some("Default")),
                param("language", "Caption language; empty follows the source", None),
            ],
            "Add or refresh captions for the whole timeline using the {style} caption style.
1. Read the transcript word by word for each clip on the timeline (map source time to timeline time).
2. Group words into captions: at most the style's characters per line and lines per caption, break at punctuation and natural pauses, never split a name or a number, 1 to 6 s on screen each.
3. Fix obvious transcription errors using the source facts (names, spellings, literal quotes). Do not rewrite what people said.
4. If a language is set ({language}), translate each caption into it, keeping timing.
5. Create them with add_caption on the caption track; edit existing captions with edit_caption instead of duplicating them.",
        ),
        recipe(
            5,
            "zooms",
            "Targeted zooms",
            vec![
                param("every", "Rough spacing between zooms, in seconds", Some("20")),
                param("scale", "Zoom scale; empty uses the profile preset", None),
            ],
            "Add punch-in zooms that emphasise key moments, about one every {every} s.
1. Read the transcript and pick emphasis points: punchlines, key numbers, strong statements, reactions.
2. Use view_range on each point to find the subject (face or object) and where it sits in the frame.
3. Add each zoom with add_zoom centred on the subject, using scale {scale} (or the profile zoom preset) and the profile ease; hold it for the length of the phrase.
4. Do not zoom on shots that are already tight, on text or screen recordings that would crop, or across a cut.
5. Vary them: not every zoom the same size or direction.",
        ),
        recipe(
            6,
            "music",
            "Music and loudness",
            vec![
                param("lufs", "Target integrated loudness; empty uses the profile preset", None),
                param("ducking", "Music reduction under speech in dB; empty uses the profile preset", None),
            ],
            "Balance music and dialogue and hit the delivery loudness.
1. Measure loudness for each audio track and the full mix.
2. Set dialogue tracks to sit consistently with set_gain and set_track_mix; fix clips that jump in level.
3. Turn on ducking with set_ducking: music ducks by {ducking} dB (or the profile preset) under speech, with smooth attack and release.
4. Add short fades (add_fade) at music starts, ends and hard cuts.
5. Set the master to {lufs} LUFS (or the profile preset) integrated with a true peak at or below the profile limit; report the measured values before and after.",
        ),
        recipe(
            7,
            "feedback",
            "Apply feedback",
            vec![param(
                "notes",
                "The feedback, pasted or typed; timestamps like 1:23 are resolved on the timeline",
                None,
            )],
            "Apply this feedback to the timeline:
{notes}

1. Split the feedback into separate requests. Resolve each timestamp and reference to a timeline position, clip or word range.
2. For each request, read and view the range, then propose the smallest edit that satisfies it, with the request quoted in the item's reason.
3. If a request is ambiguous or conflicts with a protected range or an approved style rule, ask with ask_user instead of guessing.
4. If a request reads like a lasting preference (\"always\", \"never\", \"we don't\"), also propose it as a style rule with propose_rule, citing the feedback as evidence.
5. List the requests you applied, asked about or skipped.",
        ),
    ]
}
