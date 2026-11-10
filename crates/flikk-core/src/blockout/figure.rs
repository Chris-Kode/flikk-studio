//! Jointed bodies: a person and a four-legged animal, each a handful of
//! round pieces posed from joint angles. Their legs are paced by how far
//! the object has travelled, so feet keep up with the ground, and a
//! person's [`Action`] poses the rest.
//!
//! A body faces +Z. Its `+X` side is side 0, its `-X` side is side 1.

use super::render::{Form, Piece};
use super::space::{Place, Turn, Vec3, mix, smoothstep};
use super::{Action, Object, Scene, Shape, kit};
use std::f32::consts::TAU;

/// The height a person's speeds and sizes are worked out at, in metres.
const TALL: f32 = 1.75;
/// Cycles a second of a walk and of a run on the spot.
const WALK_ON_SPOT: f32 = 0.95;
const RUN_ON_SPOT: f32 = 1.45;
/// Below this speed, for a body of [`TALL`], it is on the spot.
const STILL: f32 = 0.3;

/// Where a limb's two bones point: each a swing forward and a spread
/// outward, in degrees, from hanging straight down.
#[derive(Clone, Copy)]
struct Limb {
    upper: (f32, f32),
    lower: (f32, f32),
}

impl Limb {
    const fn new(upper: (f32, f32), lower: (f32, f32)) -> Self {
        Self { upper, lower }
    }

    fn blend(self, to: Self, u: f32) -> Self {
        let pair = |a: (f32, f32), b: (f32, f32)| (mix(a.0, b.0, u), mix(a.1, b.1, u));
        Self::new(pair(self.upper, to.upper), pair(self.lower, to.lower))
    }
}

/// The way a bone points when swung `forward` and spread outward on
/// `side` (1 for +X, -1 for -X), from straight down.
fn swing((forward, spread): (f32, f32), side: f32) -> Vec3 {
    let ((sf, cf), (ss, cs)) = (
        forward.to_radians().sin_cos(),
        spread.to_radians().sin_cos(),
    );
    Vec3::new(side * ss, -cs * cf, cs * sf)
}

/// How a person holds itself: angles in degrees.
#[derive(Clone, Copy)]
struct Pose {
    /// The spine tipped forward, and the shoulders turned about it.
    lean: f32,
    twist: f32,
    /// The head turned down and to side 0.
    nod: f32,
    look: f32,
    /// How far off the ground its feet are, as a share of its height.
    air: f32,
    /// How far it is laid on its back, 0..1.
    laid: f32,
    arms: [Limb; 2],
    legs: [Limb; 2],
}

impl Pose {
    fn blend(self, to: Self, u: f32) -> Self {
        Self {
            lean: mix(self.lean, to.lean, u),
            twist: mix(self.twist, to.twist, u),
            nod: mix(self.nod, to.nod, u),
            look: mix(self.look, to.look, u),
            air: mix(self.air, to.air, u),
            laid: mix(self.laid, to.laid, u),
            arms: [0, 1].map(|i| self.arms[i].blend(to.arms[i], u)),
            legs: [0, 1].map(|i| self.legs[i].blend(to.legs[i], u)),
        }
    }

    /// Standing at ease, with the small drift of someone alive.
    fn standing(t: f32) -> Self {
        let arm = Limb::new((3.0, 8.0), (14.0, 5.0));
        let leg = Limb::new((0.0, 4.0), (0.0, 2.0));
        Self {
            lean: 1.0 + 0.8 * (t * 1.1).sin(),
            twist: 1.5 * (t * 0.6).sin(),
            nod: 0.0,
            look: 0.0,
            air: 0.0,
            laid: 0.0,
            arms: [arm; 2],
            legs: [leg; 2],
        }
    }

    /// A stride `phase` cycles in: `run` 0 is a walk, 1 a run.
    fn striding(phase: f32, run: f32) -> Self {
        let pick = |walk: f32, running: f32| mix(walk, running, run);
        let leg = |turn: f32| {
            // The knee folds as the leg swings through to the front.
            let fold = (turn + pick(0.5, 0.6)).cos().max(0.0);
            let hip = pick(24.0, 38.0) * turn.sin() + pick(2.0, 12.0);
            let knee = pick(4.0, 18.0) + pick(52.0, 85.0) * fold * fold;
            Limb::new((hip, 3.0), (hip - knee, 2.0))
        };
        // An arm swings against the leg of its side.
        let arm = |turn: f32| {
            let shoulder = -pick(16.0, 40.0) * turn.sin() + pick(2.0, -4.0);
            Limb::new((shoulder, 7.0), (shoulder + pick(18.0, 85.0), 4.0))
        };
        let turn = phase * TAU;
        Self {
            lean: pick(4.0, 14.0),
            twist: pick(7.0, 12.0) * turn.sin(),
            nod: 0.0,
            look: 0.0,
            // A run leaves the ground twice a stride, legs at their widest.
            air: run * 0.035 * (-(2.0 * turn).cos()).max(0.0),
            laid: 0.0,
            arms: [arm(turn), arm(turn + TAU / 2.0)],
            legs: [leg(turn), leg(turn + TAU / 2.0)],
        }
    }
}

/// What a pose is worked out from at one moment.
struct Moment {
    t: f32,
    /// How far through its strides it is, in cycles.
    phase: f32,
    /// Its speed over the ground, as a person of [`TALL`] would go.
    pace: f32,
}

impl Moment {
    /// Standing, walking or running, as fast as it goes.
    fn legs(&self) -> Pose {
        let stride = Pose::striding(self.phase, smoothstep(2.2, 3.4, self.pace));
        Pose::standing(self.t).blend(stride, smoothstep(0.08, 0.5, self.pace))
    }

    /// A wave of `rate` cycles a second, -1..1, `ahead` cycles on.
    fn beat(&self, rate: f32, ahead: f32) -> f32 {
        ((self.t * rate + ahead) * TAU).sin()
    }

    fn pose(&self, action: Action) -> Pose {
        let standing = Pose::standing(self.t);
        let both = |limb: Limb| [limb; 2];
        match action {
            Action::Auto => self.legs(),
            Action::Stand => standing,
            Action::Walk => Pose::striding(self.phase, 0.0),
            Action::Run => Pose::striding(self.phase, 1.0),
            Action::Sit => Pose {
                lean: -3.0,
                arms: both(Limb::new((30.0, 9.0), (72.0, 0.0))),
                legs: both(Limb::new((84.0, 6.0), (4.0, 3.0))),
                ..standing
            },
            Action::Ride => Pose {
                lean: 20.0,
                nod: -14.0,
                arms: both(Limb::new((52.0, 10.0), (74.0, 2.0))),
                legs: both(Limb::new((58.0, 20.0), (-28.0, 12.0))),
                ..standing
            },
            Action::Crouch => Pose {
                lean: 30.0,
                nod: -18.0,
                arms: both(Limb::new((42.0, 10.0), (98.0, 0.0))),
                legs: both(Limb::new((100.0, 12.0), (-40.0, 6.0))),
                ..standing
            },
            Action::Lie => Pose {
                laid: 1.0,
                arms: both(Limb::new((0.0, 14.0), (4.0, 12.0))),
                legs: both(Limb::new((0.0, 6.0), (0.0, 5.0))),
                ..standing
            },
            Action::Jump => Pose {
                lean: 6.0,
                arms: both(Limb::new((150.0, 18.0), (166.0, 10.0))),
                legs: both(Limb::new((40.0, 8.0), (-46.0, 4.0))),
                ..standing
            },
            Action::Dance => {
                let limb = |ahead: f32| {
                    let (up, bounce) = (self.beat(1.0, ahead), self.beat(2.0, 0.0).max(0.0));
                    let arm = Limb::new((52.0 + 26.0 * up, 28.0), (128.0 + 22.0 * up, 10.0));
                    let knee = 16.0 + 20.0 * bounce;
                    (arm, Limb::new((knee * 0.55, 9.0), (-knee * 0.6, 4.0)))
                };
                let (near, far) = (limb(0.0), limb(0.5));
                Pose {
                    lean: 4.0,
                    twist: 16.0 * self.beat(1.0, 0.0),
                    look: 10.0 * self.beat(0.5, 0.2),
                    arms: [near.0, far.0],
                    legs: [near.1, far.1],
                    ..standing
                }
            }
            Action::Cheer => {
                let pump = self.beat(1.7, 0.0);
                let knee = 8.0 + 10.0 * (-pump).max(0.0);
                Pose {
                    nod: -8.0,
                    air: 0.025 * pump.max(0.0),
                    arms: both(Limb::new((166.0 + 8.0 * pump, 20.0), (174.0, 12.0))),
                    legs: both(Limb::new((knee * 0.5, 5.0), (-knee * 0.5, 3.0))),
                    ..standing
                }
            }
            Action::Wave => {
                let mut pose = self.legs();
                let hand = 164.0 + 22.0 * self.beat(1.8, 0.0);
                pose.arms[1] = Limb::new((8.0, 104.0), (4.0, hand));
                pose
            }
            Action::Point => {
                let mut pose = self.legs();
                pose.arms[1] = Limb::new((86.0, 5.0), (89.0, 2.0));
                pose
            }
            Action::Talk => {
                let mut pose = self.legs();
                let arm = |ahead: f32| {
                    let lift = 96.0 + 20.0 * self.beat(0.9, ahead);
                    let out = 8.0 + 9.0 * self.beat(0.6, ahead + 0.3);
                    Limb::new((22.0 + 6.0 * self.beat(0.7, ahead), 12.0), (lift, out))
                };
                pose.arms = [arm(0.0), arm(0.4)];
                pose.nod = 4.0 * self.beat(1.3, 0.0);
                pose
            }
            Action::Carry => {
                let mut pose = self.legs();
                pose.arms = both(Limb::new((18.0, 10.0), (100.0, -5.0)));
                pose
            }
            Action::Push => {
                let mut pose = self.legs();
                pose.arms = both(Limb::new((70.0, 8.0), (86.0, 0.0)));
                (pose.lean, pose.twist) = (24.0, pose.twist * 0.3);
                pose
            }
            Action::Reach => {
                let mut pose = self.legs();
                pose.arms[1] = Limb::new((168.0, 10.0), (175.0, 4.0));
                pose.nod = -22.0;
                pose
            }
            Action::Fight => {
                let mut pose = self.legs();
                let arm = |ahead: f32| {
                    let jab = self.beat(1.1, ahead).max(0.0).powi(4);
                    let guard = Limb::new((55.0, 22.0), (150.0, 10.0));
                    guard.blend(Limb::new((92.0, 4.0), (92.0, 0.0)), jab)
                };
                pose.arms = [arm(0.0), arm(0.5)];
                (pose.lean, pose.twist) = (pose.lean + 8.0, 18.0 * self.beat(1.1, 0.1));
                pose
            }
        }
    }
}

/// The pieces of a person `size` large doing what `object` does at `t`.
/// `seat` is where the seat it rides is, in its own space: seated, it is
/// put there.
pub(super) fn person(
    object: &Object,
    size: Vec3,
    t: f64,
    seat: Option<Vec3>,
    out: &mut Vec<Piece>,
) {
    let tall = size.y;
    let scaled = TALL / tall.max(0.01);
    let phase = object.paced(t, |speed, at| {
        let pace = speed * scaled;
        match object.action_now(at).1 {
            Action::Walk if pace < STILL => WALK_ON_SPOT,
            Action::Run if pace < STILL => RUN_ON_SPOT,
            _ => speed / (tall * (0.5 + 0.25 * pace)),
        }
    });
    let moment = Moment {
        t: t as f32,
        phase,
        pace: object.speed(t) * scaled,
    };
    let (was, is, begun) = object.action_now(t);
    let pose = moment.pose(was).blend(moment.pose(is), begun);
    let seated = |action| matches!(action, Action::Sit | Action::Ride) as u8 as f32;
    let seat = seat.map(|seat| (seat, mix(seated(was), seated(is), begun)));
    build_person(&pose, size, seat, out);
}

/// The solids of a person in `pose`.
fn build_person(pose: &Pose, size: Vec3, seat: Option<(Vec3, f32)>, out: &mut Vec<Piece>) {
    let h = size.y;
    // How thick it is, against a person of the usual build.
    let (wide, deep) = (size.x / 0.5, size.z / 0.5);
    let thick = (wide * deep).sqrt().min(wide.min(deep) * 1.5) * (h / TALL);
    let (thigh, shin, ankle) = (0.25 * h, 0.235 * h, 0.045 * h);
    let (spine, neck) = (0.29 * h, 0.105 * h);
    let (upper_arm, forearm) = (0.165 * h, 0.15 * h);
    let (hip, shoulder) = (0.085 * wide * h / TALL, 0.185 * wide * h / TALL);

    // The legs first, hung from hips at the origin: the lower foot is on
    // the ground, which says how high the hips are.
    let legs = [1.0f32, -1.0].map(|side| {
        let leg = &pose.legs[(side < 0.0) as usize];
        let top = Vec3::new(side * hip, 0.0, 0.0);
        let knee = top + swing(leg.upper, side) * thigh;
        (top, knee, knee + swing(leg.lower, side) * shin)
    });
    let lowest = legs[0].2.y.min(legs[1].2.y);
    let hips = Vec3::new(0.0, ankle - lowest + pose.air * h, 0.0);

    let mut made = Vec::with_capacity(18);
    for (top, knee, foot) in legs {
        made.push(Piece::limb(hips + top, hips + knee, 0.066 * thick));
        made.push(Piece::limb(hips + knee, hips + foot, 0.048 * thick));
        let sole = Vec3::new(0.05 * thick, 0.035 * h / TALL, 0.125 * h / TALL);
        let middle = hips + foot + Vec3::new(0.0, sole.y - ankle, sole.z * 0.45);
        made.push(Piece::new(Form::Ellipsoid, middle, sole));
    }

    // The trunk, tipped at the hips and turned at the shoulders.
    let trunk = Turn::of([pose.lean as f64, pose.twist as f64, 0.0]);
    let up = |along: f32| hips + trunk.y * along;
    let part = |along: f32, half: Vec3| Piece::new(Form::Ellipsoid, up(along), half).turned(trunk);
    let (w, d) = (wide * h / TALL, deep * h / TALL);
    made.push(part(0.02 * h, Vec3::new(0.15 * w, 0.085 * h, 0.105 * d)));
    made.push(part(0.14 * h, Vec3::new(0.135 * w, 0.1 * h, 0.095 * d)));
    made.push(part(0.235 * h, Vec3::new(0.2 * w, 0.085 * h, 0.115 * d)));
    for side in [1.0f32, -1.0] {
        let arm = &pose.arms[(side < 0.0) as usize];
        let top = up(spine - 0.015 * h) + trunk.x * (side * shoulder);
        let elbow = top + trunk.outward(swing(arm.upper, side)) * upper_arm;
        let wrist = elbow + trunk.outward(swing(arm.lower, side)) * forearm;
        made.push(Piece::limb(top, elbow, 0.042 * thick));
        made.push(Piece::limb(elbow, wrist, 0.035 * thick));
        let hand = wrist + trunk.outward(swing(arm.lower, side)) * (0.035 * h);
        let palm = Vec3::new(0.04, 0.05, 0.045) * (thick.min(h / TALL * 1.2));
        made.push(Piece::new(Form::Ellipsoid, hand, palm));
    }
    made.push(Piece::limb(up(spine), up(spine + neck * 0.6), 0.03 * thick));
    let head = trunk.then(Turn::of([pose.nod as f64, pose.look as f64, 0.0]));
    let skull = Vec3::new(0.06, 0.072, 0.068) * h;
    let crown = up(spine + neck);
    made.push(Piece::new(Form::Ellipsoid, crown, skull).turned(head));
    // A visor, so the way it faces reads at any size.
    let visor = Vec3::new(skull.x * 0.72, skull.y * 0.24, skull.z * 0.2);
    let front = crown + head.z * (skull.z * 0.9) + head.y * (skull.y * 0.12);
    made.push(Piece::new(Form::Box, front, visor).turned(head).toned(0.45));

    // Seated on something, its hips go to the seat; laid down, it turns
    // onto its back about its heels.
    let on_seat = seat.map_or(Vec3::ZERO, |(seat, seated)| {
        (seat + Vec3::new(0.0, 0.08 * h, 0.0) - hips) * seated
    });
    let rest = Place {
        at: on_seat + Vec3::new(0.0, pose.laid * 0.11 * d, 0.0),
        turn: Turn::of([-90.0 * pose.laid as f64, 0.0, 0.0]),
    };
    out.extend(made.into_iter().map(|piece| piece.moved(&rest)));
}

/// The pieces of an animal `size` large (width, height to the top of its
/// head, length) at `t`: it stands, walks and gallops as fast as its
/// keys move it.
pub(super) fn animal(object: &Object, size: Vec3, t: f64, out: &mut Vec<Piece>) {
    let (w, h, l) = (size.x, size.y, size.z);
    let girth = (0.42 * w).min(0.17 * h);
    let back = 0.62 * h;
    let leg = back - girth * 0.4;
    let pace = object.speed(t) / leg.max(0.01);
    let phase = object.paced(t, |speed, _| speed / (leg * 1.5 + 0.4 * speed));
    let going = smoothstep(0.05, 0.4, pace);
    let gallop = smoothstep(2.6, 4.6, pace);

    // Each leg: where its top is along the body, and when in the stride
    // it steps, walking (one foot after another) and galloping (hind
    // pair, then fore pair).
    let legs = [
        (-0.27, 1.0, 0.0, 0.0),
        (0.2, 1.0, 0.25, 0.55),
        (-0.27, -1.0, 0.5, 0.1),
        (0.2, -1.0, 0.75, 0.65),
    ]
    .map(|(along, side, walking, galloping): (f32, f32, f32, f32)| {
        let turn = (phase + mix(walking, galloping, gallop)) * TAU;
        let fold = (turn + 0.5).cos().max(0.0);
        let hip = going * mix(20.0, 40.0, gallop) * turn.sin();
        let knee = 6.0 + going * mix(40.0, 75.0, gallop) * fold * fold;
        let top = Vec3::new(side * 0.3 * w, 0.0, along * l);
        let joint = top + swing((hip + knee * 0.35, 2.0), side) * (leg * 0.5);
        (
            top,
            joint,
            joint + swing((hip - knee * 0.65, 1.0), side) * (leg * 0.5),
        )
    });
    let lowest = legs.iter().map(|leg| leg.2.y).fold(f32::INFINITY, f32::min);
    // A gallop leaves the ground once a stride.
    let air = gallop * 0.06 * h * (phase * TAU).sin().max(0.0);
    let hips = Vec3::new(0.0, air - lowest + 0.02 * h, 0.0);
    let hoof = 0.035 * h;
    for (top, joint, foot) in legs {
        out.push(Piece::limb(hips + top, hips + joint, girth * 0.34));
        out.push(Piece::limb(hips + joint, hips + foot, girth * 0.24));
        out.push(Piece::new(
            Form::Ellipsoid,
            hips + foot,
            Vec3::new(hoof, hoof * 0.8, hoof * 1.2),
        ));
    }

    let spine = |along: f32, above: f32| hips + Vec3::new(0.0, above, along * l);
    out.push(Piece::limb(spine(-0.25, 0.0), spine(0.18, 0.0), girth));
    // The neck nods with the stride.
    let nod = going * 0.02 * h * (phase * 2.0 * TAU).sin();
    let poll = spine(0.36, h - back - girth * 0.6 + nod);
    out.push(Piece::limb(spine(0.2, girth * 0.4), poll, girth * 0.5));
    let muzzle = poll + Vec3::new(0.0, -0.09 * h, 0.1 * l);
    out.push(Piece::limb(poll, muzzle, girth * 0.42));
    out.push(
        Piece::limb(
            muzzle,
            muzzle + Vec3::new(0.0, -0.02 * h, 0.03 * l),
            girth * 0.34,
        )
        .toned(0.6),
    );
    let dock = spine(-0.27, girth * 0.6);
    let swish = going * 0.04 * l * (phase * TAU).sin();
    out.push(
        Piece::limb(
            dock,
            dock + Vec3::new(swish, -0.28 * h, -0.07 * l),
            girth * 0.16,
        )
        .toned(0.8),
    );
}

/// Where a rider's seat on an animal `size` large is, in its own space.
pub(super) fn animal_seat(size: Vec3) -> Vec3 {
    let girth = (0.42 * size.x).min(0.17 * size.y);
    Vec3::new(0.0, 0.64 * size.y + girth * 0.6, -0.04 * size.z)
}

impl Scene {
    /// Where the seat object `index` rides is, in the object's own space
    /// (it stands at `place`): for a person whose `parent` has a seat.
    pub(super) fn seat(&self, index: usize, place: &Place, t: f64) -> Option<Vec3> {
        let object = &self.objects[index];
        if object.shape != Shape::Figure {
            return None;
        }
        let ridden = self.index(object.parent.as_deref()?)?;
        let parent = &self.objects[ridden];
        let size = Vec3::of(parent.size_now(t));
        let seat = match parent.shape {
            Shape::Animal => animal_seat(size),
            shape => kit::seat(shape, size)?,
        };
        let in_scene = self.place(ridden, t).point(seat);
        Some(place.turn.inward(in_scene - place.at))
    }
}
