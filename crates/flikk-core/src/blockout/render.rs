//! Drawing a scene: one ray a pixel against its solids, lit by a sun and
//! the sky, with a line round each thing and its parts, so depth, contact
//! and the camera's move read at a glance. Greys only: the look is the
//! video model's to make.
//!
//! A full picture also softens the edge of a shadow as the sun's width
//! does, darkens surfaces where another is close, and draws edges again
//! from several rays. Light is worked out as light, and turned into the
//! greys a screen shows at the end.

use super::Scene;
use super::space::Vec3;
use super::world::World;
use crate::services::Frame;
use std::f32::consts::TAU;
use std::sync::{Mutex, OnceLock};

/// Pictures smaller than this are drawn on the calling thread.
const THREADED_PIXELS: usize = 32_768;
/// Rows a thread draws before it takes more.
const BAND: usize = 8;
/// Light, as shares of what a white surface facing it gives back: the
/// sun's; the sky's, on what faces up; the ground's, on what faces down;
/// and a fill from across the sun, so the side the sun doesn't reach
/// keeps its form.
const SUN_LIGHT: f32 = 0.7;
const SKY_LIGHT: f32 = 0.3;
const GROUND_LIGHT: f32 = 0.12;
const FILL_LIGHT: f32 = 0.2;
/// The sky at the horizon and overhead, as greys on a screen.
const SKY_HORIZON: f32 = 0.76;
const SKY_ZENITH: f32 = 0.9;
/// Metres at which the haze has taken about two thirds of a surface.
const HAZE: f32 = 140.0;
/// Metres within which one surface darkens another it is close to, the
/// most pixels of the picture that is looked for across, and how many
/// looks are taken.
const CLOSE: f32 = 0.7;
const CLOSE_PIXELS: f32 = 40.0;
const LOOKS: usize = 8;
/// How much darker the closest surfaces get.
const CLOSE_DARK: f32 = 0.6;
/// Rays toward the sun from a place its edge shows from.
const SUNS: usize = 8;
/// How much darker than what it runs between the line round a thing is,
/// and the one between its parts.
const OUTLINE: f32 = 0.65;
const INLINE: f32 = 0.3;
/// The longer side, in pixels, of a picture whose lines are drawn in
/// full: in a smaller one a pixel is more of the picture, and they are
/// fainter.
const LINE_SIDE: f32 = 640.0;
/// The most an edge that sticks out is lightened, and one that goes in
/// darkened.
const RIDGE: f32 = 0.2;
const VALLEY: f32 = 0.25;
/// Where in a pixel an edge is drawn again from.
const SPOTS: [(f32, f32); 4] = [
    (0.375, 0.125),
    (0.875, 0.375),
    (0.125, 0.625),
    (0.625, 0.875),
];

/// How carefully a picture is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// One ray a pixel: fast enough to play.
    Draft,
    /// Smooth edges, soft shadows, and surfaces darkened where something
    /// is close: for the clip and for a frame that stays on screen.
    Full,
}

impl Scene {
    /// The scene at `t` seconds, `width` × `height` pixels, opaque.
    pub fn render(&self, t: f64, width: u32, height: u32, detail: Detail) -> Frame {
        let (width, height) = (width.max(1), height.max(1));
        let world = World::of(self, t.clamp(0.0, self.duration), width, height);
        let picture = Picture::of(&world, width as usize, height as usize);
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        let mut sheets = Sheets::spare();
        picture.draw(detail, &mut sheets, rgba.as_chunks_mut::<4>().0);
        sheets.keep();
        Frame {
            width,
            height,
            rgba: rgba.into(),
        }
    }
}

/// The light a surface of grey `grey` (0 to 1 on a screen) gives back.
pub(super) fn to_light(grey: f32) -> f32 {
    match grey.clamp(0.0, 1.0) {
        dark if dark <= 0.04045 => dark / 12.92,
        grey => ((grey + 0.055) / 1.055).powf(2.4),
    }
}

/// The grey a screen shows for each of 1025 lights from none to full.
fn screen_greys() -> &'static [f32; 1025] {
    static GREYS: OnceLock<[f32; 1025]> = OnceLock::new();
    GREYS.get_or_init(|| {
        std::array::from_fn(|i| match i as f32 / 1024.0 {
            dark if dark <= 0.003_130_8 => dark * 12.92,
            light => 1.055 * light.powf(1.0 / 2.4) - 0.055,
        })
    })
}

/// A number from 0 to 1 for a pixel, unlike its neighbours' and evenly
/// spread among them (Jimenez's interleaved gradient noise).
fn scatter(x: usize, y: usize) -> f32 {
    (52.982_918 * (0.067_110_56 * x as f32 + 0.005_837_15 * y as f32).fract()).fract()
}

/// What a thing or a solid is when it is none: the sky, or the ground.
const SKY: u16 = 0;
const GROUND: u16 = 1;
const SOLIDS: u16 = 2;

/// What one ray from the camera found.
#[derive(Clone, Copy, Default)]
struct Seen {
    /// The light it gives whatever the sun does, what the sun adds where
    /// all of it reaches, and the light of the haze before it (for the
    /// sky, its own).
    still: f32,
    sunned: f32,
    veil: f32,
    /// How far ahead of the camera it is; the sky is not.
    depth: f32,
    /// The way its surface faces, toward the camera: each part of it in
    /// 32767ths.
    faces: [i16; 3],
    /// Whether the sun, where it was looked for, reaches it.
    sun: bool,
    /// The thing a viewer would name it as, and the solid it is: each
    /// [`SKY`], [`GROUND`], or counted on from [`SOLIDS`].
    thing: u16,
    solid: u16,
}

impl Seen {
    /// Its light, `open` to the sky and with `sun` of the sun on it.
    fn light(&self, open: f32, sun: f32) -> f32 {
        self.veil + self.still * open + self.sunned * sun * (0.5 + 0.5 * open)
    }

    /// The way its surface faces.
    fn normal(&self) -> Vec3 {
        let [x, y, z] = self.faces.map(|part| part as f32 / i16::MAX as f32);
        Vec3::new(x, y, z)
    }

    /// Whether it is the sky or the ground: what things stand before.
    fn is_backdrop(&self) -> bool {
        self.thing < SOLIDS
    }

    /// Whether `other` is of the same solid, facing much the same way.
    fn same_surface(&self, other: &Seen) -> bool {
        // Nine tenths of the way to facing the same, in 32767ths squared.
        const ALIKE: i32 = 966_300_000;
        let alike = || {
            let along = |part: usize| self.faces[part] as i32 * other.faces[part] as i32;
            along(0) + along(1) + along(2) > ALIKE
        };
        self.solid == other.solid && (self.solid == SKY || alike())
    }

    /// It as the whole of its pixel: where no edge runs through.
    fn front(&self) -> Front {
        Front {
            depth: self.depth,
            solid: self.solid,
            thing: self.thing,
            fills: 4,
        }
    }

    /// As it is drawn where no edge runs through its pixel.
    fn plain(&self, light: f32) -> Drawn {
        Drawn {
            light,
            front_light: light,
            front: self.front(),
        }
    }
}

/// What is nearest the camera in a pixel: a solid (the backdrop only
/// where there is none), how far ahead, the thing it is of, and how many
/// quarters of the pixel it fills.
#[derive(Clone, Copy, Default)]
struct Front {
    depth: f32,
    solid: u16,
    thing: u16,
    fills: u8,
}

/// One pixel as it is drawn, short of the lines that reach it from the
/// pixels beside it.
#[derive(Clone, Copy, Default)]
struct Drawn {
    light: f32,
    /// What is nearest the camera in it, and the light of that alone.
    front_light: f32,
    front: Front,
}

/// What a picture is drawn in: a value a pixel for each stage of it. A
/// frame's worth is tens of megabytes, slow to get from the system each
/// time, so one set is kept from a picture to the next.
#[derive(Default)]
struct Sheets {
    seens: Vec<Seen>,
    drawn: Vec<Drawn>,
    suns: Vec<f32>,
    opens: Vec<f32>,
}

static SPARE: Mutex<Option<Sheets>> = Mutex::new(None);

impl Sheets {
    /// The set kept from the last picture, or a new one.
    fn spare() -> Self {
        let kept = SPARE.lock().ok().and_then(|mut spare| spare.take());
        kept.unwrap_or_default()
    }

    /// Keep this set for the next picture.
    fn keep(self) {
        if let Ok(mut spare) = SPARE.lock() {
            *spare = Some(self);
        }
    }
}

/// `sheet` with `len` values: whatever it held is drawn over.
fn sized<T: Clone + Default>(sheet: &mut Vec<T>, len: usize) -> &mut [T] {
    if sheet.len() != len {
        sheet.clear();
        sheet.resize(len, T::default());
    }
    sheet
}

/// A picture of a scene at one moment being drawn, `row` × `rows` pixels.
struct Picture<'a> {
    world: &'a World,
    row: usize,
    rows: usize,
    /// The way the fill comes from: level from across the sun, a little
    /// from above.
    fill: Vec3,
    /// The light of the sky at the horizon and overhead.
    horizon: f32,
    zenith: f32,
    /// The picture's right and its top, as directions of the scene.
    right: Vec3,
    up: Vec3,
    /// How much of their darkness lines are drawn with: less in a small
    /// picture, where a pixel is more of it.
    faint: f32,
}

impl<'a> Picture<'a> {
    fn of(world: &'a World, row: usize, rows: usize) -> Self {
        let across = Vec3::new(-world.light.x, 0.0, -world.light.z).unit_or(Vec3::AHEAD);
        let (right, up) = world.across();
        Self {
            world,
            row,
            rows,
            fill: (across + Vec3::UP * 0.35).unit_or(across),
            horizon: to_light(SKY_HORIZON),
            zenith: to_light(SKY_ZENITH),
            right,
            up,
            faint: (row.max(rows) as f32 / LINE_SIDE).clamp(0.4, 1.0),
        }
    }

    /// Draw the picture into `pixels`, using `sheets` on the way.
    fn draw(&self, detail: Detail, sheets: &mut Sheets, pixels: &mut [[u8; 4]]) {
        let count = self.row * self.rows;
        let soft = detail == Detail::Full;
        let seens = sized(&mut sheets.seens, count);
        self.each(seens, |x, y, seen| {
            let spot = soft.then(|| sun_spot((scatter(y, x) * SUNS as f32) as usize, x, y));
            *seen = self.look(x as f32 + 0.5, y as f32 + 0.5, spot);
        });
        let seens = &*seens;
        let screen = screen_greys();
        let shown = |x: usize, y: usize, light: f32| {
            // Between two of the screen's greys, some pixels take one and
            // some the other, so a slow gradient shows no steps.
            let step = light.clamp(0.0, 1.0) * (screen.len() - 1) as f32;
            let (from, to) = (screen[step as usize], screen[(step as usize + 1).min(1024)]);
            let grey = from + (to - from) * step.fract();
            let value = (grey * 255.0 + scatter(x, y)).clamp(0.0, 255.0) as u8;
            [value, value, value, 255]
        };
        if !soft {
            return self.each(pixels, |x, y, pixel| {
                let seen = &seens[y * self.row + x];
                let one_ray = |at: usize| seens[at].light(1.0, f32::from(seens[at].sun));
                let here = seen.plain(one_ray(y * self.row + x));
                let fronts = (&|at: usize| seens[at].front(), &one_ray);
                *pixel = shown(x, y, self.lined(seens, here, fronts, x, y));
            });
        }
        let suns = sized(&mut sheets.suns, count);
        self.each(suns, |x, y, sun| *sun = self.sun(seens, x, y));
        let half = self.row.div_ceil(2);
        let opens = sized(&mut sheets.opens, half * self.rows.div_ceil(2));
        self.each_of(opens, half, |x, y, open| {
            *open = self.open(seens, x * 2, y * 2)
        });
        let (suns, opens) = (&*suns, &*opens);
        let drawn = sized(&mut sheets.drawn, count);
        self.each(drawn, |x, y, pixel| {
            *pixel = self.finished(seens, suns, opens, x, y);
        });
        let drawn = &*drawn;
        self.each(pixels, |x, y, pixel| {
            let here = drawn[y * self.row + x];
            let fronts = (&|at: usize| drawn[at].front, &|at: usize| {
                drawn[at].front_light
            });
            *pixel = shown(x, y, self.lined(seens, here, fronts, x, y));
        });
    }

    /// Run `draw` for each pixel of `values`, shared out among threads.
    fn each<T: Send>(&self, values: &mut [T], draw: impl Fn(usize, usize, &mut T) + Sync) {
        self.each_of(values, self.row, draw);
    }

    /// Run `draw` for each of `values`, rows of `row`, a few rows at a
    /// time, shared out among threads.
    fn each_of<T: Send>(
        &self,
        values: &mut [T],
        row: usize,
        draw: impl Fn(usize, usize, &mut T) + Sync,
    ) {
        let band = |first: usize, values: &mut [T]| {
            for (i, value) in values.iter_mut().enumerate() {
                draw(i % row, first + i / row, value);
            }
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(16));
        if threads == 1 || values.len() < THREADED_PIXELS {
            return band(0, values);
        }
        // Small lots, taken as each thread is free: the sky draws in no
        // time and the ground doesn't, so even shares would leave threads
        // idle.
        let lots = std::sync::Mutex::new(values.chunks_mut(BAND * row).enumerate());
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    loop {
                        let lot = lots.lock().ok().and_then(|mut lots| lots.next());
                        let Some((i, rows)) = lot else {
                            break;
                        };
                        band(i * BAND, rows);
                    }
                });
            }
        });
    }

    /// What is seen at pixel (`x`, `y`) of `seens`; `None` off the picture.
    fn at<'s>(&self, seens: &'s [Seen], x: isize, y: isize) -> Option<&'s Seen> {
        let inside = x >= 0 && y >= 0 && (x as usize) < self.row && (y as usize) < self.rows;
        inside.then(|| &seens[y as usize * self.row + x as usize])
    }

    /// What the ray through (`x`, `y`) of the picture finds. The sun is
    /// looked for at `spot` on its disc, or at its middle.
    fn look(&self, x: f32, y: f32, spot: Option<(f32, f32)>) -> Seen {
        let world = self.world;
        let dir = world.ray(x, y);
        let Some(hit) = world.seen(x, y, dir) else {
            // Lighter overhead, and darker under the horizon where there
            // is no ground.
            let over = (self.zenith - self.horizon) * dir.y.max(0.0).sqrt();
            let under = 0.3 * self.horizon * (-dir.y).max(0.0).sqrt();
            return Seen {
                veil: self.horizon + over - under,
                depth: f32::INFINITY,
                ..Seen::default()
            };
        };
        let at = world.eye + dir * hit.distance;
        // Seen from behind, a surface is lit as its other side.
        let normal = if hit.normal.dot(dir) > 0.0 {
            -hit.normal
        } else {
            hit.normal
        };
        let off = at + normal * (super::solid::NEAR * 4.0);
        let facing = normal.dot(world.light).max(0.0);
        let sun = match spot {
            _ if facing <= 0.0 => false,
            Some(spot) => world.sunlit_from(off, spot),
            None => world.sunlit(off),
        };
        // The fill reaches round a surface that turns away from it.
        let filled = ((normal.dot(self.fill) + 0.5) / 1.5).clamp(0.0, 1.0);
        let around = SKY_LIGHT * (0.5 + 0.5 * normal.y)
            + GROUND_LIGHT * (0.5 - 0.5 * normal.y)
            + FILL_LIGHT * filled * filled;
        // A little darker where it turns away from the camera; then the
        // haze.
        let rim = 0.86 + 0.14 * normal.dot(dir).abs();
        let clear = (-hit.distance / HAZE).exp();
        let tone = hit.tone * rim * clear;
        Seen {
            still: tone * around,
            sunned: tone * SUN_LIGHT * facing,
            veil: self.horizon * (1.0 - clear),
            depth: hit.distance * dir.dot(world.ahead),
            faces: [normal.x, normal.y, normal.z].map(|part| (part * i16::MAX as f32) as i16),
            sun,
            thing: hit.owner.map_or(GROUND, |owner| SOLIDS + owner),
            solid: hit.solid.map_or(GROUND, |solid| SOLIDS + solid as u16),
        }
    }

    /// Whether what `of` says of each pixel differs around pixel (`x`,
    /// `y`) on its surface: each second pixel out is asked.
    fn differs(&self, seens: &[Seen], of: impl Fn(usize) -> f32, x: usize, y: usize) -> bool {
        const AROUND: [(isize, isize); 8] = [
            (-2, -2),
            (0, -2),
            (2, -2),
            (-2, 0),
            (2, 0),
            (-2, 2),
            (0, 2),
            (2, 2),
        ];
        let here = y * self.row + x;
        AROUND.iter().any(|(dx, dy)| {
            let (nx, ny) = (x as isize + dx, y as isize + dy);
            (self.at(seens, nx, ny)).is_some_and(|other| {
                let there = ny as usize * self.row + nx as usize;
                (of(there) - of(here)).abs() > 0.01 && other.same_surface(&seens[here])
            })
        })
    }

    /// How much of the sun reaches pixel (`x`, `y`). Where the one ray
    /// toward it says otherwise than the pixels around, its edge shows
    /// from there, and more rays say how much of it.
    fn sun(&self, seens: &[Seen], x: usize, y: usize) -> f32 {
        let seen = &seens[y * self.row + x];
        let first = f32::from(seen.sun);
        let one_ray = |at: usize| f32::from(seens[at].sun);
        if seen.sunned <= 0.0 || !self.differs(seens, one_ray, x, y) {
            return first;
        }
        let at = self.world.point(x as f32 + 0.5, y as f32 + 0.5, seen.depth)
            + seen.normal() * (super::solid::NEAR * 4.0);
        let taken = (scatter(y, x) * SUNS as f32) as usize;
        let more: f32 = (0..SUNS)
            .filter(|spot| *spot != taken)
            .map(|spot| f32::from(self.world.sunlit_from(at, sun_spot(spot, x, y))))
            .sum();
        (first + more) / SUNS as f32
    }

    /// How open to the sky the surface at pixel (`x`, `y`) is: 1 in the
    /// open, less where the picture shows another surface close before
    /// it.
    fn open(&self, seens: &[Seen], x: usize, y: usize) -> f32 {
        let world = self.world;
        let seen = &seens[y * self.row + x];
        if seen.thing == SKY {
            return 1.0;
        }
        let reach = (CLOSE / (seen.depth * world.pixel())).min(CLOSE_PIXELS);
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let at = world.point(px, py, seen.depth);
        if reach < 1.5 || (seen.thing == GROUND && !world.crowded(at, CLOSE)) {
            return 1.0;
        }
        let normal = seen.normal();
        // Out along a spiral, turned its own way at each pixel.
        let (sin, cos) = (scatter(x, y) * TAU).sin_cos();
        let closed: f32 = (spiral().iter())
            .filter_map(|(across, down)| {
                let (across, down) = (across * cos - down * sin, across * sin + down * cos);
                let (qx, qy) = ((px + reach * across).floor(), (py + reach * down).floor());
                let other = self.at(seens, qx as isize, qy as isize)?;
                let to = world.point(qx + 0.5, qy + 0.5, other.depth) - at;
                let apart = to.dot(to);
                let over = to.dot(normal) - 0.01 - 0.002 * seen.depth;
                (other.thing != SKY && over > 0.0 && apart < CLOSE * CLOSE)
                    .then(|| over / apart.sqrt() * (1.0 - apart / (CLOSE * CLOSE)))
            })
            .sum();
        1.0 - CLOSE_DARK * (closed / LOOKS as f32 * 3.0).min(1.0)
    }

    /// Pixel (`x`, `y`) in full: as open as the pixels around say, under
    /// as much of the sun as they say, and drawn again from four rays
    /// where an edge runs through it.
    fn finished(&self, seens: &[Seen], suns: &[f32], opens: &[f32], x: usize, y: usize) -> Drawn {
        let here = y * self.row + x;
        let seen = &seens[here];
        let (ix, iy) = (x as isize, y as isize);
        let beside = [(-1, 0), (1, 0), (0, -1), (0, 1)];
        let edge = (beside.iter()).any(|(dx, dy)| {
            (self.at(seens, ix + dx, iy + dy)).is_some_and(|other| !other.same_surface(seen))
        });
        if seen.thing == SKY && !edge {
            return seen.plain(seen.veil);
        }
        let around = |reach: isize| {
            (-reach..=reach).flat_map(move |dy| (-reach..=reach).map(move |dx| (dx, dy)))
        };
        // How open, from those around that were asked on this surface.
        let half = self.row.div_ceil(2);
        let asked = around(1).filter_map(|(dx, dy)| {
            let (cx, cy) = (ix / 2 + dx, iy / 2 + dy);
            let other = self.at(seens, cx * 2, cy * 2)?;
            (other.same_surface(seen)).then(|| opens[cy as usize * half + cx as usize])
        });
        let (sum, count) = asked.fold((0.0, 0), |(sum, count), open| (sum + open, count + 1));
        let open = match count {
            _ if seen.thing == SKY => 1.0,
            0 => self.open(seens, x, y),
            count => sum / count as f32,
        };
        // The sun's edge is evened out among those around.
        let sun = match seen.sunned > 0.0 && self.differs(seens, |at| suns[at], x, y) {
            true => {
                let near = around(2).filter_map(|(dx, dy)| {
                    let other = self.at(seens, ix + dx, iy + dy)?;
                    (other.same_surface(seen))
                        .then(|| suns[(iy + dy) as usize * self.row + (ix + dx) as usize])
                });
                let (sum, count) = near.fold((0.0, 0), |(sum, count), sun| (sum + sun, count + 1));
                sum / count.max(1) as f32
            }
            false => suns[here],
        };
        if !edge {
            return seen.plain(seen.light(open, sun));
        }
        let spots = SPOTS.map(|(dx, dy)| self.look(x as f32 + dx, y as f32 + dy, None));
        let lights = spots.map(|spot| match spot.solid == seen.solid {
            true => spot.light(open, sun),
            false => spot.light(open, f32::from(spot.sun)),
        });
        // The solid nearest the camera fills some of the pixel; the line
        // round it runs under the rest.
        let nearer = |a: &&Seen, b: &&Seen| a.depth.total_cmp(&b.depth);
        let front = (spots.iter().filter(|spot| !spot.is_backdrop())).min_by(nearer);
        let Some(front) = front else {
            return seen.plain(lights.iter().sum::<f32>() / 4.0);
        };
        let of_front = |spot: &Seen| spot.solid == front.solid;
        let fills = spots.iter().filter(|spot| of_front(spot)).count();
        let front_light = (spots.iter().zip(&lights))
            .filter(|(spot, _)| of_front(spot))
            .map(|(_, light)| light)
            .sum::<f32>()
            / fills as f32;
        let light: f32 = (spots.iter().zip(&lights))
            .map(|(spot, light)| match of_front(spot) {
                true => *light,
                false => self.line(*light, front_light, spot.thing != front.thing),
            })
            .sum();
        Drawn {
            light: light / 4.0,
            front_light,
            front: Front {
                fills: fills as u8,
                ..front.front()
            },
        }
    }

    /// The light of the line beside a solid of light `front`, drawn over
    /// what has light `under`: darker than both. `round` a thing it is
    /// darker than between the parts of one.
    fn line(&self, under: f32, front: f32, round: bool) -> f32 {
        let dark = if round { OUTLINE } else { INLINE };
        under.min(front) * (1.0 - dark * self.faint)
    }

    /// The light of pixel (`x`, `y`), drawn as `here`, with the lines
    /// that reach it: a solid nearer the camera in a pixel beside it has
    /// its line here, a pixel wide in all, darker round a thing than
    /// between its parts. And a solid's own edge is a little lighter where
    /// it sticks out, darker where it goes in. `fronts` say what is
    /// nearest the camera in each pixel, and its light.
    fn lined(
        &self,
        seens: &[Seen],
        here: Drawn,
        fronts: (&impl Fn(usize) -> Front, &impl Fn(usize) -> f32),
        x: usize,
        y: usize,
    ) -> f32 {
        let (ix, iy) = (x as isize, y as isize);
        let beside = [(-1, 0), (1, 0), (0, -1), (0, 1)].map(|(dx, dy)| (ix + dx, iy + dy));
        let inside = |(nx, ny): (isize, isize)| {
            (nx >= 0 && ny >= 0 && (nx as usize) < self.row && (ny as usize) < self.rows)
                .then(|| ny as usize * self.row + nx as usize)
        };
        let own = here.front;
        let light = (beside.iter())
            .filter_map(|at| inside(*at))
            .filter_map(|at| {
                let other = fronts.0(at);
                let before = other.solid >= SOLIDS
                    && other.solid != own.solid
                    && (own.solid < SOLIDS || other.depth < own.depth);
                // As much of the line as the solid fills of its own pixel:
                // the rest of the line is drawn there.
                before.then(|| {
                    let line = self.line(here.light, fronts.1(at), other.thing != own.thing);
                    here.light + (line - here.light) * other.fills as f32 * 0.25
                })
            })
            .fold(here.light, f32::min);

        // How the surface bends, from the way it faces either side.
        let seen = &seens[y * self.row + x];
        let facing = |at: (isize, isize)| {
            self.at(seens, at.0, at.1)
                .filter(|other| other.solid == seen.solid)
                .map(|other| other.normal())
        };
        let bend = match seen.solid >= SOLIDS {
            true => match beside.map(facing) {
                [Some(left), Some(right), Some(above), Some(below)] => {
                    (right - left).dot(self.right) + (above - below).dot(self.up)
                }
                _ => 0.0,
            },
            false => 0.0,
        };
        let bent = (bend * 0.3).clamp(-VALLEY, RIDGE);
        light * (1.0 + bent)
    }
}

/// [`LOOKS`] places spread evenly over a disc, its edge one unit out.
fn spiral() -> &'static [(f32, f32); LOOKS] {
    static SPIRAL: OnceLock<[(f32, f32); LOOKS]> = OnceLock::new();
    SPIRAL.get_or_init(|| {
        std::array::from_fn(|look| {
            let out = ((look as f32 + 0.5) / LOOKS as f32).sqrt();
            let (sin, cos) = (look as f32 * 2.4).sin_cos();
            (out * cos, out * sin)
        })
    })
}

/// Spot `spot` of [`SUNS`] on the sun's disc, spread over it evenly and
/// turned its own way for pixel (`x`, `y`).
fn sun_spot(spot: usize, x: usize, y: usize) -> (f32, f32) {
    let out = ((spot as f32 + 0.5) / SUNS as f32).sqrt();
    let (sin, cos) = (scatter(x, y) * TAU + spot as f32 * 2.4).sin_cos();
    (out * cos, out * sin)
}
