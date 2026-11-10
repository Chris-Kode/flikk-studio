//! Props: things a scene needs often, each a few solids sized to the box
//! the object gives. They face +Z.

use super::Shape;
use super::render::{Form, Piece};
use super::space::{Turn, Vec3};

/// The pieces of the prop `shape`, `size` large, in its own space.
/// Nothing for a shape that is no prop.
pub(super) fn prop(shape: Shape, size: Vec3, out: &mut Vec<Piece>) {
    let (w, h, l) = (size.x, size.y, size.z);
    // A box whose middle is at `at` and whose half size is `half`, both
    // as shares of the prop's size.
    let block = |at: (f32, f32, f32), half: (f32, f32, f32)| {
        Piece::new(
            Form::Box,
            Vec3::new(at.0 * w, at.1 * h, at.2 * l),
            Vec3::new(half.0 * w, half.1 * h, half.2 * l),
        )
    };
    // A wheel standing on the ground, its axle across the prop.
    let wheel = |x: f32, z: f32, radius: f32, width: f32| {
        Piece::new(
            Form::Cylinder,
            Vec3::new(x, radius, z),
            Vec3::new(radius, width * 0.5, radius),
        )
        .turned(Turn::of([0.0, 0.0, 90.0]))
        .toned(0.55)
    };
    match shape {
        Shape::Car => {
            let radius = (0.24 * h).min(0.12 * l);
            let width = 0.14 * w;
            for (side, end) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let x = side * (w - width) * 0.5;
                out.push(wheel(x, end * 0.3 * l, radius, width));
            }
            out.push(block((0.0, 0.37, 0.0), (0.46, 0.19, 0.5)));
            out.push(block((0.0, 0.765, -0.09), (0.42, 0.205, 0.21)));
            // Glass: sloped ahead and behind, a band along each side.
            let glass = |at, half| Piece {
                form: Form::Wedge,
                ..block(at, half).toned(0.42)
            };
            out.push(glass((0.0, 0.765, 0.19), (0.42, 0.205, 0.07)));
            let rear = glass((0.0, 0.765, -0.35), (0.42, 0.205, 0.05));
            out.push(rear.turned(Turn::of([0.0, 180.0, 0.0])));
            out.push(block((0.0, 0.79, -0.09), (0.425, 0.11, 0.18)).toned(0.42));
            // A grille, so its front reads from far off.
            out.push(block((0.0, 0.34, 0.5), (0.3, 0.05, 0.004)).toned(0.45));
        }
        Shape::Bike => {
            let radius = (0.3 * h).min(0.24 * l);
            let (rear, fore) = (-0.5 * l + radius, 0.5 * l - radius);
            out.push(wheel(0.0, rear, radius, 0.09));
            out.push(wheel(0.0, fore, radius, 0.09));
            let bar = |from: Vec3, to: Vec3, radius: f32| Piece::limb(from, to, radius);
            let (hub, axle) = (Vec3::new(0.0, radius, rear), Vec3::new(0.0, radius, fore));
            let head = Vec3::new(0.0, 0.86 * h, 0.24 * l);
            let saddle = Vec3::new(0.0, 0.72 * h, -0.16 * l);
            let thin = 0.028 * h;
            out.push(bar(hub, saddle, thin));
            out.push(bar(hub, Vec3::new(0.0, 0.45 * h, 0.02 * l), thin * 1.6));
            out.push(bar(saddle, head, thin * 1.6));
            out.push(bar(Vec3::new(0.0, 0.45 * h, 0.02 * l), head, thin * 1.6));
            out.push(bar(axle, head, thin));
            let grip = Vec3::new(0.5 * w, 0.0, 0.0);
            let bars = head + Vec3::new(0.0, 0.08 * h, -0.02 * l);
            out.push(bar(bars - grip, bars + grip, thin * 0.8));
            let seat = Vec3::new(0.16 * w, 0.03 * h, 0.09 * l);
            out.push(
                Piece::new(
                    Form::Ellipsoid,
                    saddle + Vec3::new(0.0, 0.03 * h, 0.0),
                    seat,
                )
                .toned(0.6),
            );
        }
        Shape::Tree => {
            let trunk = Vec3::new(0.06 * w, 0.26 * h, 0.06 * l);
            out.push(Piece::new(Form::Cylinder, Vec3::new(0.0, trunk.y, 0.0), trunk).toned(0.7));
            let leaves = |at: (f32, f32, f32), half: (f32, f32, f32)| Piece {
                form: Form::Ellipsoid,
                ..block(at, half)
            };
            out.push(leaves((0.0, 0.7, 0.0), (0.44, 0.3, 0.44)));
            out.push(leaves((0.2, 0.56, 0.12), (0.3, 0.19, 0.3)));
            out.push(leaves((-0.19, 0.6, -0.14), (0.31, 0.21, 0.31)));
            out.push(leaves((-0.05, 0.58, 0.22), (0.26, 0.17, 0.26)));
        }
        Shape::House => {
            out.push(block((0.0, 0.31, 0.0), (0.48, 0.31, 0.48)));
            let roof = Piece {
                form: Form::Roof,
                ..block((0.0, 0.81, 0.0), (0.5, 0.19, 0.5))
            };
            out.push(roof.toned(0.78));
            // A door on its front and windows beside it.
            let door = Vec3::new(0.45f32.min(0.1 * w), 1.05f32.min(0.2 * h), 0.03);
            let at = Vec3::new(0.0, door.y, 0.48 * l);
            out.push(Piece::new(Form::Box, at, door).toned(0.5));
            for side in [-1.0, 1.0] {
                out.push(block((side * 0.28, 0.38, 0.48), (0.09, 0.09, 0.004)).toned(0.6));
            }
        }
        Shape::Chair => {
            out.push(block((0.0, 0.47, 0.0), (0.5, 0.03, 0.5)));
            out.push(block((0.0, 0.75, -0.45), (0.5, 0.25, 0.05)));
            for (side, end) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                out.push(block((side * 0.44, 0.22, end * 0.44), (0.05, 0.22, 0.05)));
            }
        }
        Shape::Table => {
            out.push(block((0.0, 0.96, 0.0), (0.5, 0.04, 0.5)));
            let thick = (0.04 * w.min(l)).max(0.02);
            for (side, end) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let inset = thick * 2.0;
                let at = Vec3::new(side * (w * 0.5 - inset), 0.46 * h, end * (l * 0.5 - inset));
                out.push(Piece::new(Form::Box, at, Vec3::new(thick, 0.46 * h, thick)));
            }
        }
        _ => {}
    }
}

/// Where someone sits on the prop `shape`, `size` large: the top of its
/// seat, in its own space. `None` for a shape without a seat.
pub(super) fn seat(shape: Shape, size: Vec3) -> Option<Vec3> {
    let (h, l) = (size.y, size.z);
    match shape {
        Shape::Chair => Some(Vec3::new(0.0, 0.5 * h, 0.02 * l)),
        Shape::Bike => Some(Vec3::new(0.0, 0.78 * h, -0.16 * l)),
        _ => None,
    }
}
