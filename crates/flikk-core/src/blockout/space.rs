//! The geometry a blockout is worked out in: points, turns, and the places
//! things stand in.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const UP: Self = Self::new(0.0, 1.0, 0.0);
    pub const AHEAD: Self = Self::new(0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn of(v: [f64; 3]) -> Self {
        Self::new(v[0] as f32, v[1] as f32, v[2] as f32)
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// One unit long; `fallback` when it has no length.
    pub fn unit_or(self, fallback: Self) -> Self {
        let length = self.length();
        if length > 1e-6 {
            self * (1.0 / length)
        } else {
            fallback
        }
    }

    /// Each part over the same part of `o`.
    pub fn over(self, o: Self) -> Self {
        Self::new(self.x / o.x, self.y / o.y, self.z / o.z)
    }

    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs(), self.z.abs())
    }

    pub fn min(self, o: Self) -> Self {
        Self::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }

    pub fn max(self, o: Self) -> Self {
        Self::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }

    /// Its part along axis 0 (X), 1 (Y) or 2 (Z).
    #[inline(always)]
    pub fn part(self, axis: usize) -> f32 {
        match axis {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// How something is turned: its own axes, in the space it stands in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Turn {
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
}

impl Turn {
    pub const NONE: Self = Self {
        x: Vec3::new(1.0, 0.0, 0.0),
        y: Vec3::UP,
        z: Vec3::AHEAD,
    };

    /// From turns in degrees about X, Y and Z: rolled about Z, tipped
    /// about X, then turned about the vertical.
    pub fn of(rotate: [f64; 3]) -> Self {
        let [(sx, cx), (sy, cy), (sz, cz)] = rotate.map(|d| (d.to_radians() as f32).sin_cos());
        Self {
            x: Vec3::new(cy * cz + sy * sx * sz, cx * sz, -sy * cz + cy * sx * sz),
            y: Vec3::new(-cy * sz + sy * sx * cz, cx * cz, sy * sz + cy * sx * cz),
            z: Vec3::new(sy * cx, -sx, cy * cx),
        }
    }

    /// Its Y axis along `up`, its Z axis as near `ahead` as that allows.
    pub fn along(up: Vec3, ahead: Vec3) -> Self {
        let y = up.unit_or(Vec3::UP);
        let x = y
            .cross(ahead)
            .unit_or(y.cross(Vec3::new(1.0, 0.0, 0.0)).unit_or(Vec3::AHEAD));
        Self {
            x,
            y,
            z: x.cross(y),
        }
    }

    /// A turn of `radians` about `axis`, one unit long.
    pub fn about(axis: Vec3, radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        // Each axis turned: what of it lies along `axis` stays.
        let turned = |v: Vec3| v * cos + axis.cross(v) * sin + axis * (axis.dot(v) * (1.0 - cos));
        Self {
            x: turned(Self::NONE.x),
            y: turned(Self::NONE.y),
            z: turned(Self::NONE.z),
        }
    }

    /// The least turn that takes the direction `from` toward `to` (one
    /// unit each), by no more than `most` radians.
    pub fn toward(from: Vec3, to: Vec3, most: f32) -> Self {
        let across = from.cross(to);
        let apart = across.length().atan2(from.dot(to)).min(most);
        // Facing straight away from it, any way round is as short.
        let other = if from.x.abs() < 0.9 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::UP
        };
        let axis = across.unit_or(from.cross(other).unit_or(Vec3::UP));
        Self::about(axis, apart)
    }

    /// A direction of its own, in the space it stands in.
    pub fn outward(&self, v: Vec3) -> Vec3 {
        self.x * v.x + self.y * v.y + self.z * v.z
    }

    /// A direction of the space it stands in, in its own axes.
    pub fn inward(&self, v: Vec3) -> Vec3 {
        Vec3::new(self.x.dot(v), self.y.dot(v), self.z.dot(v))
    }

    /// `other`, a turn of the space it stands in, in its own axes: what
    /// [`Turn::then`] gives `other` back from.
    pub fn inward_turn(&self, other: Turn) -> Turn {
        Turn {
            x: self.inward(other.x),
            y: self.inward(other.y),
            z: self.inward(other.z),
        }
    }

    /// `inner`, a turn in its axes, in the space it stands in.
    pub fn then(&self, inner: Turn) -> Turn {
        Turn {
            x: self.outward(inner.x),
            y: self.outward(inner.y),
            z: self.outward(inner.z),
        }
    }
}

/// Where something stands and how it is turned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Place {
    pub at: Vec3,
    pub turn: Turn,
}

impl Place {
    pub const ORIGIN: Self = Self {
        at: Vec3::ZERO,
        turn: Turn::NONE,
    };

    /// A point of its own, in the space it stands in.
    pub fn point(&self, local: Vec3) -> Vec3 {
        self.at + self.turn.outward(local)
    }

    /// `inner`, a place in its axes, in the space it stands in.
    pub fn then(&self, inner: Place) -> Place {
        Place {
            at: self.point(inner.at),
            turn: self.turn.then(inner.turn),
        }
    }

    /// `other`, a place of the space it stands in, in its own axes: what
    /// [`Place::then`] gives `other` back from.
    pub fn holds(&self, other: Place) -> Place {
        Place {
            at: self.turn.inward(other.at - self.at),
            turn: self.turn.inward_turn(other.turn),
        }
    }
}

/// 0 below `from`, 1 above `to`, a smooth rise between.
pub(super) fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let u = ((x - from) / (to - from)).clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}
