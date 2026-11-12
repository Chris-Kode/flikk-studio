//! Writing a scene out into the parts that are drawn. Every copy of a
//! model becomes its parts, named `copy/part`, scaled with the copy and
//! with what the copy does its own way put in; names are swapped for
//! places in the list; and what the motion needs every frame (how each
//! part's keys move it, the order parts are placed in, the box each thing
//! fills, the ground each has covered) is worked out once.

use super::motion::{Moves, Rig, Track, rest_place};
use super::space::{Place, Vec3};
use super::{
    AHEAD, Anchor, CAMERA, Ease, FPS, GROUND, MAIN, MAX_PARTS, MID_GREY, NONE, Object, PATH, Scene,
    Target,
};
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
    /// What its front turns to, when it says, and what its keys have it
    /// turn to later.
    pub aims: Vec<(f64, AimAt, Ease)>,
    /// What its keys have it go with, in order of time.
    pub held: Vec<Held>,
    /// The limb it is the upper part of, when the end of one is brought
    /// somewhere.
    pub limb: Option<Limb>,
    /// What it stands on, when it says.
    pub rest: Option<Rest>,
    /// How its keys move it.
    pub moves: Moves,
    /// The box it fills in its own space, low corner and high. For a
    /// joint or a copy: what rides it, as the scene starts.
    pub bounds: ([f64; 3], [f64; 3]),
    /// Metres its keys have moved it over the ground by each frame; empty
    /// when no motion asks.
    pub travelled: Vec<f32>,
    /// The strides its feet that step have taken by each frame; empty
    /// when it has none.
    pub strides: Vec<f32>,
    /// How far into their rounds its cycle and its steps start, on top
    /// of what they say: each one of a repeat its own.
    pub stagger: f64,
    /// The repeat it is one of: the joint that stands for them all.
    pub one_of: Option<usize>,
    /// Where it stands and its size, when nothing ever moves it: worked
    /// out once.
    pub still: Option<(Place, Vec3)>,
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

/// What a part's front turns to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum AimAt {
    /// Where it faces anyway.
    Ahead,
    Camera,
    Part(usize),
    Point([f64; 3]),
}

/// A part going with another from a time on, or with nothing.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Held {
    pub t: f64,
    /// What it goes with; `None` once let go.
    pub by: Option<usize>,
    /// Where it is within that (in the scene, once let go), and where it
    /// was in the scene at the moment. Not known until the scene is built
    /// that far.
    pub at: Option<(Place, Place)>,
}

/// A limb of two parts whose end is brought somewhere: this part, the
/// one that rides it, and the end that rides that.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Limb {
    pub lower: usize,
    pub end: usize,
    pub goal: Goal,
}

/// Where a limb's end is brought.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Goal {
    /// To something, from one time until another: the start and the end
    /// where none is given.
    Reach {
        to: AimAt,
        from: Option<f64>,
        until: Option<f64>,
    },
    /// To where a foot that steps is.
    Step(Stepping),
}

/// How a foot steps.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stepping {
    /// The thing that travels: what the foot is part of.
    pub root: usize,
    /// The strides it has taken by each frame, and how far into one it
    /// starts.
    pub strides: Vec<f32>,
    pub shift: f64,
    /// How long its leg is, and how high it is lifted as it swings.
    pub leg: f32,
    pub lift: f32,
    /// Where it stands under that thing, to its right and ahead of it.
    pub rest: (f32, f32),
}

impl Limb {
    /// How its end steps, when it is a foot that does.
    pub fn steps(&self) -> Option<&Stepping> {
        match &self.goal {
            Goal::Step(stepping) => Some(stepping),
            Goal::Reach { .. } => None,
        }
    }
}

/// What a part stands on, and what of it could touch that.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Rest {
    /// The solids of what it stands on; none for the ground.
    pub surface: Vec<usize>,
    /// Its own solids: itself and all that rides it.
    pub solids: Vec<usize>,
    /// The upper parts of its legs that step.
    pub legs: Vec<usize>,
}

/// An object written out, with the copy of a model it is in and whether
/// it is one itself.
struct Out {
    object: Object,
    inside: Option<usize>,
    is_copy: bool,
    stagger: f64,
    one_of: Option<usize>,
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
    /// How far into their rounds its cycles and steps start.
    stagger: f64,
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
            stagger: 0.0,
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
            let aim = |target: &Target| match target {
                Target::Point(point) => Ok(AimAt::Point(*point)),
                Target::Object(aimed) if aimed == AHEAD => Ok(AimAt::Ahead),
                Target::Object(aimed) if aimed == CAMERA => Ok(AimAt::Camera),
                Target::Object(aimed) => (place(aimed).map(AimAt::Part)).ok_or_else(|| {
                    format!(
                        "`{name}` aims at `{aimed}`, which is no object's name, `{CAMERA}` or \
                         `{AHEAD}`"
                    )
                }),
            };
            let moves = Moves::of(&out.object, self.duration);
            // From the start it aims at what it says, or ahead until a key
            // says.
            let keyed = (moves.times.iter().zip(&out.object.keys))
                .filter_map(|(t, key)| Some((*t, key.aim.as_ref()?, key.ease)));
            let first = (out.object.aim.as_ref()).map(|target| (0.0, target, Ease::Linear));
            let mut aims = first
                .into_iter()
                .chain(keyed)
                .map(|(t, target, ease)| Ok((t, aim(target)?, ease)))
                .collect::<Result<Vec<_>, String>>()?;
            if aims.first().is_some_and(|(t, ..)| *t > 0.0) {
                aims.insert(0, (0.0, AimAt::Ahead, Ease::Linear));
            }
            parts.push(Part {
                object: out.object.clone(),
                parent,
                whole: 0,
                thing: 0,
                facing,
                aims,
                held: (moves.times.iter().zip(&out.object.keys))
                    .filter_map(|(t, key)| Some((*t, key.parent.as_ref()?)))
                    .map(|(t, by)| {
                        let by = match by.as_str() {
                            NONE => None,
                            by => Some(place(by).ok_or_else(|| {
                                format!(
                                    "a key has `{name}` go with `{by}`, which is no object's name \
                                     or `{NONE}`"
                                )
                            })?),
                        };
                        Ok(Held { t, by, at: None })
                    })
                    .collect::<Result<_, String>>()?,
                limb: None,
                rest: None,
                moves,
                bounds: solid_bounds(&out.object),
                travelled: Vec::new(),
                strides: Vec::new(),
                stagger: out.stagger,
                one_of: out.one_of,
                still: None,
            });
        }
        for index in 0..parts.len() {
            let top = top_parent(&parts, index)?;
            let whole = match (&outs[index], outs[index].inside) {
                (out, _) if out.is_copy => index,
                (_, Some(copy)) => copy,
                // One of a repeat that rides nothing is a thing itself.
                (out, None) if out.one_of == Some(top) => index,
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
        for end in 0..parts.len() {
            let Some(reach) = &parts[end].object.reach else {
                continue;
            };
            let name = &parts[end].object.name;
            let to = match &reach.to {
                Target::Point(point) => AimAt::Point(*point),
                Target::Object(to) if to == CAMERA => AimAt::Camera,
                Target::Object(to) => AimAt::Part(place(to).ok_or_else(|| {
                    format!("`{name}` reaches to `{to}`, which is no object's name or `{CAMERA}`")
                })?),
            };
            let limb = Limb {
                lower: 0,
                end,
                goal: Goal::Reach {
                    to,
                    from: reach.from,
                    until: reach.until,
                },
            };
            hang_limb(&mut parts, limb)?;
        }
        let rig = |camera, what: &str| {
            Rig::of(camera, what, self.duration, place, |whole| {
                solids_of(&parts, whole)
            })
        };
        let mut rigs = vec![rig(&self.camera, "the camera")?];
        for (name, camera) in &self.cameras {
            rigs.push(rig(camera, &format!("camera `{name}`"))?);
        }
        let named = |name: &str| match name {
            MAIN => Some(0),
            name => self
                .cameras
                .keys()
                .position(|other| other == name)
                .map(|i| i + 1),
        };
        let mut live = Vec::with_capacity(self.cuts.len());
        for cut in &self.cuts {
            let Some(rig) = named(&cut.camera) else {
                return Err(format!(
                    "a cut goes to camera `{}`, which is none of `cameras` or `{MAIN}`",
                    cut.camera
                ));
            };
            live.push((cut.t, rig));
        }
        for index in 0..parts.len() {
            let Some(stood_on) = &parts[index].object.on else {
                continue;
            };
            let name = &parts[index].object.name;
            let surface = match place(stood_on) {
                Some(surface) if rides(&parts, surface, index) => {
                    return Err(format!(
                        "`{name}` stands on `{stood_on}`, which is part of it"
                    ));
                }
                Some(surface) => solids_of(&parts, surface),
                None if stood_on == GROUND => Vec::new(),
                None => {
                    return Err(format!(
                        "`{name}` stands on `{stood_on}`, which is no object's name or `{GROUND}`"
                    ));
                }
            };
            parts[index].rest = Some(Rest {
                surface,
                solids: solids_of(&parts, index),
                legs: Vec::new(),
            });
        }
        self.hang_legs(&mut parts)?;
        let mounts: Vec<usize> = rigs.iter().filter_map(|rig| rig.mount).collect();
        let in_step = |part: &Part| {
            let paced = part
                .object
                .cycle
                .as_ref()
                .is_some_and(|cycle| cycle.strides.is_some());
            paced && parts[part.whole].strides.is_empty()
        };
        if let Some(part) = parts.iter().find(|part| in_step(part)) {
            return Err(format!(
                "`{}` has a `cycle` by `strides`, and nothing of `{}` steps: give its feet `step`",
                part.object.name, parts[part.whole].object.name
            ));
        }
        let (order, loops) = placing_order(&parts, &mounts);
        let stepping = |part: &Part| matches!(&part.limb, Some(limb) if limb.steps().is_some());
        self.rough = loops
            || parts
                .iter()
                .any(|part| part.rest.is_some() || stepping(part));
        self.order = order;
        (self.rigs, self.live) = (rigs, live);
        self.parts = parts;
        self.bound_joints();
        self.take_hold();
        self.keep_still();
        Ok(())
    }

    /// Work out once where each part stands that nothing ever moves:
    /// most of a set.
    fn keep_still(&mut self) {
        let standing = self.stand(0.0);
        for index in self.order.clone() {
            let part = &self.parts[index];
            let object = &part.object;
            let keyed = (object.keys.iter())
                .any(|key| key.at.is_some() || key.rotate.is_some() || key.size.is_some());
            let moved = keyed
                || object.spin != [0.0; 3]
                || object.cycle.is_some()
                || object.shake.is_some()
                || part.facing != Facing::Nothing
                || !part.aims.is_empty()
                || !part.held.is_empty()
                || part.limb.is_some()
                || part.rest.is_some();
            let carried = part
                .parent
                .is_some_and(|parent| self.parts[parent].still.is_none());
            if !moved && !carried {
                self.parts[index].still = Some((standing.places[index], standing.sizes[index]));
            }
        }
    }

    /// Give each leg whose foot steps its limb: how long a stride is and
    /// which foot goes first, where they don't say, and where the foot
    /// stands under the thing it is part of.
    fn hang_legs(&self, parts: &mut [Part]) -> Result<(), String> {
        let feet: Vec<usize> = (0..parts.len())
            .filter(|part| parts[*part].object.step.is_some())
            .collect();
        for &end in &feet {
            let root = parts[end].whole;
            let Some(step) = parts[end].object.step.clone() else {
                continue;
            };
            if root == end {
                return Err(format!(
                    "`{}` steps, so it is part of a thing that travels: a foot of a model, or \
                     riding what does the travelling",
                    parts[end].object.name
                ));
            }
            let above = |part: usize| parts[part].parent.unwrap_or(part);
            let (lower, upper) = (above(end), above(above(end)));
            let stands = |part: usize| rest_place(parts, part, root).at;
            let (foot, knee, hip) = (stands(end), stands(lower), stands(upper));
            let leg = ((knee - hip).length() + (foot - knee).length()) as f64;
            // The feet of one thing take turns, in the order written.
            let beside: Vec<usize> = (feet.iter().copied())
                .filter(|other| parts[*other].whole == root)
                .collect();
            let turn = beside.iter().position(|other| *other == end).unwrap_or(0);
            if parts[root].travelled.is_empty() {
                parts[root].travelled = travelled(&parts[root].moves.places, self.duration);
            }
            let taken = strides(&parts[root].travelled, step.metres, leg);
            if parts[root].strides.is_empty() {
                parts[root].strides = taken.clone();
            }
            let limb = Limb {
                lower,
                end,
                goal: Goal::Step(Stepping {
                    root,
                    strides: taken,
                    shift: step.shift.unwrap_or(turn as f64 / beside.len() as f64)
                        + parts[end].stagger,
                    leg: leg as f32,
                    lift: step.lift.map_or(leg as f32 * 0.12, |lift| lift as f32),
                    rest: (foot.x, foot.z),
                }),
            };
            hang_limb(parts, limb)?;
            if let Some(rest) = parts[root].rest.as_mut() {
                rest.legs.push(upper);
            }
        }
        Ok(())
    }

    /// Add `object`, and the parts of the model it is a copy of, to `outs`.
    fn write_out(
        &self,
        object: &Object,
        within: Within,
        outs: &mut Vec<Out>,
    ) -> Result<(), String> {
        if let Some(copies) = object.repeated() {
            return self.write_repeat(object, copies, within, outs);
        }
        let Within {
            prefix,
            beside,
            scale,
            tone,
            copy,
            depth,
            stagger,
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
        // A part names those beside it without the copy's name.
        let in_full = |named: &mut String| {
            if is_beside(named) {
                *named = format!("{prefix}{named}");
            }
        };
        let targets = (out.face.iter_mut())
            .chain(&mut out.aim)
            .chain(out.reach.iter_mut().map(|reach| &mut reach.to))
            .chain(out.keys.iter_mut().filter_map(|key| key.aim.as_mut()));
        for target in targets {
            if let Target::Object(named) = target {
                in_full(named);
            }
        }
        out.on.iter_mut().for_each(in_full);
        (out.keys.iter_mut())
            .filter_map(|key| key.parent.as_mut())
            .for_each(in_full);
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
        if let Some(step) = out.step.as_mut() {
            step.metres
                .iter_mut()
                .chain(&mut step.lift)
                .for_each(|n| *n *= scale);
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
            stagger,
            one_of: None,
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
            stagger,
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

    /// Add the `copies` that `object` repeats as, each riding a joint of
    /// its name that stands for them all.
    fn write_repeat(
        &self,
        object: &Object,
        copies: Vec<(Object, f64)>,
        within: Within,
        outs: &mut Vec<Out>,
    ) -> Result<(), String> {
        let all = outs.len();
        let joint = Object::joint(object.name.clone(), object.parent.clone());
        self.write_out(&joint, within, outs)?;
        for (copy, stagger) in copies {
            let index = outs.len();
            let within = Within {
                stagger: within.stagger + stagger,
                ..within
            };
            self.write_out(&copy, within, outs)?;
            outs[index].one_of = Some(all);
        }
        Ok(())
    }

    /// Work out where each part that goes with another is within it: where
    /// it was at the moment it was taken, each in order of time, the scene
    /// built as far as the one before.
    fn take_hold(&mut self) {
        let mut taken: Vec<(f64, usize, usize)> = (self.parts.iter().enumerate())
            .flat_map(|(part, held)| (0..held.held.len()).map(move |i| (held.held[i].t, part, i)))
            .collect();
        taken.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (t, part, hold) in taken {
            let places = self.stand(t).places;
            let within = match self.parts[part].held[hold].by {
                Some(by) => places[by].holds(places[part]),
                None => places[part],
            };
            self.parts[part].held[hold].at = Some((within, places[part]));
        }
    }

    /// Give each joint and copy the box that holds what rides it as the
    /// scene starts, in its own space.
    fn bound_joints(&mut self) {
        let places = self.stand(0.0).places;
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

/// Give the part two above the end of `limb` the limb: the end rides
/// its lower part, and that its upper. Refused where there are not two
/// parts above it, or the upper already has a limb.
fn hang_limb(parts: &mut [Part], mut limb: Limb) -> Result<(), String> {
    let name = &parts[limb.end].object.name;
    let above = |part: usize| parts[part].parent;
    let (Some(lower), Some(upper)) = (above(limb.end), above(limb.end).and_then(above)) else {
        return Err(format!(
            "`{name}` is brought somewhere as the end of a limb, so it rides a part that rides \
             another: a hand its forearm, and that its upper arm"
        ));
    };
    if let Some(other) = &parts[upper].limb {
        return Err(format!(
            "`{name}` and `{}` are both brought somewhere by bending `{}`: one limb has one end",
            parts[other.end].object.name, parts[upper].object.name
        ));
    }
    limb.lower = lower;
    parts[upper].limb = Some(limb);
    Ok(())
}

/// Whether part `index` is `whole`, or rides it.
fn rides(parts: &[Part], index: usize, whole: usize) -> bool {
    let mut above = Some(index);
    while let Some(part) = above {
        if part == whole {
            return true;
        }
        above = parts[part].parent;
    }
    false
}

/// The solids of part `whole`: itself and all that rides it.
fn solids_of(parts: &[Part], whole: usize) -> Vec<usize> {
    (0..parts.len())
        .filter(|index| parts[*index].object.shape.is_some() && rides(parts, *index, whole))
        .collect()
}

/// The order parts are placed in at a moment: each after what it rides,
/// and after what it faces and stands on, unless that goes round in a
/// loop; whether some did. `mounts` are the parts cameras ride, which
/// whatever faces the camera is placed after.
fn placing_order(parts: &[Part], mounts: &[usize]) -> (Vec<usize>, bool) {
    let faced = |part: &Part| {
        let mut faced = match part.facing {
            Facing::Part(faced) => vec![faced],
            Facing::Camera => mounts.to_vec(),
            Facing::Nothing | Facing::Path | Facing::Point(_) => Vec::new(),
        };
        let reached = part.limb.iter().filter_map(|limb| match &limb.goal {
            Goal::Reach { to, .. } => Some(to),
            Goal::Step(_) => None,
        });
        faced.extend(part.held.iter().filter_map(|held| held.by));
        // A leg that steps comes down on what its thing stands on.
        let trodden = (part.limb.iter())
            .filter_map(|limb| parts[limb.steps()?.root].rest.as_ref())
            .flat_map(|rest| rest.surface.iter().copied());
        faced.extend(trodden);
        for aim in part.aims.iter().map(|(_, aim, _)| aim).chain(reached) {
            match aim {
                AimAt::Part(aimed) => faced.push(*aimed),
                AimAt::Camera => faced.extend(mounts),
                AimAt::Ahead | AimAt::Point(_) => {}
            }
        }
        faced
    };
    let mut waits = vec![0usize; parts.len()];
    let mut waited_on: Vec<Vec<usize>> = vec![Vec::new(); parts.len()];
    for (index, part) in parts.iter().enumerate() {
        let stood_on = part
            .rest
            .iter()
            .flat_map(|rest| rest.surface.iter().copied());
        for other in (part.parent.into_iter().chain(faced(part))).chain(stood_on) {
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

/// The strides a leg `leg` metres long has taken by each frame, the
/// thing it carries having `travelled` so far by each. A stride is
/// `metres` when given. Else it is as long as legs of that length take at
/// the speed: longer the faster, as Alexander measured of animals from
/// their tracks (2.3 leg lengths times the 0.3rd power of speed squared
/// over gravity and the leg's length): 1.3 m at a walk, 2.4 m at a run.
fn strides(travelled: &[f32], metres: Option<f64>, leg: f64) -> Vec<f32> {
    let mut taken = 0.0;
    let mut strides = Vec::with_capacity(travelled.len());
    strides.push(0.0);
    for pair in travelled.windows(2) {
        let covered = (pair[1] - pair[0]) as f64;
        let speed = covered * FPS as f64;
        let stride = metres.unwrap_or_else(|| {
            let froude = speed * speed / (9.81 * leg.max(1e-3));
            (2.3 * leg * froude.powf(0.3)).max(0.4 * leg)
        });
        taken += covered / stride.max(0.02);
        strides.push(taken as f32);
    }
    strides
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
