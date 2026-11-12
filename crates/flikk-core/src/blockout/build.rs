//! Writing a scene out into the parts that are drawn. Every copy of a
//! model becomes its parts, named `copy/part`, scaled with the copy and
//! with what the copy does its own way put in; names are swapped for
//! places in the list; and what the motion needs every frame (how each
//! part's keys move it, the order parts are placed in, the box each thing
//! fills, the ground each has covered) is worked out once.

use super::motion::{CameraMoves, Moves, Track};
use super::space::Vec3;
use super::{Anchor, CAMERA, FPS, MAX_PARTS, MID_GREY, Object, PATH, Scene, Target};
use std::collections::{HashMap, VecDeque};

/// How deep a model may hold copies of models.
const MAX_NESTING: usize = 4;

/// One object as it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Part {
    /// As it stands in the scene: its name in full, its size and shade
    /// given, scaled with the copy it is in.
    pub object: Object,
    pub parent: Option<usize>,
    /// The thing it is part of: the copy of a model it is in, else its
    /// top parent; itself when it stands alone. Motion paced by the
    /// ground follows how far this has travelled.
    pub whole: usize,
    /// The thing a viewer would name it as: its whole, or the copy that
    /// holds that.
    pub thing: usize,
    pub facing: Facing,
    /// How its keys move it.
    pub moves: Moves,
    /// The box it fills in its own space, low corner and high. For a
    /// joint or a copy: what rides it, as the scene starts.
    pub bounds: ([f64; 3], [f64; 3]),
    /// Metres its keys have moved it over the ground by each frame; empty
    /// when no motion asks.
    pub travelled: Vec<f32>,
}

/// What a part turns to about the vertical.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Facing {
    Nothing,
    /// The way it travels.
    Path,
    Camera,
    Part(usize),
    Point([f64; 3]),
}

/// An object written out, with the copy of a model it is in and whether
/// it is one itself.
struct Out {
    object: Object,
    inside: Option<usize>,
    is_copy: bool,
}

/// What a copy passes on to its parts.
#[derive(Clone, Copy)]
struct Within<'a> {
    /// What goes before a part's name: `copy/`.
    prefix: &'a str,
    /// The parts beside it, which it may ride and face by their own names.
    beside: &'a [Object],
    scale: f64,
    /// How light the copy is against its model.
    tone: f64,
    copy: Option<usize>,
    depth: usize,
}

impl Scene {
    /// Write the objects out into `parts`.
    pub(super) fn build(&mut self) -> Result<(), String> {
        let top = Within {
            prefix: "",
            beside: &[],
            scale: 1.0,
            tone: 1.0,
            copy: None,
            depth: 0,
        };
        let mut outs = Vec::new();
        for object in &self.objects {
            self.write_out(object, top, &mut outs)?;
        }
        let mut places = HashMap::new();
        for (index, out) in outs.iter().enumerate() {
            if places.insert(out.object.name.clone(), index).is_some() {
                return Err(format!("two objects are called `{}`", out.object.name));
            }
        }
        let place = |name: &str| places.get(name).copied();
        let mut parts = Vec::with_capacity(outs.len());
        for out in &outs {
            let name = &out.object.name;
            let parent = match &out.object.parent {
                Some(parent) => Some(place(parent).ok_or_else(|| {
                    format!("`{name}` rides `{parent}`, which is no object's name")
                })?),
                None => None,
            };
            let facing = match &out.object.face {
                None => Facing::Nothing,
                Some(Target::Point(point)) => Facing::Point(*point),
                Some(Target::Object(faced)) if faced == PATH => Facing::Path,
                Some(Target::Object(faced)) if faced == CAMERA => Facing::Camera,
                Some(Target::Object(faced)) => Facing::Part(place(faced).ok_or_else(|| {
                    format!(
                        "`{name}` faces `{faced}`, which is no object's name, `{PATH}` or \
                         `{CAMERA}`"
                    )
                })?),
            };
            parts.push(Part {
                object: out.object.clone(),
                parent,
                whole: 0,
                thing: 0,
                facing,
                moves: Moves::of(&out.object, self.duration),
                bounds: solid_bounds(&out.object),
                travelled: Vec::new(),
            });
        }
        for index in 0..parts.len() {
            let top = top_parent(&parts, index)?;
            let whole = match (&outs[index], outs[index].inside) {
                (out, _) if out.is_copy => index,
                (_, Some(copy)) => copy,
                (_, None) => top,
            };
            // Out of any copy that holds its whole.
            let mut thing = whole;
            while let Some(holds) = outs[thing].inside {
                thing = holds;
            }
            (parts[index].whole, parts[index].thing) = (whole, thing);
        }
        let paced: Vec<usize> = (parts.iter())
            .filter(|part| {
                part.object
                    .cycle
                    .as_ref()
                    .is_some_and(|c| c.metres.is_some())
            })
            .map(|part| part.whole)
            .collect();
        for whole in paced {
            if parts[whole].travelled.is_empty() {
                parts[whole].travelled = travelled(&parts[whole].moves.places, self.duration);
            }
        }
        self.mount = match &self.camera.parent {
            Some(parent) => Some(place(parent).ok_or_else(|| {
                format!("the camera rides `{parent}`, which is no object's name")
            })?),
            None => None,
        };
        self.camera_moves = CameraMoves::of(&self.camera, self.duration);
        (self.order, self.loops) = placing_order(&parts, self.mount);
        self.parts = parts;
        self.bound_joints();
        Ok(())
    }

    /// Add `object`, and the parts of the model it is a copy of, to `outs`.
    fn write_out(
        &self,
        object: &Object,
        within: Within,
        outs: &mut Vec<Out>,
    ) -> Result<(), String> {
        let Within {
            prefix,
            beside,
            scale,
            tone,
            copy,
            depth,
        } = within;
        let name = format!("{prefix}{}", object.name);
        let is_beside = |other: &str| beside.iter().any(|part| part.name == other);
        let mut out = object.clone();
        // Inside a copy, a part rides the copy unless it rides a part
        // beside it, and names those beside it without the copy's name.
        out.parent = match &object.parent {
            Some(parent) if is_beside(parent) => Some(format!("{prefix}{parent}")),
            Some(parent) => Some(parent.clone()),
            None => prefix.strip_suffix('/').map(str::to_owned),
        };
        if let Some(Target::Object(faced)) = &mut out.face
            && is_beside(faced)
        {
            *faced = format!("{prefix}{faced}");
        }
        out.name = name.clone();
        (out.model, out.parts, out.scale) = (None, Default::default(), 1.0);
        let grow = |v: &mut [f64; 3]| v.iter_mut().for_each(|n| *n *= scale);
        grow(&mut out.at);
        out.size = object.shape.map(|_| {
            let mut size = object.size.unwrap_or([1.0; 3]);
            grow(&mut size);
            size
        });
        for key in &mut out.keys {
            key.at.iter_mut().chain(&mut key.size).for_each(grow);
        }
        for key in out.cycle.iter_mut().flat_map(|cycle| &mut cycle.keys) {
            key.at.iter_mut().chain(&mut key.size).for_each(grow);
        }
        if let Some(shake) = out.shake.as_mut() {
            grow(&mut shake.at);
        }
        out.shade = object
            .shape
            .map(|_| (object.shade.unwrap_or(MID_GREY) * tone).clamp(0.0, 1.0));
        let index = outs.len();
        if index >= MAX_PARTS {
            return Err(format!(
                "more than {MAX_PARTS} parts once every copy is written out: build them of fewer"
            ));
        }
        outs.push(Out {
            object: out,
            inside: copy,
            is_copy: object.model.is_some(),
        });
        let Some(model) = &object.model else {
            return Ok(());
        };
        let Some(parts) = self.models.get(model) else {
            return Err(format!(
                "`{name}` is a copy of `{model}`, which is no model's name"
            ));
        };
        if depth >= MAX_NESTING {
            return Err(format!(
                "`{name}`: models hold copies of models more than {MAX_NESTING} deep"
            ));
        }
        if let Some(stray) =
            (object.parts.keys()).find(|part| parts.iter().all(|p| p.name != **part))
        {
            return Err(format!(
                "`{name}` changes a part `{stray}`, which model `{model}` doesn't have"
            ));
        }
        let within = Within {
            prefix: &format!("{name}/"),
            beside: parts,
            scale: scale * object.scale,
            tone: tone * object.shade.map_or(1.0, |shade| shade / MID_GREY),
            copy: Some(index),
            depth: depth + 1,
        };
        for part in parts {
            let part = match object.parts.get(&part.name) {
                Some(changes) => changed(part, changes).map_err(|e| format!("`{name}`: {e}"))?,
                None => part.clone(),
            };
            self.write_out(&part, within, outs)?;
        }
        Ok(())
    }

    /// Give each joint and copy the box that holds what rides it as the
    /// scene starts, in its own space.
    fn bound_joints(&mut self) {
        let places = self.stand(0.0).0;
        let mut boxes: Vec<Option<(Vec3, Vec3)>> = vec![None; self.parts.len()];
        for (index, part) in self.parts.iter().enumerate() {
            if part.object.shape.is_none() {
                continue;
            }
            let (low, high) = (Vec3::of(part.bounds.0), Vec3::of(part.bounds.1));
            let corners: [Vec3; 8] = std::array::from_fn(|i| {
                let pick =
                    |bit: usize, low: f32, high: f32| if i >> bit & 1 == 0 { low } else { high };
                places[index].point(Vec3::new(
                    pick(0, low.x, high.x),
                    pick(1, low.y, high.y),
                    pick(2, low.z, high.z),
                ))
            });
            let mut above = part.parent;
            while let Some(joint) = above {
                if self.parts[joint].object.shape.is_none() {
                    for corner in corners {
                        let own = places[joint].turn.inward(corner - places[joint].at);
                        let (low, high) = boxes[joint].unwrap_or((own, own));
                        boxes[joint] = Some((low.min(own), high.max(own)));
                    }
                }
                above = self.parts[joint].parent;
            }
        }
        let plain = |v: Vec3| [v.x as f64, v.y as f64, v.z as f64];
        for (part, held) in self.parts.iter_mut().zip(boxes) {
            if let Some((low, high)) = held {
                part.bounds = (plain(low), plain(high));
            }
        }
    }
}

/// `part` of a model with `changes`, the fields a copy gives in its place.
fn changed(
    part: &Object,
    changes: &serde_json::Map<String, serde_json::Value>,
) -> Result<Object, String> {
    let wrong = |e: serde_json::Error| format!("part `{}`: {e}", part.name);
    let serde_json::Value::Object(mut fields) = serde_json::to_value(part).map_err(wrong)? else {
        return Ok(part.clone());
    };
    for (field, value) in changes.iter().filter(|(field, _)| *field != "name") {
        fields.insert(field.clone(), value.clone());
    }
    let mut changed: Object = serde_json::from_value(fields.into()).map_err(wrong)?;
    changed.settle()?;
    Ok(changed)
}

/// The part at the top of what `index` rides; refused when it rides
/// itself by way of others.
fn top_parent(parts: &[Part], index: usize) -> Result<usize, String> {
    let mut top = index;
    for _ in 0..=parts.len() {
        match parts[top].parent {
            Some(parent) => top = parent,
            None => return Ok(top),
        }
    }
    Err(format!(
        "`{}` rides itself by way of its `parent`",
        parts[index].object.name
    ))
}

/// The box a solid fills in its own space: around its anchor.
fn solid_bounds(object: &Object) -> ([f64; 3], [f64; 3]) {
    let [w, h, d] = object.size.unwrap_or([0.0; 3]);
    let under = match object.anchor {
        Anchor::Base => 0.0,
        Anchor::Middle => -h * 0.5,
        Anchor::Top => -h,
    };
    ([-w * 0.5, under, -d * 0.5], [w * 0.5, under + h, d * 0.5])
}

/// The order parts are placed in at a moment: each after what it rides,
/// and after what it faces, unless facing goes round in a loop; whether
/// some did. `mount` is the part the camera rides, which whatever faces
/// the camera is placed after.
fn placing_order(parts: &[Part], mount: Option<usize>) -> (Vec<usize>, bool) {
    let faced = |part: &Part| match part.facing {
        Facing::Part(faced) => Some(faced),
        Facing::Camera => mount,
        Facing::Nothing | Facing::Path | Facing::Point(_) => None,
    };
    let mut waits = vec![0usize; parts.len()];
    let mut waited_on: Vec<Vec<usize>> = vec![Vec::new(); parts.len()];
    for (index, part) in parts.iter().enumerate() {
        for other in [part.parent, faced(part)].into_iter().flatten() {
            if other != index {
                waits[index] += 1;
                waited_on[other].push(index);
            }
        }
    }
    let mut ready: VecDeque<usize> = (0..parts.len()).filter(|i| waits[*i] == 0).collect();
    let mut placed = vec![false; parts.len()];
    let mut order = Vec::with_capacity(parts.len());
    let mut loops = false;
    while order.len() < parts.len() {
        let Some(index) = ready.pop_front().or_else(|| {
            // All that is left waits on a loop of facing: break it at a
            // part whose parent is placed, which then faces as best it can.
            loops = true;
            (0..parts.len()).find(|i| !placed[*i] && parts[*i].parent.is_none_or(|p| placed[p]))
        }) else {
            break;
        };
        if std::mem::replace(&mut placed[index], true) {
            continue;
        }
        order.push(index);
        for &waiting in &waited_on[index] {
            waits[waiting] -= 1;
            if waits[waiting] == 0 && !placed[waiting] {
                ready.push_back(waiting);
            }
        }
    }
    (order, loops)
}

/// Metres the keys of `places` have moved a thing over the ground by each
/// frame of `duration` seconds.
fn travelled(places: &Track, duration: f64) -> Vec<f32> {
    let frames = (duration * FPS as f64).ceil() as usize + 1;
    let mut gone = 0.0;
    let mut was = places.at(0.0);
    (0..=frames)
        .map(|frame| {
            let is = places.at(frame as f64 / FPS as f64);
            gone += (is[0] - was[0]).hypot(is[2] - was[2]);
            was = is;
            gone as f32
        })
        .collect()
}
