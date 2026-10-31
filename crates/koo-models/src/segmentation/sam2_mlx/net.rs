//! SAM 2.1 tiny's network over `mlx-rs`, module by module as SAM 2 defines
//! it, channels last throughout (MLX's convolutions take N×H×W×C): Hiera
//! with its feature pyramid, the prompt encoder, the two-way mask decoder,
//! the memory encoder and the memory attention with axial rotary
//! positions. Weights are kept in single precision. Fixed position tables
//! and the prompt's handful of points are made on the CPU.

use super::config::{
    DECODER_HEADS, EMBED, EPS, EPS_FINE, GLOBAL_BLOCKS, GRID, HEADS, HIDDEN, INPUT, LOGITS, MEMORY,
    MEMORY_BIAS, MEMORY_LAYERS, MEMORY_SCALE, NO_OBJECT, POOLED_STAGES, POSITION_AT_INPUT,
    STABILITY, STABILITY_DELTA, STAGES, WINDOWS,
};
use anyhow::{Result, anyhow};
use mlx_rs::fast::{self, ScaledDotProductAttentionMask};
use mlx_rs::ops::indexing::TryIndexOp;
use mlx_rs::{Array, Dtype, nn, ops};
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::path::Path;

/// Tokens of the coarsest grid.
const TOKENS: i32 = GRID * GRID;

/// One Hiera block.
struct Block {
    dim: i32,
    dim_out: i32,
    heads: i32,
    /// It pools its queries, halving the grid.
    pooled: bool,
    /// Side of its attention windows; 0 attends globally.
    window: i32,
}

/// Hiera's blocks: a stage's first block takes the previous stage's window
/// and pools to the new stage's grid.
fn blocks() -> (Vec<Block>, Vec<usize>) {
    let ends: Vec<usize> = (1..=STAGES.len())
        .map(|i| STAGES[..i].iter().sum::<usize>() - 1)
        .collect();
    let pooled: Vec<usize> = ends[..ends.len() - 1]
        .iter()
        .map(|e| e + 1)
        .take(POOLED_STAGES)
        .collect();
    let (mut dim, mut heads, mut stage) = (EMBED, HEADS, 0);
    let mut out = Vec::new();
    for i in 0..ends[ends.len() - 1] + 1 {
        let mut dim_out = dim;
        let window = if GLOBAL_BLOCKS.contains(&i) {
            0
        } else {
            WINDOWS[stage]
        };
        if i > 0 && ends.contains(&(i - 1)) {
            dim_out = dim * 2;
            heads *= 2;
            stage += 1;
        }
        out.push(Block {
            dim,
            dim_out,
            heads,
            pooled: pooled.contains(&i),
            window,
        });
        dim = dim_out;
    }
    (out, ends)
}

/// What the encoder makes of a picture, channels last: the decoder's two
/// finer feature maps (projected), and the coarsest features.
pub struct Features {
    pub fine: [Array; 2],
    pub coarse: Array,
}

/// What the decoder makes of a frame's features and a prompt.
pub struct Decoded {
    /// 1 × 256 × 256 × 1 logits.
    pub logits: Array,
    pub score: f32,
    pub object_score: f32,
    /// 1 × 256.
    pub pointer: Array,
}

/// A frame's memory: its tokens and their positions (4096 × 64 each), and
/// the object's pointer (1 × 256).
pub struct Memory {
    pub tokens: Array,
    pub positions: Array,
    pub pointer: Array,
}

pub struct Net {
    w: HashMap<String, Array>,
    blocks: Vec<Block>,
    stage_ends: Vec<usize>,
    /// The prompt encoder's random Fourier matrix (2 × 128), its label
    /// embeddings and its padding, on the CPU.
    gaussian: Vec<f32>,
    labels: [Vec<f32>; 4],
    not_a_point: Vec<f32>,
    /// The decoder's dense positions of the coarsest grid (1 × 4096 × 256),
    /// the sine positions of its features (1 × 4096 × 256) and of the
    /// memory (4096 × 64), and the rotary angles of the grid (4096 × 128).
    dense_positions: Array,
    vision_positions: Array,
    memory_positions: Array,
    rope: (Array, Array),
}

fn err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

impl Net {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = Array::load_safetensors(path)
            .map_err(|e| anyhow!("could not load {}: {e}", path.display()))?;
        let mut w = HashMap::with_capacity(raw.len());
        for (k, v) in raw {
            let v = v.as_dtype(Dtype::Float32).map_err(err)?;
            v.eval().map_err(err)?;
            w.insert(k, v);
        }
        let host = |w: &HashMap<String, Array>, key: &str| -> Result<Vec<f32>> {
            let a = w
                .get(key)
                .ok_or_else(|| anyhow!("the subject mask model has no {key}"))?;
            Ok(a.as_slice::<f32>().to_vec())
        };
        let gaussian = host(
            &w,
            "sam_prompt_encoder.pe_layer.positional_encoding_gaussian_matrix",
        )?;
        let labels = [0, 1, 2, 3].map(|i| {
            host(
                &w,
                &format!("sam_prompt_encoder.point_embeddings.{i}.weight"),
            )
            .unwrap_or_default()
        });
        let not_a_point = host(&w, "sam_prompt_encoder.not_a_point_embed.weight")?;
        if gaussian.len() != 256 || labels.iter().any(|l| l.len() != 256) {
            return Err(anyhow!(
                "the subject mask model's prompt encoder is not SAM 2.1's"
            ));
        }
        let (blocks, stage_ends) = blocks();
        let dense: Vec<f32> = (0..TOKENS)
            .flat_map(|t| {
                let (x, y) = ((t % GRID) as f32, (t / GRID) as f32);
                let g = GRID as f32;
                fourier(&gaussian, [(x + 0.5) / g, (y + 0.5) / g])
            })
            .collect();
        let rope = rope_angles(HIDDEN, GRID);
        Ok(Self {
            dense_positions: Array::from_slice(&dense, &[1, TOKENS, HIDDEN]),
            vision_positions: Array::from_slice(
                &sine_positions(GRID, HIDDEN),
                &[1, TOKENS, HIDDEN],
            ),
            memory_positions: Array::from_slice(&sine_positions(GRID, MEMORY), &[TOKENS, MEMORY]),
            rope: (
                Array::from_slice(&rope.0, &[TOKENS, HIDDEN / 2]),
                Array::from_slice(&rope.1, &[TOKENS, HIDDEN / 2]),
            ),
            w,
            blocks,
            stage_ends,
            gaussian,
            labels,
            not_a_point,
        })
    }

    fn w(&self, key: &str) -> Result<&Array> {
        self.w
            .get(key)
            .ok_or_else(|| anyhow!("the subject mask model has no {key}"))
    }

    fn linear(&self, x: &Array, name: &str) -> Result<Array> {
        let weight = self.w(&format!("{name}.weight"))?;
        let bias = self.w(&format!("{name}.bias"))?;
        ops::matmul(x, weight.transpose_axes(&[1, 0]).map_err(err)?)
            .and_then(|y| y.add(bias))
            .map_err(err)
    }

    fn conv(&self, x: &Array, name: &str, stride: i32, padding: i32, groups: i32) -> Result<Array> {
        let weight = self.w(&format!("{name}.weight"))?;
        let bias = self.w(&format!("{name}.bias"))?;
        ops::conv2d(
            x,
            weight,
            (stride, stride),
            (padding, padding),
            (1, 1),
            groups,
        )
        .and_then(|y| y.add(bias))
        .map_err(err)
    }

    /// A 2 × 2, stride 2 transposed convolution.
    fn up_conv(&self, x: &Array, name: &str) -> Result<Array> {
        let weight = self.w(&format!("{name}.weight"))?;
        let bias = self.w(&format!("{name}.bias"))?;
        ops::conv_transpose2d(x, weight, (2, 2), (0, 0), (1, 1), (0, 0), 1)
            .and_then(|y| y.add(bias))
            .map_err(err)
    }

    /// Layer norm over the last axis (channels, channels last).
    fn norm(&self, x: &Array, name: &str, eps: f32) -> Result<Array> {
        let weight = self.w(&format!("{name}.weight"))?;
        let bias = self.w(&format!("{name}.bias"))?;
        fast::layer_norm(x, Some(weight), Some(bias), eps).map_err(err)
    }

    /// `layers` linear layers, ReLU (or GELU) between them.
    fn mlp(&self, x: &Array, name: &str, layers: usize, gelu: bool) -> Result<Array> {
        let mut x = x.clone();
        for i in 0..layers {
            x = self.linear(&x, &format!("{name}.layers.{i}"))?;
            if i + 1 < layers {
                x = if gelu {
                    nn::gelu(&x).map_err(err)?
                } else {
                    nn::relu(&x).map_err(err)?
                };
            }
        }
        Ok(x)
    }

    /// Attention as SAM's decoder does it: projections, `heads` heads, out
    /// projection.
    fn attention(&self, q: &Array, k: &Array, v: &Array, name: &str, heads: i32) -> Result<Array> {
        let q = split_heads(&self.linear(q, &format!("{name}.q_proj"))?, heads)?;
        let k = split_heads(&self.linear(k, &format!("{name}.k_proj"))?, heads)?;
        let v = split_heads(&self.linear(v, &format!("{name}.v_proj"))?, heads)?;
        let out = attend(&q, &k, &v)?;
        self.linear(&join_heads(&out)?, &format!("{name}.out_proj"))
    }

    // ---------------------------------------------------------- encoder

    /// The features of a normalized 1024² picture (channels first, as
    /// `plan::square` gives it).
    pub fn encode(&self, pixels: &[f32]) -> Result<Features> {
        let x = Array::from_slice(pixels, &[1, 3, INPUT, INPUT])
            .transpose_axes(&[0, 2, 3, 1])
            .map_err(err)?;
        let mut x = self.conv(&x, "trunk.patch_embed.proj", 4, 3, 1)?;
        x = x.add(self.w("trunk.pos_embed_full")?).map_err(err)?;
        let mut stages = Vec::with_capacity(self.stage_ends.len());
        for (i, block) in self.blocks.iter().enumerate() {
            x = self.block(&x, i, block)?;
            if self.stage_ends.contains(&i) {
                stages.push(x.clone());
            }
        }
        // The feature pyramid, coarsest first: the two coarsest levels sum
        // top-down; the coarsest is then left out.
        let n = stages.len() - 1;
        let mut levels: Vec<Option<Array>> = vec![None; stages.len()];
        let mut prev: Option<Array> = None;
        for i in (0..=n).rev() {
            let lateral = self.conv(&stages[i], &format!("neck.convs.{}", n - i), 1, 0, 1)?;
            let level = match &prev {
                Some(p) if i + 2 > n => lateral.add(upsample2(p)?).map_err(err)?,
                _ => lateral,
            };
            levels[i] = Some(level.clone());
            prev = Some(level);
        }
        let level = |i: usize| levels[i].clone().expect("every level made");
        let fine = [
            self.conv(&level(0), "sam_mask_decoder.conv_s0", 1, 0, 1)?,
            self.conv(&level(1), "sam_mask_decoder.conv_s1", 1, 0, 1)?,
        ];
        let out = Features {
            fine,
            coarse: level(2),
        };
        mlx_rs::transforms::eval([&out.fine[0], &out.fine[1], &out.coarse]).map_err(err)?;
        Ok(out)
    }

    fn block(&self, x: &Array, i: usize, b: &Block) -> Result<Array> {
        let name = format!("trunk.blocks.{i}");
        let normed = self.norm(x, &format!("{name}.norm1"), EPS_FINE)?;
        let shortcut = if b.dim != b.dim_out {
            max_pool2(&self.linear(&normed, &format!("{name}.proj"))?)?
        } else {
            x.clone()
        };
        let (h, w) = (normed.shape()[1], normed.shape()[2]);
        let mut y = normed;
        let mut padded = (h, w);
        if b.window > 0 {
            (y, padded) = partition(&y, b.window)?;
        }
        y = self.multi_scale_attention(&y, &name, b)?;
        let (mut window, mut size) = (b.window, (h, w));
        if b.pooled {
            window = b.window / 2;
            size = (shortcut.shape()[1], shortcut.shape()[2]);
            padded = (
                size.0 + (window - size.0 % window) % window,
                size.1 + (window - size.1 % window) % window,
            );
        }
        if b.window > 0 {
            y = unpartition(&y, window, padded, size)?;
        }
        let x = shortcut.add(&y).map_err(err)?;
        let mlp = self.mlp(
            &self.norm(&x, &format!("{name}.norm2"), EPS_FINE)?,
            &format!("{name}.mlp"),
            2,
            true,
        )?;
        x.add(&mlp).map_err(err)
    }

    fn multi_scale_attention(&self, x: &Array, name: &str, b: &Block) -> Result<Array> {
        let (n, h, w) = (x.shape()[0], x.shape()[1], x.shape()[2]);
        let qkv = self
            .linear(x, &format!("{name}.attn.qkv"))?
            .reshape(&[n, h * w, 3, b.heads, -1])
            .map_err(err)?;
        let parts = qkv.split_equal(3, 2).map_err(err)?;
        let take = |i: usize| parts[i].squeeze_axes(&[2]).map_err(err);
        let (mut q, k, v) = (take(0)?, take(1)?, take(2)?);
        let (mut h, mut w) = (h, w);
        if b.pooled {
            q = max_pool2(&q.reshape(&[n, h, w, -1]).map_err(err)?)?;
            (h, w) = (h / 2, w / 2);
            q = q.reshape(&[n, h * w, b.heads, -1]).map_err(err)?;
        }
        let order = [0, 2, 1, 3];
        let out = attend(
            &q.transpose_axes(&order).map_err(err)?,
            &k.transpose_axes(&order).map_err(err)?,
            &v.transpose_axes(&order).map_err(err)?,
        )?;
        let out = out
            .transpose_axes(&order)
            .and_then(|o| o.reshape(&[n, h, w, -1]))
            .map_err(err)?;
        self.linear(&out, &format!("{name}.attn.proj"))
    }

    // ---------------------------------------------------------- decoder

    /// The coarsest features of a frame picked by itself: with the "no
    /// memory" embedding added.
    pub fn without_memory(&self, coarse: &Array) -> Result<Array> {
        let embed = self
            .w("no_mem_embed")?
            .reshape(&[1, 1, 1, HIDDEN])
            .map_err(err)?;
        coarse.add(embed).map_err(err)
    }

    /// The object `points` (labelled, in the model's square) pick in
    /// features whose coarsest map is `cond`. Several answers when there is
    /// a single point or none, the best kept; else one, or the best of the
    /// several when it is unstable.
    pub fn decode(
        &self,
        fine: &[Array; 2],
        cond: &Array,
        points: &[([f32; 2], i32)],
    ) -> Result<Decoded> {
        let multimask = points.len() <= 1;
        // No click: a padding point, as SAM 2 gives the prompt encoder,
        // which pads once more.
        let pad = ([0.0, 0.0], -1);
        let given: &[([f32; 2], i32)] = if points.is_empty() {
            std::slice::from_ref(&pad)
        } else {
            points
        };
        let mut sparse: Vec<f32> = Vec::with_capacity((given.len() + 1) * HIDDEN as usize);
        for (p, label) in given.iter().chain(std::iter::once(&pad)) {
            if *label < 0 {
                sparse.extend_from_slice(&self.not_a_point);
                continue;
            }
            let s = INPUT as f32;
            let pe = fourier(&self.gaussian, [(p[0] + 0.5) / s, (p[1] + 0.5) / s]);
            let embed = &self.labels[(*label).clamp(0, 3) as usize];
            sparse.extend(pe.iter().zip(embed).map(|(a, b)| a + b));
        }
        let count = (given.len() + 1) as i32;
        let sparse = Array::from_slice(&sparse, &[1, count, HIDDEN]);
        let tokens = ops::concatenate(
            &[
                self.w("sam_mask_decoder.obj_score_token.weight")?.clone(),
                self.w("sam_mask_decoder.iou_token.weight")?.clone(),
                self.w("sam_mask_decoder.mask_tokens.weight")?.clone(),
            ],
            0,
        )
        .and_then(|t| t.expand_dims(0))
        .map_err(err)?;
        let tokens = ops::concatenate(&[tokens, sparse], 1).map_err(err)?;
        let no_mask = self
            .w("sam_prompt_encoder.no_mask_embed.weight")?
            .reshape(&[1, 1, 1, HIDDEN])
            .map_err(err)?;
        let src = cond
            .add(no_mask)
            .and_then(|s| s.reshape(&[1, TOKENS, HIDDEN]))
            .map_err(err)?;
        let (queries, keys) = self.two_way(&src, &tokens)?;

        let token = |i: i32| queries.try_index((.., i, ..)).map_err(err);
        let up = keys.reshape(&[1, GRID, GRID, HIDDEN]).map_err(err)?;
        let x = self
            .up_conv(&up, "sam_mask_decoder.output_upscaling_0")?
            .add(&fine[1])
            .map_err(err)?;
        let x = nn::gelu(self.norm(&x, "sam_mask_decoder.output_upscaling_1", EPS_FINE)?)
            .map_err(err)?;
        let x = self
            .up_conv(&x, "sam_mask_decoder.output_upscaling_3")?
            .add(&fine[0])
            .map_err(err)?;
        let upscaled = nn::gelu(&x).map_err(err)?;
        let hyper: Vec<Array> = (0..4)
            .map(|i| {
                self.mlp(
                    &token(2 + i)?,
                    &format!("sam_mask_decoder.output_hypernetworks_mlps.{i}"),
                    3,
                    false,
                )
            })
            .collect::<Result<_>>()?;
        let hyper = ops::stack(&hyper, 1).map_err(err)?;
        let flat = upscaled
            .reshape(&[1, LOGITS * LOGITS, -1])
            .and_then(|u| u.transpose_axes(&[0, 2, 1]))
            .map_err(err)?;
        let masks = ops::matmul(&hyper, &flat).map_err(err)?;
        let ious =
            ops::sigmoid(self.mlp(&token(1)?, "sam_mask_decoder.iou_prediction_head", 3, false)?)
                .map_err(err)?;
        let object = self.mlp(&token(0)?, "sam_mask_decoder.pred_obj_score_head", 3, false)?;

        // Which answer: read the scores (and the single mask's stability).
        let single = masks.try_index((.., 0, ..)).map_err(err)?;
        let inner = single.gt(Array::from_f32(STABILITY_DELTA)).map_err(err)?;
        let outer = single.gt(Array::from_f32(-STABILITY_DELTA)).map_err(err)?;
        let areas = ops::stack(
            &[
                inner
                    .as_dtype(Dtype::Float32)
                    .and_then(|a| a.sum(None))
                    .map_err(err)?,
                outer
                    .as_dtype(Dtype::Float32)
                    .and_then(|a| a.sum(None))
                    .map_err(err)?,
            ],
            0,
        )
        .map_err(err)?;
        mlx_rs::transforms::eval([&ious, &object, &areas]).map_err(err)?;
        let ious_host = ious.as_slice::<f32>().to_vec();
        let object_score = object.as_slice::<f32>()[0];
        let area = areas.as_slice::<f32>();
        let best_of_three = 1
            + (1..4)
                .max_by(|a, b| ious_host[*a].total_cmp(&ious_host[*b]))
                .map_or(0, |i| i - 1);
        let stable = area[1] <= 0.0 || area[0] / area[1] >= STABILITY;
        let (mask, pointer_token) = if multimask {
            (best_of_three, best_of_three)
        } else if stable {
            (0, 0)
        } else {
            (best_of_three, 0)
        };
        let logits = masks
            .try_index((.., mask as i32, ..))
            .and_then(|m| m.reshape(&[1, LOGITS, LOGITS, 1]))
            .map_err(err)?;
        let present = object_score > 0.0;
        let logits = if present {
            logits
        } else {
            ops::full::<f32>(&[1, LOGITS, LOGITS, 1], Array::from_f32(NO_OBJECT)).map_err(err)?
        };
        let pointer = if present {
            self.mlp(&token(2 + pointer_token as i32)?, "obj_ptr_proj", 3, false)?
        } else {
            self.w("no_obj_ptr")?.clone()
        };
        Ok(Decoded {
            logits,
            score: ious_host[mask],
            object_score,
            pointer,
        })
    }

    /// The two-way transformer: tokens attend to the picture and the
    /// picture to them.
    fn two_way(&self, src: &Array, tokens: &Array) -> Result<(Array, Array)> {
        let pe = &self.dense_positions;
        let (mut q, mut k) = (tokens.clone(), src.clone());
        for layer in 0..2 {
            let name = format!("sam_mask_decoder.transformer.layers.{layer}");
            let n = |part: &str| format!("{name}.{part}");
            q = if layer == 0 {
                self.attention(&q, &q, &q, &n("self_attn"), DECODER_HEADS)?
            } else {
                let qp = q.add(tokens).map_err(err)?;
                q.add(self.attention(&qp, &qp, &q, &n("self_attn"), DECODER_HEADS)?)
                    .map_err(err)?
            };
            q = self.norm(&q, &n("norm1"), EPS)?;
            let (qp, kp) = (q.add(tokens).map_err(err)?, k.add(pe).map_err(err)?);
            let attended =
                self.attention(&qp, &kp, &k, &n("cross_attn_token_to_image"), DECODER_HEADS)?;
            q = self.norm(&q.add(attended).map_err(err)?, &n("norm2"), EPS)?;
            let mlp = self.mlp(&q, &n("mlp"), 2, false)?;
            q = self.norm(&q.add(mlp).map_err(err)?, &n("norm3"), EPS)?;
            let (qp, kp) = (q.add(tokens).map_err(err)?, k.add(pe).map_err(err)?);
            let attended =
                self.attention(&kp, &qp, &q, &n("cross_attn_image_to_token"), DECODER_HEADS)?;
            k = self.norm(&k.add(attended).map_err(err)?, &n("norm4"), EPS)?;
        }
        let (qp, kp) = (q.add(tokens).map_err(err)?, k.add(pe).map_err(err)?);
        let name = "sam_mask_decoder.transformer.final_attn_token_to_image";
        let attended = self.attention(&qp, &kp, &k, name, DECODER_HEADS)?;
        let q = self.norm(
            &q.add(attended).map_err(err)?,
            "sam_mask_decoder.transformer.norm_final_attn",
            EPS,
        )?;
        Ok((q, k))
    }

    // ----------------------------------------------------------- memory

    /// The memory of a frame from its coarsest features and its logits; a
    /// prompted frame's mask is made binary.
    pub fn memorize(&self, coarse: &Array, d: &Decoded, prompted: bool) -> Result<Memory> {
        let high = upsample_bilinear(&d.logits, INPUT)?;
        let mask = if prompted {
            high.gt(Array::from_f32(0.0))
                .and_then(|m| m.as_dtype(Dtype::Float32))
                .map_err(err)?
        } else {
            ops::sigmoid(&high).map_err(err)?
        };
        let mut m = mask
            .multiply(Array::from_f32(MEMORY_SCALE))
            .and_then(|m| m.add(Array::from_f32(MEMORY_BIAS)))
            .map_err(err)?;
        for i in 0..4 {
            let name = format!("memory_encoder.mask_downsampler.conv{i}");
            m = self.conv(&m, &name, 2, 1, 1)?;
            let name = format!("memory_encoder.mask_downsampler.norm{i}");
            m = nn::gelu(self.norm(&m, &name, EPS_FINE)?).map_err(err)?;
        }
        m = self.conv(&m, "memory_encoder.mask_downsampler.conv4", 1, 0, 1)?;
        let mut x = self
            .conv(coarse, "memory_encoder.pix_feat_proj", 1, 0, 1)?
            .add(&m)
            .map_err(err)?;
        for i in 0..2 {
            let name = format!("memory_encoder.fuser.{i}");
            let n = |part: &str| format!("{name}.{part}");
            let y = self.conv(&x, &n("dwconv"), 1, 3, HIDDEN)?;
            let y = self.norm(&y, &n("norm"), EPS_FINE)?;
            let y = self.linear(
                &nn::gelu(self.linear(&y, &n("pwconv1"))?).map_err(err)?,
                &n("pwconv2"),
            )?;
            let y = y.multiply(self.w(&n("gamma"))?).map_err(err)?;
            x = x.add(&y).map_err(err)?;
        }
        let mut x = self.conv(&x, "memory_encoder.out_proj", 1, 0, 1)?;
        if d.object_score <= 0.0 {
            x = x.add(self.w("no_obj_embed_spatial")?).map_err(err)?;
        }
        let memory = Memory {
            tokens: x.reshape(&[TOKENS, MEMORY]).map_err(err)?,
            positions: self.memory_positions.clone(),
            pointer: d.pointer.clone(),
        };
        mlx_rs::transforms::eval([&memory.tokens, &memory.pointer]).map_err(err)?;
        Ok(memory)
    }

    /// Positions of object pointers at normalized distances: their sine
    /// encoding through the temporal projection (pointers × 64).
    pub fn pointer_positions(&self, distances: &[f32]) -> Result<Array> {
        let half = HIDDEN / 2;
        let mut sine = Vec::with_capacity(distances.len() * HIDDEN as usize);
        for d in distances {
            let angles: Vec<f32> = (0..half)
                .map(|k| d / 10000f32.powf(2.0 * (k / 2) as f32 / half as f32))
                .collect();
            sine.extend(angles.iter().map(|a| a.sin()));
            sine.extend(angles.iter().map(|a| a.cos()));
        }
        let sine = Array::from_slice(&sine, &[distances.len() as i32, HIDDEN]);
        self.linear(&sine, "obj_ptr_tpos_proj")
    }

    /// Temporal position row `row` (64).
    pub fn temporal(&self, row: usize) -> Result<Array> {
        self.w("maskmem_tpos_enc")?
            .reshape(&[-1, MEMORY])
            .and_then(|t| t.try_index(row as i32))
            .map_err(err)
    }

    /// `coarse` features conditioned on `memory` (tokens × 64) and its
    /// positions, its last `pointers` tokens object pointers (left out of
    /// the rotary positions).
    pub fn attend_memory(
        &self,
        coarse: &Array,
        memory: &Array,
        positions: &Array,
        pointers: i32,
    ) -> Result<Array> {
        let curr = coarse.reshape(&[1, TOKENS, HIDDEN]).map_err(err)?;
        let mut x = curr
            .add(
                self.vision_positions
                    .multiply(Array::from_f32(POSITION_AT_INPUT))
                    .map_err(err)?,
            )
            .map_err(err)?;
        let memory = memory.expand_dims(0).map_err(err)?;
        let keys = memory
            .add(positions.expand_dims(0).map_err(err)?)
            .map_err(err)?;
        for layer in 0..MEMORY_LAYERS {
            let name = format!("memory_attention.layers.{layer}");
            let n = |part: &str| format!("{name}.{part}");
            let y = self.norm(&x, &n("norm1"), EPS)?;
            x = x
                .add(self.rope_attention(&y, &y, &y, &n("self_attn"), 0)?)
                .map_err(err)?;
            let y = self.norm(&x, &n("norm2"), EPS)?;
            x = x
                .add(self.rope_attention(&y, &keys, &memory, &n("cross_attn_image"), pointers)?)
                .map_err(err)?;
            let y = self.norm(&x, &n("norm3"), EPS)?;
            let y = self.linear(
                &nn::relu(self.linear(&y, &n("linear1"))?).map_err(err)?,
                &n("linear2"),
            )?;
            x = x.add(&y).map_err(err)?;
        }
        let x = self.norm(&x, "memory_attention.norm", EPS)?;
        x.reshape(&[1, GRID, GRID, HIDDEN]).map_err(err)
    }

    /// One-head attention with axial rotary positions on the queries and on
    /// the keys but the last `exclude` (the keys repeat the grid's angles
    /// once per memory frame).
    fn rope_attention(
        &self,
        q: &Array,
        k: &Array,
        v: &Array,
        name: &str,
        exclude: i32,
    ) -> Result<Array> {
        let q = split_heads(&self.linear(q, &format!("{name}.q_proj"))?, 1)?;
        let k = split_heads(&self.linear(k, &format!("{name}.k_proj"))?, 1)?;
        let v = split_heads(&self.linear(v, &format!("{name}.v_proj"))?, 1)?;
        let (cos, sin) = &self.rope;
        let q = rotate(&q, cos, sin)?;
        let len = k.shape()[2];
        let rotated = len - exclude;
        let repeat = rotated / TOKENS;
        let tile = |a: &Array| ops::tile(a, &[repeat.max(1), 1]).map_err(err);
        let k = if rotated > 0 && repeat > 0 {
            let (kc, ks) = (tile(cos)?, tile(sin)?);
            let head = rotate(
                &k.try_index((.., .., ..rotated, ..)).map_err(err)?,
                &kc,
                &ks,
            )?;
            if exclude > 0 {
                let tail = k.try_index((.., .., rotated.., ..)).map_err(err)?;
                ops::concatenate(&[head, tail], 2).map_err(err)?
            } else {
                head
            }
        } else {
            k
        };
        let out = attend(&q, &k, &v)?;
        self.linear(&join_heads(&out)?, &format!("{name}.out_proj"))
    }
}

/// `x` (… × tokens × 256) turned by the angles of each token: consecutive
/// pairs of channels as complex numbers.
fn rotate(x: &Array, cos: &Array, sin: &Array) -> Result<Array> {
    let shape = x.shape().to_vec();
    let mut pairs_shape = shape.clone();
    let last = pairs_shape.pop().expect("a channel axis");
    pairs_shape.extend([last / 2, 2]);
    let pairs = x.reshape(&pairs_shape).map_err(err)?;
    let parts = pairs.split_equal(2, -1).map_err(err)?;
    let even = parts[0].squeeze_axes(&[-1]).map_err(err)?;
    let odd = parts[1].squeeze_axes(&[-1]).map_err(err)?;
    let re = even
        .multiply(cos)
        .and_then(|a| a.subtract(odd.multiply(sin)?))
        .map_err(err)?;
    let im = even
        .multiply(sin)
        .and_then(|a| a.add(odd.multiply(cos)?))
        .map_err(err)?;
    ops::stack(&[re, im], -1)
        .and_then(|s| s.reshape(&shape))
        .map_err(err)
}

fn attend(q: &Array, k: &Array, v: &Array) -> Result<Array> {
    let d = *q.shape().last().expect("a channel axis") as f32;
    fast::scaled_dot_product_attention(
        q,
        k,
        v,
        1.0 / d.sqrt(),
        None::<ScaledDotProductAttentionMask>,
        None,
    )
    .map_err(err)
}

/// B × N × C → B × heads × N × C/heads.
fn split_heads(x: &Array, heads: i32) -> Result<Array> {
    let (b, n) = (x.shape()[0], x.shape()[1]);
    x.reshape(&[b, n, heads, -1])
        .and_then(|x| x.transpose_axes(&[0, 2, 1, 3]))
        .map_err(err)
}

fn join_heads(x: &Array) -> Result<Array> {
    let (b, n) = (x.shape()[0], x.shape()[2]);
    x.transpose_axes(&[0, 2, 1, 3])
        .and_then(|x| x.reshape(&[b, n, -1]))
        .map_err(err)
}

/// 2 × 2 max pooling, stride 2 (N × H × W × C, H and W even).
fn max_pool2(x: &Array) -> Result<Array> {
    let s = x.shape();
    x.reshape(&[s[0], s[1] / 2, 2, s[2] / 2, 2, s[3]])
        .and_then(|x| x.max_axes(&[2, 4], false))
        .map_err(err)
}

/// Nearest 2× upsampling (N × H × W × C).
fn upsample2(x: &Array) -> Result<Array> {
    let s = x.shape();
    x.reshape(&[s[0], s[1], 1, s[2], 1, s[3]])
        .and_then(|x| ops::broadcast_to(&x, &[s[0], s[1], 2, s[2], 2, s[3]]))
        .and_then(|x| x.reshape(&[s[0], s[1] * 2, s[2] * 2, s[3]]))
        .map_err(err)
}

/// Bilinear resampling of N × H × W × 1 to `side`², sampling at pixel
/// centres with the edges repeated (as `align_corners=False`).
fn upsample_bilinear(x: &Array, side: i32) -> Result<Array> {
    let along = |x: &Array, axis: i32| -> Result<Array> {
        let n = x.shape()[axis as usize];
        let scale = n as f32 / side as f32;
        let (mut lo, mut hi, mut wt) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..side {
            let src = (i as f32 + 0.5) * scale - 0.5;
            let f = src.floor();
            lo.push((f as i32).clamp(0, n - 1));
            hi.push((f as i32 + 1).clamp(0, n - 1));
            wt.push(src - f);
        }
        let mut shape = vec![1; x.ndim()];
        shape[axis as usize] = side;
        let wt = Array::from_slice(&wt, &shape);
        let a = x
            .take_axis(Array::from_slice(&lo, &[side]), axis)
            .map_err(err)?;
        let b = x
            .take_axis(Array::from_slice(&hi, &[side]), axis)
            .map_err(err)?;
        let one = Array::from_f32(1.0);
        a.multiply(one.subtract(&wt).map_err(err)?)
            .and_then(|a| a.add(b.multiply(&wt)?))
            .map_err(err)
    };
    along(&along(x, 1)?, 2)
}

/// N × H × W × C cut into `side`² windows (padded with zeros), and the
/// padded size.
fn partition(x: &Array, side: i32) -> Result<(Array, (i32, i32))> {
    let s = x.shape().to_vec();
    let (ph, pw) = ((side - s[1] % side) % side, (side - s[2] % side) % side);
    let x = if ph > 0 || pw > 0 {
        ops::pad(x, &[(0, 0), (0, ph), (0, pw), (0, 0)], None, None).map_err(err)?
    } else {
        x.clone()
    };
    let (hp, wp) = (s[1] + ph, s[2] + pw);
    let windows = x
        .reshape(&[s[0], hp / side, side, wp / side, side, s[3]])
        .and_then(|x| x.transpose_axes(&[0, 1, 3, 2, 4, 5]))
        .and_then(|x| x.reshape(&[-1, side, side, s[3]]))
        .map_err(err)?;
    Ok((windows, (hp, wp)))
}

/// Windows back into N × H × W × C, the padding cut away.
fn unpartition(
    windows: &Array,
    side: i32,
    (hp, wp): (i32, i32),
    (h, w): (i32, i32),
) -> Result<Array> {
    let b = windows.shape()[0] / (hp / side * (wp / side));
    let x = windows
        .reshape(&[b, hp / side, wp / side, side, side, -1])
        .and_then(|x| x.transpose_axes(&[0, 1, 3, 2, 4, 5]))
        .and_then(|x| x.reshape(&[b, hp, wp, -1]))
        .map_err(err)?;
    if hp > h || wp > w {
        x.try_index((.., ..h, ..w, ..)).map_err(err)
    } else {
        Ok(x)
    }
}

/// The prompt encoder's random Fourier features of a point (0..1 across
/// the picture): 128 sines then 128 cosines.
fn fourier(gaussian: &[f32], p: [f32; 2]) -> Vec<f32> {
    let (x, y) = (2.0 * p[0] - 1.0, 2.0 * p[1] - 1.0);
    let half = gaussian.len() / 2;
    let angles: Vec<f32> = (0..half)
        .map(|j| TAU * (x * gaussian[j] + y * gaussian[half + j]))
        .collect();
    angles
        .iter()
        .map(|a| a.sin())
        .chain(angles.iter().map(|a| a.cos()))
        .collect()
}

/// Sine positions of a `side`² grid, `channels` per token (half for rows,
/// then half for columns, alternating sine and cosine), token by token.
fn sine_positions(side: i32, channels: i32) -> Vec<f32> {
    let half = (channels / 2) as usize;
    let freq: Vec<f32> = (0..half)
        .map(|k| 10000f32.powf(2.0 * (k / 2) as f32 / half as f32))
        .collect();
    let embed = |v: i32| (v + 1) as f32 / (side as f32 + 1e-6) * TAU;
    let mut out = Vec::with_capacity((side * side) as usize * channels as usize);
    for y in 0..side {
        for x in 0..side {
            for value in [embed(y), embed(x)] {
                out.extend((0..half).map(|k| {
                    let a = value / freq[k];
                    if k % 2 == 0 { a.sin() } else { a.cos() }
                }));
            }
        }
    }
    out
}

/// Axial rotary angles of a `side`² grid for heads `dim` wide: the column's
/// then the row's, `dim / 4` frequencies each; cosines and sines.
fn rope_angles(dim: i32, side: i32) -> (Vec<f32>, Vec<f32>) {
    let freqs: Vec<f32> = (0..dim / 4)
        .map(|k| 1.0 / 10000f32.powf((4 * k) as f32 / dim as f32))
        .collect();
    let (mut cos, mut sin) = (Vec::new(), Vec::new());
    for t in 0..side * side {
        let (x, y) = ((t % side) as f32, (t / side) as f32);
        for phase in freqs
            .iter()
            .map(|f| x * f)
            .chain(freqs.iter().map(|f| y * f))
        {
            cos.push(phase.cos());
            sin.push(phase.sin());
        }
    }
    (cos, sin)
}
