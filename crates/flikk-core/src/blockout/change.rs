//! Changing a scene by saying only what differs. Writing a whole scene
//! again to move one thing is slow and loses what was not meant to move;
//! a change names what it touches and leaves the rest as it is.
//!
//! A change is the scene's JSON with only what differs, where `objects`,
//! the parts of each model under `models`, and `cameras` are held by
//! name: a name that is there has the fields given put in place of its
//! own, one that is not yet there is added, and `null` takes it away.

use super::Scene;
use serde_json::{Map, Value};

impl Scene {
    /// The scene with `changes` made, checked and built as a scene written
    /// whole is; or what is wrong with them, said for whoever wrote them.
    pub fn changed(&self, changes: &Value) -> Result<Self, String> {
        let wrong = |e: serde_json::Error| format!("blockout: {e}");
        let Value::Object(changes) = changes else {
            return Err("blockout: `changes` is an object holding what differs".into());
        };
        let Value::Object(mut scene) = serde_json::to_value(self).map_err(wrong)? else {
            return Err("blockout: the scene could not be read".into());
        };
        for (field, change) in changes {
            match (field.as_str(), change) {
                ("objects", change) => {
                    let objects = scene
                        .entry("objects")
                        .or_insert_with(|| Value::Array(vec![]));
                    change_named(objects, change, "objects")?;
                }
                ("models", Value::Object(models)) => {
                    let held = scene
                        .entry("models")
                        .or_insert_with(|| Value::Object(Map::new()));
                    let Value::Object(held) = held else {
                        continue;
                    };
                    for (model, parts) in models {
                        if parts.is_null() {
                            held.remove(model);
                            continue;
                        }
                        let list = held.entry(model).or_insert_with(|| Value::Array(vec![]));
                        change_named(list, parts, &format!("model `{model}`"))?;
                    }
                }
                ("camera", Value::Object(fields)) => {
                    if let Some(Value::Object(camera)) = scene.get_mut("camera") {
                        put_camera_fields(camera, fields);
                    }
                }
                ("cameras", Value::Object(cameras)) => {
                    let held = scene
                        .entry("cameras")
                        .or_insert_with(|| Value::Object(Map::new()));
                    let Value::Object(held) = held else {
                        continue;
                    };
                    for (name, fields) in cameras {
                        match (held.get_mut(name), fields) {
                            (_, Value::Null) => {
                                held.remove(name);
                            }
                            (Some(Value::Object(camera)), Value::Object(fields)) => {
                                put_camera_fields(camera, fields);
                            }
                            (_, fields) => {
                                held.insert(name.clone(), fields.clone());
                            }
                        }
                    }
                }
                ("duration" | "ground" | "hills" | "gravity" | "sun" | "cuts", Value::Null) => {
                    scene.remove(field);
                }
                ("duration" | "ground" | "hills" | "gravity" | "sun" | "cuts", value) => {
                    scene.insert(field.clone(), value.clone());
                }
                ("models" | "camera" | "cameras", _) => {
                    return Err(format!(
                        "blockout: `{field}` in `changes` is an object holding what differs"
                    ));
                }
                _ => {
                    return Err(format!(
                        "blockout: `changes` can hold objects, models, camera, cameras, cuts, \
                         duration, ground, hills, gravity and sun, not `{field}`"
                    ));
                }
            }
        }
        Self::parse(&Value::Object(scene))
    }
}

/// Make `change` to `list`, objects or a model's parts as their JSON has
/// them: held by name, each with the fields that differ, or `null` to
/// take it out. A list is taken too, each of its items by its `name`.
fn change_named(list: &mut Value, change: &Value, what: &str) -> Result<(), String> {
    let Value::Array(list) = list else {
        return Ok(());
    };
    let by_name: Vec<(String, &Value)> = match change {
        Value::Object(named) => named.iter().map(|(name, to)| (name.clone(), to)).collect(),
        Value::Array(items) => (items.iter())
            .map(|item| match item.get("name").and_then(Value::as_str) {
                Some(name) => Ok((name.to_owned(), item)),
                None => Err(format!("blockout: a change to {what} has no `name`")),
            })
            .collect::<Result<_, _>>()?,
        _ => {
            return Err(format!(
                "blockout: a change to {what} holds them by name: {{\"name\": {{…}}}}"
            ));
        }
    };
    for (name, to) in by_name {
        let found =
            (list.iter()).position(|held| held.get("name").and_then(Value::as_str) == Some(&name));
        match (found, to) {
            (Some(index), Value::Null) => {
                list.remove(index);
            }
            (None, Value::Null) => {}
            (Some(index), Value::Object(fields)) => {
                if let Value::Object(held) = &mut list[index] {
                    put_fields(held, fields);
                }
            }
            (None, Value::Object(fields)) => {
                let mut added = fields.clone();
                added.insert("name".into(), Value::String(name));
                list.push(Value::Object(added));
            }
            _ => {
                return Err(format!(
                    "blockout: the change to `{name}` is its fields that differ, or null to \
                     remove it"
                ));
            }
        }
    }
    Ok(())
}

/// Put each of `fields` in place of a camera's own. A `lens` takes the
/// place of its `fov` and a `fov` of its `lens`: each says the other.
fn put_camera_fields(camera: &mut Map<String, Value>, fields: &Map<String, Value>) {
    for (given, other) in [("lens", "fov"), ("fov", "lens")] {
        if fields.get(given).is_some_and(|value| !value.is_null()) {
            camera.remove(other);
        }
    }
    put_fields(camera, fields);
}

/// Put each of `fields` in place of `held`'s own; `null` takes one away.
/// The name stays.
fn put_fields(held: &mut Map<String, Value>, fields: &Map<String, Value>) {
    for (field, value) in fields.iter().filter(|(field, _)| *field != "name") {
        match value {
            Value::Null => held.remove(field),
            value => held.insert(field.clone(), value.clone()),
        };
    }
}
