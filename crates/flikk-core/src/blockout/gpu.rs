//! A picture of a scene drawn on the GPU, by a [`ComputeService`]: the
//! CPU still works out where everything stands at the moment (a
//! [`Plan`]); here that is written flat into buffers, and the program
//! made of each module's `.wgsl` traces it as the CPU does.
//!
//! Each module writes what it knows for the GPU beside its own code: a
//! solid and its holes (`solid.rs`), the ground's heights (`land.rs`), the
//! box tree and the camera (`world.rs`), and how the picture is drawn
//! (`render.rs`). Each `shader()` gives its `.wgsl` with the constants it
//! shares with the CPU.

use super::render::{Look, Mode, Plan, TABLES};
use super::space::Vec3;
use super::{land, render, solid, world};
use crate::services::{ComputeJob, ComputeService, ComputeStep, Frame, ServiceResult};
use std::sync::OnceLock;

/// Pixels along the side of a workgroup of `render.wgsl`.
const GROUP: u32 = 8;
/// Bytes of one value of each sheet `render.wgsl` writes: a `Seen`, and a
/// `Drawn`.
const SEEN: u64 = 48;
const DRAWN: u64 = 32;
/// Bytes of one `Solid` and one `Node`, the least their buffers hold.
const SOLID: usize = 96;
const NODE: usize = 32;

/// Four-byte values as the GPU reads them.
#[derive(Default)]
pub(super) struct Words(Vec<u32>);

impl Words {
    pub fn float(&mut self, value: f32) {
        self.0.push(value.to_bits());
    }

    pub fn floats<const N: usize>(&mut self, values: [f32; N]) {
        values.into_iter().for_each(|value| self.float(value));
    }

    pub fn uints<const N: usize>(&mut self, values: [u32; N]) {
        self.0.extend(values);
    }

    /// `v` and `w` as a `vec4<f32>`.
    pub fn vec(&mut self, v: Vec3, w: f32) {
        self.floats([v.x, v.y, v.z, w]);
    }

    /// Them as bytes, at least `least` of them: a storage buffer holds at
    /// least one of what it lists.
    fn bytes(&self, least: usize) -> Vec<u8> {
        let mut bytes: Vec<u8> = self.0.iter().flat_map(|word| word.to_le_bytes()).collect();
        bytes.resize(bytes.len().max(least), 0);
        bytes
    }
}

/// The program: every module's part, once.
fn source() -> &'static str {
    static SOURCE: OnceLock<String> = OnceLock::new();
    SOURCE.get_or_init(|| {
        [
            solid::shader(),
            land::shader(TABLES),
            world::shader(),
            render::shader(),
        ]
        .concat()
    })
}

/// `plan` drawn by `engine`, the guides of a look from outside drawn over
/// it on the CPU from the depth the GPU found.
pub(super) fn draw(engine: &dyn ComputeService, plan: &Plan) -> ServiceResult<Frame> {
    let (width, height) = plan.size;
    let picture = plan.picture();
    let (mut params, mut nodes, mut solids, mut tables) = Default::default();
    render::pack_tables(&mut tables);
    (plan.world).pack(&mut params, &mut nodes, &mut solids, &mut tables);
    picture.pack(plan.look, plan.near, &mut params);
    let inputs = [solids.bytes(SOLID), nodes.bytes(NODE), tables.bytes(4)];

    let pixels = (width * height) as u64;
    let half = [width.div_ceil(2) as u32, height.div_ceil(2) as u32];
    let full = matches!(Mode::of(plan.look), Mode::Full);
    // Seen, shades (the sun on each pixel, then how open each surface is
    // at half the size), drawn, and the picture with its depth.
    let shades = pixels + half[0] as u64 * half[1] as u64;
    let scratch = match full {
        true => [pixels * SEEN, shades * 4, pixels * DRAWN, pixels * 8],
        false => [pixels * SEEN, 4, DRAWN, pixels * 8],
    };
    let groups = |[x, y]: [u32; 2]| [x.div_ceil(GROUP), y.div_ceil(GROUP)];
    let every = |entry| ComputeStep {
        entry,
        groups: groups([width as u32, height as u32]),
    };
    let steps = match plan.look {
        Look::Pass(_) => vec![every("look_all"), every("pass_out")],
        _ if full => vec![
            every("look_all"),
            every("sun_all"),
            ComputeStep {
                entry: "open_all",
                groups: groups(half),
            },
            every("finish_all"),
            every("full_out"),
        ],
        _ => vec![every("look_all"), every("draft_out")],
    };
    let with_depth = !plan.guides.is_empty();
    let read = pixels * if with_depth { 8 } else { 4 };
    let bytes = engine.run(&ComputeJob {
        label: "flikk blockout",
        source: source(),
        uniforms: &params.bytes(0),
        inputs: &inputs.each_ref().map(Vec::as_slice),
        scratch: &scratch,
        steps: &steps,
        read,
    })?;
    if bytes.len() as u64 != read {
        return Err(crate::services::ServiceError(format!(
            "the GPU gave back {} bytes of a picture of {read}",
            bytes.len()
        )));
    }
    let (rgba, depths) = bytes.split_at(pixels as usize * 4);
    let mut rgba = rgba.to_vec();
    if with_depth {
        let depth = |at: usize| {
            let bytes = [0, 1, 2, 3].map(|k| depths[at * 4 + k]);
            f32::from_le_bytes(bytes)
        };
        picture.stroke_all(&plan.guides, depth, rgba.as_chunks_mut::<4>().0);
    }
    Ok(Frame {
        width: width as u32,
        height: height as u32,
        rgba: rgba.into(),
    })
}
