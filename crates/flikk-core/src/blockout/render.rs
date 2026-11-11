//! Drawing a scene: one ray a pixel against its solids, lit by one light
//! with a shadow, so depth, contact and the
//! camera's move read at a glance. Greys only: the look is the video
//! model's to make.

use super::motion::View;
use super::space::{Turn, Vec3};
use super::{Anchor, Scene, Shape};
use crate::services::Frame;

/// Pictures smaller than this are drawn on the calling thread.
const THREADED_PIXELS: usize = 32_768;
/// How near a hit counts, so a surface doesn't shadow itself.
const NEAR: f32 = 1e-3;
/// Where the light comes from unless a scene says: around the vertical
/// from +Z, and above the horizon, in degrees.
const SUN: [f64; 2] = [47.0, 55.0];
/// How much of a surface's grey the light adds, and how much it keeps in
/// shadow: enough that a street between tall walls still reads.
const LIGHT: f32 = 0.44;
const SHADE: f32 = 0.44;
const GROUND_GREY: f32 = 0.5;
const SKY_HORIZON: f32 = 0.76;
const SKY_ZENITH: f32 = 0.9;
/// Metres at which the haze has taken about two thirds of a surface.
const HAZE: f32 = 140.0;
/// Metres within which one surface darkens another it is close to.
const CLOSE: f32 = 1.1;
/// How much darker the closest surfaces get.
const CLOSE_DARK: f32 = 0.45;
/// Neighbouring pixels this far apart in grey are an edge, drawn again
/// from several rays.
const EDGE: f32 = 0.05;
/// Pixels along the side of a tile of the picture that lists its solids.
const TILE: usize = 16;
/// The most cells along a side of the light's grid.
const LIGHT_CELLS: usize = 96;
/// Rows a thread draws before it takes more.
const BAND: usize = 8;
/// The most solids a leaf of the tree holds.
const LEAF: usize = 2;

/// How carefully a picture is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// One ray a pixel: fast enough to play.
    Draft,
    /// Smooth edges, and flat surfaces darkened where something is close:
    /// for the clip and for a frame that stays on screen.
    Full,
}

impl Scene {
    /// The scene at `t` seconds, `width` × `height` pixels, opaque.
    pub fn render(&self, t: f64, width: u32, height: u32, detail: Detail) -> Frame {
        let (width, height) = (width.max(1), height.max(1));
        let world = World::of(self, t.clamp(0.0, self.duration), width, height, detail);
        let row = width as usize;
        let greys = match detail {
            Detail::Draft => {
                let mut greys = vec![0f32; row * height as usize];
                in_bands(&mut greys, row, |first, band| {
                    for (i, grey) in band.iter_mut().enumerate() {
                        let (x, y) = (i % row, first + i / row);
                        *grey = world.sample(x as f32 + 0.5, y as f32 + 0.5).grey(1.0);
                    }
                });
                greys
            }
            Detail::Full => world.finished(row, height as usize),
        };
        let mut rgba = vec![0u8; greys.len() * 4];
        for (pixel, grey) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(&greys) {
            let value = (grey.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            *pixel = [value, value, value, 255];
        }
        Frame {
            width,
            height,
            rgba: rgba.into(),
        }
    }
}

/// What one ray from the camera found, short of how open its surface is
/// to the sky, which is worked out for fewer pixels and shared.
#[derive(Clone, Copy, Default)]
struct Sample {
    /// Its grey is `lit + ambient × openness`.
    lit: f32,
    ambient: f32,
    /// How far along the ray to ask how open the surface is, and the way
    /// it faces there. No distance where there is nothing to ask: only a
    /// flat surface with a solid close by is asked, in a full picture.
    probed: f32,
    normal: Vec3,
}

impl Sample {
    fn grey(&self, open: f32) -> f32 {
        self.lit + self.ambient * open
    }
}

/// Run `draw` over `values` (rows of `row`) a few rows at a time, shared
/// out among threads; it is given the first row of each lot.
fn in_bands<T: Send>(values: &mut [T], row: usize, draw: impl Fn(usize, &mut [T]) + Sync) {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(16));
    if threads == 1 || values.len() < THREADED_PIXELS {
        return draw(0, values);
    }
    // Small lots, taken as each thread is free: the sky draws in no time
    // and the ground doesn't, so even shares would leave threads idle.
    let lots = std::sync::Mutex::new(values.chunks_mut(BAND * row).enumerate());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let lot = lots.lock().ok().and_then(|mut lots| lots.next());
                    let Some((i, rows)) = lot else {
                        break;
                    };
                    draw(i * BAND, rows);
                }
            });
        }
    });
}

impl Shape {
    /// Whether every side of it is flat.
    fn is_flat(self) -> bool {
        matches!(self, Self::Box | Self::Wedge | Self::Roof | Self::Pyramid)
    }
}

/// One solid placed in the scene.
struct Solid {
    shape: Shape,
    centre: Vec3,
    turn: Turn,
    half: Vec3,
    /// A sphere about `centre` holding it, for a quick miss.
    reach: f32,
    shade: f32,
    /// The thing a viewer would name it as: its place among the parts.
    owner: u16,
}

impl Solid {
    /// The corners of a box along the scene's axes that holds it.
    fn bounds(&self) -> (Vec3, Vec3) {
        let (x, y, z) = (self.turn.x.abs(), self.turn.y.abs(), self.turn.z.abs());
        let reach = x * self.half.x + y * self.half.y + z * self.half.z;
        (self.centre - reach, self.centre + reach)
    }

    /// The eight corners of that box.
    fn corners(&self) -> [Vec3; 8] {
        let (low, high) = self.bounds();
        std::array::from_fn(|i| {
            let pick = |bit: usize, low: f32, high: f32| if i >> bit & 1 == 0 { low } else { high };
            Vec3::new(
                pick(0, low.x, high.x),
                pick(1, low.y, high.y),
                pick(2, low.z, high.z),
            )
        })
    }

    /// Where the ray from `origin` along `dir` (one unit long) first meets
    /// it: how far, and the surface's direction there.
    fn hit(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
        let to = self.centre - origin;
        let along = to.dot(dir);
        if along < -self.reach || to.dot(to) - along * along > self.reach * self.reach {
            return None;
        }
        let (o, d) = (
            self.turn.inward(origin - self.centre),
            self.turn.inward(dir),
        );
        let h = self.half;
        let (t, normal) = match self.shape {
            Shape::Box => hit_box(o, d, h),
            Shape::Sphere => hit_ellipsoid(o, d, h),
            Shape::Cylinder => hit_cylinder(o, d, h),
            Shape::Cone => hit_cone(o, d, h),
            Shape::Capsule => hit_capsule(o, d, h),
            Shape::Dome => hit_dome(o, d, h),
            Shape::Wedge => {
                let slope = (Vec3::new(0.0, h.z, h.y), 0.0);
                hit_hull(
                    o,
                    d,
                    &[base(h), back(h), side(h, 1.0), side(h, -1.0), slope],
                )
            }
            Shape::Roof => {
                let faces = [base(h), back(h), front(h), pitch(h, 1.0), pitch(h, -1.0)];
                hit_hull(o, d, &faces)
            }
            Shape::Pyramid => {
                let ahead = (Vec3::new(0.0, h.z, 2.0 * h.y), h.z * h.y);
                let behind = (Vec3::new(0.0, h.z, -2.0 * h.y), h.z * h.y);
                hit_hull(
                    o,
                    d,
                    &[base(h), pitch(h, 1.0), pitch(h, -1.0), ahead, behind],
                )
            }
        }?;
        Some((t, self.turn.outward(normal).unit_or(Vec3::UP)))
    }
}

/// The faces of a solid of half size `h`, each as a direction out of it
/// and how far along that direction the face is.
type Face = (Vec3, f32);

fn base(h: Vec3) -> Face {
    (Vec3::new(0.0, -1.0, 0.0), h.y)
}

fn back(h: Vec3) -> Face {
    (Vec3::new(0.0, 0.0, -1.0), h.z)
}

fn front(h: Vec3) -> Face {
    (Vec3::AHEAD, h.z)
}

fn side(h: Vec3, sign: f32) -> Face {
    (Vec3::new(sign, 0.0, 0.0), h.x)
}

/// A face sloping from the top's middle down to the side at `sign`.
fn pitch(h: Vec3, sign: f32) -> Face {
    (Vec3::new(sign * 2.0 * h.y, h.x, 0.0), h.x * h.y)
}

/// Where a ray enters the solid inside all of `faces`.
fn hit_hull(o: Vec3, d: Vec3, faces: &[Face]) -> Option<(f32, Vec3)> {
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    let mut normal = Vec3::ZERO;
    for (out, offset) in faces {
        let (toward, inside) = (out.dot(d), offset - out.dot(o));
        if toward.abs() < 1e-9 {
            if inside < 0.0 {
                return None;
            }
            continue;
        }
        let t = inside / toward;
        if toward < 0.0 {
            if t > near {
                (near, normal) = (t, *out);
            }
        } else {
            far = far.min(t);
        }
    }
    (near <= far && near > NEAR).then_some((near, normal))
}

/// The nearer root of `a·t² + 2·b·t + c = 0` past [`NEAR`], and the
/// farther one.
fn roots(a: f32, b: f32, c: f32) -> Option<(f32, f32)> {
    if a.abs() < 1e-9 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let (t0, t1) = ((-b - root) / a, (-b + root) / a);
    Some(if t0 <= t1 { (t0, t1) } else { (t1, t0) })
}

fn hit_box(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    let mut normal = Vec3::ZERO;
    let axes = [
        (o.x, d.x, half.x, Vec3::new(1.0, 0.0, 0.0)),
        (o.y, d.y, half.y, Vec3::new(0.0, 1.0, 0.0)),
        (o.z, d.z, half.z, Vec3::new(0.0, 0.0, 1.0)),
    ];
    for (o, d, half, axis) in axes {
        if d.abs() < 1e-9 {
            if o.abs() > half {
                return None;
            }
            continue;
        }
        let (a, b) = ((-half - o) / d, (half - o) / d);
        let (enter, leave) = if a < b { (a, b) } else { (b, a) };
        if enter > near {
            near = enter;
            normal = axis * -d.signum();
        }
        far = far.min(leave);
    }
    (near <= far && near > NEAR).then_some((near, normal))
}

fn hit_ellipsoid(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (o, d) = (o.over(half), d.over(half));
    let (t, _) = roots(d.dot(d), o.dot(d), o.dot(o) - 1.0)?;
    (t > NEAR).then(|| (t, (o + d * t).over(half)))
}

/// The upper half of an ellipsoid twice as high as `half`, standing on
/// the underside of its box.
fn hit_dome(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let o = Vec3::new(o.x, o.y + half.y, o.z);
    let radii = Vec3::new(half.x, half.y * 2.0, half.z);
    let (os, ds) = (o.over(radii), d.over(radii));
    let (enter, leave) = roots(ds.dot(ds), os.dot(ds), os.dot(os) - 1.0)?;
    if enter > NEAR && o.y + d.y * enter >= 0.0 {
        let at = os + ds * enter;
        return Some((enter, at.over(radii)));
    }
    // Under the round part: in through the flat underside, if at all.
    let under = -o.y / d.y;
    (d.y > 1e-9 && under > NEAR && under >= enter && under <= leave)
        .then_some((under, Vec3::new(0.0, -1.0, 0.0)))
}

fn hit_cylinder(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (ox, oz, dx, dz) = (o.x / half.x, o.z / half.z, d.x / half.x, d.z / half.z);
    let side = roots(
        dx * dx + dz * dz,
        ox * dx + oz * dz,
        ox * ox + oz * oz - 1.0,
    )
    .map(|(t, _)| t)
    .filter(|t| *t > NEAR && (o.y + d.y * t).abs() <= half.y)
    .map(|t| {
        let normal = Vec3::new((ox + dx * t) / half.x, 0.0, (oz + dz * t) / half.z);
        (t, normal)
    });
    // The end it comes in through, if it comes in through one.
    let end = (d.y.abs() > 1e-9)
        .then(|| {
            let y = -half.y * d.y.signum();
            ((y - o.y) / d.y, Vec3::new(0.0, y.signum(), 0.0))
        })
        .filter(|(t, _)| {
            let (x, z) = (ox + dx * t, oz + dz * t);
            *t > NEAR && x * x + z * z <= 1.0
        });
    nearer(side, end)
}

fn hit_cone(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let (ox, oz, dx, dz) = (o.x / half.x, o.z / half.z, d.x / half.x, d.z / half.z);
    // Its radius is 1 at the base and 0 at the point: `k` a metre down.
    let k = 1.0 / (2.0 * half.y);
    let k2 = k * k;
    let below = half.y - o.y;
    let (a, b, c) = (
        dx * dx + dz * dz - k2 * d.y * d.y,
        ox * dx + oz * dz + k2 * below * d.y,
        ox * ox + oz * oz - k2 * below * below,
    );
    let on_side = |t: &f32| *t > NEAR && (o.y + d.y * t).abs() <= half.y;
    let side = match roots(a, b, c) {
        Some((t0, t1)) => [t0, t1].into_iter().find(on_side),
        // Along its slope: it meets the side once.
        None if a.abs() < 1e-9 && b.abs() > 1e-9 => Some(-c / (2.0 * b)).filter(on_side),
        None => None,
    }
    .map(|t| {
        let (x, z) = (ox + dx * t, oz + dz * t);
        let down = half.y - (o.y + d.y * t);
        (t, Vec3::new(x / half.x, k2 * down, z / half.z))
    });
    let base = (d.y > 1e-9)
        .then(|| ((-half.y - o.y) / d.y, Vec3::new(0.0, -1.0, 0.0)))
        .filter(|(t, _)| {
            let (x, z) = (ox + dx * t, oz + dz * t);
            *t > NEAR && x * x + z * z <= 1.0
        });
    nearer(side, base)
}

fn hit_capsule(o: Vec3, d: Vec3, half: Vec3) -> Option<(f32, Vec3)> {
    let radius = half.x.min(half.z).min(half.y);
    // Its spine runs from one ball's middle to the other's.
    let reach = half.y - radius;
    let side = roots(d.x * d.x + d.z * d.z, o.x * d.x + o.z * d.z, {
        o.x * o.x + o.z * o.z - radius * radius
    })
    .map(|(t, _)| t)
    .filter(|t| *t > NEAR && (o.y + d.y * t).abs() <= reach)
    .map(|t| (t, Vec3::new(o.x + d.x * t, 0.0, o.z + d.z * t)));
    let ball = |y: f32| {
        let from = Vec3::new(o.x, o.y - y, o.z);
        let (t, _) = roots(1.0, from.dot(d), from.dot(from) - radius * radius)?;
        (t > NEAR).then(|| (t, from + d * t))
    };
    nearer(side, nearer(ball(reach), ball(-reach)))
}

fn nearer<T>(a: Option<(f32, T)>, b: Option<(f32, T)>) -> Option<(f32, T)> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// What a ray met.
struct Hit {
    distance: f32,
    normal: Vec3,
    shade: f32,
    /// The object met; `None` for the ground.
    owner: Option<u16>,
    /// Whether the surface met is flat: the ground, or a face of a solid
    /// with faces.
    flat: bool,
}

/// A box along the scene's axes holding some solids, which are in two
/// smaller boxes or, in a leaf, listed.
struct Node {
    low: Vec3,
    high: Vec3,
    /// A leaf's solids: the first, and how many. An inner node has none,
    /// and its halves are the next node and `second`.
    first: u32,
    count: u32,
    second: u32,
    /// The axis an inner node's solids are halved along: the next node
    /// holds those lower on it.
    axis: u8,
}

/// The solids of a scene, sorted into a tree of boxes so a ray tries only
/// those near it.
struct Solids {
    solids: Vec<Solid>,
    nodes: Vec<Node>,
}

impl Solids {
    fn new(mut solids: Vec<Solid>) -> Self {
        let mut nodes = Vec::with_capacity(solids.len() * 2);
        if !solids.is_empty() {
            Self::grow(&mut solids, 0, &mut nodes);
        }
        Self { solids, nodes }
    }

    /// Add the tree of `solids`, which start at `first` of them all.
    fn grow(solids: &mut [Solid], first: usize, nodes: &mut Vec<Node>) {
        let (mut low, mut high) = solids[0].bounds();
        let (mut least, mut most) = (solids[0].centre, solids[0].centre);
        for solid in &solids[1..] {
            let (l, h) = solid.bounds();
            (low, high) = (low.min(l), high.max(h));
            (least, most) = (least.min(solid.centre), most.max(solid.centre));
        }
        let index = nodes.len();
        nodes.push(Node {
            low,
            high,
            first: first as u32,
            count: solids.len() as u32,
            second: 0,
            axis: 0,
        });
        if solids.len() <= LEAF {
            return;
        }
        // Halve them along the axis their middles are spread widest on.
        let spread = most - least;
        let axis = (0..3)
            .max_by(|a, b| spread.part(*a).total_cmp(&spread.part(*b)))
            .unwrap_or(0);
        let middle = solids.len() / 2;
        solids.select_nth_unstable_by(middle, |a, b| {
            a.centre.part(axis).total_cmp(&b.centre.part(axis))
        });
        let (left, right) = solids.split_at_mut(middle);
        nodes[index].count = 0;
        nodes[index].axis = axis as u8;
        Self::grow(left, first, nodes);
        nodes[index].second = nodes.len() as u32;
        Self::grow(right, first + middle, nodes);
    }

    /// The nearest solid the ray meets within `reach`.
    fn first(&self, origin: Vec3, dir: Vec3, mut reach: f32) -> Option<Hit> {
        let mut first: Option<Hit> = None;
        let inverse = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
        let mut stack = [0u32; 48];
        let mut depth = usize::from(!self.nodes.is_empty());
        while depth > 0 {
            depth -= 1;
            let index = stack[depth];
            let node = &self.nodes[index as usize];
            if !crosses(node, origin, inverse, reach) {
                continue;
            }
            if node.count == 0 {
                // A tree this deep has more solids than a scene can hold.
                if depth + 2 <= stack.len() {
                    // The half the ray comes from is tried first, so a
                    // hit there shortens the reach for the other.
                    let (near, far) = match dir.part(node.axis as usize) >= 0.0 {
                        true => (index + 1, node.second),
                        false => (node.second, index + 1),
                    };
                    (stack[depth], stack[depth + 1]) = (far, near);
                    depth += 2;
                }
                continue;
            }
            let (from, count) = (node.first as usize, node.count as usize);
            for solid in &self.solids[from..from + count] {
                if let Some((distance, normal)) = solid.hit(origin, dir)
                    && distance < reach
                {
                    reach = distance;
                    first = Some(Hit {
                        distance,
                        normal,
                        shade: solid.shade,
                        owner: Some(solid.owner),
                        flat: solid.shape.is_flat(),
                    });
                }
            }
        }
        first
    }

    /// Whether any solid's box is within `reach` of `point`.
    fn near(&self, point: Vec3, reach: f32) -> bool {
        let within = |node: &Node| {
            let outside = (node.low - point).max(point - node.high).max(Vec3::ZERO);
            outside.dot(outside) <= reach * reach
        };
        let mut stack = [0u32; 48];
        let mut depth = usize::from(!self.nodes.is_empty());
        while depth > 0 {
            depth -= 1;
            let index = stack[depth];
            let node = &self.nodes[index as usize];
            if !within(node) {
                continue;
            }
            if node.count > 0 {
                return true;
            }
            if depth + 2 <= stack.len() {
                (stack[depth], stack[depth + 1]) = (node.second, index + 1);
                depth += 2;
            }
        }
        false
    }
}

/// Whether a ray from `origin`, `inverse` one over its direction, passes
/// through `node`'s box within `reach`.
#[inline(always)]
fn crosses(node: &Node, origin: Vec3, inverse: Vec3, reach: f32) -> bool {
    // Compared by hand: the library's `min` and `max` also order NaN,
    // which costs more than the rest of this.
    let less = |a: f32, b: f32| if a < b { a } else { b };
    let more = |a: f32, b: f32| if a > b { a } else { b };
    let (mut enter, mut leave) = (0.0, reach);
    for axis in 0..3 {
        let (low, high) = (node.low.part(axis), node.high.part(axis));
        let (from, slope) = (origin.part(axis), inverse.part(axis));
        let (a, b) = ((low - from) * slope, (high - from) * slope);
        enter = more(enter, less(a, b));
        leave = less(leave, more(a, b));
    }
    enter <= leave
}

/// Solids listed by where they show across a flat view: the cells of a
/// grid, each with the solids that reach into it. Rays that all leave one
/// point (the camera's) or all run one way (the light's) cross such a
/// view in one cell each, and so try only that cell's solids.
struct Cells {
    cols: usize,
    rows: usize,
    /// Where each cell's solids start in `listed`, and one past the last.
    starts: Vec<u32>,
    listed: Vec<u32>,
}

/// The cells one solid reaches into: columns and rows, first to last.
type Reach = (usize, usize, usize, usize);

impl Cells {
    /// A grid of `cols` × `rows` in which solid `i` reaches `reaches[i]`,
    /// or shows nowhere.
    fn new(cols: usize, rows: usize, reaches: &[Option<Reach>]) -> Self {
        let mut starts = vec![0u32; cols * rows + 1];
        let cells = |reach: &Reach| {
            let (c0, c1, r0, r1) = *reach;
            (r0..=r1).flat_map(move |row| (c0..=c1).map(move |col| row * cols + col))
        };
        for reach in reaches.iter().flatten() {
            cells(reach).for_each(|cell| starts[cell + 1] += 1);
        }
        for cell in 0..cols * rows {
            starts[cell + 1] += starts[cell];
        }
        let mut next = starts.clone();
        let mut listed = vec![0u32; starts[cols * rows] as usize];
        for (solid, reach) in reaches.iter().enumerate() {
            for cell in reach.iter().flat_map(cells) {
                listed[next[cell] as usize] = solid as u32;
                next[cell] += 1;
            }
        }
        Self {
            cols,
            rows,
            starts,
            listed,
        }
    }

    /// The solids that reach into the cell at (`x`, `y`), counted in
    /// cells from the grid's corner; none outside it.
    fn at(&self, x: f32, y: f32) -> &[u32] {
        if x < 0.0 || y < 0.0 || x >= self.cols as f32 || y >= self.rows as f32 {
            return &[];
        }
        let cell = y as usize * self.cols + x as usize;
        &self.listed[self.starts[cell] as usize..self.starts[cell + 1] as usize]
    }

    /// The cells a box from (`x0`, `y0`) to (`x1`, `y1`), in cells, reaches
    /// into; `None` when it is outside the grid.
    fn reach(&self, (x0, y0): (f32, f32), (x1, y1): (f32, f32)) -> Option<Reach> {
        let (cols, rows) = (self.cols as f32, self.rows as f32);
        if x1 < 0.0 || y1 < 0.0 || x0 >= cols || y0 >= rows {
            return None;
        }
        let cell = |v: f32, most: f32| v.clamp(0.0, most - 1.0) as usize;
        Some((
            cell(x0, cols),
            cell(x1, cols),
            cell(y0, rows),
            cell(y1, rows),
        ))
    }

    fn empty(cols: usize, rows: usize) -> Self {
        Self::new(cols, rows, &[])
    }
}

/// The light's view of the solids: a grid across the way the light
/// travels, so a ray toward the light tries only what stands in its way.
struct Shade {
    cells: Cells,
    /// Across and up the grid, and its corner and cell size along them.
    across: Vec3,
    up: Vec3,
    corner: (f32, f32),
    cell: f32,
}

impl Shade {
    fn of(solids: &[Solid], light: Vec3) -> Self {
        let turn = Turn::along(light, Vec3::new(0.3, 0.1, 0.95));
        let (across, up) = (turn.x, turn.z);
        let flat = |point: Vec3| (point.dot(across), point.dot(up));
        // Each solid's box as the light sees it.
        let boxes: Vec<_> = (solids.iter())
            .map(|solid| {
                let seen = solid.corners().map(flat);
                let low = seen
                    .iter()
                    .fold((f32::MAX, f32::MAX), |a, b| (a.0.min(b.0), a.1.min(b.1)));
                let high = seen
                    .iter()
                    .fold((f32::MIN, f32::MIN), |a, b| (a.0.max(b.0), a.1.max(b.1)));
                (low, high)
            })
            .collect();
        let corner = (boxes.iter()).fold((f32::MAX, f32::MAX), |a, b| {
            (a.0.min(b.0.0), a.1.min(b.0.1))
        });
        let far = (boxes.iter()).fold((f32::MIN, f32::MIN), |a, b| {
            (a.0.max(b.1.0), a.1.max(b.1.1))
        });
        let side = (far.0 - corner.0).max(far.1 - corner.1).max(1e-3);
        let count = ((solids.len() as f32).sqrt() * 6.0).clamp(1.0, LIGHT_CELLS as f32) as usize;
        let cell = side / count as f32 * 1.0001;
        let mut shade = Self {
            cells: Cells::empty(count, count),
            across,
            up,
            corner,
            cell,
        };
        let reaches: Vec<_> = (boxes.iter())
            .map(|(low, high)| (shade.cells).reach(shade.in_cells(*low), shade.in_cells(*high)))
            .collect();
        shade.cells = Cells::new(count, count, &reaches);
        shade
    }

    /// A place across the light's view, counted in cells.
    fn in_cells(&self, (x, y): (f32, f32)) -> (f32, f32) {
        (
            (x - self.corner.0) / self.cell,
            (y - self.corner.1) / self.cell,
        )
    }

    /// The solids that may stand between `point` and the light.
    fn toward_light(&self, point: Vec3) -> &[u32] {
        let (x, y) = self.in_cells((point.dot(self.across), point.dot(self.up)));
        self.cells.at(x, y)
    }
}

/// A scene at one moment, seen through its camera.
pub(super) struct World {
    solids: Solids,
    /// The solids by the tile of the picture they show in, and by where
    /// they stand in the light.
    tiles: Cells,
    shade: Shade,
    ground: bool,
    /// The way to the light.
    light: Vec3,
    eye: Vec3,
    ahead: Vec3,
    /// Across the picture and down it, a pixel long at one unit ahead.
    right: Vec3,
    down: Vec3,
    /// The picture's middle, in pixels.
    middle: (f32, f32),
    detail: Detail,
}

impl World {
    pub fn of(scene: &Scene, t: f64, width: u32, height: u32, detail: Detail) -> Self {
        let mut solids = Vec::new();
        for (index, part) in scene.parts.iter().enumerate() {
            let Some(shape) = part.object.shape else {
                continue;
            };
            if scene.hidden(index, t) {
                continue;
            }
            let (place, size) = scene.placed(index, t, 0);
            let half = size * 0.5;
            let above = match part.object.anchor {
                Anchor::Base => half.y,
                Anchor::Middle => 0.0,
                Anchor::Top => -half.y,
            };
            solids.push(Solid {
                shape,
                centre: place.point(Vec3::new(0.0, above, 0.0)),
                turn: place.turn,
                half,
                reach: half.length(),
                shade: part.object.shade_now(t) as f32,
                owner: part.thing as u16,
            });
        }
        let View {
            eye,
            ahead,
            right,
            up,
            fov,
        } = scene.view(t);
        let pixel = 2.0 * (fov.to_radians() * 0.5).tan() / width.max(height) as f32;
        let [around, above] = scene.sun.unwrap_or(SUN).map(|d| d.to_radians() as f32);
        let light = Vec3::new(
            around.sin() * above.cos(),
            above.sin(),
            around.cos() * above.cos(),
        );
        let solids = Solids::new(solids);
        let (cols, rows) = (
            (width as usize).div_ceil(TILE),
            (height as usize).div_ceil(TILE),
        );
        let mut world = Self {
            shade: Shade::of(&solids.solids, light),
            tiles: Cells::empty(cols, rows),
            solids,
            ground: scene.ground,
            light,
            eye,
            ahead,
            right: right * pixel,
            down: up * -pixel,
            middle: (width as f32 * 0.5, height as f32 * 0.5),
            detail,
        };
        let reaches: Vec<_> = (world.solids.solids.iter())
            .map(|solid| world.tiles_of(solid))
            .collect();
        world.tiles = Cells::new(cols, rows, &reaches);
        world
    }

    /// The tiles of the picture `solid` may show in.
    fn tiles_of(&self, solid: &Solid) -> Option<Reach> {
        let (cols, rows) = (self.tiles.cols as f32, self.tiles.rows as f32);
        let mut behind = 0;
        let (mut low, mut high) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for corner in solid.corners() {
            match self.pixel_of(corner) {
                Some((x, y, _)) => {
                    let (x, y) = (x / TILE as f32, y / TILE as f32);
                    (low, high) = ((low.0.min(x), low.1.min(y)), (high.0.max(x), high.1.max(y)));
                }
                None => behind += 1,
            }
        }
        match behind {
            0 => self.tiles.reach(low, high),
            8 => None,
            // Partly behind the camera, it may show anywhere.
            _ => self.tiles.reach((0.0, 0.0), (cols - 1.0, rows - 1.0)),
        }
    }

    pub fn eye(&self) -> Vec3 {
        self.eye
    }

    /// Where a point of the scene is in the picture, in pixels, and how
    /// far ahead of the camera; `None` behind it.
    pub fn pixel_of(&self, point: Vec3) -> Option<(f32, f32, f32)> {
        let to = point - self.eye;
        let depth = to.dot(self.ahead);
        if depth < 0.05 {
            return None;
        }
        let scale = 1.0 / (depth * self.right.dot(self.right));
        Some((
            self.middle.0 + to.dot(self.right) * scale,
            self.middle.1 + to.dot(self.down) * scale,
            depth,
        ))
    }

    /// What stands between the camera and `point` of object `index`:
    /// `None` for nothing, `Some(None)` for the ground, else the object.
    pub fn in_front(&self, point: Vec3, index: u16) -> Option<Option<u16>> {
        let to = point - self.eye;
        let reach = to.length();
        let hit = self.first(self.eye, to.unit_or(self.ahead), reach * 0.999)?;
        (hit.owner != Some(index)).then_some(hit.owner)
    }

    /// What shows at pixel (`x`, `y`): `None` for the sky and the ground.
    pub fn thing_at(&self, x: f32, y: f32) -> Option<u16> {
        self.seen(x, y, self.ray(x, y))?.owner
    }

    /// Whether the camera is inside an object, and which.
    pub fn inside(&self) -> Option<u16> {
        // From inside a closed solid, every way out meets it from behind.
        let ways = [Vec3::UP, self.ahead, self.right.unit_or(Vec3::UP)];
        let met = |dir: Vec3| self.solids.first(self.eye, dir, f32::INFINITY);
        let owner = met(ways[0])
            .filter(|hit| hit.normal.dot(ways[0]) > 0.0)?
            .owner?;
        ways[1..]
            .iter()
            .all(|dir| met(*dir).is_some_and(|hit| hit.owner == Some(owner)))
            .then_some(owner)
    }

    /// The first thing the ray meets within `reach`: a solid, else the
    /// ground.
    fn first(&self, origin: Vec3, dir: Vec3, reach: f32) -> Option<Hit> {
        let first = self.solids.first(origin, dir, reach);
        self.or_floor(first, origin, dir, reach)
    }

    /// `first`, the solid a ray met, or the ground where that is nearer
    /// and within `reach`.
    fn or_floor(&self, first: Option<Hit>, origin: Vec3, dir: Vec3, reach: f32) -> Option<Hit> {
        let floor = (self.ground && dir.y < -1e-6 && origin.y > 0.0).then(|| -origin.y / dir.y);
        let nearest = first.as_ref().map_or(reach, |f| f.distance);
        match floor {
            Some(distance) if distance < nearest => {
                let at = origin + dir * distance;
                Some(Hit {
                    distance,
                    normal: Vec3::UP,
                    shade: floor_grey(at, distance),
                    owner: None,
                    flat: true,
                })
            }
            _ => first,
        }
    }

    /// What the camera sees along `dir` through pixel (`x`, `y`): only
    /// the solids of that pixel's tile are tried.
    fn seen(&self, x: f32, y: f32, dir: Vec3) -> Option<Hit> {
        let mut first: Option<Hit> = None;
        for index in self.tiles.at(x / TILE as f32, y / TILE as f32) {
            let solid = &self.solids.solids[*index as usize];
            if let Some((distance, normal)) = solid.hit(self.eye, dir)
                && first.as_ref().is_none_or(|f| distance < f.distance)
            {
                first = Some(Hit {
                    distance,
                    normal,
                    shade: solid.shade,
                    owner: Some(solid.owner),
                    flat: solid.shape.is_flat(),
                });
            }
        }
        self.or_floor(first, self.eye, dir, f32::INFINITY)
    }

    /// Whether a solid stands between `at` and the light.
    fn shadowed(&self, at: Vec3) -> bool {
        (self.shade.toward_light(at).iter()).any(|index| {
            self.solids.solids[*index as usize]
                .hit(at, self.light)
                .is_some()
        })
    }

    /// How open the surface at `at`, facing `normal`, is to the sky: 1 in
    /// the open, less where another surface is close.
    fn open(&self, at: Vec3, normal: Vec3) -> f32 {
        let turn = Turn::along(normal, Vec3::new(0.31, 0.2, 0.93));
        // Five ways out, leaning away from the surface all round it.
        let closed: f32 = (0..5)
            .map(|i| {
                let (sin, cos) = (i as f32 * 1.2566 + 0.4).sin_cos();
                let way = turn.outward(Vec3::new(cos * 0.74, 0.67, sin * 0.74));
                let near = self.first(at, way, CLOSE).map_or(CLOSE, |hit| hit.distance);
                let closeness = 1.0 - near / CLOSE;
                closeness * closeness
            })
            .sum();
        1.0 - CLOSE_DARK * closed / 5.0
    }

    /// The way the ray through pixel (`x`, `y`) goes.
    fn ray(&self, x: f32, y: f32) -> Vec3 {
        let dir = self.ahead + self.right * (x - self.middle.0) + self.down * (y - self.middle.1);
        dir.unit_or(self.ahead)
    }

    /// Where `sample`, found through pixel (`x`, `y`), is to be asked how
    /// open it is, and the way its surface faces; `None` when it isn't.
    fn probe(&self, sample: &Sample, x: f32, y: f32) -> Option<(Vec3, Vec3)> {
        let at = self.eye + self.ray(x, y) * sample.probed;
        (sample.probed > 0.0).then(|| (at + sample.normal * (NEAR * 4.0), sample.normal))
    }

    /// What the ray through pixel (`x`, `y`) finds.
    fn sample(&self, x: f32, y: f32) -> Sample {
        let dir = self.ray(x, y);
        let sky = match dir.y {
            up if up >= 0.0 => SKY_HORIZON + (SKY_ZENITH - SKY_HORIZON) * up.sqrt(),
            down => SKY_HORIZON - 0.12 * (-down).sqrt(),
        };
        let Some(hit) = self.seen(x, y, dir) else {
            return Sample {
                lit: sky,
                ..Sample::default()
            };
        };
        let at = self.eye + dir * hit.distance;
        // Seen from behind, a surface is lit as its other side.
        let normal = if hit.normal.dot(dir) > 0.0 {
            -hit.normal
        } else {
            hit.normal
        };
        let off = at + normal * (NEAR * 4.0);
        let facing = normal.dot(self.light).max(0.0);
        let shadowed = facing > 0.0 && self.shadowed(off);
        let lit = if shadowed { 0.0 } else { facing };
        // Its own grey under the light, the sky's fill from above, and a
        // darker rim where it turns away from the camera; then the haze.
        let fill = 0.5 + 0.5 * normal.y;
        let rim = 0.86 + 0.14 * normal.dot(dir).abs();
        let haze = 1.0 - (-hit.distance / HAZE).exp();
        let own = hit.shade * rim * (1.0 - haze);
        // Only flat surfaces: on a round one so few rays show as stripes.
        let probed = self.detail == Detail::Full && hit.flat && self.solids.near(off, CLOSE);
        Sample {
            lit: own * LIGHT * lit + SKY_HORIZON * haze,
            ambient: own * (SHADE + 0.16 * fill),
            probed: if probed { hit.distance } else { 0.0 },
            normal,
        }
    }

    /// The picture, `row` × `rows` pixels, in full: surfaces darkened
    /// where another is close, and edges drawn again from four rays a
    /// pixel so they don't step.
    fn finished(&self, row: usize, rows: usize) -> Vec<f32> {
        let middle = |x: usize, y: usize| (x as f32 + 0.5, y as f32 + 0.5);
        let mut samples = vec![Sample::default(); row * rows];
        in_bands(&mut samples, row, |first, band| {
            for (i, sample) in band.iter_mut().enumerate() {
                let (x, y) = middle(i % row, first + i / row);
                *sample = self.sample(x, y);
            }
        });
        let probe = |x: usize, y: usize| {
            let (px, py) = middle(x, y);
            self.probe(&samples[y * row + x], px, py)
        };
        // How open each second pixel of each second row is: it changes
        // slowly across a surface, and is most of the work.
        let half = row.div_ceil(2);
        let mut coarse = vec![1f32; half * rows.div_ceil(2)];
        in_bands(&mut coarse, half, |first, band| {
            for (i, open) in band.iter_mut().enumerate() {
                if let Some((at, normal)) = probe(i % half * 2, (first + i / half) * 2) {
                    *open = self.open(at, normal);
                }
            }
        });
        // Each pixel takes it from those around it on the same surface.
        let mut opens = vec![1f32; samples.len()];
        in_bands(&mut opens, row, |first, band| {
            for (i, open) in band.iter_mut().enumerate() {
                let (x, y) = (i % row, first + i / row);
                let Some((at, normal)) = probe(x, y) else {
                    continue;
                };
                let (mut sum, mut weight) = (0.0, 0.0);
                for (cx, cy) in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
                    let (cx, cy) = (cx / 2, cy / 2);
                    let same = cx < half && cy * 2 < rows && {
                        let other = &samples[cy * 2 * row + cx * 2];
                        other.probed > 0.0 && other.normal.dot(normal) > 0.95
                    };
                    if same {
                        sum += coarse[cy * half + cx];
                        weight += 1.0;
                    }
                }
                *open = match weight > 0.0 {
                    true => sum / weight,
                    false => self.open(at, normal),
                };
            }
        });
        let greys: Vec<f32> = (samples.iter().zip(&opens))
            .map(|(sample, open)| sample.grey(*open))
            .collect();
        let mut smooth = greys.clone();
        in_bands(&mut smooth, row, |first, band| {
            for (i, grey) in band.iter_mut().enumerate() {
                let (x, y) = (i % row, first + i / row);
                let beside = [
                    (x > 0).then(|| greys[y * row + x - 1]),
                    (x + 1 < row).then(|| greys[y * row + x + 1]),
                    (y > 0).then(|| greys[(y - 1) * row + x]),
                    (y + 1 < rows).then(|| greys[(y + 1) * row + x]),
                ];
                let edge = (beside.iter().flatten()).any(|other| (other - *grey).abs() > EDGE);
                if edge {
                    let spots = [
                        (0.375, 0.125),
                        (0.875, 0.375),
                        (0.125, 0.625),
                        (0.625, 0.875),
                    ];
                    let open = opens[y * row + x];
                    let sum: f32 = (spots.iter())
                        .map(|(dx, dy)| {
                            let sample = self.sample(x as f32 + dx, y as f32 + dy);
                            sample.grey(if sample.probed > 0.0 { open } else { 1.0 })
                        })
                        .sum();
                    *grey = sum / 4.0;
                }
            }
        });
        smooth
    }
}

/// The ground's grey at `at`: metre tiles a touch lighter and darker, so
/// a camera move reads against it, fading out with `distance` before they
/// shimmer.
fn floor_grey(at: Vec3, distance: f32) -> f32 {
    let odd = ((at.x.floor() as i64 + at.z.floor() as i64) & 1) as f32;
    let fade = (-distance / 30.0).exp();
    GROUND_GREY + (odd - 0.5) * 0.07 * fade
}
