//! A scene at one moment, as rays meet it: its solids where they stand,
//! sorted so a ray tries few of them, the camera's rays through the
//! picture, and the way to the light.

use super::Scene;
use super::motion::{Moment, View};
use super::render::to_light;
use super::solid::Solid;
use super::space::{Turn, Vec3};

/// Where the light comes from unless a scene says: around the vertical
/// from +Z, and above the horizon, in degrees.
const SUN: [f64; 2] = [47.0, 55.0];
/// How wide the sun is: the slope of its edge off its middle. Wider than
/// the real one, so a shadow's edge softens within a few metres.
const SUN_WIDTH: f32 = 0.02;
/// The ground's grey, and how far its metre tiles are off it.
const GROUND_GREY: f32 = 0.5;
const TILE_GREY: f32 = 0.035;
/// Pixels along the side of a tile of the picture that lists its solids.
const TILE: usize = 16;
/// The most cells along a side of the light's grid.
const LIGHT_CELLS: usize = 96;
/// The most solids a leaf of the tree holds.
const LEAF: usize = 2;

/// What a ray met.
pub(super) struct Hit {
    pub distance: f32,
    pub normal: Vec3,
    /// How much of the light on it the surface gives back.
    pub tone: f32,
    /// The object met, and which of the scene's solids; `None` for the
    /// ground.
    pub owner: Option<u16>,
    pub solid: Option<u32>,
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
            for (index, solid) in self.solids[from..from + count].iter().enumerate() {
                if let Some((distance, normal)) = solid.hit(origin, dir)
                    && distance < reach
                {
                    reach = distance;
                    first = Some(Hit {
                        distance,
                        normal,
                        tone: solid.tone,
                        owner: Some(solid.owner),
                        solid: Some((from + index) as u32),
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
/// travels, so a ray toward the sun, its middle or its edge, tries only
/// what stands in its way.
struct Shade {
    cells: Cells,
    /// Across and up the grid, and its corner and cell size along them.
    across: Vec3,
    up: Vec3,
    corner: (f32, f32),
    cell: f32,
}

impl Shade {
    fn of(solids: &[Solid], light: Vec3, ground: bool) -> Self {
        let turn = Turn::along(light, Vec3::new(0.3, 0.1, 0.95));
        let (across, up) = (turn.x, turn.z);
        let flat = |point: Vec3| (point.dot(across), point.dot(up));
        // How far along the light the scene goes: nothing is shadowed
        // from further.
        let along = |solid: &Solid| solid.corners().map(|corner| corner.dot(light));
        let least = |a: f32, b: [f32; 8]| b.into_iter().fold(a, f32::min);
        let deepest = (solids.iter()).fold(f32::MAX, |deep, solid| least(deep, along(solid)));
        // Each solid's box as the light sees it, wider by as far as a ray
        // toward the sun's edge strays before it is past what the solid
        // could shadow.
        let boxes: Vec<_> = (solids.iter())
            .map(|solid| {
                let corners = solid.corners();
                let top = (corners.iter()).fold(f32::MIN, |top, corner| top.max(corner.dot(light)));
                // Down to the ground, where there is one; else to the far
                // side of the scene.
                let shadows = match ground {
                    true => (corners.iter()).fold(0f32, |far, c| far.max(c.y / light.y.max(0.05))),
                    false => top - deepest,
                };
                let strays = SUN_WIDTH * shadows;
                let seen = corners.map(flat);
                let low = seen
                    .iter()
                    .fold((f32::MAX, f32::MAX), |a, b| (a.0.min(b.0), a.1.min(b.1)));
                let high = seen
                    .iter()
                    .fold((f32::MIN, f32::MIN), |a, b| (a.0.max(b.0), a.1.max(b.1)));
                (
                    (low.0 - strays, low.1 - strays),
                    (high.0 + strays, high.1 + strays),
                )
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
    /// What the ground's darker and lighter tiles give back of the light.
    floor: [f32; 2],
    /// The way to the light.
    pub light: Vec3,
    pub eye: Vec3,
    pub ahead: Vec3,
    /// Across the picture and down it, a pixel long at one unit ahead.
    right: Vec3,
    down: Vec3,
    /// The picture's middle, in pixels.
    middle: (f32, f32),
    /// Where everything stands.
    pub moment: Moment,
}

impl World {
    pub fn of(scene: &Scene, t: f64, width: u32, height: u32) -> Self {
        let moment = scene.moment(t, (width, height));
        let mut solids = Vec::new();
        for (index, part) in scene.parts.iter().enumerate() {
            let Some(shape) = part.object.shape else {
                continue;
            };
            if moment.gone[index] {
                continue;
            }
            let mut solid = Solid::standing(
                shape,
                part.object.anchor,
                moment.places[index],
                moment.sizes[index],
            )
            .tapered(part.object.taper as f32);
            solid.tone = to_light(part.moves.shade(t) as f32);
            solid.owner = part.thing as u16;
            solids.push(solid);
        }
        let View {
            eye,
            ahead,
            right,
            up,
            fov,
        } = moment.view;
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
            shade: Shade::of(&solids.solids, light, scene.ground),
            tiles: Cells::empty(cols, rows),
            solids,
            ground: scene.ground,
            floor: [-TILE_GREY, TILE_GREY].map(|off| to_light(GROUND_GREY + off)),
            light,
            eye,
            ahead,
            right: right * pixel,
            down: up * -pixel,
            middle: (width as f32 * 0.5, height as f32 * 0.5),
            moment,
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

    /// How high the lowest point of object `index` is; `None` when
    /// nothing of it is in the scene.
    pub fn lowest(&self, index: u16) -> Option<f32> {
        let own = self
            .solids
            .solids
            .iter()
            .filter(|solid| solid.owner == index);
        own.map(|solid| solid.lowest().y).reduce(f32::min)
    }

    /// Each pair of objects of which the first has the middle of one of
    /// its solids inside a solid of the second: it is in it, or through
    /// it. None in a scene of more than 400 solids.
    pub fn through(&self) -> Vec<(u16, u16)> {
        let solids = &self.solids.solids;
        // Every solid against every other: not worth it in a crowd.
        if solids.len() > 400 {
            return Vec::new();
        }
        let mut pairs: Vec<(u16, u16)> = solids
            .iter()
            .flat_map(|inner| {
                (solids.iter())
                    .filter(move |outer| outer.owner != inner.owner && outer.holds(inner.centre))
                    .map(move |outer| (inner.owner, outer.owner))
            })
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        pairs
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
                // Metre tiles a touch lighter and darker, so a camera move
                // reads against the ground, fading out before they shimmer.
                let odd = (at.x.floor() as i64 + at.z.floor() as i64) & 1;
                let even_tone = (self.floor[0] + self.floor[1]) * 0.5;
                let fade = (-distance / 30.0).exp();
                Some(Hit {
                    distance,
                    normal: Vec3::UP,
                    tone: even_tone + (self.floor[odd as usize] - even_tone) * fade,
                    owner: None,
                    solid: None,
                })
            }
            _ => first,
        }
    }

    /// What the camera sees along `dir` through pixel (`x`, `y`): only
    /// the solids of that pixel's tile are tried.
    pub fn seen(&self, x: f32, y: f32, dir: Vec3) -> Option<Hit> {
        let mut first: Option<Hit> = None;
        for index in self.tiles.at(x / TILE as f32, y / TILE as f32) {
            let solid = &self.solids.solids[*index as usize];
            if let Some((distance, normal)) = solid.hit(self.eye, dir)
                && first.as_ref().is_none_or(|f| distance < f.distance)
            {
                first = Some(Hit {
                    distance,
                    normal,
                    tone: solid.tone,
                    owner: Some(solid.owner),
                    solid: Some(*index),
                });
            }
        }
        self.or_floor(first, self.eye, dir, f32::INFINITY)
    }

    /// Whether the middle of the sun shows from `at`: no solid stands
    /// between.
    pub fn sunlit(&self, at: Vec3) -> bool {
        !(self.shade.toward_light(at).iter()).any(|index| {
            self.solids.solids[*index as usize]
                .hit(at, self.light)
                .is_some()
        })
    }

    /// Whether a spot on the sun shows from `at`: `spot` is across and up
    /// its disc from the middle, its edge one unit out.
    pub fn sunlit_from(&self, at: Vec3, spot: (f32, f32)) -> bool {
        let off = self.shade.across * spot.0 + self.shade.up * spot.1;
        let way = (self.light + off * SUN_WIDTH).unit_or(self.light);
        !(self.shade.toward_light(at).iter())
            .any(|index| self.solids.solids[*index as usize].hit(at, way).is_some())
    }

    /// Whether a solid is within `reach` of `point`.
    pub fn crowded(&self, point: Vec3, reach: f32) -> bool {
        self.solids.near(point, reach)
    }

    /// The way the ray through pixel (`x`, `y`) goes.
    pub fn ray(&self, x: f32, y: f32) -> Vec3 {
        let dir = self.ahead + self.right * (x - self.middle.0) + self.down * (y - self.middle.1);
        dir.unit_or(self.ahead)
    }

    /// The point seen through pixel (`x`, `y`), `depth` ahead of the
    /// camera.
    pub fn point(&self, x: f32, y: f32, depth: f32) -> Vec3 {
        let way = self.ahead + self.right * (x - self.middle.0) + self.down * (y - self.middle.1);
        self.eye + way * depth
    }

    /// How large a pixel is one unit ahead of the camera.
    pub fn pixel(&self) -> f32 {
        self.right.length()
    }

    /// The picture's right and its top, as directions of the scene.
    pub fn across(&self) -> (Vec3, Vec3) {
        (self.right.unit_or(Vec3::UP), (-self.down).unit_or(Vec3::UP))
    }
}
